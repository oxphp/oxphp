//! `Shared\Pool` — process-wide resource pool with per-thread affinity
//! (strict strategy, v1).
//!
//! Design notes:
//!
//! * **Affinity key is a raw pthread id (`u64`).** Worker threads spawn
//!   via `std::thread::spawn`, but the FFI boundary needs a type that
//!   can cross C — `std::thread::ThreadId` has no stable `as_u64`. We
//!   use `libc::pthread_self() as usize as u64` which matches what the
//!   cross-thread fcc spike and the SAPI worker pool already treat as
//!   canonical.
//! * `factory_fcc` / `destroy_fcc` are stored as `*mut c_void`. The
//!   creating FFI path (`oxphp_shared_pool_create`) calls
//!   `oxphp_pool_fcc_new` in the C bridge, which emalloc's a
//!   `oxphp_pool_fcc_t` (fcc + ZVAL_COPY'd callable) and hands back a
//!   heap pointer. The spike verified that the stored fcc is safe to
//!   invoke from any worker under ZTS.
//! * **Best-effort destroy on drop/shutdown.** `on_shutdown_notify` and
//!   `on_drop` walk every idle deque and call `oxphp_pool_destroy_invoke`
//!   — which runs `$destroy($resource)` and `zval_ptr_dtor`+`efree` on
//!   the slot. The C helper guards against a missing Zend context
//!   (`EG(current_execute_data) == NULL`): if we're not inside a PHP
//!   request frame we skip `$destroy` but still release the slot
//!   (refcount arithmetic is safe anywhere). Exceptions thrown inside
//!   `$destroy` are captured via `oxphp_bridge_capture_fatal` and
//!   cleared — drain has no user frame to propagate to.
//!   After drain, `oxphp_pool_fcc_free` releases the factory/destroy
//!   fccs. Resources in-flight at SIGKILL or panic-before-drain are
//!   reaped by the OS process teardown.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::plugins::ox_shared::registry::{SharedId, SharedInner, SharedType};
use crate::plugins::ox_shared::types::timeout::{parse_timeout, read_timeout_arg, Wait};
use crate::plugins::ox_shared::value::{SharedRef, SharedValue};

/// Stable per-thread identifier used as the idle-deque key and as the
/// FFI-level `owner` tag. Obtained via `pthread_self` so it round-trips
/// across the Rust↔C bridge without losing information.
pub type ThreadKey = u64;

/// Canonical thread key for the calling thread. Matches what the
/// cross-thread fcc spike and the C-bridge pool helpers report via
/// `pthread_self`, so tags minted here are comparable to tags observed
/// from either language.
#[inline]
pub fn current_thread_key() -> ThreadKey {
    // SAFETY: `pthread_self` is documented to always succeed and return
    // the opaque handle for the calling thread; it has no failure path
    // to misuse. Casting via `usize` handles both macOS (pointer type)
    // and Linux (integer type).
    unsafe { libc::pthread_self() as usize as u64 }
}

/// One resource slot — either checked out to a worker or sitting in
/// its owner's idle deque.
///
/// `resource` is an opaque raw pointer. In production flow it is a
/// heap-allocated `zval*` minted by `oxphp_pool_factory_invoke` in the
/// C bridge (ZVAL_COPY of the factory's return — the pool is the sole
/// owner). Tests synthesise slots with `std::ptr::null_mut()`.
#[derive(Debug)]
pub struct PoolSlot {
    pub resource: *mut c_void,
    pub owner: ThreadKey,
    pub last_active: Instant,
}

impl PoolSlot {
    /// Internal constructor used by tests and by the FFI-facing
    /// `deposit_new` path after the factory returns a fresh resource.
    pub fn new(resource: *mut c_void, owner: ThreadKey) -> Self {
        Self {
            resource,
            owner,
            last_active: Instant::now(),
        }
    }
}

// SAFETY: `resource` is an opaque owning pointer. Crossing a thread
// boundary with a `PoolSlot` (e.g. on release from a non-owner thread)
// is explicit and audited — the code paths that do so are guarded by
// `slot.owner` checks. Per the cross-thread fcc spike, PHP refcounted values
// referenced through a `zval*` are safe to hand between ZTS worker
// threads provided no single worker is executing user code on the same
// zval concurrently, which is upheld by the per-thread idle discipline.
unsafe impl Send for PoolSlot {}

// ─── Core state ────────────────────────────────────────────────────────

/// Rust-side storage for one `Shared\Pool` instance.
///
/// `size` is the authoritative capacity gauge (`in_use + idle` across
/// every thread). It is bumped atomically by `try_reserve_budget`
/// before a factory call, decremented by `release_budget` on factory
/// failure or destroy, and never touched by `try_acquire_local` /
/// `release` (which only shuffle slots between "held by a worker"
/// and "parked in the idle deque" — `size` stays constant either way).
pub struct PoolInner {
    max_size: usize,
    idle_timeout: Duration,

    /// Per-thread idle deques. Only the owning thread reads/writes
    /// its own deque (strict strategy — no cross-thread steal). A
    /// non-owner thread may push into the owner's deque via `release`
    /// — that's the one cross-thread write we permit, gated by
    /// `slot.owner`.
    idle: DashMap<ThreadKey, Mutex<VecDeque<PoolSlot>>>,

    /// Authoritative capacity gauge: `in_use + idle` across all threads.
    size: AtomicU64,

    /// Per-thread "currently checked out" counter. Incremented on
    /// every successful `acquire` (local-hit *and* fresh factory
    /// mint) and decremented on `release`. Used by the chaos-merge-
    /// gate budget reclaim hook: when a worker panics mid-acquire
    /// and its thread exits, `reclaim_budget_for_dead_worker` snapshots
    /// this counter for the dead key and refunds that many units of
    /// `size` so the budget gauge stays accurate across the worker's
    /// death + respawn cycle.
    in_flight: DashMap<ThreadKey, AtomicU64>,

    /// Waiter count (diagnostics + gauge). Protected by the Condvar
    /// mutex so waiters-cv wake-ups see a consistent count.
    waiters: Mutex<u64>,
    waiters_cv: Condvar,

    /// Factory/destroy callables. Owned by the PHP wrapper via explicit
    /// refcount; typed invocation lives in the FFI layer. `destroy_fcc`
    /// is nullable when the user passes `null` for `$destroy`.
    factory_fcc: *mut c_void,
    destroy_fcc: *mut c_void,

    closed: AtomicBool,
    self_id: OnceLock<SharedId>,

    // ── Observability counters ──────────────────────────────
    //
    // Exported as Prometheus `oxphp_shared_pool_*` series and surfaced
    // in the `/__ox_shared/entries/:id` JSON `type_specific` block.
    acquire_ok_total: AtomicU64,
    acquire_timeout_total: AtomicU64,
    acquire_closed_total: AtomicU64,
    evicted_idle_total: AtomicU64,
    evicted_manual_total: AtomicU64,
    evicted_shutdown_total: AtomicU64,
    /// Wait-time histogram. Non-cumulative bucket counts; cumulative
    /// values are computed at export. Buckets:
    ///   [0] ≤   1ms  [1] ≤ 10ms  [2] ≤ 100ms
    ///   [3] ≤   1s   [4] ≤ 10s   [5] >  10s (+Inf)
    wait_buckets: [AtomicU64; 6],
    /// Sum of observed wait-times in nanoseconds. Divided by 1e9 at
    /// export for the `_sum` series.
    wait_sum_nanos: AtomicU64,
    /// Total number of wait-time observations. Matches `_count`.
    wait_count: AtomicU64,
}

/// Reason label for `oxphp_shared_pool_evicted_total`. Drives the
/// bump between `evicted_idle_total` / `evicted_manual_total` /
/// `evicted_shutdown_total`.
#[derive(Clone, Copy)]
pub enum EvictReason {
    /// Scheduler-driven (`Shared\Pool idle-timeout eviction`).
    IdleTimeout,
    /// User-driven (`$pool->evict()` FFI call).
    Manual,
    /// Shutdown-drain (`SharedRegistry::drain` → `on_shutdown_notify`).
    Shutdown,
}

// SAFETY: factory_fcc/destroy_fcc are refcounted C-allocated fccs; the
// creating FFI path bumps PHP-side refs so the pointers outlive this
// struct. The spike above verified that these fccs are safe to invoke
// across ZTS worker threads via `zend_call_known_function`. All other
// fields are already Send+Sync (atomics, std::sync primitives, DashMap
// of Mutex<VecDeque<PoolSlot>> where PoolSlot is Send).
unsafe impl Send for PoolInner {}
unsafe impl Sync for PoolInner {}

impl PoolInner {
    pub fn new(
        factory_fcc: *mut c_void,
        destroy_fcc: *mut c_void,
        max_size: usize,
        idle_timeout: Duration,
    ) -> Self {
        debug_assert!(max_size > 0, "PoolInner::new: max_size must be > 0");
        Self {
            max_size,
            idle_timeout,
            idle: DashMap::new(),
            size: AtomicU64::new(0),
            in_flight: DashMap::new(),
            waiters: Mutex::new(0),
            waiters_cv: Condvar::new(),
            factory_fcc,
            destroy_fcc,
            closed: AtomicBool::new(false),
            self_id: OnceLock::new(),
            acquire_ok_total: AtomicU64::new(0),
            acquire_timeout_total: AtomicU64::new(0),
            acquire_closed_total: AtomicU64::new(0),
            evicted_idle_total: AtomicU64::new(0),
            evicted_manual_total: AtomicU64::new(0),
            evicted_shutdown_total: AtomicU64::new(0),
            wait_buckets: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            wait_sum_nanos: AtomicU64::new(0),
            wait_count: AtomicU64::new(0),
        }
    }

    pub fn bind_id(&self, id: SharedId) {
        let _ = self.self_id.set(id);
    }

    pub fn self_id(&self) -> Option<SharedId> {
        self.self_id.get().copied()
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }

    pub fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    /// Opaque factory handle. Invoked via `zend_call_known_function`.
    #[allow(dead_code)]
    pub(crate) fn factory_fcc(&self) -> *mut c_void {
        self.factory_fcc
    }

    /// Opaque destroy handle. May be null.
    #[allow(dead_code)]
    pub(crate) fn destroy_fcc(&self) -> *mut c_void {
        self.destroy_fcc
    }

    // ── acquire / release primitives ──────────────────────────────────

    /// Pop an idle slot from the calling thread's own deque. Returns
    /// `None` if the caller's deque is empty.
    ///
    /// Strict strategy: the caller cannot peek into another thread's
    /// idle list here, even if the budget is full. That is what makes
    /// thread-unsafe resources safe to pool.
    pub fn try_acquire_local(&self) -> Option<PoolSlot> {
        let me = current_thread_key();
        let shard = self.idle.get(&me)?;
        let mut deque = shard.lock().ok()?;
        deque.pop_front()
    }

    /// Attempt to reserve one slot in the global budget. Returns
    /// `true` if the caller is now responsible for either minting a
    /// resource (and calling `deposit_new`) or undoing the reservation
    /// via `release_budget`.
    ///
    /// Uses a CAS loop — a compare-and-swap on `size` — so concurrent
    /// reservations from multiple threads cannot breach `max_size`.
    pub fn try_reserve_budget(&self) -> bool {
        let max = self.max_size as u64;
        self.size
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |cur| {
                if cur < max {
                    Some(cur + 1)
                } else {
                    None
                }
            })
            .is_ok()
    }

    /// Undo a `try_reserve_budget`. Used when the PHP factory threw
    /// or a slot got destroyed out-of-band.
    pub fn release_budget(&self) {
        self.size.fetch_sub(1, Ordering::AcqRel);
        // Destroying a slot frees capacity — nudge one waiter.
        self.waiters_cv.notify_one();
    }

    /// Increment the current thread's in-flight counter. Called from
    /// the FFI `acquire` path immediately after a slot is handed to
    /// the caller — both the local-idle hit and the fresh factory-
    /// mint branches. The counter is the chaos-reclaim hook's view
    /// of "how many slots owned by thread K are currently checked
    /// out and would leak if K died".
    pub fn track_acquired_by_me(&self) {
        let me = current_thread_key();
        self.in_flight
            .entry(me)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement the in-flight counter for `owner`. Called from the
    /// FFI `release` path before the slot transitions to "parked"
    /// or "destroyed". Saturates at zero — double-untrack is a
    /// should-not-happen but we refuse to underflow the counter.
    pub fn untrack_released(&self, owner: ThreadKey) {
        if let Some(entry) = self.in_flight.get(&owner) {
            let prev = entry.load(Ordering::Acquire);
            if prev == 0 {
                return;
            }
            entry.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Chaos-reclaim hook. Atomically take the current in-flight count
    /// for `key`, refund that many units of budget via `release_budget`,
    /// and remove the key's entry from the map. Returns the number of
    /// reclaimed slots for diagnostics.
    ///
    /// Called from `worker_liveness::unregister_worker` on every
    /// worker-thread exit path (clean shutdown and panic unwind), so
    /// a worker that panicked while holding N slots refunds N budget
    /// units and the pool's `size` gauge stays accurate.
    pub fn reclaim_budget_for_dead_worker(&self, key: ThreadKey) -> u64 {
        let count = self
            .in_flight
            .remove(&key)
            .map(|(_, counter)| counter.load(Ordering::Acquire))
            .unwrap_or(0);
        for _ in 0..count {
            self.release_budget();
        }
        count
    }

    /// Park a freshly minted slot into the calling thread's idle
    /// deque. The caller must have already obtained budget via
    /// `try_reserve_budget`. Size stays constant: the slot is
    /// transitioning from "in-flight during factory call" to "idle".
    pub fn deposit_new(&self, slot: PoolSlot) {
        let me = current_thread_key();
        debug_assert_eq!(
            slot.owner, me,
            "deposit_new: slot.owner must match current thread"
        );
        self.idle
            .entry(me)
            .or_insert_with(|| Mutex::new(VecDeque::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(slot);
    }

    /// Return a checked-out slot to its owning thread's idle deque.
    /// Creates the owner's deque on demand.
    ///
    /// Three paths:
    ///
    /// * **Same-thread release (hot path).** `owner == current_thread_key()`
    ///   — park unconditionally, no liveness check (we are the owner,
    ///   by definition alive).
    /// * **Cross-thread release, owner alive.** Park in the owner's
    ///   deque; the owner will re-acquire it on its next request.
    /// * **Cross-thread release, owner gone.** Destroy the slot
    ///   inline via `destroy_slot`. Prevents the slot from parking
    ///   in a deque nobody will drain before shutdown.
    ///
    /// Liveness uses the explicit `worker_liveness` registry, NOT
    /// `pthread_kill(tid, 0)` — see that module for the pthread_t
    /// reuse hazard that motivates the DashSet-based design.
    ///
    /// Notifies one waiter on the normal park path: a released
    /// resource may unblock a worker parked on `wait_for_release`.
    pub fn release(&self, slot: PoolSlot) {
        let me = current_thread_key();
        let owner = slot.owner;

        if owner != me && !crate::plugins::ox_shared::worker_liveness::is_thread_alive(owner) {
            self.destroy_slot(slot);
            return;
        }

        let shard = self
            .idle
            .entry(owner)
            .or_insert_with(|| Mutex::new(VecDeque::new()));
        let mut deque = shard.lock().unwrap_or_else(|e| e.into_inner());
        // Reset the age clock: idle-timeout eviction measures time
        // since the slot was last in use, not since it was minted.
        let mut slot = slot;
        slot.last_active = Instant::now();
        deque.push_back(slot);
        drop(deque);
        drop(shard);
        self.waiters_cv.notify_one();
    }

    /// Return the set of owner `ThreadKey`s whose idle deque front is
    /// older than this pool's `idle_timeout`. Called by the central
    /// eviction scheduler to decide which workers to signal. Peeking
    /// the *front* is sufficient because slots in a deque are age-
    /// ordered — everything behind a non-stale front is also fresh.
    ///
    /// Takes a `now` argument so a single scan pass uses a consistent
    /// reference point across Pools.
    pub fn stale_owners(&self, now: Instant) -> Vec<ThreadKey> {
        let timeout = self.idle_timeout;
        let mut out = Vec::new();
        for entry in self.idle.iter() {
            let deque = entry
                .value()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(front) = deque.front() {
                if now.saturating_duration_since(front.last_active) >= timeout {
                    out.push(*entry.key());
                }
            }
        }
        out
    }

    /// Drain stale slots from the **current thread's** own idle deque.
    /// Called from `eviction::drain_stale_for_current_thread` inside
    /// the SAPI request frame after a positive `take_evict_request`.
    /// Runs `destroy_slot` on every slot whose `last_active` is older
    /// than `idle_timeout`, stopping at the first fresh slot (deques
    /// are age-ordered, front = oldest).
    pub fn evict_stale_on_current_thread(&self, reason: EvictReason) -> u64 {
        let me = current_thread_key();
        let timeout = self.idle_timeout;
        let Some(shard) = self.idle.get(&me) else {
            return 0;
        };
        let mut popped = Vec::new();
        {
            let mut deque = shard.lock().unwrap_or_else(|poison| poison.into_inner());
            let now = Instant::now();
            while let Some(front) = deque.front() {
                if now.saturating_duration_since(front.last_active) < timeout {
                    break;
                }
                // pop_front is cheap; do it before dropping the lock.
                popped.push(deque.pop_front().expect("front peek succeeded"));
            }
        }
        // Destroy outside the lock — `destroy_slot` calls
        // release_budget which notifies the waiter condvar and we
        // want no chance of deadlock.
        let count = popped.len() as u64;
        for slot in popped {
            self.destroy_slot(slot);
        }
        self.record_evicted(count, reason);
        count
    }

    /// Block the calling thread until another thread releases a slot
    /// or `remaining` elapses. Returns `true` if woken (possibly
    /// spuriously — caller must re-check conditions), `false` on
    /// timeout.
    ///
    /// Also wakes on `close()` via `notify_all`; the caller is
    /// expected to inspect `is_closed()` after waking.
    pub fn wait_for_release(&self, remaining: Duration) -> bool {
        let guard = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
        let mut guard = guard;
        *guard += 1;
        let (mut guard, result) = self
            .waiters_cv
            .wait_timeout(guard, remaining)
            .unwrap_or_else(|e| e.into_inner());
        *guard = guard.saturating_sub(1);
        !result.timed_out()
    }

    // ── lifecycle ─────────────────────────────────────────────────────

    /// Mark the pool closed. New `acquire` calls fail fast and every
    /// blocked waiter is woken so they can observe the closed state
    /// and bail. Idle resources are not drained here — that's the
    /// `on_shutdown_notify` / `on_drop` job.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.waiters_cv.notify_all();
    }

    /// Best-effort teardown of a single slot: invoke `$destroy` via
    /// the C bridge (which also `zval_ptr_dtor`s and `efree`s the
    /// slot zval), then account for the freed capacity.
    ///
    /// Called from `on_shutdown_notify` and `on_drop`. The bridge
    /// helper guards `EG` context internally and captures `$destroy`
    /// throws, so we never observe a failure path here — hence the
    /// `()` return.
    fn destroy_slot(&self, slot: PoolSlot) {
        // SAFETY: `slot.resource` was minted by `oxphp_pool_factory_invoke`
        // (or is `null_mut()` in host tests — the mock is a no-op, and
        // the production helper short-circuits on NULL). `destroy_fcc`
        // is either `null_mut()` (user opted out of $destroy) or the
        // emalloc'd fcc from `oxphp_pool_fcc_new`, kept alive for the
        // PoolInner's lifetime.
        unsafe { ffi::oxphp_pool_destroy_invoke(self.destroy_fcc, slot.resource) };
        self.release_budget();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    // ── observability helpers ─────────────────────────────────────────

    /// Authoritative capacity gauge: `in_use + idle`. This is what
    /// the PHP `size()` method returns.
    pub fn size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    /// Sum of idle-deque lengths across every thread. Approximate
    /// under concurrent writes — callers treat it as diagnostic,
    /// never as a correctness invariant.
    pub fn idle_count(&self) -> usize {
        self.idle
            .iter()
            .map(|e| {
                e.value()
                    .lock()
                    .map(|d| d.len())
                    .unwrap_or_else(|poison| poison.into_inner().len())
            })
            .sum()
    }

    /// `size - idle_count`, clamped at 0 under racy reads.
    pub fn in_use_count(&self) -> usize {
        let size = self.size() as usize;
        let idle = self.idle_count();
        size.saturating_sub(idle)
    }

    pub fn waiting_count(&self) -> u64 {
        *self.waiters.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Snapshot of idle counts per thread. Used by the `/entry`
    /// observability endpoint for the `idle_by_thread` JSON field.
    /// Keys are raw pthread ids (`ThreadKey`).
    pub fn idle_by_thread(&self) -> Vec<(ThreadKey, usize)> {
        self.idle
            .iter()
            .map(|e| {
                let n = e
                    .value()
                    .lock()
                    .map(|d| d.len())
                    .unwrap_or_else(|poison| poison.into_inner().len());
                (*e.key(), n)
            })
            .collect()
    }

    // ── metric recorders ────────────────────────────────────

    #[inline]
    pub fn record_acquire_ok(&self) {
        self.acquire_ok_total.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_acquire_timeout(&self) {
        self.acquire_timeout_total.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_acquire_closed(&self) {
        self.acquire_closed_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an acquire wait observation into the histogram. Called
    /// with `dur = Duration::ZERO` for uncontended acquires (the
    /// ≤1ms bucket absorbs those).
    pub fn record_wait(&self, dur: Duration) {
        let nanos = dur.as_nanos().min(u64::MAX as u128) as u64;
        self.wait_sum_nanos.fetch_add(nanos, Ordering::Relaxed);
        self.wait_count.fetch_add(1, Ordering::Relaxed);
        // Bucket thresholds in nanoseconds: 1ms, 10ms, 100ms, 1s, 10s.
        // `wait_buckets[5]` is the +Inf overflow (> 10s).
        let idx = if nanos <= 1_000_000 {
            0
        } else if nanos <= 10_000_000 {
            1
        } else if nanos <= 100_000_000 {
            2
        } else if nanos <= 1_000_000_000 {
            3
        } else if nanos <= 10_000_000_000 {
            4
        } else {
            5
        };
        self.wait_buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_evicted(&self, count: u64, reason: EvictReason) {
        if count == 0 {
            return;
        }
        match reason {
            EvictReason::IdleTimeout => {
                self.evicted_idle_total.fetch_add(count, Ordering::Relaxed);
            }
            EvictReason::Manual => {
                self.evicted_manual_total
                    .fetch_add(count, Ordering::Relaxed);
            }
            EvictReason::Shutdown => {
                self.evicted_shutdown_total
                    .fetch_add(count, Ordering::Relaxed);
            }
        }
    }

    // ── metric getters ──────────────────────────────────────

    pub fn acquire_ok_total(&self) -> u64 {
        self.acquire_ok_total.load(Ordering::Relaxed)
    }
    pub fn acquire_timeout_total(&self) -> u64 {
        self.acquire_timeout_total.load(Ordering::Relaxed)
    }
    pub fn acquire_closed_total(&self) -> u64 {
        self.acquire_closed_total.load(Ordering::Relaxed)
    }
    pub fn evicted_idle_total(&self) -> u64 {
        self.evicted_idle_total.load(Ordering::Relaxed)
    }
    pub fn evicted_manual_total(&self) -> u64 {
        self.evicted_manual_total.load(Ordering::Relaxed)
    }
    pub fn evicted_shutdown_total(&self) -> u64 {
        self.evicted_shutdown_total.load(Ordering::Relaxed)
    }

    /// Snapshot the wait histogram: cumulative bucket counts matching
    /// the Prometheus le= ordering, plus sum-in-seconds and count.
    /// Bucket ceilings: [0.001, 0.01, 0.1, 1.0, 10.0, +Inf].
    pub fn wait_histogram_snapshot(&self) -> ([u64; 6], f64, u64) {
        let raw: [u64; 6] = [
            self.wait_buckets[0].load(Ordering::Relaxed),
            self.wait_buckets[1].load(Ordering::Relaxed),
            self.wait_buckets[2].load(Ordering::Relaxed),
            self.wait_buckets[3].load(Ordering::Relaxed),
            self.wait_buckets[4].load(Ordering::Relaxed),
            self.wait_buckets[5].load(Ordering::Relaxed),
        ];
        let mut cumulative = [0u64; 6];
        let mut running = 0u64;
        for (i, v) in raw.iter().enumerate() {
            running = running.saturating_add(*v);
            cumulative[i] = running;
        }
        let sum_s = self.wait_sum_nanos.load(Ordering::Relaxed) as f64 / 1e9;
        let count = self.wait_count.load(Ordering::Relaxed);
        (cumulative, sum_s, count)
    }
}

/// Worker-exit hook: iterate every Pool in the `SharedRegistry` and
/// reclaim budget for slots the dead worker was holding in-flight.
/// Called from `worker_liveness::unregister_worker` on both clean
/// shutdown and panic-unwind exit paths, so a panicking worker's
/// leaked budget is refunded before the next worker respawns.
pub fn reclaim_all_pools_for_dead_worker(key: ThreadKey) {
    let Some(reg) = crate::plugins::ox_shared::registry::REGISTRY.get() else {
        return;
    };
    for entry in reg.iter_entries() {
        if let Some(pool) = entry.inner.as_any_pool() {
            pool.reclaim_budget_for_dead_worker(key);
        }
    }
}

impl SharedInner for PoolInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Pool
    }

    fn debug_snapshot(&self) -> SharedValue {
        // Matches MapInner's pattern: expose the primary gauge as
        // Long. /entry surfaces it as `type_specific.size`.
        SharedValue::Long(self.size() as i64)
    }

    fn mem_bytes(&self) -> usize {
        // Approximate per 26-type-pool.md §Observability. Base struct
        // overhead + per-slot ~64B (VecDeque slot + PoolSlot). The
        // underlying PHP resource memory is accounted by the PHP
        // allocator, not here.
        let slots = self.size() as usize;
        128 + slots * 64
    }

    fn on_drop(&self) {
        // Registry evicted this Pool (last SharedId ref released) or
        // a worker is tearing down the pool directly. Drain idle
        // resources first (destroy each slot best-effort), then free
        // the factory/destroy fccs.
        //
        // Runs on whichever thread triggered the drop. That thread
        // may or may not be inside a PHP request frame; the C bridge
        // helper (`oxphp_pool_destroy_invoke` / `oxphp_pool_fcc_free`)
        // guards against a missing `EG(current_execute_data)` by
        // skipping PHP-touching ops and only doing the refcount
        // arithmetic that is always safe.
        self.on_shutdown_notify();

        // SAFETY: `factory_fcc` was emalloc'd by `oxphp_pool_fcc_new`
        // when the pool was created and has not been touched since.
        // `destroy_fcc` is either NULL (helper short-circuits) or an
        // emalloc'd fcc from the same path. Both are freed exactly
        // once here — PoolInner is not cloned and `on_drop` runs at
        // most once per instance.
        if !self.factory_fcc.is_null() {
            unsafe { ffi::oxphp_pool_fcc_free(self.factory_fcc) };
        }
        if !self.destroy_fcc.is_null() {
            unsafe { ffi::oxphp_pool_fcc_free(self.destroy_fcc) };
        }
    }

    fn on_shutdown_notify(&self) {
        // `SharedRegistry::drain` calls this on every entry during
        // graceful shutdown. Close the pool first so new acquires
        // fail fast and blocked waiters wake and bail, then walk
        // every idle deque and destroy each slot.
        //
        // Safe to invoke redundantly — a second call sees empty
        // deques and exits without touching the fccs (those are
        // freed exclusively by `on_drop`).
        self.close();

        let mut drained: u64 = 0;
        for entry in self.idle.iter() {
            let mut deque = entry
                .value()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            while let Some(slot) = deque.pop_front() {
                // Drop the mutex briefly is not strictly required
                // (we own the shard exclusively during drain), but
                // destroy_slot calls release_budget which notifies
                // the waiter condvar — harmless re-entry, no lock.
                self.destroy_slot(slot);
                drained += 1;
            }
        }
        self.record_evicted(drained, EvictReason::Shutdown);
    }

    fn children(&self, _out: &mut Vec<SharedRef>) {
        // Pool resources are raw zvals (arbitrary PHP objects), not
        // SharedValue::Shared. They never participate in the Map
        // cycle-detection graph, so there's nothing to expose here.
    }
}

// Helper trait for downcasting `Arc<dyn SharedInner>` to `&PoolInner`.
// Mirrors the `SharedInnerChannelExt` / `SharedInnerMapExt` pattern so
// the FFI can resolve a PoolId → &PoolInner without juggling
// `Any`. Implemented on `dyn SharedInner` (bare, no `+ Send + Sync`)
// to match the actual trait-object type stored in `Entry.inner`.
pub trait SharedInnerPoolExt {
    fn as_any_pool(&self) -> Option<&PoolInner>;
}

impl SharedInnerPoolExt for dyn SharedInner {
    fn as_any_pool(&self) -> Option<&PoolInner> {
        if self.type_tag() == SharedType::Pool {
            // SAFETY: SharedType::Pool guarantees the concrete type
            // stored behind this `dyn SharedInner` is `PoolInner`.
            Some(unsafe { &*(self as *const dyn SharedInner as *const PoolInner) })
        } else {
            None
        }
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────
//
// PHP-facing surface for `Shared\Pool`. Called from the Pool method
// handlers after unwrapping arguments. The boundary is kept narrow:
// Rust owns the bookkeeping, C bridge owns `zval`/`fcc` memory via
// `ext/bridge/oxphp_bridge.c §Shared\Pool helpers`.
//
// Ownership summary:
//
// * `factory_callable_zval` / `destroy_callable_zval` on create: C
//   allocates a `oxphp_pool_fcc_t` (ZVAL_COPY'd callable + cached fcc)
//   and returns its heap pointer; Rust stores that pointer in
//   `PoolInner.factory_fcc` / `.destroy_fcc`. v1 leaks these on pool
//   drop.
// * Per-resource `slot_zv_heap`: C allocates at
//   `oxphp_pool_factory_invoke` time (ZVAL_COPY of the factory's
//   object return). Pool is the sole owner. `pool_acquire` hands the
//   heap pointer to the caller, which wraps it into a
//   `Shared\Pool\Handle`. `pool_release` takes the heap pointer
//   back and parks the slot in the owner thread's idle deque.
// * `owner_tid`: pthread id captured at mint time; stored alongside
//   the slot in the Handle's `rust_data` slot. Used by
//   `pool_release` to route the slot to the correct idle deque.
//
// PHP exceptions thrown by the user's factory/body propagate via
// `EG(exception)`. The FFI returns `SharedError::Generic` in that
// case; the Pool method handlers check
// `Z_TYPE(EG(exception)) != IS_UNDEF` before throwing a Shared\*
// exception themselves to avoid double-exception.

use std::os::raw::c_int;
use std::sync::Arc;

use crate::bridge::ffi;
use crate::plugins::ox_shared::error::{ffi_entry, set_last_error};
use crate::plugins::ox_shared::registry::registry;

/// Resolve an id to `&PoolInner`. Returns `Err(Type)` if the id
/// belongs to a different Shared type. The returned `Arc<Entry>`
/// keeps the inner alive for the caller's borrow lifetime.
fn lookup_pool(
    id: SharedId,
) -> Result<
    (
        &'static crate::plugins::ox_shared::registry::SharedRegistry,
        Arc<crate::plugins::ox_shared::registry::Entry>,
    ),
    crate::plugins::ox_shared::error::SharedError,
> {
    let reg = registry();
    let entry = reg.lookup(id)?;
    if entry.type_tag != SharedType::Pool {
        set_last_error(format!(
            "id {id} is not a Shared\\Pool (tag={:?})",
            entry.type_tag
        ));
        return Err(crate::plugins::ox_shared::error::SharedError::Type);
    }
    Ok((reg, entry))
}

/// Create a new `Shared\Pool`. The factory callable is captured via
/// the C bridge (`oxphp_pool_fcc_new`) into a heap-allocated fcc slot
/// and stored opaquely on the PoolInner. `destroy_callable_zval` may
/// be null.
///
/// `idle_timeout_s` ≤ 0 falls back to 300 seconds.
///
/// # Safety
/// `factory_callable_zval` must be a valid PHP callable zval; the
/// C bridge checks via `zend_fcall_info_init`. `out_id` must be
/// valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_create(
    factory_callable_zval: *mut std::ffi::c_void,
    destroy_callable_zval: *mut std::ffi::c_void,
    max_size: u64,
    idle_timeout_s: f64,
    out_id: *mut u64,
) -> c_int {
    if out_id.is_null() {
        set_last_error("out_id is null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if factory_callable_zval.is_null() {
        set_last_error("factory callable is null");
        return crate::plugins::ox_shared::error::SharedError::Type.code();
    }
    if max_size == 0 {
        set_last_error("max_size must be > 0");
        return crate::plugins::ox_shared::error::SharedError::Type.code();
    }

    ffi_entry(|| {
        let mut factory_fcc: *mut std::ffi::c_void = std::ptr::null_mut();
        let rc = unsafe { ffi::oxphp_pool_fcc_new(factory_callable_zval, &mut factory_fcc) };
        if rc != 0 {
            set_last_error("factory argument is not a valid PHP callable");
            return Err(crate::plugins::ox_shared::error::SharedError::Type);
        }

        let mut destroy_fcc: *mut std::ffi::c_void = std::ptr::null_mut();
        if !destroy_callable_zval.is_null() {
            let rc = unsafe { ffi::oxphp_pool_fcc_new(destroy_callable_zval, &mut destroy_fcc) };
            if rc != 0 {
                unsafe { ffi::oxphp_pool_fcc_free(factory_fcc) };
                set_last_error("destroy argument is not a valid PHP callable");
                return Err(crate::plugins::ox_shared::error::SharedError::Type);
            }
        }

        let idle_timeout = if idle_timeout_s > 0.0 {
            Duration::from_secs_f64(idle_timeout_s)
        } else {
            Duration::from_secs(300)
        };

        let inner: Arc<dyn SharedInner> = Arc::new(PoolInner::new(
            factory_fcc,
            destroy_fcc,
            max_size as usize,
            idle_timeout,
        ));
        let reg = registry();
        let id = reg.insert(SharedType::Pool, Arc::clone(&inner))?;
        (*inner)
            .as_any_pool()
            .expect("just inserted PoolInner")
            .bind_id(id);
        unsafe { *out_id = id };
        Ok(())
    })
}

/// Acquire a resource from the pool. On success, writes the pool-owned
/// `slot_zv_heap` pointer and the pthread-id of the owning thread; the
/// caller (`Shared\Pool::acquire`) wraps these into a
/// `Shared\Pool\Handle` whose `rust_data` slot carries both.
///
/// Flow:
/// 1. `try_acquire_local` from the caller's own idle deque.
/// 2. `try_reserve_budget` → on success call the C bridge factory;
///    refund + propagate on throw.
/// 3. Budget full → `wait_for_release` up to the deadline, then loop.
///
/// `timeout_ms`: -1 = wait forever, 0 = try only, >0 = milliseconds.
///
/// # Safety
/// `out_slot_zv_heap` and `out_owner_tid` must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_acquire(
    id: u64,
    timeout_ms: i64,
    out_slot_zv_heap: *mut *mut std::ffi::c_void,
    out_owner_tid: *mut u64,
) -> c_int {
    if out_slot_zv_heap.is_null() || out_owner_tid.is_null() {
        set_last_error("out params must be non-null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    unsafe {
        *out_slot_zv_heap = std::ptr::null_mut();
        *out_owner_tid = 0;
    }

    ffi_entry(|| {
        let (reg, entry) = lookup_pool(id)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;

        let call_start = Instant::now();
        let (deadline_opt, try_only) = match parse_timeout(timeout_ms) {
            Wait::Forever => (None, false),
            Wait::Try => (Some(call_start), true),
            Wait::Bounded(d) => (
                Some(
                    call_start
                        .checked_add(d)
                        .unwrap_or_else(|| call_start + Duration::from_secs(86_400)),
                ),
                false,
            ),
        };

        // Wrapped loop so every exit path records the acquire result
        // + wait-histogram observation at one place. The histogram
        // sees every acquire, including uncontended (wait ≈ 0); the
        // result counter covers ok / timeout / closed. Factory failures
        // (Generic / Type) are *not* acquire outcomes per spec —
        // those live in the error surface, not the counter labels.
        let result: Result<(), crate::plugins::ox_shared::error::SharedError> = 'acquire: loop {
            if pool.is_closed() {
                break 'acquire Err(crate::plugins::ox_shared::error::SharedError::Closed);
            }

            // 1. Local idle.
            if let Some(slot) = pool.try_acquire_local() {
                reg.record_op(id);
                unsafe {
                    *out_slot_zv_heap = slot.resource;
                    *out_owner_tid = slot.owner;
                }
                // Chaos-reclaim bookkeeping: the caller now owns a
                // slot attributed to us. Decremented by the matching
                // `untrack_released` in `oxphp_shared_pool_release`,
                // or reclaimed by `reclaim_budget_for_dead_worker`
                // if this thread dies before releasing.
                pool.track_acquired_by_me();
                break 'acquire Ok(());
            }

            // 2. Budget for a fresh factory invocation.
            if pool.try_reserve_budget() {
                let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
                let rc =
                    unsafe { ffi::oxphp_pool_factory_invoke(pool.factory_fcc(), &mut slot_heap) };
                if rc == 0 {
                    reg.record_op(id);
                    unsafe {
                        *out_slot_zv_heap = slot_heap;
                        *out_owner_tid = current_thread_key();
                    }
                    pool.track_acquired_by_me();
                    break 'acquire Ok(());
                }
                // Factory did not mint a resource — refund capacity
                // so other waiters can try, then surface the error.
                // Skip the wait histogram for factory failures: they
                // are not acquire outcomes, they are upstream errors
                // that propagate via `SharedError::{Generic, Type}`.
                pool.release_budget();
                let err = if rc == -1 {
                    set_last_error("Shared\\Pool factory threw (see EG(exception))");
                    crate::plugins::ox_shared::error::SharedError::Generic
                } else if rc == -2 {
                    set_last_error(
                        "Shared\\Pool factory must return an object \
                         (v1 does not support scalar/array resources)",
                    );
                    crate::plugins::ox_shared::error::SharedError::Type
                } else {
                    set_last_error(format!("factory invocation failed with rc={rc}"));
                    crate::plugins::ox_shared::error::SharedError::Generic
                };
                return Err(err);
            }

            // 3. Budget full — check try-only or compute remaining wait.
            if try_only {
                break 'acquire Err(crate::plugins::ox_shared::error::SharedError::Timeout);
            }

            let remaining = match deadline_opt {
                None => {
                    // Wait::Forever: poll with a 50ms quantum so close-
                    // detection can progress without a busy-spin.
                    Duration::from_millis(50)
                }
                Some(t) => {
                    let now = Instant::now();
                    if now >= t {
                        break 'acquire Err(crate::plugins::ox_shared::error::SharedError::Timeout);
                    }
                    t - now
                }
            };
            pool.wait_for_release(remaining);
            // Loop back; the top-of-loop `is_closed` check converts
            // a close-during-wait wake-up into a `Closed` result.
        };

        // Record wait + outcome in one place.
        pool.record_wait(call_start.elapsed());
        match &result {
            Ok(()) => pool.record_acquire_ok(),
            Err(crate::plugins::ox_shared::error::SharedError::Timeout) => {
                pool.record_acquire_timeout()
            }
            Err(crate::plugins::ox_shared::error::SharedError::Closed) => {
                pool.record_acquire_closed()
            }
            Err(_) => { /* unreachable: factory errors return early */ }
        }
        result
    })
}

/// Return a resource to the pool. `slot_zv_heap` is the pointer
/// previously handed out by `oxphp_shared_pool_acquire`. `owner_tid`
/// routes the slot to the correct idle deque — callers extract this
/// from the Handle wrapper's `rust_data`.
///
/// v1 contract: release always succeeds as long as the pool id is
/// alive and the slot pointer is non-null. If the owner's deque
/// does not yet exist (factory-minted slot never deposited, or
/// owner thread terminated) it is created lazily. The shutdown-walker
/// destroys orphaned deques during drain.
///
/// # Safety
/// `slot_zv_heap` must be a pointer that originated from this same
/// pool's acquire path (not forged, not from a different pool).
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_release(
    id: u64,
    slot_zv_heap: *mut std::ffi::c_void,
    owner_tid: u64,
) -> c_int {
    if slot_zv_heap.is_null() {
        set_last_error("slot_zv_heap is null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }

    ffi_entry(|| {
        let (reg, entry) = lookup_pool(id)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;

        let slot = PoolSlot::new(slot_zv_heap, owner_tid);
        // Pair the release with the in-flight decrement so
        // `reclaim_budget_for_dead_worker` does not double-count a
        // cleanly released slot. Attributed to `owner_tid` (not the
        // current thread) — cross-thread release crediting still
        // lands on the original minting thread.
        pool.untrack_released(owner_tid);
        pool.release(slot);
        reg.record_op(id);
        Ok(())
    })
}

/// Convenience: acquire, invoke `body($resource)`, release — even on
/// body throw. On success `user_out_zv` receives the body's return
/// via `ZVAL_COPY`. On body throw the FFI leaves `EG(exception)` set
/// and returns `Generic`, but the slot still returns to the pool.
///
/// `timeout_ms`: -1 = wait forever, 0 = try only, >0 = milliseconds.
///
/// # Safety
/// `body_callable_zv` and `user_out_zv` must be valid PHP zvals.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_with(
    id: u64,
    timeout_ms: i64,
    body_callable_zv: *mut std::ffi::c_void,
    user_out_zv: *mut std::ffi::c_void,
) -> c_int {
    if body_callable_zv.is_null() || user_out_zv.is_null() {
        set_last_error("body/out must be non-null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }

    // Step 1: acquire. Bubble errors directly.
    let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut owner: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_acquire(id, timeout_ms, &mut slot_heap, &mut owner) };
    if rc != 0 {
        return rc;
    }

    // Step 2: body($resource). Always attempt release afterwards.
    let body_rc = unsafe { ffi::oxphp_pool_body_invoke(body_callable_zv, slot_heap, user_out_zv) };

    // Step 3: release — never skipped. `with()`'s contract is
    // resource-safe even on body throw.
    let release_rc = unsafe { oxphp_shared_pool_release(id, slot_heap, owner) };

    if body_rc == -2 {
        // Body threw; EG(exception) is set. Caller must propagate.
        if release_rc != 0 {
            // Don't overwrite the PHP exception, but record the warning.
            set_last_error(
                "body threw AND release failed — slot leaked; \
                 see EG(exception) for the body throw",
            );
        }
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if body_rc == -1 {
        set_last_error("body is not a valid PHP callable");
        return crate::plugins::ox_shared::error::SharedError::Type.code();
    }
    // Body succeeded. Surface any release-time error.
    release_rc
}

/// Idle-timeout eviction. v1 stub: returns 0 evicted. The background
/// walker (see `eviction.rs`) destroys resources idle for longer
/// than `idle_timeout`.
///
/// # Safety
/// `out_evicted` must be valid for a `u64` write if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_evict(id: u64, out_evicted: *mut u64) -> c_int {
    ffi_entry(|| {
        let (_reg, entry) = lookup_pool(id)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;
        // User-driven synchronous counterpart to the background
        // scheduler's request_evict flag. Destroy runs on the caller's
        // thread, which is the owner of the slots we're reaping — no
        // cross-thread hazard. Same constraint as Shared\Pool::acquire:
        // callers must be inside a live PHP request.
        let evicted = pool.evict_stale_on_current_thread(EvictReason::Manual);
        if !out_evicted.is_null() {
            unsafe { *out_evicted = evicted };
        }
        Ok(())
    })
}

/// Read the three gauge counters the PHP API exposes via
/// `$pool->inUse() / ->idle() / ->waiting()`.
///
/// # Safety
/// Each non-null out pointer must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_stats(
    id: u64,
    out_in_use: *mut u64,
    out_idle: *mut u64,
    out_waiting: *mut u64,
) -> c_int {
    ffi_entry(|| {
        let (_reg, entry) = lookup_pool(id)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;
        if !out_in_use.is_null() {
            unsafe { *out_in_use = pool.in_use_count() as u64 };
        }
        if !out_idle.is_null() {
            unsafe { *out_idle = pool.idle_count() as u64 };
        }
        if !out_waiting.is_null() {
            unsafe { *out_waiting = pool.waiting_count() };
        }
        Ok(())
    })
}

/// `$pool->size()` — `in_use + idle` across every thread.
///
/// # Safety
/// `out_size` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_size(id: u64, out_size: *mut u64) -> c_int {
    if out_size.is_null() {
        set_last_error("out_size is null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    ffi_entry(|| {
        let (_reg, entry) = lookup_pool(id)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;
        unsafe { *out_size = pool.size() };
        Ok(())
    })
}

/// `$pool->maxSize()` equivalent. Read-only after construction.
///
/// # Safety
/// `out_max` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_max_size(id: u64, out_max: *mut u64) -> c_int {
    if out_max.is_null() {
        set_last_error("out_max is null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    ffi_entry(|| {
        let (_reg, entry) = lookup_pool(id)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;
        unsafe { *out_max = pool.max_size() as u64 };
        Ok(())
    })
}

// ─── PHP class registration ───────────────────────────────────────────
//
// Two classes land here: `OxPHP\Shared\Pool` (user-facing) and
// `OxPHP\Shared\Pool\Handle` (returned from `acquire()`, consumed by
// `release()`). Handle's storage layout mirrors the one read/written
// by the C bridge helpers in §Shared\Pool\Handle rust_data wrapper
// helpers — see `oxphp_bridge.c` for the canonical offsets.

use crate::bridge::call::NativeCall;
use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::error::{read_last_error_message, SharedError};

/// Custom storage attached to every `Shared\Pool\Handle` instance via
/// `with_storage`. Layout is `#[repr(C)]` so the C bridge helpers
/// (`oxphp_shared_pool_handle_{alloc,read,clear}`) can read/write
/// fields at fixed offsets.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct PoolHandleStorage {
    pub pool_id: SharedId,
    pub owner_tid: ThreadKey,
    /// NULL means "already released" (explicit release zeros this);
    /// Drop then becomes a no-op.
    pub slot_zv_ptr: *mut c_void,
}

impl PoolHandleStorage {
    fn default_storage() -> Self {
        Self {
            pool_id: 0,
            owner_tid: 0,
            slot_zv_ptr: std::ptr::null_mut(),
        }
    }
}

// SAFETY: `slot_zv_ptr` is an opaque owning pointer whose lifetime
// follows the same per-thread-affinity discipline as `PoolInner`'s
// idle deques. Cross-thread access is explicit (Drop auto-release
// may run off the owner thread) and routes through `PoolInner::release`
// which is designed for exactly that case.
unsafe impl Send for PoolHandleStorage {}
unsafe impl Sync for PoolHandleStorage {}

impl Drop for PoolHandleStorage {
    fn drop(&mut self) {
        // No-op if the storage was explicitly cleared by release() —
        // slot_zv_ptr is zeroed via the C bridge's handle_clear.
        if self.slot_zv_ptr.is_null() {
            return;
        }
        // Best-effort auto-release: user let the Handle fall out of
        // scope without calling release(). Route the slot back to the
        // owner's idle deque. If the pool was evicted or the owner's
        // deque is gone, we silently leak — consistent with v1's
        // drop-leak policy (see 99-deferred.md).
        if let Some(reg) = crate::plugins::ox_shared::registry::REGISTRY.get() {
            if let Ok(entry) = reg.lookup(self.pool_id) {
                if let Some(pool) = (*entry.inner).as_any_pool() {
                    let slot = PoolSlot::new(self.slot_zv_ptr, self.owner_tid);
                    pool.release(slot);
                }
            }
        }
    }
}

/// Map an FFI status code to the appropriate `Shared\*Exception`.
/// Special-cases `Generic` (-1) — used by the pool FFI to signal
/// "PHP callable threw; EG(exception) already set" — by returning
/// `PhpError::Custom` so the framework doesn't stomp the user's
/// exception. Same pattern as `Mutex::with`.
fn pool_rc_to_phperr(rc: std::os::raw::c_int, context: &str) -> PhpError {
    if rc == SharedError::Generic.code() {
        // Generic (-1) signals either "PHP callable threw" (EG(exception)
        // already set — we must not stomp it with zend_throw_exception)
        // or an internal error with a thread-local last-error message.
        // PhpError::Custom lets the framework propagate without throwing
        // a fresh exception; the message is included so debug logs show
        // what actually happened.
        let msg = read_last_error_message();
        return PhpError::Custom(format!("{context}: {msg}"));
    }
    let class = match rc {
        -2 => "OxPHP\\Shared\\StaleHandleException",
        -3 => "OxPHP\\Shared\\TypeException",
        -4 => "OxPHP\\Shared\\CapacityException",
        -6 => "OxPHP\\Shared\\ClosedException",
        -7 => "OxPHP\\Shared\\TimeoutException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        _ => "OxPHP\\Shared\\SharedException",
    };
    PhpError::Exception {
        class: class.to_string(),
        message: read_last_error_message(),
        code: 0,
    }
}

fn throw_clone_forbidden() -> PhpError {
    PhpError::Exception {
        class: "OxPHP\\Shared\\SharedException".to_string(),
        message: "Shared instances cannot be cloned. Use cross-thread \
                  transfer via oxphp_async(fn() use ($this) {...}) for \
                  sharing, or explicitly create a new instance for an \
                  independent copy."
            .to_string(),
        code: 0,
    }
}

/// Register `OxPHP\Shared\Pool` and `OxPHP\Shared\Pool\Handle`.
/// Called from the plugin init; mirrors the pattern other Shared\*
/// types (Counter, Map, Channel) use.
pub fn register_classes(ctx: &mut PluginContext) -> Result<(), PluginError> {
    register_handle_class(ctx)?;
    register_pool_class(ctx)?;
    Ok(())
}

fn register_handle_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Pool\\Handle")
        .with_storage(PoolHandleStorage::default_storage)
        .magic(MagicMethod::Clone)
        .handler(|_call| Err(throw_clone_forbidden()))
        // `get()` returns a ZVAL_COPY of the pool-owned resource.
        // The Handle retains its own slot reference; Drop / explicit
        // release decides when it actually goes back to idle.
        .method("get")
        .returns(PhpType::Mixed)
        .handler(handle_get)
        .build()?;
    Ok(())
}

fn handle_get(call: &mut NativeCall) -> Result<(), PhpError> {
    let storage = call.storage::<PoolHandleStorage>()?;
    if storage.slot_zv_ptr.is_null() {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\SharedException".to_string(),
            message: "Shared\\Pool\\Handle::get(): handle already released".to_string(),
            code: 0,
        });
    }
    // SAFETY: retval_ptr is valid for the handler's lifetime;
    // slot_zv_ptr was populated by pool_acquire via the C bridge
    // and stays alive until the Handle's Drop (or explicit release).
    unsafe {
        ffi::oxphp_pool_slot_to_user(storage.slot_zv_ptr, call.retval_ptr());
    }
    Ok(())
}

fn register_pool_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Pool")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| crate::plugins::ox_shared::handle::SharedHandle::new(SharedType::Pool))
        .magic(MagicMethod::Clone)
        .handler(|_call| Err(throw_clone_forbidden()))
        // __construct(callable $factory, ?callable $destroy = null,
        //             int $maxSize = 32, float $idleTimeout = 300.0,
        //             ?float $defaultAcquireTimeout = null)
        .method("__construct")
        .param("factory", PhpType::Callable)
        .optional_param("destroy", PhpType::Callable, PhpValue::Null)
        .optional_param("maxSize", PhpType::Int, PhpValue::Int(32))
        .optional_param("idleTimeout", PhpType::Float, PhpValue::Float(300.0))
        .handler(pool_construct)
        // acquire(?float $timeout = null): Shared\Pool\Handle
        .method("acquire")
        .optional_param("timeout", PhpType::Float, PhpValue::Null)
        .returns(PhpType::Object)
        .handler(pool_acquire)
        // release(Shared\Pool\Handle $handle): void
        .method("release")
        .param("handle", PhpType::Object)
        .returns(PhpType::Void)
        .handler(pool_release)
        // with(callable $body, ?float $timeout = null): mixed
        .method("with")
        .param("body", PhpType::Callable)
        .optional_param("timeout", PhpType::Float, PhpValue::Null)
        .returns(PhpType::Mixed)
        .handler(pool_with)
        // evict(): int — v1 stub returning 0
        .method("evict")
        .returns(PhpType::Int)
        .handler(pool_evict)
        // size(): int
        .method("size")
        .returns(PhpType::Int)
        .handler(pool_size)
        // inUse(): int
        .method("inUse")
        .returns(PhpType::Int)
        .handler(pool_in_use)
        // idle(): int
        .method("idle")
        .returns(PhpType::Int)
        .handler(pool_idle)
        // waiting(): int
        .method("waiting")
        .returns(PhpType::Int)
        .handler(pool_waiting)
        // maxSize(): int
        .method("maxSize")
        .returns(PhpType::Int)
        .handler(pool_max_size_method)
        // id(): int — registry id, mirrors other Shared\* types
        .method("id")
        .returns(PhpType::Int)
        .handler(pool_id_method)
        .build()?;
    Ok(())
}

fn pool_construct(call: &mut NativeCall) -> Result<(), PhpError> {
    // factory (required).
    let factory_zv = unsafe { call.raw_arg_ptr(0) };
    if factory_zv.is_null() {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: "factory callable is required".to_string(),
            code: 0,
        });
    }
    // destroy (optional, may be PHP null → null zval).
    let destroy_zv = if call.argc() > 1 {
        let p = unsafe { call.raw_arg_ptr(1) };
        if call.arg_is_null(1).unwrap_or(false) {
            std::ptr::null_mut()
        } else {
            p
        }
    } else {
        std::ptr::null_mut()
    };

    let max_size = if call.argc() > 2 {
        call.arg_long(2).unwrap_or(32)
    } else {
        32
    };
    if max_size <= 0 {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: "maxSize must be > 0".to_string(),
            code: 0,
        });
    }

    let idle_timeout_s = if call.argc() > 3 {
        call.arg_double(3).unwrap_or(300.0)
    } else {
        300.0
    };

    let mut out_id: u64 = 0;
    let rc = unsafe {
        oxphp_shared_pool_create(
            factory_zv,
            destroy_zv,
            max_size as u64,
            idle_timeout_s,
            &mut out_id,
        )
    };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::__construct"));
    }

    let handle = call.storage_mut::<crate::plugins::ox_shared::handle::SharedHandle>()?;
    handle.shared_id = out_id;
    handle.type_tag = SharedType::Pool as u8;
    Ok(())
}

fn pool_get_id(call: &NativeCall) -> Result<u64, PhpError> {
    let handle = call.storage::<crate::plugins::ox_shared::handle::SharedHandle>()?;
    if !handle.is_initialized() {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\UninitializedException".to_string(),
            message: "Shared\\Pool wrapper is uninitialised".to_string(),
            code: 0,
        });
    }
    Ok(handle.shared_id)
}

fn pool_acquire(call: &mut NativeCall) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    let timeout_ms: i64 = read_timeout_arg(call, 0)?;

    let mut slot_heap: *mut c_void = std::ptr::null_mut();
    let mut owner: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_acquire(id, timeout_ms, &mut slot_heap, &mut owner) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::acquire"));
    }

    // Wrap (slot_heap, owner) in a Shared\Pool\Handle. The C bridge
    // allocates the object into retval and populates rust_data.
    let rc_alloc =
        unsafe { ffi::oxphp_shared_pool_handle_alloc(call.retval_ptr(), id, owner, slot_heap) };
    if rc_alloc != 0 {
        // Could not construct the Handle — route the slot back so
        // we don't leak it, then surface the error.
        let _ = unsafe { oxphp_shared_pool_release(id, slot_heap, owner) };
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\SharedException".to_string(),
            message: "Shared\\Pool::acquire: could not allocate Handle wrapper \
                      (class OxPHP\\Shared\\Pool\\Handle not registered?)"
                .to_string(),
            code: 0,
        });
    }
    Ok(())
}

fn pool_release(call: &mut NativeCall) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    let handle_zv = unsafe { call.raw_arg_ptr(0) };
    if handle_zv.is_null() {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: "handle is required".to_string(),
            code: 0,
        });
    }

    let mut pool_id: u64 = 0;
    let mut owner_tid: u64 = 0;
    let mut slot_heap: *mut c_void = std::ptr::null_mut();
    let rc_read = unsafe {
        ffi::oxphp_shared_pool_handle_read(handle_zv, &mut pool_id, &mut owner_tid, &mut slot_heap)
    };
    if rc_read != 0 {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: "argument is not a Shared\\Pool\\Handle \
                      (or has already been released)"
                .to_string(),
            code: 0,
        });
    }
    if pool_id != id {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: "handle belongs to a different pool".to_string(),
            code: 0,
        });
    }

    let rc = unsafe { oxphp_shared_pool_release(id, slot_heap, owner_tid) };
    // Clear the storage regardless of release rc — avoids
    // double-release in Drop if release succeeded; on failure the
    // slot is already leaked by design (dead owner path).
    unsafe { ffi::oxphp_shared_pool_handle_clear(handle_zv) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::release"));
    }
    Ok(())
}

fn pool_with(call: &mut NativeCall) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    let body_zv = unsafe { call.raw_arg_ptr(0) };
    let timeout_ms: i64 = read_timeout_arg(call, 1)?;
    let rc = unsafe { oxphp_shared_pool_with(id, timeout_ms, body_zv, call.retval_ptr()) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::with"));
    }
    Ok(())
}

fn pool_evict(call: &mut NativeCall) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    let mut evicted: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_evict(id, &mut evicted) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::evict"));
    }
    call.ret_long(evicted as i64);
    Ok(())
}

fn pool_size(call: &mut NativeCall) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    let mut size: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_size(id, &mut size) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::size"));
    }
    call.ret_long(size as i64);
    Ok(())
}

fn pool_stats_field(call: &mut NativeCall, which: StatsField) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    let mut in_use: u64 = 0;
    let mut idle: u64 = 0;
    let mut waiting: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_stats(id, &mut in_use, &mut idle, &mut waiting) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::stats"));
    }
    let value = match which {
        StatsField::InUse => in_use,
        StatsField::Idle => idle,
        StatsField::Waiting => waiting,
    };
    call.ret_long(value as i64);
    Ok(())
}

enum StatsField {
    InUse,
    Idle,
    Waiting,
}

fn pool_in_use(call: &mut NativeCall) -> Result<(), PhpError> {
    pool_stats_field(call, StatsField::InUse)
}
fn pool_idle(call: &mut NativeCall) -> Result<(), PhpError> {
    pool_stats_field(call, StatsField::Idle)
}
fn pool_waiting(call: &mut NativeCall) -> Result<(), PhpError> {
    pool_stats_field(call, StatsField::Waiting)
}

fn pool_max_size_method(call: &mut NativeCall) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    let mut max: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_max_size(id, &mut max) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::maxSize"));
    }
    call.ret_long(max as i64);
    Ok(())
}

fn pool_id_method(call: &mut NativeCall) -> Result<(), PhpError> {
    let id = pool_get_id(call)?;
    call.ret_long(id as i64);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn inner(max: usize) -> Arc<PoolInner> {
        Arc::new(PoolInner::new(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            max,
            Duration::from_secs(300),
        ))
    }

    /// Variant of `inner()` with a custom idle_timeout. Used by the
    /// eviction-walker tests.
    fn inner_with_idle(max: usize, idle_timeout: Duration) -> Arc<PoolInner> {
        Arc::new(PoolInner::new(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            max,
            idle_timeout,
        ))
    }

    fn dummy_slot(owner: ThreadKey) -> PoolSlot {
        PoolSlot::new(std::ptr::null_mut(), owner)
    }

    // ── construction & accessors ──────────────────────────────────────

    #[test]
    fn new_pool_starts_empty() {
        let p = inner(4);
        assert_eq!(p.size(), 0);
        assert_eq!(p.idle_count(), 0);
        assert_eq!(p.in_use_count(), 0);
        assert_eq!(p.waiting_count(), 0);
        assert_eq!(p.max_size(), 4);
        assert!(!p.is_closed());
        assert!(p.self_id().is_none());
    }

    #[test]
    fn type_tag_matches_pool() {
        let p = inner(1);
        assert_eq!(p.type_tag(), SharedType::Pool);
    }

    #[test]
    fn debug_snapshot_reflects_size() {
        let p = inner(2);
        assert!(matches!(p.debug_snapshot(), SharedValue::Long(0)));
        assert!(p.try_reserve_budget());
        assert!(matches!(p.debug_snapshot(), SharedValue::Long(1)));
    }

    #[test]
    fn bind_id_is_once_only() {
        let p = inner(1);
        p.bind_id(42);
        p.bind_id(99); // silently ignored by OnceLock
        assert_eq!(p.self_id(), Some(42));
    }

    // ── budget semantics ──────────────────────────────────────────────

    #[test]
    fn reserve_then_deposit_increments_size_and_idle() {
        let p = inner(2);
        assert!(p.try_reserve_budget());
        assert_eq!(p.size(), 1);
        // After reserve, before deposit: conceptually "in use" (factory
        // in flight). idle is still 0.
        assert_eq!(p.idle_count(), 0);
        assert_eq!(p.in_use_count(), 1);

        p.deposit_new(dummy_slot(current_thread_key()));
        assert_eq!(p.size(), 1); // deposit doesn't change size
        assert_eq!(p.idle_count(), 1);
        assert_eq!(p.in_use_count(), 0);
    }

    #[test]
    fn budget_full_blocks_further_reserve() {
        let p = inner(2);
        assert!(p.try_reserve_budget());
        assert!(p.try_reserve_budget());
        assert!(!p.try_reserve_budget(), "third reserve must fail");
        assert_eq!(p.size(), 2);
    }

    #[test]
    fn release_budget_refunds_capacity() {
        let p = inner(1);
        assert!(p.try_reserve_budget());
        assert!(!p.try_reserve_budget());
        p.release_budget();
        assert!(p.try_reserve_budget(), "refunded slot should reopen budget");
    }

    #[test]
    fn concurrent_reserve_never_exceeds_budget() {
        let p = inner(3);
        let barrier = Arc::new(Barrier::new(8));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let p = Arc::clone(&p);
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                b.wait();
                let mut wins = 0;
                for _ in 0..1000 {
                    if p.try_reserve_budget() {
                        wins += 1;
                        p.release_budget();
                    }
                }
                wins
            }));
        }
        for h in handles {
            let _ = h.join();
        }
        // After the storm, size is back at 0 (every win was refunded).
        assert_eq!(p.size(), 0);
        assert!(p.size() <= 3);
    }

    // ── per-thread affinity ───────────────────────────────────────────

    #[test]
    fn local_acquire_returns_owning_threads_slot() {
        let p = inner(1);
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(current_thread_key()));
        let slot = p.try_acquire_local().expect("own slot must be visible");
        assert_eq!(slot.owner, current_thread_key());
        assert_eq!(p.idle_count(), 0);
    }

    #[test]
    fn local_acquire_empty_returns_none() {
        let p = inner(1);
        assert!(p.try_acquire_local().is_none());
    }

    #[test]
    fn cross_thread_steal_is_forbidden() {
        let p = inner(4);
        // Thread A deposits.
        let p_a = Arc::clone(&p);
        let a_tid = thread::spawn(move || {
            assert!(p_a.try_reserve_budget());
            let me = current_thread_key();
            p_a.deposit_new(dummy_slot(me));
            me
        })
        .join()
        .unwrap();
        assert_eq!(p.idle_count(), 1);
        assert_ne!(a_tid, current_thread_key());

        // Thread B (main) cannot see A's idle slot, even though
        // budget has room and A's slot is sitting there.
        assert!(
            p.try_acquire_local().is_none(),
            "strict strategy must NOT let a foreign thread steal A's resource"
        );
        assert_eq!(p.idle_count(), 1);
    }

    #[test]
    fn release_deposits_into_owner_not_caller() {
        use crate::plugins::ox_shared::worker_liveness;

        let p = inner(2);
        // A mints and leaves the slot in flight (acquired by A, but
        // stack-owned for the purposes of the test — simulating a
        // resource being handed across threads before release).
        let p_a = Arc::clone(&p);
        let (a_tid, slot_ptr) = thread::spawn(move || {
            assert!(p_a.try_reserve_budget());
            let me = current_thread_key();
            // Deposit + immediately re-acquire to simulate "A minted
            // and is currently holding".
            p_a.deposit_new(dummy_slot(me));
            let slot = p_a.try_acquire_local().unwrap();
            (me, slot.resource as usize)
        })
        .join()
        .unwrap();

        // Simulate A still being a live SAPI worker — otherwise the
        // cross-thread release path treats A as dead and destroys the
        // slot inline instead of parking. In production, A's worker
        // loop keeps its ThreadKey in the registry until the thread
        // tears down.
        worker_liveness::force_insert(a_tid);

        // Main thread (B) calls release with A's slot.
        p.release(PoolSlot::new(slot_ptr as *mut c_void, a_tid));

        // Slot landed in A's deque, not main thread's.
        let counts: std::collections::HashMap<_, _> = p.idle_by_thread().into_iter().collect();
        assert_eq!(counts.get(&a_tid).copied(), Some(1));
        assert_eq!(counts.get(&current_thread_key()).copied(), None);

        worker_liveness::force_remove(a_tid);
    }

    #[test]
    fn release_cross_thread_dead_owner_destroys_inline() {
        // A ghost owner key that was never registered (or has since
        // unregistered). Cross-thread release treats it as dead and
        // calls `destroy_slot` — the mock bridge's destroy helper is
        // a no-op, so we only observe the bookkeeping: size and
        // idle_count both drop to zero.
        let p = inner(2);
        const GHOST_KEY: ThreadKey = 0xDEAD_0000_0000_0001;

        assert!(p.try_reserve_budget());
        p.release(dummy_slot(GHOST_KEY));

        assert_eq!(p.size(), 0, "destroy path must refund budget");
        assert_eq!(
            p.idle_count(),
            0,
            "slot must not park in a dead owner's deque"
        );
        assert!(
            p.idle.get(&GHOST_KEY).is_none()
                || p.idle.get(&GHOST_KEY).unwrap().lock().unwrap().is_empty(),
            "dead-owner deque must stay empty"
        );
    }

    #[test]
    fn release_cross_thread_live_owner_parks() {
        use crate::plugins::ox_shared::worker_liveness;

        // Same shape as dead-owner test, but the owner is registered
        // as a live worker — release must park in the owner's deque
        // and the budget must remain held.
        let p = inner(2);
        const LIVE_KEY: ThreadKey = 0xBEEF_0000_0000_0001;
        worker_liveness::force_insert(LIVE_KEY);

        assert!(p.try_reserve_budget());
        p.release(dummy_slot(LIVE_KEY));

        assert_eq!(p.size(), 1, "budget stays held for a parked slot");
        assert_eq!(p.idle_count(), 1, "slot parked in live owner's deque");
        let shard = p.idle.get(&LIVE_KEY).expect("live owner's deque exists");
        assert_eq!(shard.lock().unwrap().len(), 1);

        worker_liveness::force_remove(LIVE_KEY);
    }

    #[test]
    fn release_same_thread_bypasses_liveness_check() {
        use crate::plugins::ox_shared::worker_liveness;

        // Force-unregister the current thread — same-thread release
        // must still park, because the hot-path bypasses the liveness
        // check. Without this invariant, every non-SAPI test context
        // (unit tests, cargo-test threads) would hit the destroy path
        // on its own release calls.
        let p = inner(1);
        let me = current_thread_key();
        worker_liveness::force_remove(me);
        assert!(!worker_liveness::is_thread_alive(me));

        assert!(p.try_reserve_budget());
        p.release(dummy_slot(me));

        assert_eq!(p.size(), 1);
        assert_eq!(p.idle_count(), 1, "same-thread release always parks");
    }

    // ── waiter queue & condvar ────────────────────────────────────────

    #[test]
    fn wait_for_release_times_out() {
        let p = inner(1);
        let start = Instant::now();
        let woken = p.wait_for_release(Duration::from_millis(50));
        let elapsed = start.elapsed();
        assert!(!woken, "timeout must report false");
        assert!(
            elapsed >= Duration::from_millis(45),
            "timed out too fast: {elapsed:?}"
        );
        assert_eq!(p.waiting_count(), 0, "counter must decrement on timeout");
    }

    #[test]
    fn wait_for_release_wakes_on_release() {
        let p = inner(2);
        let me = current_thread_key();
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        let slot = p.try_acquire_local().unwrap();

        let p_w = Arc::clone(&p);
        let waiter = thread::spawn(move || {
            let start = Instant::now();
            let woken = p_w.wait_for_release(Duration::from_millis(500));
            (woken, start.elapsed())
        });

        // Give the waiter time to park on the condvar.
        thread::sleep(Duration::from_millis(50));
        p.release(slot);

        let (woken, elapsed) = waiter.join().unwrap();
        assert!(woken, "release must wake the waiter");
        assert!(
            elapsed < Duration::from_millis(400),
            "wake took too long: {elapsed:?}"
        );
    }

    #[test]
    fn close_wakes_all_waiters() {
        let p = inner(1);
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..3 {
            let p = Arc::clone(&p);
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                b.wait();
                let start = Instant::now();
                let woken = p.wait_for_release(Duration::from_secs(5));
                (woken, start.elapsed(), p.is_closed())
            }));
        }

        barrier.wait();
        thread::sleep(Duration::from_millis(100));
        p.close();

        for h in handles {
            let (woken, elapsed, closed) = h.join().unwrap();
            assert!(woken, "close must wake every waiter");
            assert!(closed, "is_closed must be visible post-wake");
            assert!(elapsed < Duration::from_secs(1), "slow wake: {elapsed:?}");
        }
    }

    #[test]
    fn release_budget_nudges_one_waiter() {
        let p = inner(1);
        assert!(p.try_reserve_budget());

        let p_w = Arc::clone(&p);
        let waiter = thread::spawn(move || {
            let start = Instant::now();
            let woken = p_w.wait_for_release(Duration::from_millis(500));
            (woken, start.elapsed())
        });

        thread::sleep(Duration::from_millis(50));
        p.release_budget();

        let (woken, elapsed) = waiter.join().unwrap();
        assert!(woken, "release_budget must wake a waiter (capacity freed)");
        assert!(elapsed < Duration::from_millis(400));
    }

    // ── lifecycle & traits ────────────────────────────────────────────

    #[test]
    fn is_closed_roundtrip() {
        let p = inner(1);
        assert!(!p.is_closed());
        p.close();
        assert!(p.is_closed());
    }

    #[test]
    fn on_shutdown_notify_closes() {
        let p = inner(1);
        assert!(!p.is_closed());
        p.on_shutdown_notify();
        assert!(p.is_closed());
    }

    #[test]
    fn on_shutdown_notify_drains_idle_deques() {
        use crate::plugins::ox_shared::worker_liveness;
        // Populate three slots across two live owner keys, then
        // drain. Under the mock bridge, `oxphp_pool_destroy_invoke`
        // is a no-op returning 0 — we only observe the Rust-side
        // bookkeeping (size, idle_count, closed flag). Both owner
        // keys are registered so the cross-thread release parks
        // rather than destroying inline.
        let p = inner(4);
        let me = current_thread_key();
        const ALIVE_B: ThreadKey = 0xBEEF_0000_0000_1234;
        worker_liveness::force_insert(ALIVE_B);

        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert!(p.try_reserve_budget());
        p.release(dummy_slot(ALIVE_B)); // parks in B's deque

        assert_eq!(p.size(), 3);
        assert_eq!(p.idle_count(), 3);

        p.on_shutdown_notify();

        assert!(p.is_closed(), "drain must close the pool");
        assert_eq!(p.size(), 0, "every destroyed slot frees budget");
        assert_eq!(p.idle_count(), 0, "all idle deques drained");

        worker_liveness::force_remove(ALIVE_B);
    }

    #[test]
    fn on_shutdown_notify_is_idempotent() {
        // Second drain pass must see an empty pool and not panic.
        let p = inner(2);
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(current_thread_key()));

        p.on_shutdown_notify();
        assert_eq!(p.size(), 0);

        p.on_shutdown_notify(); // no-op second call
        assert_eq!(p.size(), 0);
        assert!(p.is_closed());
    }

    #[test]
    fn on_drop_drains_and_frees_fccs() {
        // NULL fccs + null-resource slots + mock bridge means this
        // reduces to exercising the Rust-side drain path. We check
        // that on_drop:
        //   (1) closes the pool,
        //   (2) empties the idle deques,
        //   (3) decrements size to zero.
        // `oxphp_pool_fcc_free` is called with NULL and short-circuits
        // inside the mock, matching the production early-return.
        let p = inner(2);
        let me = current_thread_key();
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));

        p.on_drop();

        assert!(p.is_closed());
        assert_eq!(p.size(), 0);
        assert_eq!(p.idle_count(), 0);
    }

    #[test]
    fn mem_bytes_scales_with_size() {
        let p = inner(4);
        let base = p.mem_bytes();
        assert!(p.try_reserve_budget());
        assert!(p.try_reserve_budget());
        assert!(p.mem_bytes() > base);
    }

    #[test]
    fn children_is_empty_for_pool() {
        let p = inner(1);
        let mut out = Vec::new();
        p.children(&mut out);
        assert!(
            out.is_empty(),
            "Pool resources are raw zvals, not SharedValue refs"
        );
    }

    #[test]
    fn downcast_via_shared_inner_pool_ext() {
        let p: Arc<dyn SharedInner> = inner(1);
        // `as_any_pool` must resolve for a Pool-typed trait object.
        let casted = (*p).as_any_pool();
        assert!(casted.is_some());
    }

    // ── idle-by-thread snapshot ───────────────────────────────────────

    #[test]
    fn idle_by_thread_enumerates_non_empty_deques() {
        let p = inner(4);

        // A and B run CONCURRENTLY under a barrier. With `ThreadKey =
        // pthread_self()`, sequential spawn+join tends to reuse the
        // underlying pthread_t, which would collapse A's and B's keys
        // into one bucket. Running both threads live at the same time
        // guarantees distinct pthread values.
        let start = Arc::new(Barrier::new(3)); // main + A + B
        let deposited = Arc::new(Barrier::new(3));

        let p_a = Arc::clone(&p);
        let s_a = Arc::clone(&start);
        let d_a = Arc::clone(&deposited);
        let a = thread::spawn(move || {
            s_a.wait();
            assert!(p_a.try_reserve_budget());
            let me = current_thread_key();
            p_a.deposit_new(dummy_slot(me));
            d_a.wait();
            me
        });

        let p_b = Arc::clone(&p);
        let s_b = Arc::clone(&start);
        let d_b = Arc::clone(&deposited);
        let b = thread::spawn(move || {
            s_b.wait();
            assert!(p_b.try_reserve_budget());
            assert!(p_b.try_reserve_budget());
            let me = current_thread_key();
            p_b.deposit_new(dummy_slot(me));
            p_b.deposit_new(dummy_slot(me));
            d_b.wait();
            me
        });

        start.wait();
        deposited.wait();
        let a_tid = a.join().unwrap();
        let b_tid = b.join().unwrap();
        assert_ne!(a_tid, b_tid, "concurrent threads must have distinct keys");

        let snap: std::collections::HashMap<_, _> = p.idle_by_thread().into_iter().collect();
        assert_eq!(snap.get(&a_tid).copied(), Some(1));
        assert_eq!(snap.get(&b_tid).copied(), Some(2));
        assert_eq!(p.idle_count(), 3);
        assert_eq!(p.in_use_count(), 0);
        assert_eq!(p.size(), 3);
    }

    // ── idle-timeout eviction ─────────────────────────────────────────

    #[test]
    fn stale_owners_excludes_fresh_deques() {
        // A deque whose front is younger than idle_timeout must not
        // appear in the stale-owners scan result. This is the common
        // case and the scheduler must not waste a request_evict flag
        // raise on it.
        let p = inner_with_idle(2, Duration::from_millis(100));
        let me = current_thread_key();
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        // Peek immediately — slot is fresh, no owners returned.
        assert!(p.stale_owners(Instant::now()).is_empty());
    }

    #[test]
    fn stale_owners_flags_overdue_front() {
        // Slot deposited with `last_active = now`. Advance the peek's
        // reference `now` past `idle_timeout` — the owner's key must
        // show up. This is how the scheduler triggers eviction.
        let p = inner_with_idle(2, Duration::from_millis(100));
        let me = current_thread_key();
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));

        let future = Instant::now() + Duration::from_millis(250);
        let flagged = p.stale_owners(future);
        assert_eq!(flagged, vec![me]);
    }

    #[test]
    fn evict_stale_on_current_thread_pops_aged_slots() {
        use crate::plugins::ox_shared::worker_liveness;
        // Three same-thread deposits with synthetic last_active set
        // to 200ms in the past. idle_timeout=100ms ⇒ all three are
        // past due. evict_stale_on_current_thread must drain them
        // and refund budget.
        let p = inner_with_idle(4, Duration::from_millis(100));
        let me = current_thread_key();
        worker_liveness::register_worker();

        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));

        // Age every slot by rewinding last_active.
        let aged = Instant::now() - Duration::from_millis(250);
        {
            let shard = p.idle.get(&me).unwrap();
            let mut deque = shard.lock().unwrap();
            for slot in deque.iter_mut() {
                slot.last_active = aged;
            }
        }

        assert_eq!(p.size(), 3);
        p.evict_stale_on_current_thread(EvictReason::Manual);
        assert_eq!(p.size(), 0);
        assert_eq!(p.idle_count(), 0);

        worker_liveness::unregister_worker();
    }

    #[test]
    fn evict_stale_on_current_thread_stops_at_fresh_front() {
        // Deques are age-ordered (release always push_back with a
        // fresh last_active). Mixing one aged slot at the front
        // and one fresh slot behind must drop exactly the aged one.
        let p = inner_with_idle(2, Duration::from_millis(100));
        let me = current_thread_key();

        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));

        {
            let shard = p.idle.get(&me).unwrap();
            let mut deque = shard.lock().unwrap();
            // front = aged, back = fresh
            deque.front_mut().unwrap().last_active = Instant::now() - Duration::from_millis(250);
        }

        assert_eq!(p.size(), 2);
        p.evict_stale_on_current_thread(EvictReason::Manual);
        assert_eq!(p.size(), 1, "fresh slot must survive");
        assert_eq!(p.idle_count(), 1);
    }

    #[test]
    fn release_resets_last_active() {
        // Simulates "resource was parked, then a later acquire pulled
        // it, used it for a long time, then released it". The park-
        // time last_active must be refreshed so the eviction walker
        // doesn't misclassify a recently-in-use slot as stale.
        let p = inner_with_idle(1, Duration::from_millis(100));
        let me = current_thread_key();

        assert!(p.try_reserve_budget());
        let mut slot = dummy_slot(me);
        slot.last_active = Instant::now() - Duration::from_secs(1);
        p.release(slot);

        // Post-release peek: the slot sits in me's deque with a fresh
        // last_active, so stale_owners must not flag it.
        let snap = p.stale_owners(Instant::now());
        assert!(
            snap.is_empty(),
            "release must reset last_active, got stale owners: {snap:?}"
        );
    }

    // ── observability counters ──────────────────────────────

    #[test]
    fn acquire_counters_split_by_result() {
        // Counter surface matches `oxphp_shared_pool_acquire_total`'s
        // three label values: ok / timeout / closed. Here we exercise
        // the raw recorder methods — FFI-driven round-trips are
        // covered by the docker perf/idle tests.
        let p = inner(1);
        p.record_acquire_ok();
        p.record_acquire_ok();
        p.record_acquire_timeout();
        p.record_acquire_closed();
        assert_eq!(p.acquire_ok_total(), 2);
        assert_eq!(p.acquire_timeout_total(), 1);
        assert_eq!(p.acquire_closed_total(), 1);
    }

    #[test]
    fn wait_histogram_buckets_classify_by_upper_bound() {
        // Observations land in the smallest bucket whose upper bound
        // is ≥ the observed value. Cumulative export monotonicity
        // must hold: bucket[i] ≤ bucket[i+1].
        let p = inner(1);
        // 0.5ms → bucket[0] (≤ 1ms).
        p.record_wait(Duration::from_micros(500));
        // 5ms → bucket[1] (≤ 10ms).
        p.record_wait(Duration::from_millis(5));
        // 50ms → bucket[2] (≤ 100ms).
        p.record_wait(Duration::from_millis(50));
        // 500ms → bucket[3] (≤ 1s).
        p.record_wait(Duration::from_millis(500));
        // 5s → bucket[4] (≤ 10s).
        p.record_wait(Duration::from_secs(5));
        // 100s → bucket[5] (+Inf).
        p.record_wait(Duration::from_secs(100));

        let (cum, sum_s, count) = p.wait_histogram_snapshot();
        assert_eq!(count, 6);
        // Cumulative: each bucket must be strictly monotone for
        // this input (one observation per bucket).
        assert_eq!(cum, [1, 2, 3, 4, 5, 6]);
        // Sum ≈ 105.555 s — allow slack for Duration arithmetic.
        assert!((sum_s - 105.555).abs() < 0.01, "unexpected sum: {sum_s}");
    }

    #[test]
    fn wait_histogram_boundary_lands_on_upper_bucket() {
        // Exactly 1ms must land in bucket[0] (the ≤ 1ms bucket), not
        // bucket[1]. Same rule for every other boundary; off-by-one
        // here would shift all percentile reporting by one bucket.
        let p = inner(1);
        p.record_wait(Duration::from_millis(1));
        p.record_wait(Duration::from_millis(10));
        p.record_wait(Duration::from_millis(100));
        p.record_wait(Duration::from_secs(1));
        p.record_wait(Duration::from_secs(10));
        let (cum, _sum, count) = p.wait_histogram_snapshot();
        assert_eq!(count, 5);
        // Each observation lands on the exact boundary ⇒ incremental
        // pattern 1,1,1,1,1,0 cumulatively becomes 1,2,3,4,5,5.
        assert_eq!(cum, [1, 2, 3, 4, 5, 5]);
    }

    #[test]
    fn evict_counters_split_by_reason() {
        let p = inner(1);
        p.record_evicted(3, EvictReason::IdleTimeout);
        p.record_evicted(2, EvictReason::Manual);
        p.record_evicted(7, EvictReason::Shutdown);
        p.record_evicted(0, EvictReason::IdleTimeout); // zero is no-op
        assert_eq!(p.evicted_idle_total(), 3);
        assert_eq!(p.evicted_manual_total(), 2);
        assert_eq!(p.evicted_shutdown_total(), 7);
    }

    #[test]
    fn shutdown_drain_bumps_shutdown_eviction_counter() {
        // Full path: deposit slots, on_shutdown_notify, verify the
        // shutdown bucket records the drained count.
        let p = inner(3);
        let me = current_thread_key();
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        assert_eq!(p.evicted_shutdown_total(), 0);
        p.on_shutdown_notify();
        assert_eq!(p.evicted_shutdown_total(), 2);
        // Idempotent: second drain sees empty deques, counter
        // does not bump.
        p.on_shutdown_notify();
        assert_eq!(p.evicted_shutdown_total(), 2);
    }

    #[test]
    fn evict_stale_bumps_matching_reason_counter() {
        let p = inner_with_idle(2, Duration::from_millis(50));
        let me = current_thread_key();
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(me));
        // Age the slot past the idle_timeout.
        {
            let shard = p.idle.get(&me).unwrap();
            let mut d = shard.lock().unwrap();
            d.front_mut().unwrap().last_active = Instant::now() - Duration::from_millis(100);
        }

        let n = p.evict_stale_on_current_thread(EvictReason::IdleTimeout);
        assert_eq!(n, 1);
        assert_eq!(p.evicted_idle_total(), 1);
        assert_eq!(p.evicted_manual_total(), 0);
        assert_eq!(p.evicted_shutdown_total(), 0);
    }

    // ── chaos merge-gate: budget reclaim across worker panic ──────────

    #[test]
    fn track_and_untrack_balance_to_zero() {
        // Happy path: every acquire that bumps the in-flight counter
        // must be paired with exactly one untrack. reclaim on an
        // empty counter refunds nothing.
        let p = inner(2);
        let me = current_thread_key();

        p.track_acquired_by_me();
        p.track_acquired_by_me();
        p.untrack_released(me);
        p.untrack_released(me);

        assert_eq!(p.reclaim_budget_for_dead_worker(me), 0);
    }

    #[test]
    fn reclaim_refunds_leaked_budget() {
        // Simulate a worker that minted two slots (size bumped via
        // reserve_budget) and held them in-flight when it died.
        // Reclaim must refund size to 0.
        let p = inner(4);
        let me = current_thread_key();

        assert!(p.try_reserve_budget());
        p.track_acquired_by_me();
        assert!(p.try_reserve_budget());
        p.track_acquired_by_me();

        assert_eq!(p.size(), 2);
        let reclaimed = p.reclaim_budget_for_dead_worker(me);

        assert_eq!(reclaimed, 2);
        assert_eq!(p.size(), 0, "size must be refunded on chaos reclaim");
    }

    #[test]
    fn reclaim_is_per_thread_isolated() {
        // Two threads each have one in-flight slot. Reclaiming the
        // first thread's key must not touch the second's entry.
        let p = inner(4);
        let me = current_thread_key();
        const OTHER: ThreadKey = 0xF00D_0000_0000_0001;

        assert!(p.try_reserve_budget());
        p.track_acquired_by_me();
        assert!(p.try_reserve_budget());
        // Simulate OTHER's in-flight via the map directly.
        p.in_flight
            .entry(OTHER)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::AcqRel);

        assert_eq!(p.size(), 2);
        assert_eq!(p.reclaim_budget_for_dead_worker(me), 1);
        assert_eq!(p.size(), 1, "OTHER's slot must survive reclaim");

        // OTHER still has an in-flight entry; its reclaim refunds
        // the remaining unit.
        assert_eq!(p.reclaim_budget_for_dead_worker(OTHER), 1);
        assert_eq!(p.size(), 0);
    }

    // The full end-to-end chaos test (worker thread panics while
    // holding a Pool slot; verify the reclaim hook refunds budget
    // via the SharedRegistry iteration path) lives in the FFI-tests
    // block below where `ensure_registry` / `register_pool` are in
    // scope. See `chaos_worker_panic_mid_acquire_preserves_budget`.

    // ── FFI surface tests (host, via mock bridge) ─────────────────────
    //
    // The mock bridge cannot invoke PHP, so factory-invocation paths
    // live behind docker integration tests. The FFI surface
    // tests below exercise everything that doesn't call the factory:
    // registry plumbing, argument validation, type mismatch, and the
    // stats/size/evict/release round-trip.

    use crate::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
    use crate::plugins::ox_shared::error::SharedError;
    use crate::plugins::ox_shared::registry::{init_registry, registry, SharedRegistry};

    fn ensure_registry() -> &'static SharedRegistry {
        // Idempotent — OnceLock::set drops the dupe. Mirrors map.rs.
        init_registry(SharedConfig {
            enabled: true,
            max_entries: 10_000,
            max_bytes: 1 << 30,
            soft_limit_ratio: 0.7,
            metrics_enabled: false,
            introspection_enabled: false,
            introspection_preview_enabled: false,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            shutdown_timeout_seconds: 5.0,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        });
        registry()
    }

    /// Insert a Pool directly into the registry, bypassing the FFI's
    /// factory fcc setup — tests exercise the Rust-side bookkeeping
    /// path without a live libphp.
    fn register_pool(max: usize) -> SharedId {
        let reg = ensure_registry();
        let inner: Arc<dyn SharedInner> = Arc::new(PoolInner::new(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            max,
            Duration::from_secs(300),
        ));
        let id = reg.insert(SharedType::Pool, Arc::clone(&inner)).unwrap();
        (*inner).as_any_pool().expect("just inserted").bind_id(id);
        id
    }

    #[test]
    fn ffi_create_rejects_null_factory() {
        ensure_registry();
        let mut id: u64 = 0;
        let rc = unsafe {
            oxphp_shared_pool_create(std::ptr::null_mut(), std::ptr::null_mut(), 8, 60.0, &mut id)
        };
        assert_eq!(rc, SharedError::Type.code());
        assert_eq!(id, 0);
    }

    #[test]
    fn ffi_create_rejects_zero_max_size() {
        ensure_registry();
        // Use a non-null callable sentinel — validation fires on max_size
        // before reaching the bridge (mock would fail differently).
        #[allow(clippy::manual_dangling_ptr)]
        let sentinel = 0x1 as *mut std::ffi::c_void;
        let mut id: u64 = 0;
        let rc =
            unsafe { oxphp_shared_pool_create(sentinel, std::ptr::null_mut(), 0, 60.0, &mut id) };
        assert_eq!(rc, SharedError::Type.code());
        assert_eq!(id, 0);
    }

    #[test]
    fn ffi_stats_on_fresh_pool_zero() {
        let id = register_pool(4);
        let mut in_use: u64 = 99;
        let mut idle: u64 = 99;
        let mut waiting: u64 = 99;
        let rc = unsafe { oxphp_shared_pool_stats(id, &mut in_use, &mut idle, &mut waiting) };
        assert_eq!(rc, 0);
        assert_eq!(in_use, 0);
        assert_eq!(idle, 0);
        assert_eq!(waiting, 0);
    }

    #[test]
    fn ffi_size_and_max_size_round_trip() {
        let id = register_pool(7);
        let mut size: u64 = 99;
        let mut max: u64 = 99;
        assert_eq!(unsafe { oxphp_shared_pool_size(id, &mut size) }, 0);
        assert_eq!(unsafe { oxphp_shared_pool_max_size(id, &mut max) }, 0);
        assert_eq!(size, 0);
        assert_eq!(max, 7);
    }

    #[test]
    fn ffi_stats_on_wrong_type_errors() {
        let reg = ensure_registry();
        // Insert a Counter and try to read Pool stats on it.
        let counter_id = reg
            .insert(
                SharedType::Counter,
                Arc::new(crate::plugins::ox_shared::types::counter::CounterInner::new(0)),
            )
            .unwrap();
        let mut in_use: u64 = 0;
        let mut idle: u64 = 0;
        let mut waiting: u64 = 0;
        let rc =
            unsafe { oxphp_shared_pool_stats(counter_id, &mut in_use, &mut idle, &mut waiting) };
        assert_eq!(rc, SharedError::Type.code());
    }

    #[test]
    fn ffi_stats_on_stale_errors() {
        ensure_registry();
        let rc = unsafe {
            oxphp_shared_pool_stats(
                999_999_999,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, SharedError::StaleHandle.code());
    }

    #[test]
    fn ffi_evict_is_zero_stub() {
        let id = register_pool(4);
        let mut evicted: u64 = 77;
        let rc = unsafe { oxphp_shared_pool_evict(id, &mut evicted) };
        assert_eq!(rc, 0);
        assert_eq!(evicted, 0, "v1 stub reports 0 evicted");
    }

    #[test]
    fn ffi_acquire_on_closed_pool_errors() {
        let id = register_pool(2);
        // Flip the closed flag before acquiring.
        let entry = registry().lookup(id).unwrap();
        entry.inner.as_any_pool().unwrap().close();

        let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut owner: u64 = 0;
        let rc = unsafe { oxphp_shared_pool_acquire(id, 100, &mut slot_heap, &mut owner) };
        assert_eq!(rc, SharedError::Closed.code());
        assert!(slot_heap.is_null());
    }

    #[test]
    fn ffi_acquire_local_idle_bypasses_factory() {
        // Pre-populate idle on the current thread so acquire returns
        // from the fast path — no factory call, no C bridge needed.
        let id = register_pool(2);
        let entry = registry().lookup(id).unwrap();
        let pool = entry.inner.as_any_pool().unwrap();
        assert!(pool.try_reserve_budget());
        let sentinel = 0xBABE_0042 as *mut std::ffi::c_void;
        pool.deposit_new(PoolSlot::new(sentinel, current_thread_key()));

        let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut owner: u64 = 0;
        let rc = unsafe { oxphp_shared_pool_acquire(id, 1000, &mut slot_heap, &mut owner) };
        assert_eq!(rc, 0);
        assert_eq!(slot_heap, sentinel);
        assert_eq!(owner, current_thread_key());
    }

    #[test]
    fn ffi_release_routes_back_to_owner_idle() {
        let id = register_pool(2);
        let entry = registry().lookup(id).unwrap();
        let pool = entry.inner.as_any_pool().unwrap();
        assert!(pool.try_reserve_budget());
        let sentinel = 0xBABE_0043 as *mut std::ffi::c_void;
        pool.deposit_new(PoolSlot::new(sentinel, current_thread_key()));

        // Acquire then release via FFI — slot should land back in idle.
        let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut owner: u64 = 0;
        assert_eq!(
            unsafe { oxphp_shared_pool_acquire(id, 1000, &mut slot_heap, &mut owner) },
            0
        );
        assert_eq!(pool.idle_count(), 0);

        assert_eq!(
            unsafe { oxphp_shared_pool_release(id, slot_heap, owner) },
            0
        );
        assert_eq!(pool.idle_count(), 1);
    }

    #[test]
    fn ffi_release_creates_owner_deque_lazily() {
        use crate::plugins::ox_shared::worker_liveness;
        // v1 contract: release for a live owner that has not yet
        // minted a slot creates its idle deque on demand. The owner
        // must be in the liveness registry — a release targeted at
        // an unregistered key takes the dead-owner inline-destroy
        // path (covered by `release_cross_thread_dead_owner_destroys_inline`).
        let id = register_pool(1);
        let entry = registry().lookup(id).unwrap();
        let pool = entry.inner.as_any_pool().unwrap();
        assert!(pool.try_reserve_budget());

        const LIVE_KEY: ThreadKey = 0xBEEF_0000_0000_0002;
        worker_liveness::force_insert(LIVE_KEY);

        let rc = unsafe {
            oxphp_shared_pool_release(id, 0xBABE_0044 as *mut std::ffi::c_void, LIVE_KEY)
        };
        assert_eq!(rc, 0);
        assert!(pool.idle_by_thread().iter().any(|(k, _)| *k == LIVE_KEY));

        worker_liveness::force_remove(LIVE_KEY);
    }

    #[test]
    fn ffi_acquire_times_out_when_budget_full_and_empty() {
        let id = register_pool(1);
        let entry = registry().lookup(id).unwrap();
        let pool = entry.inner.as_any_pool().unwrap();
        // Exhaust budget without populating idle. On host (mock bridge)
        // the factory would fail, but with budget already full we reach
        // the wait-loop directly and time out cleanly.
        assert!(pool.try_reserve_budget());

        let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut owner: u64 = 0;
        let start = Instant::now();
        let rc = unsafe { oxphp_shared_pool_acquire(id, 50, &mut slot_heap, &mut owner) };
        let elapsed = start.elapsed();
        assert_eq!(rc, SharedError::Timeout.code());
        assert!(
            elapsed >= Duration::from_millis(45),
            "acquire must wait the full timeout: {elapsed:?}"
        );
    }

    // ── perf-gate regression guard ────────────────────────────

    #[test]
    fn perf_regression_uncontested_acquire_release() {
        // Exit criterion: uncontested acquire+release ≤ 5μs.
        // This test is a CI guard against
        // catastrophic regressions (e.g. accidental O(workers)
        // behaviour); the authoritative number comes from
        // `cargo bench --bench pool_uncontested`. Gate is loose
        // in debug (tests default) and tight in release.
        //
        // Reference numbers (Apple M-series, release build):
        //   acquire+release cycle = ~136ns  → ~37× under 5μs budget.
        let id = register_pool(1);
        crate::plugins::ox_shared::worker_liveness::register_worker();
        let entry = registry().lookup(id).unwrap();
        let pool = entry.inner.as_any_pool().unwrap();
        assert!(pool.try_reserve_budget());
        let sentinel = 0xBABE_0044 as *mut std::ffi::c_void;
        pool.deposit_new(PoolSlot::new(sentinel, current_thread_key()));

        const ITERS: u64 = 10_000;
        let start = Instant::now();
        for _ in 0..ITERS {
            let mut slot: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut owner: u64 = 0;
            let rc = unsafe { oxphp_shared_pool_acquire(id, 0, &mut slot, &mut owner) };
            assert_eq!(rc, 0);
            let rc = unsafe { oxphp_shared_pool_release(id, slot, owner) };
            assert_eq!(rc, 0);
        }
        let elapsed = start.elapsed();
        let per_op_ns = elapsed.as_nanos() as u64 / ITERS;

        // Tight 5μs gate in release, loose 50μs in debug. Debug
        // builds still catch any order-of-magnitude regression
        // without being flaky under CI load.
        let budget_ns = if cfg!(debug_assertions) {
            50_000
        } else {
            5_000
        };
        assert!(
            per_op_ns < budget_ns,
            "acquire+release took {per_op_ns}ns/op — budget is {budget_ns}ns \
             (ref: ~136ns release). See `cargo bench pool_uncontested` for \
             authoritative numbers."
        );

        crate::plugins::ox_shared::worker_liveness::unregister_worker();
    }

    // ── merge-gate chaos test ─────────────────────────────────

    #[test]
    fn chaos_worker_panic_mid_acquire_preserves_budget_invariant() {
        // End-to-end chaos-gate exercise. A simulated SAPI worker
        // thread registers itself, reserves a budget slot, tracks it
        // as in-flight, then panics while "holding" the slot — the
        // equivalent of user code aborting between the FFI acquire
        // and Handle construction. The production worker_thread
        // wraps its work in catch_unwind and always runs the
        // unregister tail; mirror that so the reclaim hook fires.
        //
        // Invariant: after the worker exits, `pool.size()` returns
        // to zero and the pool is still usable from another thread
        // (no poisoned locks, no corrupted idle map).
        use crate::plugins::ox_shared::worker_liveness;
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let id = register_pool(2);
        let pool = registry().lookup(id).unwrap().inner.as_any_pool().unwrap() as *const PoolInner;
        // SAFETY: the Arc is held by the registry for the duration
        // of the test; we don't outlive it.
        let pool: &'static PoolInner = unsafe { &*pool };

        let worker = thread::spawn(move || {
            worker_liveness::register_worker();

            let panicked = catch_unwind(AssertUnwindSafe(|| {
                assert!(pool.try_reserve_budget());
                pool.track_acquired_by_me();
                panic!("simulated mid-acquire panic");
            }));
            assert!(panicked.is_err(), "inner work must have panicked");

            // Match production worker tail: unregister runs on
            // both clean-exit and catch-unwind-panic flows. The
            // reclaim hook inside unregister_worker walks every
            // Pool in the SharedRegistry and refunds budget for
            // in-flight slots keyed to this thread.
            worker_liveness::unregister_worker();
        });
        worker.join().expect("outer closure must not panic");

        assert_eq!(
            pool.size(),
            0,
            "budget leaked through panicked worker: size={}",
            pool.size()
        );

        // Pool stays functional on the main thread.
        assert!(pool.try_reserve_budget());
        pool.release_budget();
        assert_eq!(pool.size(), 0);
        assert_eq!(pool.idle_count(), 0);
    }
}
