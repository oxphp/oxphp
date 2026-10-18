//! `Shared\Pool` — process-wide resource pool with per-thread affinity
//! (strict strategy, v1).
//!
//! Design notes:
//!
//! * **Affinity key is a raw pthread id (`u64`).** Worker threads spawn
//!   via `std::thread::spawn`, but the FFI boundary needs a type that
//!   can cross C — `std::thread::ThreadId` has no stable `as_u64`. We
//!   use `libc::pthread_self() as usize as u64`.
//! * `factory_fcc` / `destroy_fcc` are stored as `AtomicPtr<c_void>`.
//!   The creating FFI path (`oxphp_shared_pool_create`) calls
//!   `oxphp_pool_fcc_new` in the C bridge, which emalloc's a
//!   `oxphp_pool_fcc_t` (fcc + ZVAL_COPY'd callable) and hands back a
//!   heap pointer — memory of the creating thread's current request,
//!   as is every slot the factory mints.
//! * **Outside worker mode a pool belongs to the request that made
//!   it.** A thread that runs one request after another (traditional,
//!   Framework and SPA routing) resets its request heap at the end of
//!   each request, and `oxphp run` does so once, before the shared
//!   registry is drained. A pool created on such a thread records it as
//!   `home`: `acquire` from any other thread is refused, and
//!   [`end_request_on_current_thread`], run right after
//!   `php_request_shutdown`, closes the pool and forgets its callables
//!   and the thread's idle slots without touching them. Tearing such a
//!   pool down anywhere but on `home` with its request open — the last
//!   reference can go away in another thread — forgets the same way.
//!   Threads whose request lasts as long as the thread (worker mode, the
//!   async pool) leave `home` unset.
//! * **Teardown frees only what the calling thread's open request
//!   allocated.** Every heap is per thread, and a request's is reset when
//!   it ends; freeing into any other aborts the process. `on_shutdown_notify`
//!   and `on_drop` destroy the calling thread's own idle slots through
//!   `oxphp_pool_destroy_invoke_guarded` — `$destroy($resource)`, then
//!   `zval_ptr_dtor`+`efree` on the slot — and only while its request is
//!   open. The C helper calls `$destroy` only with PHP code executing on
//!   the thread (`EG(current_execute_data) != NULL`): a throw with no frame
//!   to return to would end as a fatal error. Exceptions thrown inside
//!   `$destroy` are captured via `oxphp_bridge_capture_fatal` and cleared.
//!   A bailout is held back for the C code that the Rust code letting go
//!   of the pool returns to, which raises it again; until then no PHP
//!   runs, so the slots not destroyed yet and the callables are forgotten.
//!   The loop of an `oxphp_async()` worker thread has no such C code: a
//!   task result nobody receives any more, dropped there between tasks,
//!   can hold the last reference to a pool. `$destroy` is skipped there,
//!   with no frame, but freeing a slot still runs its object's destructor,
//!   and a bailout in it waits for the next plugin call on that thread,
//!   which raises it in a task that never failed.
//!   Other threads' slots are left to them: the registry drain at shutdown
//!   leaves them where they are, and `on_drop` forgets them — their memory
//!   and the objects in it go with their thread's request. The
//!   factory/destroy fccs are freed with `oxphp_pool_fcc_free` only by the
//!   request that created them, and forgotten otherwise. A slot of a pool
//!   with a `$destroy` that leaves without it is counted in
//!   `oxphp_shared_pool_destroy_skipped_total`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::plugins::ox_shared::registry::{
    Entry, SharedId, SharedInner, SharedType, ENTRY_MAGIC, REGISTRY,
};
use crate::plugins::ox_shared::types::timeout::{parse_timeout, read_positive_ms_arg, Wait};
use crate::plugins::ox_shared::value::{SharedRef, SharedValue};

/// Approximate per-slot footprint booked against `total_bytes` on
/// `try_reserve_budget` / `release_budget`. Mirrors the `128 + slots * 64`
/// formula in [`PoolInner::mem_bytes`].
const POOL_PER_SLOT_BYTES: isize = 64;

/// Stable per-thread identifier used as the idle-deque key and as the
/// FFI-level `owner` tag. Obtained via `pthread_self` so it round-trips
/// across the Rust↔C bridge without losing information.
pub type ThreadKey = u64;

/// Canonical thread key for the calling thread.
#[inline]
pub fn current_thread_key() -> ThreadKey {
    // SAFETY: `pthread_self` is documented to always succeed and return
    // the opaque handle for the calling thread; it has no failure path
    // to misuse. Casting via `usize` handles both macOS (pointer type)
    // and Linux (integer type).
    unsafe { libc::pthread_self() as usize as u64 }
}

/// How long the PHP request on a thread lasts, which bounds how long a
/// pool created on that thread can be used:
///
/// - `Thread`: pools live until dropped and serve every thread;
/// - `OneRequest`: pools serve that one request;
/// - `EachRequest`: pools serve the request that made them, and none can
///   be bound to a registry name.
pub use crate::plugin::php_thread::RequestScope;

thread_local! {
    static REQUEST_SCOPE: Cell<RequestScope> = const { Cell::new(RequestScope::Thread) };
    /// The pools [`end_request_on_current_thread`] visits: those this
    /// thread's request created, and those it minted slots in. Weak, so
    /// the list keeps no pool alive; a pool already gone is dropped from
    /// it on the next addition, since under `oxphp run` the hook that
    /// empties it runs only when the script ends.
    static REQUEST_POOLS: RefCell<Vec<Weak<Entry>>> = const { RefCell::new(Vec::new()) };
    /// The request open on this thread, as [`open_request_token`] numbered
    /// it; 0 before the first and once one has been shut down. A request
    /// whose startup failed is never shut down: its number stays until the
    /// next request replaces it, and its heap is not reset meanwhile.
    static OPEN_REQUEST: Cell<u64> = const { Cell::new(0) };
}

/// Source of request numbers. Process-wide, so that no two requests share
/// one, whichever threads they ran on: a thread's key can be handed to a
/// thread started after it has exited.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// Declare the calling thread's [`RequestScope`]. Called by the plugin's
/// `request_frame_opening` hook each time a PHP request is about to start
/// on the thread, before `php_request_startup`.
pub fn set_request_scope(scope: RequestScope) {
    REQUEST_SCOPE.with(|s| s.set(scope));
}

/// Number the request about to start on the calling thread. Called by the
/// plugin's `request_frame_opening` hook, before `php_request_startup`:
/// what the startup allocates already belongs to this request.
pub fn open_request_token() {
    let token = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    OPEN_REQUEST.with(|r| r.set(token));
}

/// The calling thread's request has been shut down and its heap reset.
/// Called by the plugin's `request_frame_closed` hook before anything else
/// there can tear a pool down.
pub fn close_request_token() {
    OPEN_REQUEST.with(|r| r.set(0));
}

/// The request open on the calling thread, or 0. Memory a pool took from
/// the request heap can be freed only while the request it was taken in
/// is this one.
pub(crate) fn current_request() -> u64 {
    OPEN_REQUEST.with(|r| r.get())
}

/// Why a slot of a pool with a `$destroy` left without `$destroy` being
/// called on it. Exported as `oxphp_shared_pool_destroy_skipped_total`.
#[derive(Clone, Copy)]
pub enum SkipReason {
    /// Its own thread freed it with no PHP code executing there.
    NoFrame = 0,
    /// The pool was torn down on another thread, which forgot it.
    OtherThread = 1,
    /// The request of its thread ended, and the slot with it.
    RequestEnded = 2,
}

static DESTROY_SKIPPED: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

fn record_destroy_skipped(count: u64, reason: SkipReason) {
    if count > 0 {
        DESTROY_SKIPPED[reason as usize].fetch_add(count, Ordering::Relaxed);
    }
}

/// Slots of every pool that left without `$destroy`, for `reason`.
pub fn destroy_skipped_total(reason: SkipReason) -> u64 {
    DESTROY_SKIPPED[reason as usize].load(Ordering::Relaxed)
}

/// Count the slot a destroy helper freed without calling `$destroy`.
fn note_destroy_rc(rc: c_int) {
    if rc == ffi::OXPHP_SHARED_INVOKE_SKIPPED {
        record_destroy_skipped(1, SkipReason::NoFrame);
    }
}

/// Whether a bailout a guarded helper caught on this thread is held back,
/// waiting for the C code the current call into Rust returns to to raise
/// it again.
fn bailout_held_back() -> bool {
    // SAFETY: reads a thread-local of the bridge; no arguments.
    unsafe { ffi::oxphp_bridge_deferred_bailout_pending() != 0 }
}

/// Whether `Shared\Registry` must refuse to bind a pool to a name on the
/// calling thread: the name would outlive every request that could use
/// the pool.
pub fn registry_refuses_pools_on_current_thread() -> bool {
    REQUEST_SCOPE.with(|s| s.get()) == RequestScope::EachRequest
}

/// Message of the `ClosedException` an `acquire` gets from a pool it may
/// not use; the registry's refusal of such a pool says the same thing.
pub const POOL_OUTSIDE_ITS_REQUEST: &str =
    "outside worker mode a Shared\\Pool can only be used by the request \
     that created it, and only until that request ends";

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
// `slot.owner` checks. PHP refcounted values referenced through a
// `zval*` are safe to hand between ZTS worker threads provided no single
// worker is executing user code on the same zval concurrently, which is
// upheld by the per-thread idle discipline.
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

    /// Per-thread count of acquires parked in a fiber, the other half of
    /// `waiting()`. Per thread because a parked acquire takes its share back
    /// down only once its park returns, and a fiber the server drops without
    /// resuming it — after a fatal error between two requests' slices, for
    /// one — never returns there: `reclaim_budget_for_dead_worker` drops the
    /// thread's count when the thread exits.
    parked: DashMap<ThreadKey, AtomicU64>,

    /// Factory/destroy callables. Owned by the PHP wrapper via explicit
    /// refcount; typed invocation lives in the FFI layer. `destroy_fcc`
    /// is nullable when the user passes `null` for `$destroy`. For a pool
    /// with a home thread, both are swapped to null when the request heap
    /// they live in is reset (see [`PoolInner::end_request_on`]). They are
    /// freed only if still set, and only by the request that allocated
    /// them (`created_in`).
    factory_fcc: AtomicPtr<c_void>,
    destroy_fcc: AtomicPtr<c_void>,

    /// The thread that created this pool, when that thread's request
    /// ends before the thread does. Only `home` may acquire, and only
    /// until `request_ended`. `None` on [`RequestScope::Thread`].
    home: Option<ThreadKey>,
    request_ended: AtomicBool,

    /// The request the callables were allocated in (see
    /// [`open_request_token`]); 0 if none was open.
    created_in: u64,

    closed: AtomicBool,
    self_id: OnceLock<SharedId>,
    /// Cached `Weak<Entry>` for the fast-path of [`track_size_delta`].
    /// See [`MapInner::self_entry`] for the rationale — same shape,
    /// same fallback to the id-based slow path for test fixtures.
    self_entry: OnceLock<Weak<Entry>>,

    // ── Observability counters ──────────────────────────────
    //
    // Exported as Prometheus `oxphp_shared_pool_*` series and surfaced
    // in the `/__ox_shared/entries/:id` JSON `type_specific` block.
    acquire_ok_total: AtomicU64,
    acquire_timeout_total: AtomicU64,
    acquire_closed_total: AtomicU64,
    /// Non-blocking `tryAcquire` calls that found the pool saturated.
    /// Distinct from `acquire_timeout_total` (no wait elapsed) so a
    /// backpressure poll loop is not mistaken for real wait-timeouts.
    acquire_saturated_total: AtomicU64,
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
    /// Teardown of the pool (`on_shutdown_notify`, `on_drop`), on the
    /// thread that owns the slots, with its request open. Only ever
    /// counted as the pool goes away, so no scrape sees it.
    Shutdown,
}

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
            parked: DashMap::new(),
            factory_fcc: AtomicPtr::new(factory_fcc),
            destroy_fcc: AtomicPtr::new(destroy_fcc),
            home: (REQUEST_SCOPE.with(|s| s.get()) != RequestScope::Thread)
                .then(current_thread_key),
            request_ended: AtomicBool::new(false),
            created_in: current_request(),
            closed: AtomicBool::new(false),
            self_id: OnceLock::new(),
            self_entry: OnceLock::new(),
            acquire_ok_total: AtomicU64::new(0),
            acquire_timeout_total: AtomicU64::new(0),
            acquire_closed_total: AtomicU64::new(0),
            acquire_saturated_total: AtomicU64::new(0),
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

    /// Bind this Pool to its registry entry. Production path: call from
    /// the creating FFI right after `registry.insert` with
    /// `Arc::downgrade(&entry_arc)`. Sets both id and the cached
    /// `Weak<Entry>` that [`track_size_delta`] uses to bypass the
    /// DashMap shard-lock on every `try_reserve_budget` / refund.
    pub fn bind_entry(&self, weak: Weak<Entry>) {
        if let Some(arc) = weak.upgrade() {
            let _ = self.self_id.set(arc.id);
        }
        let _ = self.self_entry.set(weak);
        self.remember_for_request_end();
    }

    /// Put this pool on the list [`end_request_on_current_thread`]
    /// visits, unless the calling thread's request lasts as long as the
    /// thread. Needs the entry bound.
    fn remember_for_request_end(&self) {
        if REQUEST_SCOPE.with(|s| s.get()) == RequestScope::Thread {
            return;
        }
        let Some(weak) = self.self_entry.get() else {
            return;
        };
        REQUEST_POOLS.with(|list| {
            let mut list = list.borrow_mut();
            list.retain(|w| w.strong_count() > 0);
            if !list.iter().any(|w| Weak::ptr_eq(w, weak)) {
                list.push(weak.clone());
            }
        });
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
        self.factory_fcc.load(Ordering::Acquire)
    }

    /// Opaque destroy handle. May be null.
    #[allow(dead_code)]
    pub(crate) fn destroy_fcc(&self) -> *mut c_void {
        self.destroy_fcc.load(Ordering::Acquire)
    }

    /// Whether what this pool keeps in a request heap is out of the
    /// calling thread's reach: the pool belongs to another thread's
    /// request, or to a request of this thread that has already ended.
    /// Such a thread is refused an `acquire` and a registry name for the
    /// pool, and tearing the pool down there forgets what it holds
    /// instead of freeing it.
    pub(crate) fn foreign_to_current_thread(&self) -> bool {
        self.home.is_some_and(|home| {
            home != current_thread_key() || self.request_ended.load(Ordering::Acquire)
        })
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
        let ok = self
            .size
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |cur| {
                if cur < max {
                    Some(cur + 1)
                } else {
                    None
                }
            })
            .is_ok();
        if ok {
            self.track_slot_delta(1);
        }
        ok
    }

    /// Undo a `try_reserve_budget`. Used when the PHP factory threw
    /// or a slot got destroyed out-of-band.
    pub fn release_budget(&self) {
        self.size.fetch_sub(1, Ordering::AcqRel);
        self.track_slot_delta(-1);
        // Destroying a slot frees capacity — nudge one waiter.
        self.waiters_cv.notify_one();
    }

    fn track_slot_delta(&self, delta_count: isize) {
        let delta = delta_count * POOL_PER_SLOT_BYTES;
        if let Some(weak) = self.self_entry.get() {
            if let Some(entry) = weak.upgrade() {
                entry.adjust_mem_bytes(delta);
            }
            return;
        }
        let Some(id) = self.self_id.get().copied() else {
            return;
        };
        let Some(reg) = REGISTRY.get() else {
            return;
        };
        reg.adjust_mem_bytes(id, delta);
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
    ///
    /// Also forgets the slots `key` had parked, refunding theirs: the
    /// thread has shut its last request down, and their memory went with
    /// it. Left in place, they would be handed out — or given to
    /// `$destroy` by the idle-timeout eviction — on a later thread that
    /// gets the same `pthread_t`.
    ///
    /// And drops the count of acquires `key` had parked in a fiber: one
    /// whose fiber the server dropped without resuming it is still counted.
    pub fn reclaim_budget_for_dead_worker(&self, key: ThreadKey) -> u64 {
        let count = self
            .in_flight
            .remove(&key)
            .map(|(_, counter)| counter.load(Ordering::Acquire))
            .unwrap_or(0);
        for _ in 0..count {
            self.release_budget();
        }
        let forgotten = self.forget_idle_of(key);
        self.record_skipped(forgotten, SkipReason::RequestEnded);
        self.parked.remove(&key);
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
    /// * **Cross-thread release, owner gone.** Forget the slot and refund
    ///   its budget: it lived in the heap of the owner's request, reset
    ///   when that request ended, so there is nothing left to free or to
    ///   hand `$destroy`.
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
            self.release_budget();
            self.record_skipped(1, SkipReason::RequestEnded);
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

    /// Drain stale slots from the **current thread's** own idle deque:
    /// every slot whose `last_active` is older than `idle_timeout`,
    /// stopping at the first fresh slot (deques are age-ordered, front =
    /// oldest). Called from `eviction::drain_stale_for_current_thread`
    /// while the thread has no request in flight, and like
    /// [`PoolInner::evict_all_on_current_thread`] it runs `$destroy` with
    /// nothing of the idle map held and a bailout held back — here for the
    /// serve loop to recover from, as it raises `thread_idle`.
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
        // Destroy outside the lock — `release_budget` notifies the waiter
        // condvar and we want no chance of deadlock. Outside the idle map
        // too: `$destroy` may use this pool, and a release parks its slot
        // through the map's write lock.
        drop(shard);
        self.destroy_taken(me, popped, reason)
    }

    /// Drain **all** idle slots from the current thread's own deque,
    /// regardless of age. Backs the manual `$pool->evict()` escape hatch
    /// (e.g. a downstream service restarted — flush idle resources so the
    /// next acquire mints fresh ones). The background idle-timeout
    /// scheduler keeps using [`PoolInner::evict_stale_on_current_thread`]
    /// for age-based policy; this method ignores `idle_timeout` entirely.
    ///
    /// Like its stale counterpart it touches only the caller's deque —
    /// cross-thread `$destroy` is unsafe under the per-thread affinity
    /// model — and runs `$destroy` with nothing of the idle map held.
    ///
    /// Called only from the `evict()` handler, so `$destroy` runs with a
    /// bailout held back for the dispatch stub to raise again.
    pub fn evict_all_on_current_thread(&self, reason: EvictReason) -> u64 {
        let me = current_thread_key();
        let Some(shard) = self.idle.get(&me) else {
            return 0;
        };
        let mut popped = Vec::new();
        {
            let mut deque = shard.lock().unwrap_or_else(|poison| poison.into_inner());
            while let Some(slot) = deque.pop_front() {
                popped.push(slot);
            }
        }
        drop(shard);
        self.destroy_taken(me, popped, reason)
    }

    /// Run `$destroy` on `taken`, slots thread `me` has just taken off its
    /// own deque, front first, with a bailout held back for whoever called
    /// into the evicting code to raise again or recover from. Returns how
    /// many were destroyed.
    ///
    /// The slot a bailout ended `$destroy` on counts as destroyed — its
    /// budget is refunded and nothing frees it — and no `$destroy` runs
    /// after it: the slots not reached yet go back to the deque, as old as
    /// they were.
    fn destroy_taken(&self, me: ThreadKey, taken: Vec<PoolSlot>, reason: EvictReason) -> u64 {
        let (destroyed, unreached) = self.destroy_until_bailout(taken);
        self.put_back(me, unreached.collect());
        self.record_evicted(destroyed, reason);
        destroyed
    }

    /// Run `$destroy` on `slots`, slots of the calling thread taken out of
    /// the pool, in order, with a bailout held back rather than raised, and
    /// stop at the first one a bailout ends. Returns how many were
    /// destroyed — the one a bailout ended counts: its budget is refunded
    /// and nothing frees it — and the slots not reached, whose budget is
    /// still taken.
    fn destroy_until_bailout(&self, slots: Vec<PoolSlot>) -> (u64, std::vec::IntoIter<PoolSlot>) {
        let mut destroyed = 0;
        let mut slots = slots.into_iter();
        for slot in slots.by_ref() {
            // SAFETY: `slot.resource` was minted by `oxphp_pool_factory_invoke`
            // on this thread, in the request still open on it (or is a
            // stand-in in host tests, which the mock never dereferences; the
            // production helper short-circuits on NULL). `destroy_fcc` is
            // either `null_mut()` (user opted out of $destroy, or
            // `end_request_on` forgot it with its request heap) or the
            // emalloc'd fcc from `oxphp_pool_fcc_new`.
            let rc = unsafe {
                ffi::oxphp_pool_destroy_invoke_guarded(self.destroy_fcc(), slot.resource)
            };
            note_destroy_rc(rc);
            self.release_budget();
            destroyed += 1;
            if rc == ffi::OXPHP_SHARED_INVOKE_BAILOUT {
                break;
            }
        }
        (destroyed, slots)
    }

    /// Return `slots`, oldest first, to `me`'s idle deque, each behind
    /// every slot there that is not younger than it. Slots can have been
    /// parked there since these were taken off — by the `$destroy` that
    /// bailed, or by a request on another fiber of this thread whose own
    /// `evict()` took slots off and put them back meanwhile — and putting
    /// these back in front of those would leave a fresh slot ahead of
    /// stale ones, where the idle-timeout scan, which reads the front only,
    /// stops.
    fn put_back(&self, me: ThreadKey, slots: Vec<PoolSlot>) {
        if slots.is_empty() {
            return;
        }
        let shard = self
            .idle
            .entry(me)
            .or_insert_with(|| Mutex::new(VecDeque::new()));
        let mut deque = shard.lock().unwrap_or_else(|e| e.into_inner());
        for slot in slots {
            let at = deque.partition_point(|s| s.last_active <= slot.last_active);
            deque.insert(at, slot);
        }
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

    /// Park the calling fiber for up to `step`, counted in `waiting()`
    /// while it is parked, and return what the park returned — see
    /// `oxphp_pool_park_guarded`. Nothing of the count is held locked across
    /// the park: the fibers that run on this thread meanwhile may read it or
    /// park here themselves.
    pub fn park_for_release(&self, step: Duration) -> c_int {
        let me = current_thread_key();
        self.parked
            .entry(me)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::AcqRel);
        let rc = unsafe { ffi::oxphp_pool_park_guarded(step.as_micros() as u64) };
        if let Some(count) = self.parked.get(&me) {
            count.fetch_sub(1, Ordering::AcqRel);
        }
        rc
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

    /// Whether the calling thread may free the idle slots it parked here:
    /// they were minted in its request, and that request is still open.
    fn frees_own_idle_here(&self) -> bool {
        current_request() != 0 && !self.foreign_to_current_thread()
    }

    /// Count `count` slots that left without `$destroy`, if there is one.
    fn record_skipped(&self, count: u64, reason: SkipReason) {
        if !self.destroy_fcc().is_null() {
            record_destroy_skipped(count, reason);
        }
    }

    /// The request on thread `me` has ended and its heap has been reset.
    /// Forget the slots `me` had parked — their memory is gone, so they
    /// are dropped without `$destroy` or any bridge call — and refund
    /// their capacity. If `me` is this pool's `home`, close the pool and
    /// forget its callables the same way.
    fn end_request_on(&self, me: ThreadKey) {
        let forgotten = self.forget_idle_of(me);
        self.record_skipped(forgotten, SkipReason::RequestEnded);
        if self.home == Some(me) {
            self.request_ended.store(true, Ordering::Release);
            self.close();
            self.factory_fcc
                .store(std::ptr::null_mut(), Ordering::Release);
            self.destroy_fcc
                .store(std::ptr::null_mut(), Ordering::Release);
        }
    }

    /// Drop the slots parked under `key` without touching them, and
    /// refund their capacity. Returns how many there were.
    fn forget_idle_of(&self, key: ThreadKey) -> u64 {
        let forgotten = self.idle.remove(&key).map_or(0, |(_, deque)| {
            deque.into_inner().unwrap_or_else(|e| e.into_inner()).len()
        });
        for _ in 0..forgotten {
            self.release_budget();
        }
        forgotten as u64
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    // ── observability helpers ─────────────────────────────────────────

    /// Authoritative capacity gauge: `in_use + idle`. This is what
    /// the PHP `count()` method returns.
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
        let on_condvar = *self.waiters.lock().unwrap_or_else(|e| e.into_inner());
        let parked: u64 = self
            .parked
            .iter()
            .map(|count| count.load(Ordering::Acquire))
            .sum();
        on_condvar + parked
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

    /// A non-blocking `tryAcquire` found the pool saturated. Not a wait
    /// timeout — record it separately and do not touch the wait histogram.
    #[inline]
    pub fn record_acquire_saturated(&self) {
        self.acquire_saturated_total.fetch_add(1, Ordering::Relaxed);
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
    pub fn acquire_saturated_total(&self) -> u64 {
        self.acquire_saturated_total.load(Ordering::Relaxed)
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

/// End-of-request hook for threads whose request ends before the thread
/// does ([`RequestScope::EachRequest`], [`RequestScope::OneRequest`]).
/// Must run right after `php_request_shutdown`: the request heap has been
/// reset by then, and with it every slot this thread had parked and the
/// callables of every pool its request created. Applies
/// [`PoolInner::end_request_on`] to the pools this request created or
/// minted slots in — no other pool can hold this thread's slots — so
/// neither a later request nor a registry drain reaches that memory.
///
/// Calls neither PHP nor the bridge. A pool whose last reference turns
/// out to be the one this hook holds is torn down here, as by any other
/// last holder, but the request's number has been cleared by then
/// ([`close_request_token`]), so that teardown frees nothing.
pub fn end_request_on_current_thread() {
    let me = current_thread_key();
    for weak in REQUEST_POOLS.with(|list| std::mem::take(&mut *list.borrow_mut())) {
        if let Some(entry) = weak.upgrade() {
            if let Some(pool) = entry.inner.as_any_pool() {
                pool.end_request_on(me);
            }
        }
    }
}

impl SharedInner for PoolInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Pool
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn debug_snapshot(&self) -> SharedValue {
        // Matches MapInner's pattern: expose the primary gauge as
        // Long. /preview renders it; /entry reads size() directly as
        // `type_specific.count`.
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
        // a worker is tearing down the pool directly: no thread can use
        // it any more. Runs on whichever thread let go of it last, with
        // a request open or not, with PHP code executing there or not.
        //
        // Only what this thread's open request allocated may be freed
        // here: freeing into another thread's heap aborts the process,
        // and an ended request's heap has already been reset. So this
        // thread's own idle slots are destroyed (`on_shutdown_notify`)
        // until a bailout ends that, every other deque is forgotten —
        // those slots, and the objects in them, go with the request of
        // the thread that minted them — and the callables are freed only
        // by the request that made them.
        self.on_shutdown_notify();

        // Keys first: removing from `idle` while iterating it deadlocks.
        // A deque of this thread still here is one it may not free: its
        // request has ended.
        let me = current_thread_key();
        let keys: Vec<ThreadKey> = self.idle.iter().map(|e| *e.key()).collect();
        for key in keys {
            let forgotten = self.forget_idle_of(key);
            let reason = if key == me {
                SkipReason::RequestEnded
            } else {
                SkipReason::OtherThread
            };
            self.record_skipped(forgotten, reason);
        }

        // SAFETY: a non-null `factory_fcc` was emalloc'd by
        // `oxphp_pool_fcc_new` when the pool was created, in the heap of
        // request `created_in` (`end_request_on` nulls it when a pool with
        // `home` outlives that request). `free` holds only while that
        // request is the one open on this thread, so the heap is this
        // thread's and has not been reset since. `destroy_fcc` is either
        // NULL or an fcc from the same path. The swap frees each at most
        // once; otherwise they are forgotten with that heap. Freeing one
        // releases its callable, which can run destructors — PHP, which may
        // not run while a bailout is held back (`on_shutdown_notify`): then
        // they are forgotten too, and go with the heap of the request open
        // here — at the end of the request the bailout ends, or, in worker
        // mode and on an `oxphp_async()` thread, when the thread exits.
        let free =
            self.created_in != 0 && self.created_in == current_request() && !bailout_held_back();
        for fcc in [&self.factory_fcc, &self.destroy_fcc] {
            let fcc = fcc.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if free && !fcc.is_null() {
                unsafe { ffi::oxphp_pool_fcc_free(fcc) };
            }
        }
    }

    fn on_shutdown_notify(&self) {
        // `SharedRegistry::drain` calls this on every entry during
        // graceful shutdown — on the main thread, with no request open;
        // in server mode the PHP threads still run then — and `on_drop`
        // calls it first. Close the pool first so new acquires fail fast
        // and blocked waiters wake and bail, then destroy the calling
        // thread's own idle slots, if it may free them.
        //
        // Every other thread's slots live in that thread's heap and are
        // left where they are: their owner can still evict them, and
        // worker code after `oxphp_worker()` returns runs while the
        // server stops. Whoever drops the pool last forgets what remains.
        //
        // Safe to invoke redundantly — a second call finds no deque of
        // this thread and leaves the fccs alone (those are `on_drop`'s).
        self.close();
        // Whatever let go of the pool is Rust, and a bailout in `$destroy`
        // is held back until it returns to the C code that raises it again;
        // no PHP may run before that. So no `$destroy` runs once one is
        // held back — from the slot that bailed here, or from an earlier
        // `$destroy` in the same call into Rust, as when a Map's clear()
        // lets go of two pools. The slots left stay in this thread's deque,
        // and `on_drop` forgets them as a request's that has ended: the
        // bailout ends this one.
        if !self.frees_own_idle_here() || bailout_held_back() {
            return;
        }
        // Out of the map before any `$destroy` runs, as in
        // `evict_all_on_current_thread`.
        let me = current_thread_key();
        let Some((_, deque)) = self.idle.remove(&me) else {
            return;
        };
        let slots = Vec::from(deque.into_inner().unwrap_or_else(|e| e.into_inner()));
        self.destroy_taken(me, slots, EvictReason::Shutdown);
    }

    fn children(&self, _out: &mut Vec<SharedRef>) {
        // Pool resources are raw zvals (arbitrary PHP objects), not
        // SharedValue::Shared. They never participate in the Map
        // cycle-detection graph, so there's nothing to expose here.
    }
}

// Helper trait for downcasting `Arc<dyn SharedInner>` to `&PoolInner`.
// Mirrors the `SharedInnerChannelExt` / `SharedInnerMapExt` pattern so
// the FFI can resolve a PoolId → &PoolInner. Implemented on
// `dyn SharedInner` (bare, no `+ Send + Sync`) to match the actual
// trait-object type stored in `Entry.inner`.
pub trait SharedInnerPoolExt {
    fn as_any_pool(&self) -> Option<&PoolInner>;
}

impl SharedInnerPoolExt for dyn SharedInner {
    fn as_any_pool(&self) -> Option<&PoolInner> {
        self.as_any().downcast_ref::<PoolInner>()
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
//   `PoolInner.factory_fcc` / `.destroy_fcc`. Freed on pool drop only
//   in the request that created the pool; dropped anywhere else, they
//   are left to that request's end.
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

/// How long a saturated acquire parks its fiber before it looks at the pool
/// again: the first step, and the cap the step doubles up to. The cap keeps a
/// slot given back on the waiter's thread from sitting idle for long; the
/// doubling keeps a long wait from waking every millisecond. The same pair the
/// SAPI polls with for a connection another fiber holds.
const POOL_PARK_STEP_FIRST: Duration = Duration::from_millis(1);
const POOL_PARK_STEP_MAX: Duration = Duration::from_millis(4);

/// Resolve a live `Entry` pointer to `&Entry`. Returns `Err(Type)` if
/// the entry belongs to a different Shared type. The caller must have
/// already null-checked `entry_ptr`.
fn lookup_pool<'a>(
    entry_ptr: *const Entry,
) -> Result<&'a Entry, crate::plugins::ox_shared::error::SharedError> {
    // SAFETY: `entry_ptr` is non-null (checked by the caller) and, per
    // the handle contract, a live `Arc::into_raw` pointer — the calling
    // PHP wrapper holds a strong ref through it.
    let entry: &Entry = unsafe { &*entry_ptr };
    debug_assert_eq!(entry.magic, ENTRY_MAGIC, "lookup_pool on freed Entry");
    if entry.type_tag != SharedType::Pool {
        set_last_error(format!(
            "id {} is not a Shared\\Pool (tag={:?})",
            entry.id, entry.type_tag
        ));
        return Err(crate::plugins::ox_shared::error::SharedError::Type);
    }
    Ok(entry)
}

/// Create a new `Shared\Pool`. The factory callable is captured via
/// the C bridge (`oxphp_pool_fcc_new`) into a heap-allocated fcc slot
/// and stored opaquely on the PoolInner. `destroy_callable_zval` may
/// be null.
///
/// `idle_timeout_s == 0` disables idle eviction (idle slots live until the
/// pool is dropped). Negative values are rejected at the PHP binding layer.
///
/// # Safety
/// `factory_callable_zval` must be a valid PHP callable zval; the
/// C bridge checks via `zend_fcall_info_init`. `out_ptr` must be
/// valid for writes of `*const Entry`.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_create(
    factory_callable_zval: *mut std::ffi::c_void,
    destroy_callable_zval: *mut std::ffi::c_void,
    max_size: u64,
    idle_timeout_s: f64,
    out_ptr: *mut *const Entry,
) -> c_int {
    if out_ptr.is_null() {
        set_last_error("out_ptr is null");
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

        // `idle_timeout_s == 0.0` → eviction disabled: idle slots live until
        // the pool is dropped (matches HikariCP `idleTimeout=0`). Negative
        // values are rejected at the PHP binding layer before reaching here.
        let idle_timeout = if idle_timeout_s > 0.0 {
            Duration::from_secs_f64(idle_timeout_s)
        } else {
            Duration::MAX
        };

        // Typed `Arc<PoolInner>` alongside the trait-object copy: lets
        // us call `bind_id` directly. The downcast would succeed here
        // by construction; keeping the typed handle avoids dressing a
        // static invariant as a runtime check.
        let typed = Arc::new(PoolInner::new(
            factory_fcc,
            destroy_fcc,
            max_size as usize,
            idle_timeout,
        ));
        let reg = registry();
        let arc = reg.insert(SharedType::Pool, typed.clone())?;
        typed.bind_entry(Arc::downgrade(&arc));
        // SAFETY: out_ptr checked non-null above.
        unsafe { *out_ptr = Arc::into_raw(arc) };
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
/// 3. Budget full → park the fiber for a step, or where there is none to
///    park, `wait_for_release` up to the deadline; then loop.
///
/// `timeout_ms`: -1 = wait forever, 0 = try only, >0 = milliseconds.
///
/// Returns `OXPHP_FIBER_UNWIND` when the fiber was destroyed while parked:
/// the caller returns to PHP without raising anything, and the VM unwinds
/// with the exit already pending. Its value is also `SharedError::Cycle`'s,
/// which no pool path returns, so callers check for it before mapping codes.
///
/// # Safety
/// `out_slot_zv_heap` and `out_owner_tid` must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_acquire(
    entry_ptr: *const Entry,
    timeout_ms: i64,
    out_slot_zv_heap: *mut *mut std::ffi::c_void,
    out_owner_tid: *mut u64,
) -> c_int {
    if out_slot_zv_heap.is_null() || out_owner_tid.is_null() {
        set_last_error("out params must be non-null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return crate::plugins::ox_shared::error::SharedError::StaleHandle.code();
    }
    unsafe {
        *out_slot_zv_heap = std::ptr::null_mut();
        *out_owner_tid = 0;
    }

    let mut unwound = false;
    let rc = ffi_entry(|| {
        let entry = lookup_pool(entry_ptr)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;

        let mut park_step = POOL_PARK_STEP_FIRST;
        let mut can_park = true;
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
            if pool.foreign_to_current_thread() {
                set_last_error(POOL_OUTSIDE_ITS_REQUEST);
                break 'acquire Err(crate::plugins::ox_shared::error::SharedError::Closed);
            }
            if pool.is_closed() {
                break 'acquire Err(crate::plugins::ox_shared::error::SharedError::Closed);
            }

            // 1. Local idle.
            if let Some(slot) = pool.try_acquire_local() {
                entry.registry.record_op(entry);
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
                    entry.registry.record_op(entry);
                    unsafe {
                        *out_slot_zv_heap = slot_heap;
                        *out_owner_tid = current_thread_key();
                    }
                    pool.track_acquired_by_me();
                    // The slot lives in this thread's request heap, and
                    // returns to this thread's idle deque.
                    pool.remember_for_request_end();
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
                } else if rc == ffi::OXPHP_SHARED_INVOKE_BAILOUT {
                    // A fatal error or a cancelled write ended the request
                    // in the factory call or around it; the dispatch stub
                    // raises it again.
                    set_last_error("Shared\\Pool factory ended by a bailout");
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

            // In a fiber, park rather than hold the thread: the holder of the
            // slot this waits for can be a request or task parked on this same
            // thread, and it gives the slot back only once it runs again. A
            // release does not wake a parked waiter, so the pool is looked at
            // again after every step, which starts short and doubles to a cap.
            if can_park {
                match pool.park_for_release(park_step.min(remaining)) {
                    1 => {
                        park_step = (park_step * 2).min(POOL_PARK_STEP_MAX);
                        continue;
                    }
                    // Nothing here to park: wait on the thread, below, for
                    // the rest of this call.
                    0 => can_park = false,
                    // A pending exit, not an error: this acquire has no
                    // outcome to record.
                    ffi::OXPHP_FIBER_UNWIND => {
                        unwound = true;
                        return Ok(());
                    }
                    -1 => {
                        set_last_error(
                            "Async task cancelled while waiting for a Shared\\Pool slot",
                        );
                        return Err(crate::plugins::ox_shared::error::SharedError::Cancelled);
                    }
                    ffi::OXPHP_SHARED_INVOKE_BAILOUT => {
                        // The server ended the request while it was parked
                        // here; the dispatch stub raises the bailout again.
                        set_last_error("Shared\\Pool wait ended by a bailout");
                        return Err(crate::plugins::ox_shared::error::SharedError::Generic);
                    }
                    rc => {
                        set_last_error(format!("Shared\\Pool fiber park failed with rc={rc}"));
                        return Err(crate::plugins::ox_shared::error::SharedError::Generic);
                    }
                }
            }
            pool.wait_for_release(remaining);
            // Loop back; the top-of-loop `is_closed` check converts
            // a close-during-wait wake-up into a `Closed` result.
        };

        // Record outcome in one place. A non-blocking `tryAcquire` that
        // found the pool saturated never waited, so it is recorded as a
        // distinct `saturated` result and skips the wait histogram —
        // otherwise it would inflate `acquire_timeout_total` and the wait
        // count, masking real wait-timeouts on the dashboard.
        match &result {
            Ok(()) => {
                pool.record_wait(call_start.elapsed());
                pool.record_acquire_ok();
            }
            Err(crate::plugins::ox_shared::error::SharedError::Timeout) if try_only => {
                pool.record_acquire_saturated();
            }
            Err(crate::plugins::ox_shared::error::SharedError::Timeout) => {
                pool.record_wait(call_start.elapsed());
                pool.record_acquire_timeout();
            }
            Err(crate::plugins::ox_shared::error::SharedError::Closed) => {
                pool.record_wait(call_start.elapsed());
                pool.record_acquire_closed();
            }
            Err(_) => { /* unreachable: factory errors and an ended park return early */ }
        }
        result
    });
    if unwound {
        ffi::OXPHP_FIBER_UNWIND
    } else {
        rc
    }
}

/// Return a resource to the pool. `slot_zv_heap` is the pointer
/// previously handed out by `oxphp_shared_pool_acquire`. `owner_tid`
/// routes the slot to the correct idle deque — callers extract this
/// from the Handle wrapper's `rust_data`.
///
/// v1 contract: release always succeeds as long as the pool id is
/// alive and the slot pointer is non-null. If the owner's deque
/// does not yet exist (factory-minted slot never deposited) it is
/// created lazily; a slot whose owner thread has exited is forgotten
/// (see `PoolInner::release`).
///
/// # Safety
/// `slot_zv_heap` must be a pointer that originated from this same
/// pool's acquire path (not forged, not from a different pool).
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_release(
    entry_ptr: *const Entry,
    slot_zv_heap: *mut std::ffi::c_void,
    owner_tid: u64,
) -> c_int {
    if slot_zv_heap.is_null() {
        set_last_error("slot_zv_heap is null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return crate::plugins::ox_shared::error::SharedError::StaleHandle.code();
    }

    ffi_entry(|| {
        let entry = lookup_pool(entry_ptr)?;
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
        entry.registry.record_op(entry);
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
    entry_ptr: *const Entry,
    timeout_ms: i64,
    body_callable_zv: *mut std::ffi::c_void,
    user_out_zv: *mut std::ffi::c_void,
) -> c_int {
    if body_callable_zv.is_null() || user_out_zv.is_null() {
        set_last_error("body/out must be non-null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return crate::plugins::ox_shared::error::SharedError::StaleHandle.code();
    }

    // Step 1: acquire. Bubble errors directly.
    let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut owner: u64 = 0;
    let rc =
        unsafe { oxphp_shared_pool_acquire(entry_ptr, timeout_ms, &mut slot_heap, &mut owner) };
    if rc != 0 {
        return rc;
    }

    // Step 2: body($resource). Always attempt release afterwards.
    let body_rc = unsafe { ffi::oxphp_pool_body_invoke(body_callable_zv, slot_heap, user_out_zv) };

    // Step 3: release — never skipped. `with()`'s contract is
    // resource-safe even on body throw.
    let release_rc = unsafe { oxphp_shared_pool_release(entry_ptr, slot_heap, owner) };

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
    if body_rc == ffi::OXPHP_SHARED_INVOKE_BAILOUT {
        // A fatal error or a cancelled write ended the request in the
        // body call or around it. The slot went back above, as on a
        // throw; the dispatch stub raises the bailout again.
        set_last_error("body ended by a bailout");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if body_rc == -1 {
        set_last_error("body is not a valid PHP callable");
        return crate::plugins::ox_shared::error::SharedError::Type.code();
    }
    // Body succeeded. Surface any release-time error.
    release_rc
}

/// Manual eviction: force-evict **all** idle slots reachable from the
/// calling worker thread now, regardless of `idleTimeoutMs`, and return
/// the count. The operational "flush idle now" escape hatch. The
/// background walker (see `eviction.rs`) handles age-based eviction
/// separately and continuously.
///
/// # Safety
/// `out_evicted` must be valid for a `u64` write if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_evict(
    entry_ptr: *const Entry,
    out_evicted: *mut u64,
) -> c_int {
    if entry_ptr.is_null() {
        return crate::plugins::ox_shared::error::SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry = lookup_pool(entry_ptr)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;
        // User-driven synchronous counterpart to the background
        // scheduler's request_evict flag. Destroy runs on the caller's
        // thread, which is the owner of the slots we're reaping — no
        // cross-thread hazard. Same constraint as Shared\Pool::acquire:
        // callers must be inside a live PHP request.
        let evicted = pool.evict_all_on_current_thread(EvictReason::Manual);
        if !out_evicted.is_null() {
            unsafe { *out_evicted = evicted };
        }
        Ok(())
    })
}

/// Read the three volatile gauge counters for `Shared\Pool::stats()`
/// (inUse / idle / waiting) as one point-in-time sample. `idle` is read
/// once and `inUse` is derived from the same sample so the pair is
/// internally consistent; the counters are not lock-coupled across the
/// pool, so this is a point-in-time read, not an atomic one.
///
/// # Safety
/// Each non-null out pointer must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_stats(
    entry_ptr: *const Entry,
    out_in_use: *mut u64,
    out_idle: *mut u64,
    out_waiting: *mut u64,
) -> c_int {
    if entry_ptr.is_null() {
        return crate::plugins::ox_shared::error::SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry = lookup_pool(entry_ptr)?;
        let pool = entry
            .inner
            .as_any_pool()
            .ok_or(crate::plugins::ox_shared::error::SharedError::Type)?;
        // Sample `idle` exactly once and derive `in_use` from that same
        // sample (rather than calling `in_use_count()`, which would read
        // `idle` a second time). This keeps the reported (in_use, idle)
        // pair internally consistent — in_use + idle == size for the size
        // read here — even though the counters are not lock-coupled across
        // the pool, so the snapshot is point-in-time, not atomic.
        let idle = pool.idle_count();
        let in_use = (pool.size() as usize).saturating_sub(idle);
        if !out_in_use.is_null() {
            unsafe { *out_in_use = in_use as u64 };
        }
        if !out_idle.is_null() {
            unsafe { *out_idle = idle as u64 };
        }
        if !out_waiting.is_null() {
            unsafe { *out_waiting = pool.waiting_count() };
        }
        Ok(())
    })
}

/// `$pool->count()` — `in_use + idle` across every thread.
///
/// # Safety
/// `out_size` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_pool_size(
    entry_ptr: *const Entry,
    out_size: *mut u64,
) -> c_int {
    if out_size.is_null() {
        set_last_error("out_size is null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return crate::plugins::ox_shared::error::SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry = lookup_pool(entry_ptr)?;
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
pub unsafe extern "C" fn oxphp_shared_pool_max_size(
    entry_ptr: *const Entry,
    out_max: *mut u64,
) -> c_int {
    if out_max.is_null() {
        set_last_error("out_max is null");
        return crate::plugins::ox_shared::error::SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return crate::plugins::ox_shared::error::SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry = lookup_pool(entry_ptr)?;
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
use crate::plugin::types::{MagicMethod, PhpType, PhpValue, Visibility};
use crate::plugin::{PhpError, PluginContext, PluginError};

/// FQN of the value object returned by `Shared\Pool::stats()`.
const STATS_FQN: &str = "OxPHP\\Shared\\Pool\\Stats";
use crate::plugins::ox_shared::error::{read_last_error_message, SharedError};

/// Custom storage attached to every `Shared\Pool\Handle` instance via
/// `with_storage`. Layout is `#[repr(C)]` so the C bridge helper
/// `oxphp_shared_pool_handle_alloc` can write the fields at fixed
/// offsets; reads and the released-state reset happen Rust-side.
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

    /// Return the slot to its pool and mark this storage released.
    ///
    /// Idempotent: a null `slot_zv_ptr` is a no-op, so a second
    /// `Handle::release()` (or a `__destruct` after an explicit release)
    /// does nothing. If the pool entry is already gone — the owner dropped
    /// the `Pool` while still holding a `Handle` — the slot is silently
    /// leaked, consistent with the v1 drop-leak policy.
    fn release_slot(&mut self) {
        if self.slot_zv_ptr.is_null() {
            return;
        }
        if let Some(reg) = crate::plugins::ox_shared::registry::REGISTRY.get() {
            if let Ok(entry) = reg.lookup(self.pool_id) {
                if let Some(pool) = (*entry.inner).as_any_pool() {
                    let slot = PoolSlot::new(self.slot_zv_ptr, self.owner_tid);
                    // Mirror `oxphp_shared_pool_release`: decrement the
                    // in-flight counter for the slot's owner before parking
                    // it. Acquire bumped it via `track_acquired_by_me`; if
                    // this handle path skipped the matching untrack, the
                    // owner's in-flight count would never fall, and a later
                    // `reclaim_budget_for_dead_worker` (worker scale-down or
                    // panic-respawn) would over-refund — `release_budget`
                    // would underflow `size` (u64) and brick the pool.
                    pool.untrack_released(self.owner_tid);
                    pool.release(slot);
                    // Observability parity with the FFI / with() release.
                    entry.registry.record_op(&entry);
                }
            }
        }
        // Zero the slot so Drop / a second release() is a no-op (the
        // `$released` guard in PHP terms).
        self.slot_zv_ptr = std::ptr::null_mut();
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
        // RAII auto-release: the user let the Handle fall out of scope
        // (including stack unwind on exception) without calling release().
        // No-op if already released (slot_zv_ptr null).
        self.release_slot();
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
        -7 => "OxPHP\\Shared\\OperationTimeoutException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        // The acquire's wait left because its async task was cancelled.
        -12 => "OxPHP\\Async\\AsyncException",
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
    register_stats_class(ctx)?;
    register_pool_class(ctx)?;
    Ok(())
}

/// `OxPHP\Shared\Pool\Stats` — an immutable snapshot of pool counters.
/// Mirrors `Channel\RecvResult`: the values live in private properties set
/// once from Rust at construction, and the public surface is read-only
/// accessor methods. There is no public mutable state, so the object is
/// effectively immutable.
fn register_stats_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class(STATS_FQN)
        .final_()
        .property("__inUse", PhpType::Int, Visibility::Private)
        .property("__idle", PhpType::Int, Visibility::Private)
        .property("__waiting", PhpType::Int, Visibility::Private)
        .property("__size", PhpType::Int, Visibility::Private)
        .property("__maxSize", PhpType::Int, Visibility::Private)
        .method("inUse")
        .returns(PhpType::Int)
        .handler(|call| stats_field(call, "__inUse"))
        .method("idle")
        .returns(PhpType::Int)
        .handler(|call| stats_field(call, "__idle"))
        .method("waiting")
        .returns(PhpType::Int)
        .handler(|call| stats_field(call, "__waiting"))
        .method("size")
        .returns(PhpType::Int)
        .handler(|call| stats_field(call, "__size"))
        .method("maxSize")
        .returns(PhpType::Int)
        .handler(|call| stats_field(call, "__maxSize"))
        .method("utilization")
        .returns(PhpType::Float)
        .handler(stats_utilization)
        .build()?;
    Ok(())
}

/// Read one private `Pool\Stats` counter property and return it as the
/// method result.
fn stats_field(call: &mut NativeCall, prop: &str) -> Result<(), PhpError> {
    let value = call.read_long_property(prop)?;
    call.ret_long(value);
    Ok(())
}

/// `Pool\Stats::utilization(): float` — `inUse / maxSize`, or `0.0` when
/// `maxSize` is 0.
fn stats_utilization(call: &mut NativeCall) -> Result<(), PhpError> {
    let in_use = call.read_long_property("__inUse")?;
    let max = call.read_long_property("__maxSize")?;
    let value = if max > 0 {
        in_use as f64 / max as f64
    } else {
        0.0
    };
    call.ret_double(value);
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
        // release(): void — return the slot to the pool now, before scope
        // end. Idempotent (a second call is a no-op) and also runs on
        // __destruct (RAII).
        .method("release")
        .returns(PhpType::Void)
        .handler(|call| {
            call.storage_mut::<PoolHandleStorage>()?.release_slot();
            Ok(())
        })
        .build()?;
    Ok(())
}

fn handle_get(call: &mut NativeCall) -> Result<(), PhpError> {
    let storage = call.storage::<PoolHandleStorage>()?;
    if storage.slot_zv_ptr.is_null() {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\StaleHandleException".to_string(),
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
        .final_()
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| crate::plugins::ox_shared::handle::SharedHandle::new(SharedType::Pool))
        .magic(MagicMethod::Clone)
        .handler(|_call| Err(throw_clone_forbidden()))
        // __construct(callable $factory, ?callable $destroy = null,
        //             int $maxSize = 32, int $idleTimeoutMs = 300_000)
        .method("__construct")
        .param("factory", PhpType::Callable)
        .optional_param("destroy", PhpType::Callable, PhpValue::Null)
        .optional_param("maxSize", PhpType::Int, PhpValue::Int(32))
        .optional_param("idleTimeoutMs", PhpType::Int, PhpValue::Int(300_000))
        .handler(pool_construct)
        // acquire(): Shared\Pool\Handle — wait forever
        .method("acquire")
        .returns(PhpType::Object)
        .handler(|call| pool_acquire_impl(call, -1, false))
        // tryAcquire(): ?Shared\Pool\Handle — non-blocking; null if saturated
        .method("tryAcquire")
        .returns(PhpType::Nullable(Box::new(PhpType::Object)))
        .handler(|call| pool_acquire_impl(call, 0, true))
        // acquireTimeout(int $ms): Shared\Pool\Handle — bounded wait; $ms > 0
        .method("acquireTimeout")
        .param("ms", PhpType::Int)
        .returns(PhpType::Object)
        .handler(|call| {
            let ms = read_positive_ms_arg(call, 0)?;
            pool_acquire_impl(call, ms, false)
        })
        // with(callable $body): mixed — wait forever
        .method("with")
        .param("body", PhpType::Callable)
        .returns(PhpType::Mixed)
        .handler(|call| pool_with_impl(call, -1))
        // withTimeout(callable $body, int $ms): mixed — bounded wait; $ms > 0
        .method("withTimeout")
        .param("body", PhpType::Callable)
        .param("ms", PhpType::Int)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let ms = read_positive_ms_arg(call, 1)?;
            pool_with_impl(call, ms)
        })
        // evict(): int — force-evict all idle slots now; returns the count
        .method("evict")
        .returns(PhpType::Int)
        .handler(pool_evict)
        // stats(): Shared\Pool\Stats — point-in-time snapshot of counters
        .method("stats")
        .returns(PhpType::Object)
        .handler(pool_stats)
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

    let idle_timeout_ms = if call.argc() > 3 {
        call.arg_long(3).unwrap_or(300_000)
    } else {
        300_000
    };
    if idle_timeout_ms < 0 {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: "idleTimeoutMs must be >= 0".to_string(),
            code: 0,
        });
    }
    // The core FFI takes seconds (f64); 0 maps to "disabled" downstream.
    let idle_timeout_s = idle_timeout_ms as f64 / 1000.0;

    let mut out_ptr: *const Entry = std::ptr::null();
    let rc = unsafe {
        oxphp_shared_pool_create(
            factory_zv,
            destroy_zv,
            max_size as u64,
            idle_timeout_s,
            &mut out_ptr,
        )
    };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::__construct"));
    }

    let handle = call.storage_mut::<crate::plugins::ox_shared::handle::SharedHandle>()?;
    handle.entry_ptr = out_ptr;
    handle.type_tag = SharedType::Pool as u8;
    Ok(())
}

fn pool_get_entry_ptr(call: &NativeCall) -> Result<*const Entry, PhpError> {
    let handle = call.storage::<crate::plugins::ox_shared::handle::SharedHandle>()?;
    if !handle.is_initialized() {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\UninitializedException".to_string(),
            message: "Shared\\Pool wrapper is uninitialised".to_string(),
            code: 0,
        });
    }
    Ok(handle.entry_ptr)
}

/// Shared implementation behind `acquire` / `tryAcquire` / `acquireTimeout`.
///
/// `timeout_ms`: -1 forever, 0 try, >0 bounded.
/// `null_on_saturation`: when true (`tryAcquire`), a saturated pool yields
/// PHP `null` instead of throwing.
fn pool_acquire_impl(
    call: &mut NativeCall,
    timeout_ms: i64,
    null_on_saturation: bool,
) -> Result<(), PhpError> {
    let entry_ptr = pool_get_entry_ptr(call)?;

    let mut slot_heap: *mut c_void = std::ptr::null_mut();
    let mut owner: u64 = 0;
    let rc =
        unsafe { oxphp_shared_pool_acquire(entry_ptr, timeout_ms, &mut slot_heap, &mut owner) };
    if rc == ffi::OXPHP_FIBER_UNWIND {
        // The fiber was destroyed while parked; the exit is already pending.
        return Ok(());
    }
    if rc != 0 {
        // tryAcquire: a saturated pool surfaces as `SharedError::Timeout`,
        // which is the expected non-blocking outcome — return PHP null
        // rather than throwing.
        if null_on_saturation && rc == SharedError::Timeout.code() {
            call.ret_null();
            return Ok(());
        }
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::acquire"));
    }

    // The Handle wrapper and the C bridge identify the pool by its
    // registry id; derive it from the entry pointer.
    let id = unsafe { crate::plugins::ox_shared::registry::oxphp_shared_entry_id(entry_ptr) };

    // Wrap (slot_heap, owner) in a Shared\Pool\Handle. The C bridge
    // allocates the object into retval and populates rust_data.
    let rc_alloc =
        unsafe { ffi::oxphp_shared_pool_handle_alloc(call.retval_ptr(), id, owner, slot_heap) };
    if rc_alloc != 0 {
        // Could not construct the Handle — route the slot back so
        // we don't leak it, then surface the error.
        let _ = unsafe { oxphp_shared_pool_release(entry_ptr, slot_heap, owner) };
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

fn pool_with_impl(call: &mut NativeCall, timeout_ms: i64) -> Result<(), PhpError> {
    let entry_ptr = pool_get_entry_ptr(call)?;
    let body_zv = unsafe { call.raw_arg_ptr(0) };
    let rc = unsafe { oxphp_shared_pool_with(entry_ptr, timeout_ms, body_zv, call.retval_ptr()) };
    if rc == ffi::OXPHP_FIBER_UNWIND {
        // The acquire's fiber was destroyed while parked; the exit is
        // already pending.
        return Ok(());
    }
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::with"));
    }
    Ok(())
}

fn pool_evict(call: &mut NativeCall) -> Result<(), PhpError> {
    let entry_ptr = pool_get_entry_ptr(call)?;
    let mut evicted: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_evict(entry_ptr, &mut evicted) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::evict"));
    }
    call.ret_long(evicted as i64);
    Ok(())
}

fn pool_stats(call: &mut NativeCall) -> Result<(), PhpError> {
    let entry_ptr = pool_get_entry_ptr(call)?;
    let mut in_use: u64 = 0;
    let mut idle: u64 = 0;
    let mut waiting: u64 = 0;
    let rc = unsafe { oxphp_shared_pool_stats(entry_ptr, &mut in_use, &mut idle, &mut waiting) };
    if rc != 0 {
        return Err(pool_rc_to_phperr(rc, "Shared\\Pool::stats"));
    }
    let mut max_size: u64 = 0;
    let rc2 = unsafe { oxphp_shared_pool_max_size(entry_ptr, &mut max_size) };
    if rc2 != 0 {
        return Err(pool_rc_to_phperr(rc2, "Shared\\Pool::stats"));
    }
    let size = in_use + idle;

    // Build a Pool\Stats value object into the retval and populate its five
    // private counters. inUse and idle came from one point-in-time sample
    // (idle read once), so `size == inUse + idle` holds for the object; the
    // counters are not lock-coupled across the pool, so treat the snapshot
    // as point-in-time, not atomic.
    let retval = call.retval_ptr();
    let rc_make = unsafe {
        ffi::oxphp_bridge_make_object(retval, STATS_FQN.as_ptr() as *const _, STATS_FQN.len())
    };
    if rc_make != 0 {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\SharedException".to_string(),
            message: "Shared\\Pool::stats: could not allocate Pool\\Stats".to_string(),
            code: 0,
        });
    }
    unsafe {
        ffi::oxphp_bridge_object_set_property_long(
            retval,
            b"__inUse".as_ptr() as *const _,
            "__inUse".len(),
            in_use as i64,
        );
        ffi::oxphp_bridge_object_set_property_long(
            retval,
            b"__idle".as_ptr() as *const _,
            "__idle".len(),
            idle as i64,
        );
        ffi::oxphp_bridge_object_set_property_long(
            retval,
            b"__waiting".as_ptr() as *const _,
            "__waiting".len(),
            waiting as i64,
        );
        ffi::oxphp_bridge_object_set_property_long(
            retval,
            b"__size".as_ptr() as *const _,
            "__size".len(),
            size as i64,
        );
        ffi::oxphp_bridge_object_set_property_long(
            retval,
            b"__maxSize".as_ptr() as *const _,
            "__maxSize".len(),
            max_size as i64,
        );
    }
    Ok(())
}

fn pool_id_method(call: &mut NativeCall) -> Result<(), PhpError> {
    let entry_ptr = pool_get_entry_ptr(call)?;
    let id = unsafe { crate::plugins::ox_shared::registry::oxphp_shared_entry_id(entry_ptr) };
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

    #[cfg(not(feature = "php"))]
    #[test]
    fn release_cross_thread_dead_owner_forgets() {
        use crate::bridge::ffi::mock_pool_heap_calls;
        // A ghost owner key that was never registered (or has since
        // unregistered). Its heap went with it: cross-thread release
        // forgets the slot, refunding its budget without freeing it.
        let p = inner_with_fccs(2);
        const GHOST_KEY: ThreadKey = 0xDEAD_0000_0000_0001;

        assert!(p.try_reserve_budget());
        let calls = mock_pool_heap_calls();
        let skipped = destroy_skipped_total(SkipReason::RequestEnded);
        p.release(dummy_slot(GHOST_KEY));

        assert_eq!(
            mock_pool_heap_calls(),
            calls,
            "a dead thread's heap is gone"
        );
        assert!(destroy_skipped_total(SkipReason::RequestEnded) > skipped);
        assert_eq!(p.size(), 0, "forgetting must refund budget");
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

    #[cfg(not(feature = "php"))]
    #[test]
    fn on_shutdown_notify_drains_idle_deques() {
        use crate::plugins::ox_shared::worker_liveness;
        // Populate three slots across two live owner keys, then
        // drain with a request open here. Under the mock bridge,
        // `oxphp_pool_destroy_invoke_guarded` is a no-op returning 0 — we only
        // observe the Rust-side bookkeeping (size, idle_count, closed
        // flag). Both owner keys are registered so the cross-thread
        // release parks rather than forgetting the slot.
        open_request_token();
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
        assert_eq!(p.size(), 1, "every destroyed slot frees budget");
        assert_eq!(
            p.idle_by_thread(),
            vec![(ALIVE_B, 1)],
            "this thread's deque drained, B's left to B"
        );

        worker_liveness::force_remove(ALIVE_B);
        close_request_token();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn on_shutdown_notify_is_idempotent() {
        // Second drain pass must see an empty pool and not panic.
        open_request_token();
        let p = inner(2);
        assert!(p.try_reserve_budget());
        p.deposit_new(dummy_slot(current_thread_key()));

        p.on_shutdown_notify();
        assert_eq!(p.size(), 0);

        p.on_shutdown_notify(); // no-op second call
        assert_eq!(p.size(), 0);
        assert!(p.is_closed());
        close_request_token();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn on_drop_drains_and_frees_fccs() {
        // NULL fccs + null-resource slots + mock bridge means this
        // reduces to exercising the Rust-side drain path. We check
        // that on_drop:
        //   (1) closes the pool,
        //   (2) empties the idle deques,
        //   (3) decrements size to zero.
        // NULL fccs are not handed to `oxphp_pool_fcc_free` at all.
        open_request_token();
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
        assert_eq!(p.evicted_shutdown_total(), 2, "drained, not forgotten");
        close_request_token();
    }

    /// `inner()` with stand-in callables, which the host mock never
    /// dereferences.
    #[cfg(not(feature = "php"))]
    fn inner_with_fccs(max: usize) -> Arc<PoolInner> {
        Arc::new(PoolInner::new(
            0xFAC0_0061 as *mut c_void,
            0xDE50_0061 as *mut c_void,
            max,
            Duration::from_secs(300),
        ))
    }

    /// Park one slot in `owner`'s deque, as a release from that thread does.
    #[cfg(not(feature = "php"))]
    fn park_under(p: &PoolInner, owner: ThreadKey) {
        assert!(p.try_reserve_budget());
        p.idle
            .entry(owner)
            .or_insert_with(|| Mutex::new(VecDeque::new()))
            .lock()
            .unwrap()
            .push_back(dummy_slot(owner));
    }

    // Counts calls on the host mock bridge; the stand-in pointers it uses
    // must never reach the real one.
    #[cfg(not(feature = "php"))]
    #[test]
    fn teardown_off_request_destroys_nothing() {
        use crate::bridge::ffi::mock_pool_heap_calls;
        const OTHER: ThreadKey = 0xBEEF_0000_0000_0061;

        // A thread with no request open, as the main thread draining the
        // registry at shutdown is: a slot of its own, one of another thread.
        thread::spawn(|| {
            let p = inner_with_fccs(2);
            let me = current_thread_key();
            assert!(p.try_reserve_budget());
            p.deposit_new(dummy_slot(me));
            park_under(&p, OTHER);
            let calls = mock_pool_heap_calls();

            p.on_shutdown_notify();

            assert!(p.is_closed());
            assert_eq!(
                mock_pool_heap_calls(),
                calls,
                "no request heap is in reach here"
            );
            assert_eq!(p.idle_by_thread().len(), 2, "both slots are left");
            assert_eq!(p.size(), 2);
            assert_eq!(p.evicted_shutdown_total(), 0);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn drop_forgets_slots_parked_by_another_thread() {
        use crate::bridge::ffi::mock_pool_heap_calls;
        const OTHER: ThreadKey = 0xBEEF_0000_0000_0062;

        thread::spawn(|| {
            open_request_token();
            let p = inner_with_fccs(4);
            let me = current_thread_key();
            assert!(p.try_reserve_budget());
            p.deposit_new(dummy_slot(me));
            park_under(&p, OTHER);
            let calls = mock_pool_heap_calls();
            let skipped = destroy_skipped_total(SkipReason::OtherThread);

            p.on_drop();

            // Its own slot and the callables its request made, nothing of
            // the other thread's.
            assert_eq!(mock_pool_heap_calls() - calls, 1 + 2);
            assert_eq!((p.size(), p.idle_count()), (0, 0));
            assert_eq!(p.evicted_shutdown_total(), 1);
            assert!(destroy_skipped_total(SkipReason::OtherThread) > skipped);
            close_request_token();
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn drop_frees_callables_only_in_the_request_that_created_them() {
        use crate::bridge::ffi::mock_pool_heap_calls;

        thread::spawn(|| {
            // Made and dropped with no request open: nothing says the heap it
            // was made in has not been reset since.
            let p = inner_with_fccs(1);
            let calls = mock_pool_heap_calls();
            p.on_drop();
            assert_eq!(mock_pool_heap_calls(), calls, "no request made them");

            open_request_token();

            // Dropped on another thread, inside a request of its own.
            let p = inner_with_fccs(1);
            let freed = {
                let p = Arc::clone(&p);
                thread::spawn(move || {
                    open_request_token();
                    let calls = mock_pool_heap_calls();
                    p.on_drop();
                    close_request_token();
                    mock_pool_heap_calls() - calls
                })
                .join()
                .unwrap()
            };
            assert_eq!(freed, 0, "another thread's heap is out of reach");
            assert!(p.factory_fcc().is_null() && p.destroy_fcc().is_null());

            // Dropped on this thread, in a later request than the one that
            // made it.
            let p = inner_with_fccs(1);
            close_request_token();
            open_request_token();
            let calls = mock_pool_heap_calls();
            p.on_drop();
            assert_eq!(mock_pool_heap_calls(), calls, "that heap has been reset");

            // Dropped in the request that made it.
            let p = inner_with_fccs(1);
            let calls = mock_pool_heap_calls();
            p.on_drop();
            assert_eq!(mock_pool_heap_calls() - calls, 2, "both callables");
            close_request_token();
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn destroy_skipped_without_a_frame_is_counted() {
        use crate::bridge::ffi::{mock_pool_destroy, OXPHP_SHARED_INVOKE_SKIPPED};

        thread::spawn(|| {
            open_request_token();
            let p = inner_with_fccs(1);
            assert!(p.try_reserve_budget());
            p.deposit_new(dummy_slot(current_thread_key()));
            mock_pool_destroy(Some(Box::new(|_| OXPHP_SHARED_INVOKE_SKIPPED)));
            let skipped = destroy_skipped_total(SkipReason::NoFrame);

            p.on_drop();

            mock_pool_destroy(None);
            assert!(destroy_skipped_total(SkipReason::NoFrame) > skipped);
            assert_eq!(
                p.evicted_shutdown_total(),
                1,
                "the slot was still freed on its own thread"
            );
            close_request_token();
        })
        .join()
        .unwrap();
    }

    /// The last reference to a pool goes away on the thread that parked
    /// three slots in it, with its request open, and `$destroy` bails on
    /// the first of them. The bailout is held back for the code that raises
    /// it again, and nothing runs PHP after it: not `$destroy` on the other
    /// two, not the release of the callables, whose destructors are PHP.
    #[cfg(not(feature = "php"))]
    #[test]
    fn drop_holds_back_a_bailout_in_destroy() {
        use crate::bridge::ffi::{
            mock_pool_destroy, mock_pool_heap_calls, mock_take_deferred_bailout,
            OXPHP_SHARED_INVOKE_BAILOUT,
        };
        on_own_thread_within_5s(|| {
            open_request_token();
            let p = inner_with_fccs(3);
            park_three(&p);
            let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
            let log = std::rc::Rc::clone(&seen);
            mock_pool_destroy(Some(Box::new(move |slot| {
                log.borrow_mut().push(slot as usize);
                OXPHP_SHARED_INVOKE_BAILOUT
            })));
            let calls = mock_pool_heap_calls();
            let skipped = destroy_skipped_total(SkipReason::RequestEnded);

            p.on_drop();
            mock_pool_destroy(None);

            assert_eq!(*seen.borrow(), vec![1], "nothing after the bailout");
            assert!(mock_take_deferred_bailout());
            assert_eq!(
                mock_pool_heap_calls() - calls,
                1,
                "the one $destroy, and neither callable"
            );
            // 1 counts as destroyed; 2 and 3 are let go without $destroy.
            // Every slot's budget is back.
            assert_eq!((p.size(), p.idle_count()), (0, 0));
            assert_eq!(p.evicted_shutdown_total(), 1);
            assert!(destroy_skipped_total(SkipReason::RequestEnded) >= skipped + 2);
            close_request_token();
        });
    }

    /// The same drop with a bailout already held back — by a `$destroy`
    /// that ran earlier in the same Rust call — runs no PHP at all.
    #[cfg(not(feature = "php"))]
    #[test]
    fn drop_with_a_bailout_held_back_runs_no_php() {
        use crate::bridge::ffi::{
            mock_hold_deferred_bailout, mock_pool_destroy, mock_pool_heap_calls,
            mock_take_deferred_bailout,
        };
        on_own_thread_within_5s(|| {
            open_request_token();
            let p = inner_with_fccs(3);
            park_three(&p);
            mock_pool_destroy(Some(Box::new(|_| 0)));
            mock_hold_deferred_bailout();
            let calls = mock_pool_heap_calls();
            let skipped = destroy_skipped_total(SkipReason::RequestEnded);

            p.on_drop();
            mock_pool_destroy(None);

            assert_eq!(mock_pool_heap_calls(), calls, "no $destroy, no callable");
            assert!(mock_take_deferred_bailout(), "left for its raiser");
            assert_eq!((p.size(), p.idle_count()), (0, 0));
            assert_eq!(p.evicted_shutdown_total(), 0, "none was destroyed");
            assert!(destroy_skipped_total(SkipReason::RequestEnded) >= skipped + 3);
            close_request_token();
        });
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

    #[cfg(not(feature = "php"))]
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

    #[cfg(not(feature = "php"))]
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

    /// Runs `f` on a thread of its own — a fresh thread key and a fresh mock
    /// `$destroy` — and fails if it has not returned within five seconds, so
    /// an evict that waits on itself is a failure rather than a hung run.
    #[cfg(not(feature = "php"))]
    fn on_own_thread_within_5s(f: impl FnOnce() + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            f();
            let _ = tx.send(());
        });
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
            rx.recv_timeout(Duration::from_secs(5))
        {
            panic!("still running after 5s: the evict waits on itself");
        }
        if let Err(panic) = handle.join() {
            std::panic::resume_unwind(panic);
        }
    }

    /// Parks three slots on the calling thread, front to back, with the
    /// stand-in resources 1, 2 and 3 and an age of ten seconds that differs
    /// from slot to slot. Returns the three ages.
    #[cfg(not(feature = "php"))]
    fn park_three(p: &PoolInner) -> Vec<Instant> {
        let me = current_thread_key();
        let base = Instant::now() - Duration::from_secs(10);
        (1..=3usize)
            .map(|n| {
                assert!(p.try_reserve_budget());
                let mut slot = PoolSlot::new(n as *mut c_void, me);
                slot.last_active = base + Duration::from_millis(n as u64);
                let age = slot.last_active;
                p.deposit_new(slot);
                age
            })
            .collect()
    }

    /// The calling thread's idle slots, front to back: resource and age.
    #[cfg(not(feature = "php"))]
    fn parked(p: &PoolInner) -> Vec<(usize, Instant)> {
        let me = current_thread_key();
        p.idle.get(&me).map_or_else(Vec::new, |shard| {
            let deque = shard.lock().unwrap();
            deque
                .iter()
                .map(|s| (s.resource as usize, s.last_active))
                .collect()
        })
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn evict_all_keeps_the_slots_a_bailout_in_destroy_left_unreached() {
        use crate::bridge::ffi::{mock_pool_destroy, OXPHP_SHARED_INVOKE_BAILOUT};
        on_own_thread_within_5s(|| {
            let p = inner(3);
            let ages = park_three(&p);
            let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
            let log = std::rc::Rc::clone(&seen);
            mock_pool_destroy(Some(Box::new(move |slot| {
                log.borrow_mut().push(slot as usize);
                if slot as usize == 2 {
                    OXPHP_SHARED_INVOKE_BAILOUT
                } else {
                    0
                }
            })));

            p.evict_all_on_current_thread(EvictReason::Manual);
            mock_pool_destroy(None);

            // $destroy ran on 1, then on 2, which a bailout ended; no
            // $destroy may run past that.
            assert_eq!(*seen.borrow(), vec![1, 2]);
            // 1 and 2 are gone, and their budget with them. 3 is idle
            // again, as old as it was.
            assert_eq!(parked(&p), vec![(3, ages[2])]);
            assert_eq!(p.size(), 1);
            assert_eq!(p.evicted_manual_total(), 2);
        });
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn evict_all_puts_unreached_slots_back_ahead_of_one_destroy_parked() {
        use crate::bridge::ffi::{mock_pool_destroy, OXPHP_SHARED_INVOKE_BAILOUT};
        on_own_thread_within_5s(|| {
            let me = current_thread_key();
            let p = inner(4);
            let ages = park_three(&p);
            let q = Arc::clone(&p);
            mock_pool_destroy(Some(Box::new(move |slot| {
                if slot as usize != 1 {
                    return 0;
                }
                // $destroy takes a fresh slot of its own pool and gives it
                // back, while evict still has 2 and 3 in hand, then bails.
                assert!(q.try_reserve_budget());
                q.release(PoolSlot::new(9 as *mut c_void, me));
                OXPHP_SHARED_INVOKE_BAILOUT
            })));

            p.evict_all_on_current_thread(EvictReason::Manual);
            mock_pool_destroy(None);

            // The deque stays oldest first: 2 and 3 in front of 9.
            let after = parked(&p);
            assert_eq!(
                after.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
                vec![2, 3, 9]
            );
            assert_eq!(&after[..2], &[(2, ages[1]), (3, ages[2])]);
            assert_eq!(p.size(), 3);
            assert_eq!(p.evicted_manual_total(), 1);
        });
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn evict_stale_lets_destroy_use_the_pool() {
        use crate::bridge::ffi::mock_pool_destroy;
        on_own_thread_within_5s(|| {
            let me = current_thread_key();
            let p = inner_with_idle(4, Duration::from_millis(100));
            park_three(&p);
            let q = Arc::clone(&p);
            mock_pool_destroy(Some(Box::new(move |slot| {
                if slot as usize == 1 {
                    // Parked after the evict took its slots off the deque,
                    // so it stays.
                    assert!(q.try_reserve_budget());
                    q.release(PoolSlot::new(9 as *mut c_void, me));
                }
                0
            })));

            assert_eq!(p.evict_stale_on_current_thread(EvictReason::IdleTimeout), 3);
            mock_pool_destroy(None);

            assert_eq!(
                parked(&p).iter().map(|(r, _)| *r).collect::<Vec<_>>(),
                vec![9]
            );
            assert_eq!(p.size(), 1);
        });
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn evict_stale_keeps_the_slots_a_bailout_in_destroy_left_unreached() {
        use crate::bridge::ffi::{
            mock_pool_destroy, mock_take_deferred_bailout, OXPHP_SHARED_INVOKE_BAILOUT,
        };
        on_own_thread_within_5s(|| {
            let p = inner_with_idle(3, Duration::from_millis(100));
            let ages = park_three(&p);
            let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
            let log = std::rc::Rc::clone(&seen);
            mock_pool_destroy(Some(Box::new(move |slot| {
                log.borrow_mut().push(slot as usize);
                if slot as usize == 2 {
                    OXPHP_SHARED_INVOKE_BAILOUT
                } else {
                    0
                }
            })));

            assert_eq!(p.evict_stale_on_current_thread(EvictReason::IdleTimeout), 2);
            mock_pool_destroy(None);

            // $destroy ran on 1, then on 2, which a bailout ended, and the
            // bailout is held back for the serve loop to recover from.
            assert_eq!(*seen.borrow(), vec![1, 2]);
            assert!(mock_take_deferred_bailout());
            // 1 and 2 are gone, and their budget with them. 3 is idle
            // again, as old as it was, so the next scan flags it again.
            assert_eq!(parked(&p), vec![(3, ages[2])]);
            assert_eq!(p.size(), 1);
            assert_eq!(p.evicted_idle_total(), 2);
            assert_eq!(p.stale_owners(Instant::now()), vec![current_thread_key()]);
        });
    }

    /// Two `evict()` calls on fibers of one thread, each with slots in hand
    /// when a bailout ends its `$destroy`: A took 1, 2 and 3, then B took Z,
    /// parked after A had taken its slots. A puts 2 and 3 back first, B puts
    /// Z back after them, and the deque has to stay oldest first — the
    /// idle-timeout scan reads its front only.
    #[cfg(not(feature = "php"))]
    #[test]
    fn slots_put_back_keep_the_deque_oldest_first() {
        on_own_thread_within_5s(|| {
            let me = current_thread_key();
            let p = inner_with_idle(4, Duration::from_millis(100));
            park_three(&p);
            let take_all = |p: &PoolInner| -> Vec<PoolSlot> {
                let shard = p.idle.get(&me).unwrap();
                let mut deque = shard.lock().unwrap();
                deque.drain(..).collect()
            };
            let mut a = take_all(&p);
            assert!(p.try_reserve_budget());
            p.release(PoolSlot::new(9 as *mut c_void, me));
            let b = take_all(&p);

            a.remove(0);
            p.release_budget();
            p.put_back(me, a);
            p.put_back(me, b);

            assert_eq!(
                parked(&p).iter().map(|(r, _)| *r).collect::<Vec<_>>(),
                vec![2, 3, 9]
            );
            assert_eq!(p.stale_owners(Instant::now()), vec![me]);
            assert_eq!(p.evict_stale_on_current_thread(EvictReason::IdleTimeout), 2);
            assert_eq!(
                parked(&p).iter().map(|(r, _)| *r).collect::<Vec<_>>(),
                vec![9]
            );
            assert_eq!(p.size(), 1);
        });
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn reclaim_forgets_the_slots_a_dead_thread_parked() {
        use crate::bridge::ffi::mock_pool_heap_calls;
        on_own_thread_within_5s(|| {
            let me = current_thread_key();
            let p = inner_with_idle(4, Duration::from_millis(100));
            park_three(&p);
            assert!(p.try_reserve_budget());
            p.track_acquired_by_me();
            let calls = mock_pool_heap_calls();

            assert_eq!(p.reclaim_budget_for_dead_worker(me), 1);

            // The parked slots went with the thread's heap: forgotten and
            // refunded, with nothing called on them, and no scan flags a
            // later thread that gets the same key.
            assert!(parked(&p).is_empty());
            assert_eq!(p.size(), 0);
            assert_eq!(mock_pool_heap_calls(), calls);
            assert!(p.stale_owners(Instant::now()).is_empty());
        });
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
        // four label values: ok / timeout / closed / saturated. Here we
        // exercise the raw recorder methods — FFI-driven round-trips are
        // covered by the docker perf/idle tests.
        let p = inner(1);
        p.record_acquire_ok();
        p.record_acquire_ok();
        p.record_acquire_timeout();
        p.record_acquire_closed();
        p.record_acquire_saturated();
        p.record_acquire_saturated();
        p.record_acquire_saturated();
        assert_eq!(p.acquire_ok_total(), 2);
        assert_eq!(p.acquire_timeout_total(), 1);
        assert_eq!(p.acquire_closed_total(), 1);
        assert_eq!(p.acquire_saturated_total(), 3);
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

    #[cfg(not(feature = "php"))]
    #[test]
    fn shutdown_drain_bumps_shutdown_eviction_counter() {
        // Full path: deposit slots, on_shutdown_notify, verify the
        // shutdown bucket records the drained count.
        open_request_token();
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
        close_request_token();
    }

    #[cfg(not(feature = "php"))]
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
            metrics_enabled: false,
            introspection_enabled: false,
            introspection_preview_enabled: false,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            max_value_size: 1 << 20,
            max_channel_bytes: 64 << 20,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        });
        registry()
    }

    /// Insert a Pool directly into the registry, bypassing the FFI's
    /// factory fcc setup — tests exercise the Rust-side bookkeeping
    /// path without a live libphp. Returns the `Arc::into_raw` pointer
    /// the FFI expects; tests must call `oxphp_shared_handle_drop` (or
    /// reclaim via `Arc::from_raw`) when done to balance the strong
    /// count. For the simple per-test usage below, the leak is bounded
    /// by the test runner's process lifetime.
    fn register_pool(max: usize) -> *const Entry {
        register_pool_with_fccs(max, std::ptr::null_mut(), std::ptr::null_mut())
    }

    /// `register_pool` with stand-in factory/destroy pointers, for tests
    /// that check whether the pool frees them.
    fn register_pool_with_fccs(
        max: usize,
        factory_fcc: *mut c_void,
        destroy_fcc: *mut c_void,
    ) -> *const Entry {
        let reg = ensure_registry();
        let inner: Arc<dyn SharedInner> = Arc::new(PoolInner::new(
            factory_fcc,
            destroy_fcc,
            max,
            Duration::from_secs(300),
        ));
        let arc = reg.insert(SharedType::Pool, Arc::clone(&inner)).unwrap();
        (*inner)
            .as_any_pool()
            .expect("just inserted")
            .bind_entry(Arc::downgrade(&arc));
        Arc::into_raw(arc)
    }

    #[test]
    fn ffi_create_rejects_null_factory() {
        ensure_registry();
        let mut ptr: *const Entry = std::ptr::null();
        let rc = unsafe {
            oxphp_shared_pool_create(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                8,
                60.0,
                &mut ptr,
            )
        };
        assert_eq!(rc, SharedError::Type.code());
        assert!(ptr.is_null());
    }

    #[test]
    fn ffi_create_rejects_zero_max_size() {
        ensure_registry();
        // Use a non-null callable sentinel — validation fires on max_size
        // before reaching the bridge (mock would fail differently).
        #[allow(clippy::manual_dangling_ptr)]
        let sentinel = 0x1 as *mut std::ffi::c_void;
        let mut ptr: *const Entry = std::ptr::null();
        let rc =
            unsafe { oxphp_shared_pool_create(sentinel, std::ptr::null_mut(), 0, 60.0, &mut ptr) };
        assert_eq!(rc, SharedError::Type.code());
        assert!(ptr.is_null());
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
        // Insert a Counter and pass its handle to Pool stats — the
        // type-tag check fires before any pool-specific logic.
        let counter_arc = reg
            .insert(
                SharedType::Counter,
                Arc::new(crate::plugins::ox_shared::types::counter::CounterInner::new(0)),
            )
            .unwrap();
        let counter_ptr = Arc::into_raw(counter_arc);
        let mut in_use: u64 = 0;
        let mut idle: u64 = 0;
        let mut waiting: u64 = 0;
        let rc =
            unsafe { oxphp_shared_pool_stats(counter_ptr, &mut in_use, &mut idle, &mut waiting) };
        assert_eq!(rc, SharedError::Type.code());
        // SAFETY: reclaim the strong ref we leaked via into_raw above.
        unsafe { drop(Arc::from_raw(counter_ptr)) };
    }

    #[test]
    fn ffi_stats_on_stale_errors() {
        ensure_registry();
        // A NULL `entry_ptr` is the new "stale" sentinel — the FFI
        // shape no longer carries an integer id that could be made up.
        let rc = unsafe {
            oxphp_shared_pool_stats(
                std::ptr::null(),
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
        // SAFETY: `id` is the `Arc::into_raw` pointer returned by
        // `register_pool`, kept alive for the duration of this test.
        let entry: &Entry = unsafe { &*id };
        entry.inner.as_any_pool().unwrap().close();

        let mut slot_heap: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut owner: u64 = 0;
        let rc = unsafe { oxphp_shared_pool_acquire(id, 100, &mut slot_heap, &mut owner) };
        assert_eq!(rc, SharedError::Closed.code());
        assert!(slot_heap.is_null());
    }

    // ── a saturated acquire parks its fiber ──────────────────────────
    //
    // The park is the mock's: each test hands it a hook standing for what
    // the SAPI's fiber sleep returns. Each runs on a thread of its own, so
    // the hook, the last error and a held-back bailout stay with it.

    /// A pool of one slot that is checked out: the budget is spent and
    /// nothing is idle.
    #[cfg(not(feature = "php"))]
    fn saturated_pool() -> *const Entry {
        let id = register_pool(1);
        // SAFETY: `id` is the `Arc::into_raw` pointer returned by
        // `register_pool`, kept alive for the duration of the test.
        let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
        assert!(pool.try_reserve_budget());
        id
    }

    /// `acquire_for_test` with a wait: `timeout_ms` -1 forever, >0 bounded.
    #[cfg(not(feature = "php"))]
    fn acquire_waiting(id: *const Entry, timeout_ms: i64) -> (c_int, *mut c_void, String) {
        crate::plugins::ox_shared::error::clear_last_error();
        let mut slot_heap: *mut c_void = std::ptr::null_mut();
        let mut owner: u64 = 0;
        let rc = unsafe { oxphp_shared_pool_acquire(id, timeout_ms, &mut slot_heap, &mut owner) };
        let message = crate::plugins::ox_shared::error::read_last_error_message();
        (rc, slot_heap, message)
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn saturated_acquire_parks_and_takes_the_slot_given_back_meanwhile() {
        use crate::bridge::ffi::mock_pool_park;
        thread::spawn(|| {
            let id = saturated_pool();
            let id_addr = id as usize;
            let sentinel = 0xBABE_0101_usize;
            let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
            let log = std::rc::Rc::clone(&seen);
            mock_pool_park(Some(Box::new(move |us| {
                // SAFETY: as in `saturated_pool`.
                let pool = unsafe { &*(id_addr as *const Entry) }
                    .inner
                    .as_any_pool()
                    .unwrap();
                log.borrow_mut().push((us, pool.waiting_count()));
                // The holder, on this same thread, runs while the waiter is
                // parked and gives its slot back.
                pool.deposit_new(PoolSlot::new(sentinel as *mut c_void, current_thread_key()));
                1
            })));
            let (rc, slot, _) = acquire_waiting(id, -1);
            mock_pool_park(None);

            assert_eq!(rc, 0);
            assert_eq!(slot as usize, sentinel);
            assert_eq!(
                *seen.borrow(),
                vec![(1000, 1)],
                "parked once, for the first step, counted as waiting meanwhile"
            );
            // SAFETY: as in `saturated_pool`.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
            assert_eq!(pool.waiting_count(), 0);
            assert_eq!(pool.acquire_ok_total(), 1);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn parked_acquire_backs_off_to_its_cap_and_keeps_its_deadline() {
        use crate::bridge::ffi::mock_pool_park;
        thread::spawn(|| {
            let id = saturated_pool();
            let steps = std::rc::Rc::new(RefCell::new(Vec::new()));
            let log = std::rc::Rc::clone(&steps);
            mock_pool_park(Some(Box::new(move |us| {
                log.borrow_mut().push(us);
                thread::sleep(Duration::from_micros(us));
                1
            })));
            let started = Instant::now();
            let (rc, slot, _) = acquire_waiting(id, 50);
            let waited = started.elapsed();
            mock_pool_park(None);

            assert_eq!(rc, SharedError::Timeout.code());
            assert!(slot.is_null());
            assert!(waited >= Duration::from_millis(50), "{waited:?}");
            let steps = steps.borrow();
            assert_eq!(steps[..3], [1000, 2000, 4000], "{steps:?}");
            assert!(steps[3..].iter().all(|&us| us <= 4000), "{steps:?}");
            // SAFETY: as in `saturated_pool`.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
            assert_eq!(pool.waiting_count(), 0);
            assert_eq!(pool.acquire_timeout_total(), 1);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn parked_step_is_cut_to_what_is_left_of_the_deadline() {
        use crate::bridge::ffi::mock_pool_park;
        thread::spawn(|| {
            let id = saturated_pool();
            let steps = std::rc::Rc::new(RefCell::new(Vec::new()));
            let log = std::rc::Rc::clone(&steps);
            // Every park comes back at once, so the wait goes round many
            // times in the last 4 ms before its deadline.
            mock_pool_park(Some(Box::new(move |us| {
                log.borrow_mut().push(us);
                1
            })));
            let (rc, _, _) = acquire_waiting(id, 20);
            mock_pool_park(None);

            assert_eq!(rc, SharedError::Timeout.code());
            let steps = steps.borrow();
            assert_eq!(steps[..3], [1000, 2000, 4000], "{steps:?}");
            // Past the cap the step stays at 4 ms until less than that is
            // left; from there each step asks only for what is left.
            assert!(steps[3..].iter().all(|&us| us <= 4000), "{steps:?}");
            assert!(steps[3..].iter().any(|&us| us < 4000), "{steps:?}");
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn acquire_with_no_fiber_to_park_waits_on_the_thread() {
        use crate::bridge::ffi::mock_pool_park;
        let id = saturated_pool();
        let id_addr = id as usize;
        let parks = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&parks);
        let waiter = thread::spawn(move || {
            mock_pool_park(Some(Box::new(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                0
            })));
            let (rc, _, _) = acquire_waiting(id_addr as *const Entry, -1);
            mock_pool_park(None);
            rc
        });

        // SAFETY: as in `saturated_pool`.
        let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while parks.load(Ordering::SeqCst) == 0 && Instant::now() < until {
            thread::sleep(Duration::from_millis(1));
        }
        // Long past the park the mock refused, and past two of the forever
        // wait's 50 ms turns: still waiting, still counted, and the park
        // never asked for again — the waiter is on the condvar.
        thread::sleep(Duration::from_millis(120));
        assert_eq!(pool.waiting_count(), 1, "the waiter is not waiting");
        assert_eq!(
            parks.load(Ordering::SeqCst),
            1,
            "asked once, then waited on the thread"
        );

        pool.close();
        assert_eq!(waiter.join().unwrap(), SharedError::Closed.code());
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn cancelled_park_ends_the_acquire_as_cancelled() {
        use crate::bridge::ffi::mock_pool_park;
        thread::spawn(|| {
            let id = saturated_pool();
            mock_pool_park(Some(Box::new(|_| -1)));
            let (rc, slot, message) = acquire_waiting(id, -1);
            mock_pool_park(None);

            assert_eq!(rc, SharedError::Cancelled.code());
            assert!(slot.is_null());
            assert!(message.contains("cancelled"), "{message}");
            // SAFETY: as in `saturated_pool`.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
            assert_eq!(pool.waiting_count(), 0);
            // Not an acquire outcome: nothing recorded.
            assert_eq!(pool.wait_histogram_snapshot().2, 0);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn unwinding_park_returns_fiber_unwind_without_an_error() {
        use crate::bridge::ffi::{mock_pool_park, OXPHP_FIBER_UNWIND};
        thread::spawn(|| {
            let id = saturated_pool();
            mock_pool_park(Some(Box::new(|_| OXPHP_FIBER_UNWIND)));
            let (rc, slot, message) = acquire_waiting(id, -1);
            mock_pool_park(None);

            assert_eq!(rc, OXPHP_FIBER_UNWIND);
            assert!(slot.is_null());
            assert_eq!(message, "", "an unwind is not an error of the pool's");
            // SAFETY: as in `saturated_pool`.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
            assert_eq!(pool.waiting_count(), 0);
            assert_eq!(pool.wait_histogram_snapshot().2, 0);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn bailout_in_park_is_held_back_and_the_waiter_count_comes_down() {
        use crate::bridge::ffi::{
            mock_pool_park, mock_take_deferred_bailout, OXPHP_SHARED_INVOKE_BAILOUT,
        };
        thread::spawn(|| {
            let id = saturated_pool();
            mock_pool_park(Some(Box::new(|_| OXPHP_SHARED_INVOKE_BAILOUT)));
            let (rc, slot, message) = acquire_waiting(id, -1);
            mock_pool_park(None);

            assert_eq!(rc, SharedError::Generic.code());
            assert!(slot.is_null());
            assert!(message.contains("bailout"), "{message}");
            assert!(
                mock_take_deferred_bailout(),
                "the bailout is held for the stub"
            );
            // SAFETY: as in `saturated_pool`.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
            assert_eq!(pool.waiting_count(), 0);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn park_that_never_returns_stops_counting_when_its_thread_exits() {
        use crate::bridge::ffi::mock_pool_park;
        thread::spawn(|| {
            let id = saturated_pool();
            // The park unwinds out past the acquire's frames instead of
            // returning, which leaves the count where a fiber the server
            // drops without resuming it leaves it: nothing after the park
            // runs.
            mock_pool_park(Some(Box::new(|_| panic!("the fiber is never resumed"))));
            let (rc, _, _) = acquire_waiting(id, -1);
            mock_pool_park(None);
            assert_eq!(rc, SharedError::Panicked.code());

            // SAFETY: as in `saturated_pool`.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
            assert_eq!(pool.waiting_count(), 1, "the dropped park still counts");
            pool.reclaim_budget_for_dead_worker(current_thread_key());
            assert_eq!(pool.waiting_count(), 0, "until its thread exits");
        })
        .join()
        .unwrap();
    }

    #[test]
    fn cancelled_acquire_maps_to_async_exception() {
        match pool_rc_to_phperr(SharedError::Cancelled.code(), "Shared\\Pool::acquire") {
            PhpError::Exception { class, .. } => assert_eq!(class, "OxPHP\\Async\\AsyncException"),
            other => panic!("expected an exception, got {other:?}"),
        }
    }

    // ── pools of a request that ends before its thread ───────────────
    //
    // Each test runs on a thread of its own: the scope is per thread, and
    // the end-of-request hook acts on the calling thread's key.

    /// Acquire through the FFI with the last error cleared first, so a
    /// refusal is told apart from any other `Closed` by its message.
    fn acquire_for_test(id: *const Entry) -> (c_int, *mut c_void, String) {
        crate::plugins::ox_shared::error::clear_last_error();
        let mut slot_heap: *mut c_void = std::ptr::null_mut();
        let mut owner: u64 = 0;
        let rc = unsafe { oxphp_shared_pool_acquire(id, 0, &mut slot_heap, &mut owner) };
        let message = crate::plugins::ox_shared::error::read_last_error_message();
        (rc, slot_heap, message)
    }

    #[test]
    fn request_scope_sets_home_and_registry_refusal() {
        let observed = thread::spawn(|| {
            let mut out = Vec::new();
            for scope in [
                RequestScope::Thread,
                RequestScope::OneRequest,
                RequestScope::EachRequest,
            ] {
                set_request_scope(scope);
                let home = inner(1).home;
                out.push((scope, home, registry_refuses_pools_on_current_thread()));
            }
            (current_thread_key(), out)
        })
        .join()
        .unwrap();
        let (me, out) = observed;
        assert_eq!(
            out,
            vec![
                (RequestScope::Thread, None, false),
                (RequestScope::OneRequest, Some(me), false),
                (RequestScope::EachRequest, Some(me), true),
            ]
        );
    }

    #[test]
    fn acquire_is_refused_outside_the_creating_request() {
        thread::spawn(|| {
            set_request_scope(RequestScope::EachRequest);
            let id = register_pool(2);
            // SAFETY: `id` is the `Arc::into_raw` pointer returned by
            // `register_pool`, kept alive for the duration of this test.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
            assert!(pool.try_reserve_budget());
            let sentinel = 0xBABE_0051 as *mut c_void;
            pool.deposit_new(PoolSlot::new(sentinel, current_thread_key()));

            // The creating request is served.
            let (rc, slot, _) = acquire_for_test(id);
            assert_eq!(rc, 0, "the creating request must be served");
            assert_eq!(slot, sentinel);
            assert_eq!(
                unsafe { oxphp_shared_pool_release(id, slot, current_thread_key()) },
                0
            );

            // Another thread, while the creating request still runs.
            let id_addr = id as usize;
            let (rc, slot, message) = thread::spawn(move || {
                let (rc, slot, message) = acquire_for_test(id_addr as *const Entry);
                (rc, slot as usize, message)
            })
            .join()
            .unwrap();
            assert_eq!(
                rc,
                SharedError::Closed.code(),
                "another thread must be refused"
            );
            assert_eq!(slot, 0);
            assert_eq!(message, POOL_OUTSIDE_ITS_REQUEST);
            assert_eq!(
                pool.idle_count(),
                1,
                "the refusal must leave the pool as it was"
            );

            // The creating thread, once its request has ended.
            end_request_on_current_thread();
            let (rc, slot, message) = acquire_for_test(id);
            assert_eq!(
                rc,
                SharedError::Closed.code(),
                "an ended request's pool must refuse"
            );
            assert!(slot.is_null());
            assert_eq!(message, POOL_OUTSIDE_ITS_REQUEST);
        })
        .join()
        .unwrap();
    }

    // Counts calls on the host mock bridge; the stand-in pointers it uses
    // must never reach the real one.
    #[cfg(not(feature = "php"))]
    #[test]
    fn end_request_forgets_without_touching_the_reset_heap() {
        use crate::bridge::ffi::mock_pool_heap_calls;
        const OTHER: ThreadKey = 0xBEEF_0000_0000_0051;

        thread::spawn(|| {
            let me = current_thread_key();
            let factory = 0xFAC0_0051 as *mut c_void;
            let destroy = 0xDE50_0051 as *mut c_void;

            // A pool with no home (made where requests last as long as
            // the thread): one slot parked here, one on another thread.
            let foreign_id = register_pool_with_fccs(4, factory, destroy);
            // SAFETY: as above — kept alive for the whole test.
            let foreign = unsafe { &*foreign_id }.inner.as_any_pool().unwrap();
            assert!(foreign.try_reserve_budget());
            foreign.deposit_new(dummy_slot(me));
            assert!(foreign.try_reserve_budget());
            foreign
                .idle
                .entry(OTHER)
                .or_insert_with(|| Mutex::new(VecDeque::new()))
                .lock()
                .unwrap()
                .push_back(dummy_slot(OTHER));

            // A pool with no home that this thread's request neither made
            // nor minted in, with a slot parked under this thread anyway.
            let unrelated_id = register_pool(1);
            // SAFETY: as above.
            let unrelated = unsafe { &*unrelated_id }.inner.as_any_pool().unwrap();
            assert!(unrelated.try_reserve_budget());
            unrelated.deposit_new(dummy_slot(me));

            set_request_scope(RequestScope::EachRequest);
            // As minting a slot in it during the request does.
            foreign.remember_for_request_end();
            // A pool this thread's request made, with two slots parked.
            let own_id = register_pool_with_fccs(2, factory, destroy);
            // SAFETY: as above.
            let own = unsafe { &*own_id }.inner.as_any_pool().unwrap();
            for _ in 0..2 {
                assert!(own.try_reserve_budget());
                own.deposit_new(dummy_slot(me));
            }

            let calls = mock_pool_heap_calls();
            let skipped = destroy_skipped_total(SkipReason::RequestEnded);
            end_request_on_current_thread();
            assert_eq!(
                mock_pool_heap_calls(),
                calls,
                "the hook must not touch PHP memory"
            );
            assert!(
                destroy_skipped_total(SkipReason::RequestEnded) >= skipped + 3,
                "two slots of its own pool and one of the other went without $destroy"
            );
            assert!(
                REQUEST_POOLS.with(|list| list.borrow().is_empty()),
                "the next request starts with an empty list"
            );

            assert!(own.is_closed());
            assert_eq!((own.size(), own.idle_count()), (0, 0));
            assert!(own.factory_fcc().is_null() && own.destroy_fcc().is_null());
            // Dropping the pool later frees nothing either.
            own.on_drop();
            assert_eq!(
                mock_pool_heap_calls(),
                calls,
                "the freed callables must not be freed again"
            );

            // The other pool keeps going; only the slot parked here is gone.
            assert!(!foreign.is_closed());
            assert_eq!(foreign.size(), 1);
            assert_eq!(foreign.idle_by_thread(), vec![(OTHER, 1)]);
            assert_eq!(foreign.factory_fcc(), factory);
            assert_eq!(foreign.destroy_fcc(), destroy);

            // The hook visits only the pools on its list.
            assert_eq!(unrelated.idle_by_thread(), vec![(me, 1)]);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn minting_during_a_request_puts_the_pool_on_its_end_list() {
        use crate::bridge::ffi::mock_pool_factory_mints;

        thread::spawn(|| {
            // Made where requests last as long as the thread: no home, so
            // creating it put nothing on this thread's list.
            let id = register_pool(2);
            // SAFETY: `id` is the `Arc::into_raw` pointer returned by
            // `register_pool`, kept alive for the duration of this test.
            let pool = unsafe { &*id }.inner.as_any_pool().unwrap();

            set_request_scope(RequestScope::EachRequest);
            let minted = 0xBABE_0052 as *mut c_void;
            mock_pool_factory_mints(minted);
            let (rc, slot, _) = acquire_for_test(id);
            mock_pool_factory_mints(std::ptr::null_mut());
            assert_eq!((rc, slot), (0, minted), "the factory must mint");
            assert_eq!(
                unsafe { oxphp_shared_pool_release(id, slot, current_thread_key()) },
                0
            );
            assert_eq!(pool.idle_count(), 1);

            end_request_on_current_thread();
            assert_eq!(
                (pool.size(), pool.idle_count()),
                (0, 0),
                "the slot minted in the ended request must be forgotten"
            );
            assert!(!pool.is_closed(), "a pool without home stays open");
        })
        .join()
        .unwrap();
    }

    #[cfg(not(feature = "php"))]
    #[test]
    fn teardown_out_of_reach_of_the_request_heap_forgets() {
        use crate::bridge::ffi::mock_pool_heap_calls;

        thread::spawn(|| {
            set_request_scope(RequestScope::EachRequest);
            open_request_token();
            let me = current_thread_key();
            let make = || {
                let id = register_pool_with_fccs(
                    2,
                    0xFAC0_0053 as *mut c_void,
                    0xDE50_0053 as *mut c_void,
                );
                // SAFETY: `id` is the `Arc::into_raw` pointer returned by
                // `register_pool_with_fccs`, kept alive for the whole test.
                let pool = unsafe { &*id }.inner.as_any_pool().unwrap();
                for _ in 0..2 {
                    assert!(pool.try_reserve_budget());
                    pool.deposit_new(dummy_slot(me));
                }
                pool
            };

            // The last reference goes away on another thread while the
            // creating request still runs.
            let pool = make();
            let addr = pool as *const PoolInner as usize;
            let calls = thread::spawn(move || {
                // SAFETY: the pool outlives this join.
                let pool = unsafe { &*(addr as *const PoolInner) };
                let calls = mock_pool_heap_calls();
                pool.on_drop();
                mock_pool_heap_calls() - calls
            })
            .join()
            .unwrap();
            assert_eq!(calls, 0, "another thread must not free into this heap");
            assert!(pool.is_closed());
            assert_eq!((pool.size(), pool.idle_count()), (0, 0));
            assert!(pool.factory_fcc().is_null() && pool.destroy_fcc().is_null());

            // On the creating thread with its request open, the same
            // teardown frees: two slots, two callables.
            let pool = make();
            let calls = mock_pool_heap_calls();
            pool.on_drop();
            assert_eq!(mock_pool_heap_calls() - calls, 4);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn end_list_keeps_only_pools_still_alive() {
        thread::spawn(|| {
            // `oxphp run`: one request for the whole script, so the hook
            // that empties the list runs only when the script ends.
            set_request_scope(RequestScope::OneRequest);
            // A registry of its own: a snapshot another test takes of the
            // shared one could keep a pool dropped here alive.
            let reg = SharedRegistry::new_for_test(ensure_registry().config().clone());
            for _ in 0..3 {
                let inner: Arc<dyn SharedInner> = Arc::new(PoolInner::new(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    1,
                    Duration::from_secs(300),
                ));
                let arc = reg.insert(SharedType::Pool, Arc::clone(&inner)).unwrap();
                inner
                    .as_any_pool()
                    .expect("just inserted")
                    .bind_entry(Arc::downgrade(&arc));
                drop((arc, inner));
            }
            assert_eq!(
                REQUEST_POOLS.with(|list| list.borrow().len()),
                1,
                "a pool already gone must not stay on the list"
            );
            end_request_on_current_thread();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn ffi_acquire_local_idle_bypasses_factory() {
        // Pre-populate idle on the current thread so acquire returns
        // from the fast path — no factory call, no C bridge needed.
        let id = register_pool(2);
        // SAFETY: `id` is the `Arc::into_raw` pointer returned by
        // `register_pool`, kept alive for the duration of this test.
        let entry: &Entry = unsafe { &*id };
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
        // SAFETY: `id` is the `Arc::into_raw` pointer returned by
        // `register_pool`, kept alive for the duration of this test.
        let entry: &Entry = unsafe { &*id };
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
        // an unregistered key takes the dead-owner forget path
        // (covered by `release_cross_thread_dead_owner_forgets`).
        let id = register_pool(1);
        // SAFETY: `id` is the `Arc::into_raw` pointer returned by
        // `register_pool`, kept alive for the duration of this test.
        let entry: &Entry = unsafe { &*id };
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
        // SAFETY: `id` is the `Arc::into_raw` pointer returned by
        // `register_pool`, kept alive for the duration of this test.
        let entry: &Entry = unsafe { &*id };
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
        // SAFETY: `id` is the `Arc::into_raw` pointer returned by
        // `register_pool`, kept alive for the duration of this test.
        let entry: &Entry = unsafe { &*id };
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
        // SAFETY: `id` is the `Arc::into_raw` pointer; the strong ref
        // it represents is held by `register_pool`'s leak. The Entry
        // outlives this test.
        let entry: &Entry = unsafe { &*id };
        let pool = entry.inner.as_any_pool().unwrap() as *const PoolInner;
        // SAFETY: the Arc is held for the duration of the test.
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

    /// Regression: the Handle return path (`Handle::release()` / Drop →
    /// `PoolHandleStorage::release_slot`) must decrement the in-flight
    /// counter, exactly like the FFI `oxphp_shared_pool_release` does.
    ///
    /// The acquire/release/tryAcquire API returns slots only through this
    /// path. If it skips the untrack, a worker's in-flight count grows
    /// once per acquire and never falls; on worker exit (dynamic
    /// scale-down or panic-respawn) `reclaim_budget_for_dead_worker`
    /// refunds that inflated count via `release_budget`, underflowing
    /// `size` (u64) past zero — `try_reserve_budget` then never mints
    /// again and the pool is bricked for the process's lifetime.
    ///
    /// The raw `track_and_untrack_*` / `reclaim_*` tests drive the
    /// counter methods directly and bypass `release_slot`, so they miss
    /// this. This test goes through `release_slot` end-to-end (it needs
    /// the global registry, which `register_pool` populates).
    #[test]
    fn handle_release_path_balances_in_flight() {
        let id_ptr = register_pool(2);
        // SAFETY: register_pool leaks the Arc<Entry>; it outlives the test.
        let entry: &Entry = unsafe { &*id_ptr };
        let pool_id = entry.id;
        let pool = (*entry.inner).as_any_pool().expect("downcast");
        let me = current_thread_key();

        // Cycle 1: fresh mint (reserve budget + mark in-flight), then
        // return the slot through the handle path.
        assert!(pool.try_reserve_budget());
        pool.track_acquired_by_me();
        PoolHandleStorage {
            pool_id,
            owner_tid: me,
            slot_zv_ptr: 0xCAFE_0001 as *mut std::ffi::c_void,
        }
        .release_slot();
        assert_eq!(pool.idle_count(), 1, "released slot must park as idle");

        // Cycle 2: local-idle hit reusing that same slot (no new budget),
        // marked in-flight again, then returned through the handle path.
        // Two cycles reuse one slot, so `size` stays 1 while a buggy
        // handle path would push in-flight to 2 — more than `size`.
        let slot = pool.try_acquire_local().expect("idle slot present");
        pool.track_acquired_by_me();
        PoolHandleStorage {
            pool_id,
            owner_tid: slot.owner,
            slot_zv_ptr: slot.resource,
        }
        .release_slot();
        assert_eq!(pool.size(), 1, "two reuse cycles must keep size at 1");

        // Worker dies: every slot was cleanly released, so reclaim must
        // refund nothing for in-flight slots, and only the one parked slot
        // it forgets takes `size` down (no underflow).
        let reclaimed = pool.reclaim_budget_for_dead_worker(me);
        assert_eq!(
            reclaimed, 0,
            "handle release must balance in-flight; reclaim over-refunded {reclaimed}"
        );
        assert_eq!(pool.size(), 0, "reclaim must refund the parked slot once");
    }

    /// One walk of the idle-timeout eviction over two pools with stale slots
    /// on this thread, where `$destroy` bails on the first slot it is handed:
    /// the walk stops there. A `$destroy` called after it would be refused
    /// while the bailout is held back, and its slot counted as destroyed
    /// although nothing ran.
    #[cfg(not(feature = "php"))]
    #[test]
    fn idle_drain_stops_at_the_pool_a_bailout_ended() {
        use crate::bridge::ffi::{
            mock_pool_destroy, mock_take_deferred_bailout, OXPHP_SHARED_INVOKE_BAILOUT,
        };
        use crate::plugins::ox_shared::eviction::drain_stale_for_current_thread;
        on_own_thread_within_5s(|| {
            let me = current_thread_key();
            let reg = ensure_registry();
            let inners: Vec<Arc<dyn SharedInner>> = (0..2)
                .map(|_| {
                    let inner: Arc<dyn SharedInner> = Arc::new(PoolInner::new(
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        3,
                        Duration::from_millis(100),
                    ));
                    let entry = reg.insert(SharedType::Pool, Arc::clone(&inner)).unwrap();
                    (*inner)
                        .as_any_pool()
                        .expect("just inserted")
                        .bind_entry(Arc::downgrade(&entry));
                    let _ = Arc::into_raw(entry);
                    inner
                })
                .collect();
            for inner in &inners {
                park_three((**inner).as_any_pool().unwrap());
            }
            let calls = std::rc::Rc::new(Cell::new(0));
            let count = std::rc::Rc::clone(&calls);
            mock_pool_destroy(Some(Box::new(move |_| {
                count.set(count.get() + 1);
                OXPHP_SHARED_INVOKE_BAILOUT
            })));

            drain_stale_for_current_thread();
            mock_pool_destroy(None);

            assert_eq!(calls.get(), 1);
            assert!(mock_take_deferred_bailout());
            let mut sizes: Vec<u64> = inners
                .iter()
                .map(|i| (**i).as_any_pool().unwrap().size())
                .collect();
            sizes.sort_unstable();
            assert_eq!(sizes, vec![2, 3], "the pool not reached keeps every slot");

            for inner in &inners {
                (**inner)
                    .as_any_pool()
                    .unwrap()
                    .reclaim_budget_for_dead_worker(me);
            }
        });
    }

    /// The walk holds the last reference to the first pool it visits —
    /// another thread let go of it meanwhile — and that pool still has an
    /// idle slot of this thread, so letting go of it runs `$destroy`, which
    /// bails. The walk has to stop there as it does for a bailout in the
    /// eviction itself: the second pool's stale slot stays, rather than
    /// being counted as destroyed by a `$destroy` that was refused.
    #[cfg(not(feature = "php"))]
    #[test]
    fn idle_drain_stops_at_a_bailout_from_letting_go_of_a_pool() {
        use crate::bridge::ffi::{
            mock_pool_destroy, mock_take_deferred_bailout, OXPHP_SHARED_INVOKE_BAILOUT,
        };
        use crate::plugins::ox_shared::eviction::drain_stale_in;
        on_own_thread_within_5s(|| {
            open_request_token();
            let me = current_thread_key();
            let reg = registry_no_other_walk_sees();
            let entries: Vec<Arc<Entry>> = [Duration::from_secs(300), Duration::from_millis(100)]
                .into_iter()
                .map(|idle_timeout| pool_with_a_slot_idle_for_10s(reg, idle_timeout, me))
                .collect();
            // The second pool outlives the walk; the first goes with it.
            let second = Arc::clone(&entries[1]);
            let calls = std::rc::Rc::new(Cell::new(0));
            let count = std::rc::Rc::clone(&calls);
            mock_pool_destroy(Some(Box::new(move |_| {
                count.set(count.get() + 1);
                OXPHP_SHARED_INVOKE_BAILOUT
            })));

            drain_stale_in(entries);
            mock_pool_destroy(None);

            assert_eq!(calls.get(), 1, "the $destroy of the pool let go of");
            assert!(mock_take_deferred_bailout());
            let pool = (*second.inner).as_any_pool().unwrap();
            assert_eq!(
                (pool.size(), pool.idle_count()),
                (1, 1),
                "the pool not reached keeps its stale slot"
            );
            pool.reclaim_budget_for_dead_worker(me);
            close_request_token();
        });
    }

    /// The same with a map in between: the walk holds the last reference to
    /// a map, and the map the last one to a pool with an idle slot of this
    /// thread. The map is no pool, but letting go of it runs that pool's
    /// `$destroy` all the same.
    #[cfg(not(feature = "php"))]
    #[test]
    fn idle_drain_stops_at_a_bailout_from_letting_go_of_a_map() {
        use crate::bridge::ffi::{
            mock_pool_destroy, mock_take_deferred_bailout, OXPHP_SHARED_INVOKE_BAILOUT,
        };
        use crate::plugins::ox_shared::eviction::drain_stale_in;
        use crate::plugins::ox_shared::types::map::MapInner;
        use crate::plugins::ox_shared::types::map_key::MapKey;
        use crate::plugins::ox_shared::value::{SharedRefOwned, SharedValue};
        on_own_thread_within_5s(|| {
            open_request_token();
            let me = current_thread_key();
            let reg = registry_no_other_walk_sees();
            let held = pool_with_a_slot_idle_for_10s(reg, Duration::from_secs(300), me);
            let second = pool_with_a_slot_idle_for_10s(reg, Duration::from_millis(100), me);
            let map = MapInner::new(None);
            map.set(
                MapKey::from_str("pool"),
                SharedValue::Shared(SharedRefOwned::from_arc(held)),
            )
            .unwrap();
            let map = reg.insert(SharedType::Map, Arc::new(map)).unwrap();
            let calls = std::rc::Rc::new(Cell::new(0));
            let count = std::rc::Rc::clone(&calls);
            mock_pool_destroy(Some(Box::new(move |_| {
                count.set(count.get() + 1);
                OXPHP_SHARED_INVOKE_BAILOUT
            })));

            drain_stale_in(vec![map, Arc::clone(&second)]);
            mock_pool_destroy(None);

            assert_eq!(calls.get(), 1, "the $destroy of the pool in the map");
            assert!(mock_take_deferred_bailout());
            let pool = (*second.inner).as_any_pool().unwrap();
            assert_eq!(
                (pool.size(), pool.idle_count()),
                (1, 1),
                "the pool not reached keeps its stale slot"
            );
            pool.reclaim_budget_for_dead_worker(me);
            close_request_token();
        });
    }

    /// A registry of the test's own. The tests that need the walk to hold
    /// the last reference to an entry keep theirs out of the shared one: a
    /// test walking that one on another thread meanwhile holds a reference
    /// to every entry in it.
    #[cfg(not(feature = "php"))]
    fn registry_no_other_walk_sees() -> &'static SharedRegistry {
        SharedRegistry::new_for_test(ensure_registry().config().clone())
    }

    /// A pool in `reg` with one slot of `me` parked in it, idle for 10 s.
    #[cfg(not(feature = "php"))]
    fn pool_with_a_slot_idle_for_10s(
        reg: &'static SharedRegistry,
        idle_timeout: Duration,
        me: ThreadKey,
    ) -> Arc<Entry> {
        let inner: Arc<dyn SharedInner> = Arc::new(PoolInner::new(
            0xFAC0_0062 as *mut c_void,
            0xDE50_0062 as *mut c_void,
            1,
            idle_timeout,
        ));
        let entry = reg.insert(SharedType::Pool, Arc::clone(&inner)).unwrap();
        let pool = (*inner).as_any_pool().expect("just inserted");
        pool.bind_entry(Arc::downgrade(&entry));
        assert!(pool.try_reserve_budget());
        let mut slot = PoolSlot::new(4 as *mut c_void, me);
        slot.last_active = Instant::now() - Duration::from_secs(10);
        pool.deposit_new(slot);
        entry
    }

    /// Pins the contract that `try_reserve_budget` / `release_budget`
    /// propagate per-slot growth through `adjust_mem_bytes` via
    /// `track_slot_delta`. Counterpart to `map_set_grows_registry_
    /// entry_bytes` / `channel_send_recv_track_registry_entry_bytes`.
    ///
    /// Asserts against `Entry::mem_bytes`, not registry-global
    /// `total_bytes` — see `SharedRegistry::total_bytes` for why.
    #[test]
    fn pool_reserve_release_track_registry_entry_bytes() {
        ensure_registry();
        let id = register_pool(8);
        // SAFETY: `id` is `Arc::into_raw`; the strong ref is held for
        // the test's lifetime by `register_pool`'s deliberate leak.
        let entry: &Entry = unsafe { &*id };
        let pool = (*entry.inner).as_any_pool().expect("downcast");

        let baseline = entry.mem_bytes.load(Ordering::Relaxed);
        for _ in 0..4 {
            assert!(pool.try_reserve_budget());
        }
        let grown = entry.mem_bytes.load(Ordering::Relaxed);
        assert_eq!(
            grown - baseline,
            (4 * POOL_PER_SLOT_BYTES) as usize,
            "entry mem_bytes must grow by 4 × POOL_PER_SLOT_BYTES"
        );

        for _ in 0..4 {
            pool.release_budget();
        }
        assert_eq!(
            entry.mem_bytes.load(Ordering::Relaxed),
            baseline,
            "entry mem_bytes must return to baseline after symmetric releases"
        );
    }
}
