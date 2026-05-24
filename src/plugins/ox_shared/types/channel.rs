//! Shared\Channel — bounded MPMC channel with fiber-suspending recv/send.
//!
//! Pure-Rust core: `try_send` / `try_recv` / `close` over a crossbeam
//! bounded channel, plus gauge atomics, fiber-waker lists, blocking
//! timeout path, and FFI surface.

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use crossbeam_channel::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use parking_lot::Mutex;
use smallvec::SmallVec;
use tokio::sync::Notify;

use crate::plugins::ox_async::synthetic::{self, PromisePayload};
use crate::plugins::ox_shared::error::{ffi_entry, set_last_error, SharedError};
use crate::plugins::ox_shared::registry::{
    registry, Entry, SharedId, SharedInner, SharedType, ENTRY_MAGIC, REGISTRY,
};
use crate::plugins::ox_shared::types::timeout::{parse_timeout, Wait};
use crate::plugins::ox_shared::value::{SharedRef, SharedRefOwned, SharedValue};

/// Approximate per-pending-payload footprint booked against
/// `total_bytes` on send/recv. Mirrors the `64 + pending * 32` formula
/// in [`ChannelInner::mem_bytes`] — the constant tracks crossbeam's
/// per-slot bookkeeping plus the empty `Payload` (`Vec<u8>` header).
const CHANNEL_PER_PAYLOAD_BYTES: isize = 32;

/// In-transit channel value: portbuf wire bytes plus strong refs that pin
/// every nested `Shared\*` entry alive while the value sits in the channel
/// (buffer, front-stash, or in-flight to a fiber waker). `keepalive` is
/// empty for payloads with no nested shared refs (the common case) —
/// inline, no allocation. Encoding/decoding of `bytes` lives in `value.rs`.
#[derive(Debug)]
pub struct Payload {
    bytes: Vec<u8>,
    keepalive: SmallVec<[SharedRefOwned; 1]>,
}

impl Payload {
    /// Wrap raw wire bytes with no keepalive. Used by tests and by the
    /// internal recv recirculation paths that move bytes without resolved
    /// nested shared refs.
    pub(crate) fn bytes_only(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            keepalive: SmallVec::new(),
        }
    }
}

/// Approximate bytes crossbeam allocates per bounded-channel slot: the
/// `Payload` plus a per-slot stamp word. A deliberate lower bound on the
/// true slot size — enough to convert an allocation-bomb abort into a
/// catchable CapacityException, not a precise allocator quota.
const SLOT_BYTES: u64 = (std::mem::size_of::<Payload>() + std::mem::size_of::<usize>()) as u64;

/// True iff a channel of `capacity` slots fits within `budget` bytes.
/// Overflow of `capacity * SLOT_BYTES` counts as not fitting.
fn channel_capacity_fits(capacity: u64, budget: u64) -> bool {
    match capacity.checked_mul(SLOT_BYTES) {
        Some(bytes) => bytes <= budget,
        None => false,
    }
}

/// Effective channel byte budget: never below one slot. Clamps a zero or
/// sub-slot `SHARED_MAX_CHANNEL_BYTES` up so a misconfiguration can't reject
/// a minimal capacity-1 channel; the budget only meaningfully caps large
/// capacities.
fn effective_channel_budget(configured: u64) -> u64 {
    configured.max(SLOT_BYTES)
}

/// Walk per-element sizes encoded as start `offsets` into a `concat_len`-byte
/// buffer (element i spans `offsets[i]..offsets[i+1]`, last spans
/// `offsets[last]..concat_len`). Returns `(index, size)` of the first element
/// whose size exceeds `cap`, or `None` if all fit.
fn first_oversized_element(
    offsets: &[usize],
    concat_len: usize,
    cap: usize,
) -> Option<(usize, usize)> {
    for i in 0..offsets.len() {
        let start = offsets[i];
        let end = offsets.get(i + 1).copied().unwrap_or(concat_len);
        let size = end.saturating_sub(start);
        if size > cap {
            return Some((i, size));
        }
    }
    None
}

/// Reject a serialised value larger than `cap` bytes. Pure: the caller
/// supplies the cap from `registry().config().max_value_size`. Mirrors
/// Shared\Map's per-value guard but returns a PhpError because the channel
/// send paths throw directly rather than via an FFI rc.
fn check_value_size(len: usize, cap: usize, method: &str) -> Result<(), crate::plugin::PhpError> {
    if len > cap {
        return Err(crate::plugin::PhpError::Exception {
            class: "OxPHP\\Shared\\ValueTooLargeException".into(),
            message: format!(
                "{method}: value of {len} bytes exceeds the per-value cap of \
                 {cap} bytes (SHARED_MAX_VALUE_SIZE)"
            ),
            code: 0,
        });
    }
    Ok(())
}

/// Hand a `Vec<u8>` off to C via a `libc::malloc`'d buffer. C side is
/// expected to free it via `oxphp_portable_free` when done.
/// Returns `(buf_ptr, len)` on success; returns `SharedError::Generic`
/// if `libc::malloc` fails.
///
/// # Safety
/// The returned pointer (when non-null) owns the allocation; the caller
/// must arrange for `oxphp_portable_free` (or `libc::free`) to run.
unsafe fn payload_to_malloc(bytes: Vec<u8>) -> Result<(*mut u8, usize), SharedError> {
    let n = bytes.len();
    if n == 0 {
        return Ok((std::ptr::null_mut(), 0));
    }
    let ptr = unsafe { libc::malloc(n) as *mut u8 };
    if ptr.is_null() {
        set_last_error("libc::malloc failed for channel payload");
        return Err(SharedError::Generic);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, n);
    }
    Ok((ptr, n))
}

/// Slow-path poll quantum for blocking send/recv. Small enough that a
/// concurrent `close()` wakes blocked threads promptly; large enough
/// that a full-channel wait doesn't spin. The waker lists below
/// replace this polling pattern with proper `Notify`-driven wakeups.
const POLL_QUANTUM: Duration = Duration::from_millis(20);

/// Error returned by [`ChannelInner::try_send`]. Both variants carry
/// the payload back so the caller can recover it (important for the
/// blocking/fiber paths).
#[derive(Debug)]
pub enum TrySendErr {
    /// Channel is at capacity but still open.
    Full(Payload),
    /// Channel has been closed; no further sends are accepted.
    Closed(Payload),
}

/// Error returned by [`ChannelInner::try_recv`]. Only one variant
/// today — empty-but-open — because closed+empty collapses to
/// `Ok(None)` (natural "end of stream" signalling for PHP callers).
#[derive(Debug, PartialEq, Eq)]
pub enum TryRecvErr {
    /// Channel is empty but still open; a future send may succeed.
    WouldBlockEmpty,
}

/// MPMC bounded channel state. Producers push via `tx`; consumers
/// lock `rx` and pop. `pending` tracks queue depth for gauges and
/// for `debug_snapshot`. Notify handles are plumbed so the fiber
/// waker lists below can wire them up without a struct change.
pub struct ChannelInner {
    tx: crossbeam_channel::Sender<Payload>,
    rx: Mutex<crossbeam_channel::Receiver<Payload>>,
    capacity: usize,
    pending: AtomicUsize,
    closed: AtomicBool,
    notify_recv: Arc<Notify>,
    notify_send: Arc<Notify>,
    // Fiber-suspending waker lists. Synthetic-promise ids parked
    // here by `register_recv_waiter` / `register_send_waiter` are
    // resolved by `try_send` / `try_recv` / `close` on the producing /
    // consuming / closing thread. SmallVec inline cap 4 matches the
    // expected common case (a handful of fibers awaiting one channel).
    recv_waiters: Mutex<SmallVec<[i64; 4]>>,
    send_waiters: Mutex<SmallVec<[i64; 4]>>,
    /// Front-stash for items popped from the buffer for a recv-waiter that
    /// turned out dead (cancelled mid-delivery). Every recv path drains
    /// this before the crossbeam buffer, so a re-parked item is preferred
    /// over newer buffered ones — best-effort ordering, not a strict FIFO
    /// guarantee: a concurrent receiver can still pull a newer buffered
    /// item before a re-parked one becomes visible (cross-receiver FIFO is
    /// never guaranteed in an MPMC channel). Re-deposit here is non-
    /// blocking and loss-free. Populated only on the rare dead-waiter
    /// race; inline cap 2.
    recv_front: Mutex<SmallVec<[Payload; 2]>>,
    /// Multiset of `SharedRef` views the channel currently holds in transit
    /// (crossbeam buffer + front-stash). The crossbeam buffer is not
    /// iterable, so this side-index is the only way to expose in-flight
    /// edges to the cycle walker via [`children`](SharedInner::children).
    /// Maintained alongside `bump_pending`/`drop_pending` (enter/leave).
    /// Dups allowed — the walker dedups via its visited set.
    in_flight: Mutex<Vec<SharedRef>>,
    // Exercised by blocking paths; surfaced to observability.
    senders_blocked: AtomicU32,
    receivers_blocked: AtomicU32,
    items_sent_total: AtomicU64,
    #[allow(dead_code)]
    items_dropped_total: AtomicU64,
    /// Registry id, bound once by the creating FFI path via
    /// [`ChannelInner::bind_id`] right after `registry.insert`. `None`
    /// before bind (or in Rust-only tests that skip registry insertion)
    /// — memory tracking is then a no-op because the channel is not
    /// reachable via the registry.
    self_id: OnceLock<SharedId>,
    /// Cached `Weak<Entry>` for the fast-path of [`track_payload_delta`].
    /// See [`MapInner::self_entry`] for the rationale — same shape,
    /// same fallback to the id-based slow path for test fixtures.
    self_entry: OnceLock<Weak<Entry>>,
}

impl ChannelInner {
    /// Construct a fresh channel with the given capacity. `capacity`
    /// is clamped to `>= 1` — crossbeam's bounded channel panics on
    /// zero and the FFI layer surfaces that as a TypeException before
    /// reaching this path; this internal constructor simply never
    /// panics.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (tx, rx) = crossbeam_channel::bounded::<Payload>(capacity);
        Self {
            tx,
            rx: Mutex::new(rx),
            capacity,
            pending: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            notify_recv: Arc::new(Notify::new()),
            notify_send: Arc::new(Notify::new()),
            recv_waiters: Mutex::new(SmallVec::new()),
            send_waiters: Mutex::new(SmallVec::new()),
            recv_front: Mutex::new(SmallVec::new()),
            in_flight: Mutex::new(Vec::new()),
            senders_blocked: AtomicU32::new(0),
            receivers_blocked: AtomicU32::new(0),
            items_sent_total: AtomicU64::new(0),
            items_dropped_total: AtomicU64::new(0),
            self_id: OnceLock::new(),
            self_entry: OnceLock::new(),
        }
    }

    /// Bind this Channel to its registry id. Called exactly once by the
    /// creating FFI path right after `registry.insert`. Subsequent calls
    /// are silently ignored (OnceLock semantics). Required for the
    /// per-payload memory tracking to find its entry on
    /// [`bump_pending`] / [`drop_pending`].
    pub fn bind_id(&self, id: SharedId) {
        let _ = self.self_id.set(id);
    }

    /// Bind this Channel to its registry entry. Production path: call
    /// from the creating FFI right after `registry.insert` with
    /// `Arc::downgrade(&entry_arc)`. Sets both id and the cached
    /// `Weak<Entry>` that [`track_payload_delta`] uses to bypass the
    /// DashMap shard-lock on every `bump_pending` / `drop_pending`.
    pub fn bind_entry(&self, weak: Weak<Entry>) {
        if let Some(arc) = weak.upgrade() {
            let _ = self.self_id.set(arc.id);
        }
        let _ = self.self_entry.set(weak);
    }

    /// Increment `pending` by one and book one payload's worth of
    /// memory against `total_bytes`. Centralises the two side-effects so
    /// every send path (buffer commit, blocking send, send_many) ends up
    /// doing the same accounting.
    ///
    /// Pre-existing race (predates the memory-tracking patch): both
    /// `try_send` and `send_blocking` deposit into the crossbeam buffer
    /// *before* calling [`bump_pending`]. A racing `try_recv` between
    /// those two steps observes the item, calls [`drop_pending`], and
    /// underflows `pending` (`fetch_sub` from zero wraps to
    /// `usize::MAX`). The memory-tracking side stays safe because
    /// [`SharedRegistry::adjust_mem_bytes`] saturates at zero on
    /// negative deltas — `total_bytes` cannot wrap.
    ///
    /// **Drift direction — `total_bytes` leaks UPWARD**, not down:
    /// the racing `drop_pending` saturates at 0 (no refund happens),
    /// then the lagging `bump_pending` adds `CHANNEL_PER_PAYLOAD_BYTES`
    /// against an empty buffer. Under sustained send/recv hammering a
    /// single Channel can accrete megabytes of phantom bytes and trip
    /// the global `max_bytes` cap while its actual queue is empty.
    /// The `pending` counter undercounts symmetrically.
    ///
    /// Proper fix (deferred — invasive, touches 8 mutator sites): swap
    /// the order so `bump_pending` runs *before* `tx.try_send` with a
    /// rollback `drop_pending` on `Err`, and apply the symmetric flip
    /// to recv. Or drop the parallel atomic entirely and derive
    /// `pending` from `tx.len()` (cheap on crossbeam, no race window).
    fn bump_pending(&self) {
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.track_payload_delta(1);
    }

    /// Decrement `pending` by one and refund one payload's worth of
    /// memory. Mirrors [`bump_pending`] on the consumer side; see the
    /// note there for the pre-existing send/recv interleaving caveat.
    fn drop_pending(&self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
        self.track_payload_delta(-1);
    }

    /// Snapshot the `SharedRef` views of a payload's keepalive — captured
    /// before the payload is moved into the crossbeam buffer so the enter
    /// sites can register them after a successful deposit.
    fn keepalive_views(keepalive: &[SharedRefOwned]) -> SmallVec<[SharedRef; 1]> {
        keepalive.iter().map(SharedRefOwned::as_view).collect()
    }

    /// Register in-transit refs in the cycle index (enter). No-op when empty.
    fn index_add(&self, views: &[SharedRef]) {
        if views.is_empty() {
            return;
        }
        self.in_flight.lock().extend_from_slice(views);
    }

    /// Remove one occurrence of each ref from the cycle index (leave). No-op
    /// when empty. Multiset semantics: a value sent twice is removed once per
    /// recv.
    fn index_remove(&self, keepalive: &[SharedRefOwned]) {
        if keepalive.is_empty() {
            return;
        }
        let mut idx = self.in_flight.lock();
        for r in keepalive {
            let v = r.as_view();
            if let Some(pos) = idx.iter().position(|x| *x == v) {
                idx.swap_remove(pos);
            }
        }
    }

    /// Pop the next front-stash item (re-deposited, oldest) if any,
    /// WITHOUT signalling senders. Decrements `pending`. Used by
    /// `drain_buffered_to_waiters`, which defers the send-waiter wake until
    /// it knows the item is actually being delivered — an item that bounces
    /// back to the stash (dead waiter) never left the channel, so waking a
    /// sender to refill the "freed" slot would overshoot the bound.
    /// Cheap when empty: one uncontended lock, no allocation.
    fn take_front_stash(&self) -> Option<Payload> {
        let mut front = self.recv_front.lock();
        if front.is_empty() {
            return None;
        }
        let p = front.remove(0);
        drop(front);
        self.drop_pending();
        self.index_remove(&p.keepalive);
        Some(p)
    }

    /// Pop the next front-stash item (re-deposited, oldest) if any, and —
    /// when one was present — wake a parked send-waiter, since the item is
    /// leaving the channel and occupancy genuinely drops. Recv paths call
    /// this before touching the crossbeam buffer so a re-parked item is
    /// preferred over newer buffered ones (best-effort, not a strict
    /// cross-receiver FIFO); they consume the popped item directly, so the
    /// wake is always correct.
    fn pop_front_stash(&self) -> Option<Payload> {
        let p = self.take_front_stash()?;
        self.drain_one_send_waiter_on_slot_free();
        Some(p)
    }

    /// Re-deposit a bounced item at the front of the stream. Non-blocking
    /// and loss-free; keeps it counted in `pending`. Used when a buffered
    /// item was popped for a recv-waiter that turned out dead and the
    /// freed slot was taken before it could be returned to the buffer.
    fn push_front_stash(&self, p: Payload) {
        let views = Self::keepalive_views(&p.keepalive);
        self.recv_front.lock().push(p);
        self.bump_pending();
        self.index_add(&views);
    }

    fn track_payload_delta(&self, delta_count: isize) {
        let delta = delta_count * CHANNEL_PER_PAYLOAD_BYTES;
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

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Build an in-transit [`Payload`] from raw portbuf wire bytes: scan for
    /// nested `Shared\*` refs, reject cycles, and resolve each ref into a
    /// strong keepalive that pins the entry alive while the value is in
    /// transit. The common no-shared case allocates nothing beyond the bytes.
    ///
    /// Called once per FFI send entry; core send methods take the built
    /// `Payload`. Returns [`SharedError::Cycle`] when the value would close a
    /// reference cycle back to this channel.
    pub fn build_payload(&self, bytes: Vec<u8>) -> Result<Payload, SharedError> {
        if bytes.is_empty() {
            return Ok(Payload {
                bytes,
                keepalive: SmallVec::new(),
            });
        }
        // The scan only succeeds on well-formed portbuf (which the PHP
        // serializer always emits). A non-portbuf buffer — reachable only via
        // direct FFI use, e.g. Rust-level tests — carries no resolvable shared
        // ref, so transport it opaquely with no keepalive rather than failing.
        let roots = match crate::plugins::ox_shared::value::scan_shared_refs(&bytes) {
            Ok(r) => r,
            Err(_) => return Ok(Payload::bytes_only(bytes)),
        };
        if roots.is_empty() {
            return Ok(Payload {
                bytes,
                keepalive: SmallVec::new(),
            });
        }
        self.cycle_check(&roots)?;
        let mut keepalive: SmallVec<[SharedRefOwned; 1]> = SmallVec::new();
        if let Some(reg) = REGISTRY.get() {
            for r in &roots {
                match reg.lookup(r.id) {
                    Ok(arc) => keepalive.push(SharedRefOwned::from_arc(arc)),
                    // Near-impossible: the sending zval holds these Arcs across
                    // the send() call. Defensive degrade in release; the
                    // receiver would observe NULL for this one ref as before.
                    Err(_) => debug_assert!(false, "stale shared ref at channel send"),
                }
            }
        }
        Ok(Payload { bytes, keepalive })
    }

    /// Reject a send whose value would close a strong-ref cycle back to this
    /// channel. Mirror of `Shared\Map::check_cycles`. No-op without a bound
    /// `self_id` or registry (Rust-only fixtures).
    fn cycle_check(&self, roots: &[SharedRef]) -> Result<(), SharedError> {
        use crate::plugins::ox_shared::cycle::{would_create_cycle, CycleError};
        let (Some(reg), Some(self_id)) = (REGISTRY.get(), self.self_id.get().copied()) else {
            return Ok(());
        };
        let cfg = reg.config();
        for root in roots {
            let children_of = |id, out: &mut Vec<SharedRef>| {
                if let Ok(e) = reg.lookup(id) {
                    e.inner.children(out);
                }
            };
            match would_create_cycle(
                *root,
                self_id,
                cfg.cycle_detect_depth,
                cfg.cycle_detect_edges,
                children_of,
            ) {
                Ok(()) => {}
                Err(CycleError::CycleFound(path)) => {
                    set_last_error(format!(
                        "Shared\\Channel: send would form a reference cycle: {}",
                        crate::plugins::ox_shared::cycle::format_cycle_path(&path)
                    ));
                    return Err(SharedError::Cycle);
                }
                Err(CycleError::DepthExceeded) => {
                    set_last_error(format!(
                        "Shared\\Channel: cycle detection depth limit ({}) exceeded; \
                         raise SHARED_CYCLE_DETECT_DEPTH or break the graph",
                        cfg.cycle_detect_depth
                    ));
                    return Err(SharedError::Cycle);
                }
                Err(CycleError::EdgeLimitExceeded) => {
                    set_last_error(format!(
                        "Shared\\Channel: cycle detection edge limit ({}) exceeded; \
                         raise SHARED_CYCLE_DETECT_EDGES or break the graph",
                        cfg.cycle_detect_edges
                    ));
                    return Err(SharedError::Cycle);
                }
            }
        }
        Ok(())
    }

    /// Non-blocking send. See module docs for semantics.
    ///
    /// Ordering: a parked recv-waiter (synthetic promise from a
    /// suspended fiber) takes precedence over the buffer. If no waiter
    /// accepts the payload (empty list, or all entries dead), the value
    /// is deposited into the crossbeam buffer as before.
    pub fn try_send(&self, payload: Payload) -> Result<(), TrySendErr> {
        if self.is_closed() {
            return Err(TrySendErr::Closed(payload));
        }
        let payload = match self.drain_one_recv_waiter_with(payload) {
            None => {
                // Waiter accepted — treat as a successful send for
                // bookkeeping (counter only). Do NOT tick
                // `notify_recv`: that wakes blocking receivers polling
                // the crossbeam buffer, and the payload went straight
                // to the fiber — the buffer is still empty, so any
                // woken receiver would find nothing and loop. Waking
                // is reserved for actual buffer arrivals.
                self.items_sent_total.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            Some(p) => p,
        };
        // Capture keepalive views before the payload moves into the buffer, so
        // a successful deposit can register them in the cycle index.
        let views = Self::keepalive_views(&payload.keepalive);
        match self.tx.try_send(payload) {
            Ok(()) => {
                self.bump_pending();
                self.index_add(&views);
                self.items_sent_total.fetch_add(1, Ordering::Relaxed);
                self.notify_recv.notify_one();
                // Double-check for a recv-waiter that parked in the gap
                // between the `drain_one_recv_waiter_with` above (saw no
                // live waiter) and this deposit. A fiber consumer does not
                // observe `notify_recv`, so without this it would strand
                // until its timeout / the next send. `register_recv_waiter`
                // performs the symmetric re-drain after parking; together
                // they close the park-vs-deposit race. No-op (one peek) in
                // the common no-waiter case.
                self.drain_buffered_to_waiters();
                Ok(())
            }
            Err(TrySendError::Full(p)) => Err(TrySendErr::Full(p)),
            Err(TrySendError::Disconnected(p)) => Err(TrySendErr::Closed(p)),
        }
    }

    /// Non-blocking recv. Returns:
    ///   * `Ok(Some(p))` — got an item.
    ///   * `Ok(None)`    — channel closed and drained (end of stream).
    ///   * `Err(WouldBlockEmpty)` — empty but still open; caller may retry.
    pub fn try_recv(&self) -> Result<Option<Payload>, TryRecvErr> {
        // Front-stash (re-deposited, oldest) takes precedence over the
        // crossbeam buffer. A front pop frees no crossbeam slot, so it
        // does not signal waiting senders.
        if let Some(p) = self.pop_front_stash() {
            return Ok(Some(p));
        }
        let rx = self.rx.lock();
        match rx.try_recv() {
            Ok(p) => {
                drop(rx);
                self.drop_pending();
                self.index_remove(&p.keepalive);
                self.notify_send.notify_one();
                // Slot just freed — hand it to a parked send-waiter if
                // any. Resolving with empty-Value means "you may retry
                // the send now"; the fiber re-enters try_send on its
                // next tick.
                self.drain_one_send_waiter_on_slot_free();
                Ok(Some(p))
            }
            Err(TryRecvError::Empty) => {
                if self.is_closed() {
                    Ok(None)
                } else {
                    Err(TryRecvErr::WouldBlockEmpty)
                }
            }
            Err(TryRecvError::Disconnected) => Ok(None),
        }
    }

    /// Close the channel. Idempotent; returns `true` only for the
    /// first successful close. Wakes all parked notifiers so blocked
    /// senders/receivers (blocking threads and fiber waiters) can
    /// observe the transition.
    pub fn close(&self) -> bool {
        let was_open = !self.closed.swap(true, Ordering::AcqRel);
        if was_open {
            self.notify_recv.notify_waiters();
            self.notify_send.notify_waiters();
            // Resolve any parked fibers so they unblock promptly.
            // Recv-waiters resolve as Cancelled ("no value, go bail
            // out"); send-waiters resolve as ClosedException (the
            // payload they tried to deliver never landed).
            self.cancel_all_recv_waiters();
            self.cancel_all_send_waiters_with_closed();
        }
        was_open
    }

    /// Thread-blocking send. Used by the "not inside a fiber" path per
    /// 24-type-channel.md §Fiber integration. Tries a non-blocking send
    /// first; on `Full` enters a bounded-poll wait loop so `close()`
    /// can wake us promptly.
    ///
    /// `wait` controls the blocking behaviour:
    /// - `Wait::Forever` — loop until delivery or close.
    /// - `Wait::Try` — return `Err(Timeout)` immediately if full.
    /// - `Wait::Bounded(d)` — wait up to `d` before returning `Err(Timeout)`.
    ///
    /// Note: the poll quantum exists because crossbeam's
    /// `send_timeout` only observes disconnect when all receivers are
    /// dropped, not a user-level close flag. A 20ms poll keeps the
    /// test guarantees ("wakes promptly on close") within bounds
    /// without pulling in an extra sync primitive.
    pub fn send_blocking(&self, payload: Payload, wait: Wait) -> Result<(), SharedError> {
        // Fast path: attempt a non-blocking send (which already does
        // bookkeeping + notify on success). NO guard here — a fast-path
        // success never blocked, so the `senders_blocked` gauge must not
        // transiently tick for it.
        let payload = match self.try_send(payload) {
            Ok(()) => return Ok(()),
            Err(TrySendErr::Closed(_)) => return Err(SharedError::Closed),
            Err(TrySendErr::Full(p)) => p,
        };

        // Wait::Try means non-blocking only — don't enter the slow path.
        if matches!(wait, Wait::Try) {
            return Err(SharedError::Timeout);
        }

        // Genuinely going to block — arm the gauge now, so it covers
        // only the actually-blocking span.
        let _guard = SendersBlockedGuard::new(&self.senders_blocked);

        // Slow path: poll `send_timeout` in small quanta so `close()`
        // from another thread wakes us within POLL_QUANTUM.
        let deadline = match wait {
            Wait::Forever => None,
            Wait::Try => unreachable!(),
            Wait::Bounded(d) => Some(std::time::Instant::now() + d),
        };
        let mut payload = payload;
        // Keepalive is constant across retries (the same payload bounces on
        // Timeout); capture its views once for the cycle index on deposit.
        let views = Self::keepalive_views(&payload.keepalive);
        loop {
            if self.is_closed() {
                return Err(SharedError::Closed);
            }
            let quantum = match deadline {
                None => POLL_QUANTUM,
                Some(d) => {
                    let remaining = d.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return Err(SharedError::Timeout);
                    }
                    remaining.min(POLL_QUANTUM)
                }
            };
            match self.tx.send_timeout(payload, quantum) {
                Ok(()) => {
                    self.bump_pending();
                    self.index_add(&views);
                    self.items_sent_total.fetch_add(1, Ordering::Relaxed);
                    self.notify_recv.notify_one();
                    // Bridge the buffer→fiber gap: a fiber recv-waiter
                    // parked AFTER we entered this slow path does not
                    // observe `notify_recv` (it awaits a synthetic
                    // promise), so the item just deposited would sit in
                    // the buffer invisible to it until some producer
                    // happened to hit the `try_send` fast path. Hand it
                    // off now — no-op when there are no parked waiters.
                    self.drain_buffered_to_waiters();
                    return Ok(());
                }
                Err(SendTimeoutError::Timeout(p)) => {
                    payload = p;
                    // loop — re-check close and deadline.
                }
                Err(SendTimeoutError::Disconnected(_)) => return Err(SharedError::Closed),
            }
        }
    }

    /// Thread-blocking receive. Returns `Ok(Some(p))` on delivery,
    /// `Ok(None)` on closed+drained, `Err(Timeout)` otherwise.
    ///
    /// `wait` controls the blocking behaviour:
    /// - `Wait::Forever` — loop until delivery or close.
    /// - `Wait::Try` — return `Err(Timeout)` immediately if empty (and open).
    /// - `Wait::Bounded(d)` — wait up to `d` before returning `Err(Timeout)`.
    ///
    /// The `rx` lock is held only across each poll quantum — never
    /// across the full caller-supplied timeout — so concurrent
    /// receivers serialize but make progress.
    pub fn recv_blocking(&self, wait: Wait) -> Result<Option<Payload>, SharedError> {
        // Front-stash (re-deposited, oldest) takes precedence.
        if let Some(p) = self.pop_front_stash() {
            return Ok(Some(p));
        }
        // Fast path: try once under a tight lock scope. NO guard here —
        // a fast-path hit (or a closed+empty miss) never blocked, so
        // the `receivers_blocked` gauge must not transiently tick.
        {
            let rx = self.rx.lock();
            match rx.try_recv() {
                Ok(p) => {
                    drop(rx);
                    self.drop_pending();
                    self.index_remove(&p.keepalive);
                    self.notify_send.notify_one();
                    return Ok(Some(p));
                }
                Err(TryRecvError::Empty) => {
                    if self.is_closed() {
                        return Ok(None);
                    }
                    // fall through — drop rx before entering slow path.
                }
                Err(TryRecvError::Disconnected) => return Ok(None),
            }
            // rx dropped here.
        }

        // Wait::Try means non-blocking only — don't enter the slow path.
        if matches!(wait, Wait::Try) {
            return Err(SharedError::Timeout);
        }

        // Genuinely going to block — arm the gauge now, so it covers
        // only the actually-blocking span.
        let _guard = ReceiversBlockedGuard::new(&self.receivers_blocked);

        // Slow path: poll `recv_timeout` in quanta so `close()` can
        // wake us within POLL_QUANTUM.
        let deadline = match wait {
            Wait::Forever => None,
            Wait::Try => unreachable!(),
            Wait::Bounded(d) => Some(std::time::Instant::now() + d),
        };
        loop {
            // Front-stash first — a bounced item parked here is the oldest
            // and must come out before anything in the crossbeam buffer.
            if let Some(p) = self.pop_front_stash() {
                return Ok(Some(p));
            }
            if self.is_closed() {
                // Drain-before-close: re-check the queue under the
                // lock in case a sender managed to enqueue before
                // setting closed.
                let rx = self.rx.lock();
                return match rx.try_recv() {
                    Ok(p) => {
                        drop(rx);
                        self.drop_pending();
                        self.index_remove(&p.keepalive);
                        self.notify_send.notify_one();
                        Ok(Some(p))
                    }
                    Err(_) => Ok(None),
                };
            }
            let quantum = match deadline {
                None => POLL_QUANTUM,
                Some(d) => {
                    let remaining = d.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return Err(SharedError::Timeout);
                    }
                    remaining.min(POLL_QUANTUM)
                }
            };
            let recv_res = {
                let rx = self.rx.lock();
                rx.recv_timeout(quantum)
            };
            match recv_res {
                Ok(p) => {
                    self.drop_pending();
                    self.index_remove(&p.keepalive);
                    self.notify_send.notify_one();
                    return Ok(Some(p));
                }
                Err(RecvTimeoutError::Timeout) => {
                    // loop — re-check close and deadline.
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    /// Send many payloads; returns the count actually sent. Stops on
    /// the first `send_blocking` failure (Closed or Timeout) WITHOUT
    /// raising — matches the spec's "returns how many were actually
    /// sent before full/closed/timeout" semantics.
    ///
    /// `wait` controls blocking behaviour per-item:
    /// - `Wait::Forever` — block indefinitely per-item.
    /// - `Wait::Try` — drain via `try_send` only; stop on first non-Ok.
    /// - `Wait::Bounded(d)` — amortise the budget across the batch;
    ///   each item gets the remaining time up to the deadline.
    pub fn send_many(&self, payloads: Vec<Payload>, wait: Wait) -> u64 {
        if matches!(wait, Wait::Try) {
            let mut sent = 0u64;
            for p in payloads {
                match self.try_send(p) {
                    Ok(()) => sent += 1,
                    Err(_) => break,
                }
            }
            return sent;
        }

        let deadline = match wait {
            Wait::Forever => None,
            Wait::Try => unreachable!(),
            Wait::Bounded(d) => Some(std::time::Instant::now() + d),
        };
        let mut sent = 0u64;
        for p in payloads {
            let item_wait = match deadline {
                Some(d) => {
                    let r = d.saturating_duration_since(std::time::Instant::now());
                    if r.is_zero() {
                        break;
                    }
                    Wait::Bounded(r)
                }
                None => Wait::Forever,
            };
            match self.send_blocking(p, item_wait) {
                Ok(()) => sent += 1,
                Err(_) => break, // timeout or closed — stop, do not raise.
            }
        }
        sent
    }

    /// Receive up to `max` payloads.
    ///
    /// * `max == 0` — drain whatever is currently buffered without
    ///   waiting; returns the drained items (possibly empty).
    /// * `max > 0, Wait::Forever` — block indefinitely until either
    ///   `max` items are collected or the channel is closed+empty.
    /// * `max > 0, Wait::Try` — drain immediately available items only.
    /// * `max > 0, Wait::Bounded(d)` — block up to `d` collecting
    ///   as many as possible, up to `max`. Returns whatever was
    ///   collected by the deadline (possibly empty).
    pub fn recv_many(&self, max: usize, wait: Wait) -> Vec<Payload> {
        // Small initial capacity to keep the common "drain a few" path
        // from over-allocating, with a sane upper bound for large
        // requested caps.
        let cap_hint = max.clamp(1, 64);
        let mut out = Vec::with_capacity(cap_hint);
        if max == 0 {
            // Drain what's available, no wait. try_recv stops on both
            // "empty but open" (Err) and "closed + empty" (Ok(None)).
            while let Ok(Some(p)) = self.try_recv() {
                out.push(p);
            }
            return out;
        }
        if matches!(wait, Wait::Try) {
            // Non-blocking drain: collect only what's immediately available.
            while out.len() < max {
                match self.try_recv() {
                    Ok(Some(p)) => out.push(p),
                    _ => break,
                }
            }
            return out;
        }
        let deadline = match wait {
            Wait::Forever => None,
            Wait::Try => unreachable!(),
            Wait::Bounded(d) => Some(std::time::Instant::now() + d),
        };
        while out.len() < max {
            let item_wait = match deadline {
                Some(d) => {
                    let r = d.saturating_duration_since(std::time::Instant::now());
                    if r.is_zero() {
                        break;
                    }
                    Wait::Bounded(r)
                }
                None => Wait::Forever,
            };
            match self.recv_blocking(item_wait) {
                Ok(Some(p)) => out.push(p),
                Ok(None) => break, // closed + empty
                Err(_) => break,   // timeout
            }
        }
        out
    }

    // ─── Gauge accessors ──────────────────────────────────────────────
    //
    // Kept as `&Atomic*` returns so observability code can add+load
    // in one lock-free op. The counters are initialised to 0 and
    // written by the blocking path.

    pub fn senders_blocked(&self) -> &AtomicU32 {
        &self.senders_blocked
    }

    pub fn receivers_blocked(&self) -> &AtomicU32 {
        &self.receivers_blocked
    }

    #[allow(dead_code)]
    pub fn items_sent_total(&self) -> &AtomicU64 {
        &self.items_sent_total
    }

    #[allow(dead_code)]
    pub fn items_dropped_total(&self) -> &AtomicU64 {
        &self.items_dropped_total
    }

    // ─── Fiber-suspending waker lists ─────────────────────────
    //
    // Synthetic-promise ids are parked by fibers via the FFI
    // (`register_recv_waiter` / `register_send_waiter`) and resolved
    // here from the producer / consumer / closer thread. Resolution is
    // cross-thread-safe because `synthetic::resolve` targets a tokio
    // `oneshot::Sender` stored in a global `DashMap`.

    /// Park a synthetic-promise id on the recv-waiter list. Cover two
    /// races that can only be observed *while holding the waiter lock*:
    ///
    /// 1. closed+empty — nothing will ever arrive; cancel immediately.
    /// 2. closed+drained race-window — ditto (checked after the initial
    ///    fast-path).
    /// 3. an item already sits in the buffer (a concurrent `try_send`
    ///    landed before we parked) — hand it off via
    ///    [`drain_buffered_to_waiters`].
    pub fn register_recv_waiter(&self, promise_id: i64) {
        // Fast-fail: if closed+empty the waiter would never fire.
        if self.is_closed() && self.pending() == 0 {
            synthetic::cancel(promise_id);
            return;
        }
        {
            let mut waiters = self.recv_waiters.lock();
            waiters.push(promise_id);
        }
        // Race cover: a `try_send` between the closed-check above and
        // the push, OR pre-existing buffered items that no consumer is
        // currently polling. Drain buffer into parked waiters.
        self.drain_buffered_to_waiters();
    }

    /// Park a synthetic-promise id on the send-waiter list. If the
    /// channel is closed, fail fast with a ClosedException rather than
    /// park (the sender would never be able to deposit the payload).
    pub fn register_send_waiter(&self, promise_id: i64) {
        if self.is_closed() {
            synthetic::resolve(
                promise_id,
                PromisePayload::Exception(
                    "OxPHP\\Shared\\ClosedException".into(),
                    "channel closed".into(),
                ),
            );
            return;
        }
        let mut waiters = self.send_waiters.lock();
        waiters.push(promise_id);
    }

    /// Pop one live recv-waiter and resolve it with `payload`. Returns
    /// `None` on successful handoff, `Some(payload)` if no live waiter
    /// accepted (list empty or every entry dead).
    ///
    /// A waiter is "dead" if the fiber-side receiver was dropped or the
    /// promise was already resolved by a concurrent cancel.
    ///
    /// Ownership: [`synthetic::resolve_value`] removes the waiter's sole
    /// sender before building the payload, and hands the payload back
    /// (`Some`) whenever the waiter cannot take it — either the promise was
    /// already resolved by another resolver (its own `recvTimeout` cancel /
    /// close winning the race), or the receiver was dropped (the parked
    /// fiber was torn down). Either way the payload is never consumed: we
    /// forward it to the next parked id, and if none remains, return it to
    /// the caller to re-deposit. No clone, no lost message.
    fn drain_one_recv_waiter_with(&self, mut payload: Payload) -> Option<Payload> {
        loop {
            // Pop head — FIFO: first parked wakes first.
            let id = {
                let mut waiters = self.recv_waiters.lock();
                if waiters.is_empty() {
                    return Some(payload);
                }
                waiters.remove(0)
            };
            // Carry the keepalive into the delivery so the receiving fiber's
            // AsyncResult pins the nested entries until it deserializes. Common
            // no-shared case stays a `None` (no Box allocation).
            let kb: synthetic::Keepalive = if payload.keepalive.is_empty() {
                None
            } else {
                Some(Box::new(std::mem::take(&mut payload.keepalive)))
            };
            match synthetic::resolve_value(id, payload.bytes, kb) {
                // Delivered to a live receiver.
                None => return None,
                // Waiter could not take it (resolved elsewhere, or its receiver
                // is gone) — bytes + keepalive survived; rebuild the Payload
                // intact and try the next id / re-park. Downcast only on this
                // rare dead-waiter path.
                Some((returned, kb)) => {
                    let keepalive = kb
                        .and_then(|b| b.downcast::<SmallVec<[SharedRefOwned; 1]>>().ok())
                        .map(|b| *b)
                        .unwrap_or_default();
                    payload = Payload {
                        bytes: returned,
                        keepalive,
                    };
                }
            }
        }
    }

    /// Pop one live send-waiter and wake it with an empty payload,
    /// meaning "a slot freed; retry your send". Returns silently if no
    /// live waiter exists.
    fn drain_one_send_waiter_on_slot_free(&self) {
        loop {
            let id = {
                let mut waiters = self.send_waiters.lock();
                if waiters.is_empty() {
                    return;
                }
                waiters.remove(0)
            };
            if synthetic::resolve(id, PromisePayload::Value(Vec::new(), None)) {
                return;
            }
            // Dead — skip and try the next parked id.
        }
    }

    /// Bulk drain: while we still have buffered items AND parked
    /// recv-waiters, pop one item / one live waiter and deliver. Called
    /// from `register_recv_waiter` (cover a `try_send` that landed in the
    /// buffer just before we parked) and after a slow-path buffer deposit
    /// (wake a fiber consumer that does not observe `notify_recv`).
    ///
    /// Loss-free and non-blocking: items are sourced front-stash first
    /// (oldest), then the crossbeam buffer; and when every parked waiter
    /// turns out dead, `drain_one_recv_waiter_with` hands the payload back
    /// and it is re-parked at the front via [`push_front_stash`] rather
    /// than dropped (which loses the item) or blocked on (which would
    /// starve the runtime).
    fn drain_buffered_to_waiters(&self) {
        loop {
            // Are there any parked waiters? Peek without removing.
            {
                let waiters = self.recv_waiters.lock();
                if waiters.is_empty() {
                    return;
                }
            }
            // Take one item: front-stash first (oldest), then the buffer.
            // Neither pop wakes a send-waiter here — that is deferred to a
            // successful delivery below, because an item that bounces back to
            // the stash (dead waiter) never leaves the channel and must not
            // free a slot.
            let item = if let Some(p) = self.take_front_stash() {
                p
            } else {
                let rx = self.rx.lock();
                match rx.try_recv() {
                    Ok(p) => {
                        drop(rx);
                        self.drop_pending();
                        self.index_remove(&p.keepalive);
                        self.notify_send.notify_one();
                        p
                    }
                    Err(_) => return,
                }
            };
            // Deliver to one live waiter.
            match self.drain_one_recv_waiter_with(item) {
                None => {
                    // Delivered — the item left the channel, so occupancy
                    // dropped by one and a parked send-waiter may now fit.
                    // This holds whether the item came from the buffer or the
                    // stash. (A blocking sender, if any, is woken by crossbeam
                    // itself when a buffer pop frees a slot.)
                    self.drain_one_send_waiter_on_slot_free();
                    continue;
                }
                Some(p) => {
                    // Every parked waiter was dead (and now reaped). Re-park
                    // the item at the front so the next recv drains it before
                    // the crossbeam buffer. Non-blocking, loss-free.
                    //
                    // Wake NO send-waiter: the item has NOT left the channel,
                    // it only moved buffer/stash→front-stash and still counts
                    // toward `pending`/capacity. Waking a sender to fill the
                    // slot would put `cap` items in the buffer + 1 in the
                    // stash = cap+1, overshooting the bound. The parked sender
                    // is instead woken by `pop_front_stash` once this item is
                    // actually consumed (occupancy genuinely drops).
                    self.push_front_stash(p);
                    // The item is now VISIBLE in the front-stash. Loop again
                    // to deliver it to any waiter that parked while it was
                    // briefly held in a local above — that waiter does not
                    // observe `notify_recv`, so without this re-check it
                    // would strand until its timeout / the next send. If no
                    // waiter is parked, the next peek returns and the item
                    // waits in the stash for a future recv.
                    continue;
                }
            }
        }
    }

    /// On `close()`: cancel every parked recv-waiter so suspended
    /// fibers unblock promptly instead of waiting for the channel's
    /// per-thread poll to notice `is_closed()`.
    fn cancel_all_recv_waiters(&self) {
        let drained: SmallVec<[i64; 4]> = {
            let mut waiters = self.recv_waiters.lock();
            std::mem::take(&mut *waiters)
        };
        for id in drained {
            synthetic::resolve(id, PromisePayload::Cancelled);
        }
    }

    /// On `close()`: resolve every parked send-waiter with a
    /// `ClosedException` — the fiber was trying to deposit a value,
    /// so "the channel closed underneath you" is the precise error.
    fn cancel_all_send_waiters_with_closed(&self) {
        let drained: SmallVec<[i64; 4]> = {
            let mut waiters = self.send_waiters.lock();
            std::mem::take(&mut *waiters)
        };
        for id in drained {
            synthetic::resolve(
                id,
                PromisePayload::Exception(
                    "OxPHP\\Shared\\ClosedException".into(),
                    "channel closed".into(),
                ),
            );
        }
    }
}

impl SharedInner for ChannelInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Channel
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn debug_snapshot(&self) -> SharedValue {
        SharedValue::Long(self.pending() as i64)
    }
    fn mem_bytes(&self) -> usize {
        64 + self.pending() * 32
    }
    fn on_drop(&self) {
        self.close();
    }
    fn on_shutdown_notify(&self) {
        self.close();
    }
    fn children(&self, out: &mut Vec<SharedRef>) {
        out.extend(self.in_flight.lock().iter().copied());
    }
}

// Helper trait for downcasting `Arc<dyn SharedInner>` to `&ChannelInner`.
// Implemented on `dyn SharedInner` (not `+ Send + Sync`) to match
// `Entry.inner`'s actual trait-object type — identical pattern to
// `SharedInnerOnceExt` / `SharedInnerMutexExt`.
pub trait SharedInnerChannelExt {
    fn as_any_channel(&self) -> Option<&ChannelInner>;
}

impl SharedInnerChannelExt for dyn SharedInner {
    fn as_any_channel(&self) -> Option<&ChannelInner> {
        self.as_any().downcast_ref::<ChannelInner>()
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────
//
// All functions here are the stable ABI surface consumed by
// `ext/bridge/oxphp_bridge.c` and ultimately by the PHP `Shared\Channel`
// class registration. Every function wraps its body in
// `ffi_entry` so panics and SharedError values translate uniformly to
// the negative status codes in the exception contract.
//
// Output-buffer ownership rule: on success `*out_buf` is either null
// (for empty payloads) or a pointer owned by C, allocated via
// `libc::malloc` (see `payload_to_malloc`). C releases it with
// `oxphp_portable_free`, which forwards to `libc::free`.

/// Construct a new channel with the given fixed capacity and register
/// it with the Shared registry. Capacity must be `>= 1`; zero is
/// rejected with `SharedError::Type` (surfacing as TypeException on the
/// PHP side).
///
/// # Safety
/// `out_ptr` must be valid for writes of `*const Entry` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_create(
    capacity: u64,
    out_ptr: *mut *const Entry,
) -> c_int {
    if out_ptr.is_null() {
        set_last_error("out_ptr is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        if capacity == 0 {
            set_last_error("Channel capacity must be >= 1");
            return Err(SharedError::Type);
        }
        let reg = registry();
        let budget = effective_channel_budget(reg.config().max_channel_bytes);
        if !channel_capacity_fits(capacity, budget) {
            set_last_error(format!(
                "Channel capacity {capacity} would allocate ~{} bytes for its \
                 slot array, exceeding SHARED_MAX_CHANNEL_BYTES ({budget}); \
                 lower the capacity or raise the budget",
                capacity
                    .checked_mul(SLOT_BYTES)
                    .map_or_else(|| "overflow".to_string(), |b| b.to_string())
            ));
            return Err(SharedError::CapacityExceeded);
        }
        // Hold a typed `Arc<ChannelInner>` alongside the trait-object
        // copy handed to the registry: lets us call `bind_id` directly
        // without round-tripping through `as_any_channel` (whose
        // success here is a static fact, but a downcast + `.expect`
        // makes a structural invariant look like a runtime contract).
        let typed = Arc::new(ChannelInner::new(capacity as usize));
        let arc = reg.insert(SharedType::Channel, typed.clone())?;
        typed.bind_entry(Arc::downgrade(&arc));
        // SAFETY: out_ptr checked non-null above.
        unsafe { *out_ptr = Arc::into_raw(arc) };
        Ok(())
    })
}

/// Non-blocking send. `*out_success` is `1` on successful deposit
/// (either into the buffer or handed off to a parked recv-waiter), `0`
/// when the channel is full-but-open. Returns `SharedError::Closed`
/// (-6) when the channel has already been closed.
///
/// # Safety
/// `buf` must be valid for reads of `len` bytes (may be null when
/// `len == 0`). `out_success` must be valid for a `c_int` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_try_send(
    entry_ptr: *const Entry,
    buf: *const u8,
    len: usize,
    out_success: *mut c_int,
) -> c_int {
    if out_success.is_null() {
        set_last_error("out_success null");
        return SharedError::Generic.code();
    }
    if len > 0 && buf.is_null() {
        set_last_error("buf null with non-zero len");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe { *out_success = 0 };
    ffi_entry(|| {
        // SAFETY: entry_ptr non-null and, per the handle contract, a
        // live Arc::into_raw pointer.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "channel_try_send on freed Entry");
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let bytes: Vec<u8> = if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(buf, len) }.to_vec()
        };
        // Scan + cycle-check + resolve keepalive (Err(Cycle)/etc. propagate).
        let payload: Payload = ch.build_payload(bytes)?;

        match ch.try_send(payload) {
            Ok(()) => {
                entry.registry.record_op(entry);
                unsafe { *out_success = 1 };
                Ok(())
            }
            Err(TrySendErr::Full(_)) => {
                // Not an error per FFI contract — the caller distinguishes
                // via *out_success = 0.
                entry.registry.record_op(entry);
                unsafe { *out_success = 0 };
                Ok(())
            }
            Err(TrySendErr::Closed(_)) => {
                set_last_error("try_send on closed channel");
                Err(SharedError::Closed)
            }
        }
    })
}

/// Non-blocking recv. Writes the outcome to `*out_state`:
///   * `0` — got an item; `*out_buf`/`*out_len` point at a malloc'd
///     buffer (null+0 when the payload was empty).
///   * `1` — empty but still open; caller may retry later.
///   * `2` — closed + drained; end of stream.
///
/// Never returns a negative status code under normal conditions; the
/// negative space is reserved for Rust-level panics via `ffi_entry`.
///
/// # Safety
/// `out_buf`, `out_len`, `out_state` must each be valid for writes.
/// When `*out_state == 0` and `*out_buf != null`, the caller takes
/// ownership of the allocation.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_try_recv(
    entry_ptr: *const Entry,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_state: *mut c_int,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() || out_state.is_null() {
        set_last_error("out pointers null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
        *out_state = 1;
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "channel_try_recv on freed Entry");
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        match ch.try_recv() {
            Ok(Some(payload)) => {
                let (ptr, n) = unsafe { payload_to_malloc(payload.bytes)? };
                entry.registry.record_op(entry);
                unsafe {
                    *out_buf = ptr;
                    *out_len = n;
                    *out_state = 0;
                }
                Ok(())
            }
            Err(TryRecvErr::WouldBlockEmpty) => {
                entry.registry.record_op(entry);
                unsafe { *out_state = 1 };
                Ok(())
            }
            Ok(None) => {
                entry.registry.record_op(entry);
                unsafe { *out_state = 2 };
                Ok(())
            }
        }
    })
}

/// Thread-blocking send with bounded wait. `timeout_ms` follows the wire
/// convention: `-1` = forever, `0` = try (non-blocking), `>0` = milliseconds.
/// See `timeout::parse_timeout`. Returns `0` on success,
/// `-6` (`SharedError::Closed`) on close, `-7` (`SharedError::Timeout`)
/// on deadline or when `timeout_ms == 0` and the channel is full.
///
/// # Safety
/// `buf` must be valid for reads of `len` bytes (null permitted when
/// `len == 0`).
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_send_blocking(
    entry_ptr: *const Entry,
    buf: *const u8,
    len: usize,
    timeout_ms: i64,
) -> c_int {
    if len > 0 && buf.is_null() {
        set_last_error("buf null with non-zero len");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(
            entry.magic, ENTRY_MAGIC,
            "channel_send_blocking on freed Entry"
        );
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let bytes: Vec<u8> = if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(buf, len) }.to_vec()
        };
        // Scan + cycle-check + resolve keepalive (Err(Cycle)/etc. propagate).
        let payload: Payload = ch.build_payload(bytes)?;

        let wait = parse_timeout(timeout_ms);
        let res = ch.send_blocking(payload, wait);
        match res {
            Ok(()) => {
                entry.registry.record_op(entry);
                Ok(())
            }
            Err(e @ SharedError::Closed) => {
                set_last_error("channel closed");
                Err(e)
            }
            Err(e @ SharedError::Timeout) => {
                set_last_error("send_blocking timed out");
                Err(e)
            }
            Err(other) => Err(other),
        }
    })
}

/// Thread-blocking recv. `timeout_ms` follows the wire convention:
/// `-1` = forever, `0` = try (non-blocking), `>0` = milliseconds.
/// See `timeout::parse_timeout`.
///
/// State semantics:
///   * `*out_state = 0` — got an item. `*out_buf`/`*out_len` set.
///   * `*out_state = 2` — closed + drained. No item.
///
/// Blocking timeout is NOT expressed via state — it returns
/// `SharedError::Timeout` (-7) as a hard error so the PHP wrapper can
/// emit a `RecvResult::Timeout` variant instead of returning a value.
///
/// # Safety
/// `out_buf`, `out_len`, `out_state` must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_recv_blocking(
    entry_ptr: *const Entry,
    timeout_ms: i64,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_state: *mut c_int,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() || out_state.is_null() {
        set_last_error("out pointers null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
        *out_state = 2;
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(
            entry.magic, ENTRY_MAGIC,
            "channel_recv_blocking on freed Entry"
        );
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let wait = parse_timeout(timeout_ms);
        match ch.recv_blocking(wait) {
            Ok(Some(payload)) => {
                let (ptr, n) = unsafe { payload_to_malloc(payload.bytes)? };
                entry.registry.record_op(entry);
                unsafe {
                    *out_buf = ptr;
                    *out_len = n;
                    *out_state = 0;
                }
                Ok(())
            }
            Ok(None) => {
                entry.registry.record_op(entry);
                unsafe { *out_state = 2 };
                Ok(())
            }
            Err(e @ SharedError::Timeout) => {
                set_last_error("recv_blocking timed out");
                Err(e)
            }
            Err(other) => Err(other),
        }
    })
}

/// Idempotent close. Always returns 0 — close is not an error even on
/// an already-closed channel. Wakes parked fibers and blocked threads
/// promptly.
///
/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_close(entry_ptr: *const Entry) -> c_int {
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "channel_close on freed Entry");
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;
        ch.close();
        entry.registry.record_op(entry);
        Ok(())
    })
}

/// `*out = is_closed() as c_int` on success.
///
/// # Safety
/// `out` must be valid for a `c_int` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_is_closed(
    entry_ptr: *const Entry,
    out: *mut c_int,
) -> c_int {
    if out.is_null() {
        set_last_error("out null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "channel_is_closed on freed Entry");
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;
        let v = ch.is_closed();
        entry.registry.record_op(entry);
        unsafe { *out = v as c_int };
        Ok(())
    })
}

/// `*out = pending() as u64` on success.
///
/// # Safety
/// `out` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_pending(
    entry_ptr: *const Entry,
    out: *mut u64,
) -> c_int {
    if out.is_null() {
        set_last_error("out null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "channel_pending on freed Entry");
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;
        let v = ch.pending() as u64;
        entry.registry.record_op(entry);
        unsafe { *out = v };
        Ok(())
    })
}

/// Batched send. `payloads_concat` is the concatenation of `n`
/// already-portbuf-serialized zvals. `offsets` is a `[usize; n+1]` array
/// where `offsets[0] == 0`, `offsets[n]` equals the total buffer length,
/// and the i-th payload occupies bytes `[offsets[i] .. offsets[i+1])`.
///
/// Behaviour mirrors the `send_many` Rust API: best-effort send up to
/// `timeout_ms` (`-1` = forever, `0` = try, `>0` = ms); stops on first
/// error and returns the count actually sent via `*out_sent`.
/// Closed/timeout are NOT signalled as errors — the caller inspects
/// `*out_sent` to distinguish partial vs full completion.
/// See `timeout::parse_timeout`.
///
/// # Safety
/// `payloads_concat` must be valid for reads of `offsets[n]` bytes (may
/// be null when total length is zero). `offsets` must be valid for reads
/// of `n + 1` `usize` values. `out_sent` must be writable.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_send_many(
    entry_ptr: *const Entry,
    payloads_concat: *const u8,
    offsets: *const usize,
    n: usize,
    timeout_ms: i64,
    out_sent: *mut u64,
) -> c_int {
    if out_sent.is_null() {
        set_last_error("out_sent null");
        return SharedError::Generic.code();
    }
    unsafe { *out_sent = 0 };
    if n > 0 && offsets.is_null() {
        set_last_error("offsets null with n > 0");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "channel_send_many on freed Entry");
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        if n == 0 {
            entry.registry.record_op(entry);
            return Ok(());
        }

        // Split the concatenated buffer into N Vec<u8> payloads.
        let offsets_slice = unsafe { std::slice::from_raw_parts(offsets, n + 1) };
        let total = offsets_slice[n];
        if total > 0 && payloads_concat.is_null() {
            set_last_error("payloads_concat null with non-zero total length");
            return Err(SharedError::Generic);
        }
        let mut payloads: Vec<Payload> = Vec::with_capacity(n);
        for i in 0..n {
            let start = offsets_slice[i];
            let end = offsets_slice[i + 1];
            if end < start || end > total {
                set_last_error("malformed offsets array");
                return Err(SharedError::Generic);
            }
            let len = end - start;
            let bytes = if len == 0 {
                Vec::new()
            } else {
                let slice = unsafe { std::slice::from_raw_parts(payloads_concat.add(start), len) };
                slice.to_vec()
            };
            // Build (scan + cycle-check + resolve) every element BEFORE sending
            // any, so a cyclic element rejects the whole batch atomically —
            // `?` returns Cycle here with `*out_sent` still 0, nothing sent.
            payloads.push(ch.build_payload(bytes)?);
        }

        let wait = parse_timeout(timeout_ms);
        let sent = ch.send_many(payloads, wait);
        entry.registry.record_op(entry);
        unsafe { *out_sent = sent };
        Ok(())
    })
}

/// Batched recv. Collects up to `max` payloads per the `recv_many`
/// Rust API. `timeout_ms` follows the wire convention: `-1` = forever,
/// `0` = try (non-blocking), `>0` = milliseconds. See `timeout::parse_timeout`.
/// On success writes:
///   * `*out_concat` — libc::malloc'd buffer with all payloads
///     concatenated (null when n == 0 or total length == 0).
///   * `*out_concat_len` — total length in bytes.
///   * `*out_offsets` — libc::malloc'd `[usize; n+1]` array with the
///     payload boundaries (null when n == 0).
///   * `*out_n` — number of payloads returned.
///
/// Caller owns both allocations and must free them via `libc::free`
/// (or `oxphp_portable_free`, which forwards to `libc::free`).
///
/// # Safety
/// `out_concat`, `out_concat_len`, `out_offsets`, `out_n` must all be
/// valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_recv_many(
    entry_ptr: *const Entry,
    max: u64,
    timeout_ms: i64,
    out_concat: *mut *mut u8,
    out_concat_len: *mut usize,
    out_offsets: *mut *mut usize,
    out_n: *mut u64,
) -> c_int {
    if out_concat.is_null() || out_concat_len.is_null() || out_offsets.is_null() || out_n.is_null()
    {
        set_last_error("out ptrs null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe {
        *out_concat = std::ptr::null_mut();
        *out_concat_len = 0;
        *out_offsets = std::ptr::null_mut();
        *out_n = 0;
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "channel_recv_many on freed Entry");
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let wait = parse_timeout(timeout_ms);
        let items = ch.recv_many(max as usize, wait);
        entry.registry.record_op(entry);

        let n = items.len();
        if n == 0 {
            return Ok(());
        }

        let total: usize = items.iter().map(|p| p.bytes.len()).sum();
        let concat_ptr = if total == 0 {
            std::ptr::null_mut()
        } else {
            let p = unsafe { libc::malloc(total) as *mut u8 };
            if p.is_null() {
                set_last_error("libc::malloc failed for recv_many concat");
                return Err(SharedError::Generic);
            }
            p
        };
        let offsets_bytes = (n + 1) * std::mem::size_of::<usize>();
        let offsets_ptr = unsafe { libc::malloc(offsets_bytes) as *mut usize };
        if offsets_ptr.is_null() {
            if !concat_ptr.is_null() {
                unsafe { libc::free(concat_ptr as *mut _) };
            }
            set_last_error("libc::malloc failed for recv_many offsets");
            return Err(SharedError::Generic);
        }

        let mut cursor = 0usize;
        unsafe { *offsets_ptr = 0 };
        for (i, item) in items.iter().enumerate() {
            if !item.bytes.is_empty() && !concat_ptr.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        item.bytes.as_ptr(),
                        concat_ptr.add(cursor),
                        item.bytes.len(),
                    );
                }
            }
            cursor += item.bytes.len();
            unsafe { *offsets_ptr.add(i + 1) = cursor };
        }

        unsafe {
            *out_concat = concat_ptr;
            *out_concat_len = total;
            *out_offsets = offsets_ptr;
            *out_n = n as u64;
        }
        Ok(())
    })
}

/// Schedule a timeout cancel for a synthetic promise.
///
/// Prefers the process-global Tokio runtime handle (installed in
/// `async_main` before any PHP worker starts, exposed via
/// `crate::php::sapi::async_tokio_handle`) and schedules the sleep as an
/// async task — far cheaper than a dedicated OS thread per timeout. Falls
/// back to a detached `std::thread` only when the handle has not been
/// initialized yet (early startup or unit-test context without a running
/// runtime). `synthetic::cancel` is idempotent — a late resolve makes the
/// cancel a no-op — so both paths are safe under races.
#[cfg(feature = "php")]
fn spawn_fiber_timeout(promise_id: i64, timeout_ms: u64) {
    if timeout_ms == 0 {
        return;
    }
    let dur = Duration::from_millis(timeout_ms);
    if let Some(handle) = crate::php::sapi::async_tokio_handle() {
        handle.spawn(async move {
            tokio::time::sleep(dur).await;
            synthetic::cancel(promise_id);
        });
        return;
    }
    // Fallback: no runtime handle available (e.g. unit-test context).
    std::thread::spawn(move || {
        std::thread::sleep(dur);
        synthetic::cancel(promise_id);
    });
}

/// Park the current fiber on the channel's recv-waiter list. Returns
/// a synthetic-promise id via `*out_promise_id` that the PHP layer
/// awaits via `oxphp_bridge_fiber_await`. When the fiber resumes:
///   * Value(bytes) → got an item.
///   * Value(empty bytes) → for *recv*, never emitted.
///   * Cancelled → timeout (or shutdown drain via `close()`).
///   * ClosedException → see register_recv_waiter's fast-fail path.
///
/// Allocation ordering: the synthetic promise MUST be allocated (and
/// registered with the PHP thread-local PROMISE_MAP) BEFORE calling
/// `register_recv_waiter` — otherwise a concurrent `try_send` could
/// try to resolve an id that isn't yet in the senders map.
///
/// # Safety
/// `out_promise_id` must be valid for an `i64` write. Must be called
/// from a PHP worker thread (`alloc_and_register` relies on the
/// thread-local PROMISE_MAP).
#[cfg(feature = "php")]
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_recv_fiber_register(
    entry_ptr: *const Entry,
    timeout_ms: u64,
    out_promise_id: *mut i64,
) -> c_int {
    if out_promise_id.is_null() {
        set_last_error("out_promise_id null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe { *out_promise_id = 0 };
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(
            entry.magic, ENTRY_MAGIC,
            "channel_recv_fiber_register on freed Entry"
        );
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        // 1. Allocate synthetic promise on the PHP thread. Registers the
        //    receiver with PROMISE_MAP so fiber_await(id) will drain it.
        let promise_id = synthetic::alloc_and_register();

        // 2. Park it as a recv-waiter. `register_recv_waiter` covers
        //    three races internally (closed+empty, closed+drained,
        //    already-buffered items).
        ch.register_recv_waiter(promise_id);

        // 3. Timeout path — idempotent cancel; safe even if the
        //    waiter was already resolved by a concurrent sender or
        //    `close()`.
        spawn_fiber_timeout(promise_id, timeout_ms);

        entry.registry.record_op(entry);
        unsafe { *out_promise_id = promise_id };
        Ok(())
    })
}

/// Park the current fiber on the channel's send-waiter list. When the
/// fiber resumes:
///   * Value(empty bytes) → a slot freed; retry your `try_send`.
///   * Cancelled → timeout (or the PHP wrapper's explicit cancel).
///   * ClosedException → channel was closed underneath the pending send.
///
/// See `oxphp_shared_channel_recv_fiber_register` for allocation-
/// ordering rationale.
///
/// # Safety
/// See `oxphp_shared_channel_recv_fiber_register`.
#[cfg(feature = "php")]
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_send_fiber_register(
    entry_ptr: *const Entry,
    timeout_ms: u64,
    out_promise_id: *mut i64,
) -> c_int {
    if out_promise_id.is_null() {
        set_last_error("out_promise_id null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe { *out_promise_id = 0 };
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_channel_try_send.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(
            entry.magic, ENTRY_MAGIC,
            "channel_send_fiber_register on freed Entry"
        );
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let promise_id = synthetic::alloc_and_register();
        ch.register_send_waiter(promise_id);
        spawn_fiber_timeout(promise_id, timeout_ms);

        entry.registry.record_op(entry);
        unsafe { *out_promise_id = promise_id };
        Ok(())
    })
}

// ─── RAII gauges for senders_blocked / receivers_blocked ──────────────
//
// Increment on construction, decrement on drop — `Relaxed` is enough
// since the gauge is only read for observability, never used to gate
// correctness.

struct SendersBlockedGuard<'a> {
    g: &'a AtomicU32,
}

impl<'a> SendersBlockedGuard<'a> {
    fn new(g: &'a AtomicU32) -> Self {
        g.fetch_add(1, Ordering::Relaxed);
        Self { g }
    }
}

impl Drop for SendersBlockedGuard<'_> {
    fn drop(&mut self) {
        self.g.fetch_sub(1, Ordering::Relaxed);
    }
}

struct ReceiversBlockedGuard<'a> {
    g: &'a AtomicU32,
}

impl<'a> ReceiversBlockedGuard<'a> {
    fn new(g: &'a AtomicU32) -> Self {
        g.fetch_add(1, Ordering::Relaxed);
        Self { g }
    }
}

impl Drop for ReceiversBlockedGuard<'_> {
    fn drop(&mut self) {
        self.g.fetch_sub(1, Ordering::Relaxed);
    }
}

// ─── Fiber-register shims (cfg-gated wrappers for the class handlers) ─
//
// The `*_fiber_register` FFI fns are `#[cfg(feature = "php")]` because
// `synthetic::alloc_and_register` needs the PHP worker thread's
// `PROMISE_MAP`. The non-php build needs SOMETHING to call from
// `register_class` so the source compiles in unit-test mode; provide
// a stub that returns `SharedError::Generic.code()`. At runtime the
// non-php branch is never taken — `oxphp_bridge_in_fiber` returns 0
// in the mock — so the stub body is dead code.

#[cfg(feature = "php")]
unsafe fn send_fiber_register_shim(
    entry_ptr: *const Entry,
    timeout_ms: u64,
    out: *mut i64,
) -> c_int {
    unsafe { oxphp_shared_channel_send_fiber_register(entry_ptr, timeout_ms, out) }
}

#[cfg(not(feature = "php"))]
unsafe fn send_fiber_register_shim(
    _entry_ptr: *const Entry,
    _timeout_ms: u64,
    _out: *mut i64,
) -> c_int {
    SharedError::Generic.code()
}

#[cfg(feature = "php")]
unsafe fn recv_fiber_register_shim(
    entry_ptr: *const Entry,
    timeout_ms: u64,
    out: *mut i64,
) -> c_int {
    unsafe { oxphp_shared_channel_recv_fiber_register(entry_ptr, timeout_ms, out) }
}

#[cfg(not(feature = "php"))]
unsafe fn recv_fiber_register_shim(
    _entry_ptr: *const Entry,
    _timeout_ms: u64,
    _out: *mut i64,
) -> c_int {
    SharedError::Generic.code()
}

// ─── Class registration ───────────────────────────────────────────────

/// Register the `OxPHP\Shared\Channel` PHP class with all its methods.
///
/// Exposed PHP surface:
///   __construct(int $capacity)
///   send(mixed $value): SendResult                    [forever or fiber-cancel]
///   sendTimeout(mixed $value, int $ms): SendResult    [bounded]
///   trySend(mixed $value): SendResult                 [non-blocking]
///   recv(): RecvResult                                [forever or fiber-cancel]
///   recvTimeout(int $ms): RecvResult                  [bounded]
///   tryRecv(): RecvResult                             [non-blocking]
///   close(): void
///   isClosed(): bool
///   pending(): int
///   id(): int
///   __clone → throws
pub fn register_class(
    ctx: &mut crate::plugin::PluginContext,
) -> Result<(), crate::plugin::PluginError> {
    use crate::bridge::ffi as bridge_ffi;
    use crate::plugin::types::{MagicMethod, PhpType};
    use crate::plugin::PhpError;
    use crate::plugins::ox_shared::handle::SharedHandle;
    use crate::plugins::ox_shared::results::{self, RecvKind, SendKind};
    use crate::plugins::ox_shared::types::timeout::read_positive_ms_arg;

    ctx.register_class("OxPHP\\Shared\\Channel")
        .implements("OxPHP\\Shared\\Shareable")
        .implements("Countable")
        .with_storage(|| SharedHandle::new(SharedType::Channel))
        .magic(MagicMethod::Clone)
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\SharedException".into(),
                message: "Shared instances cannot be cloned. Use cross-thread \
                          transfer via oxphp_async(fn() use (\\$this) {...})."
                    .into(),
                code: 0,
            })
        })
        // ── __construct(capacity) ───────────────────────────────────
        .method("__construct")
        .param("capacity", PhpType::Int)
        .handler(|call| {
            let cap = call.arg_long(0).unwrap_or(0);
            if cap <= 0 {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\TypeException".into(),
                    message: "Channel capacity must be positive".into(),
                    code: 0,
                });
            }
            let mut out_ptr: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_channel_create(cap as u64, &mut out_ptr) };
            super::counter::counter_rc_to_result(rc)?;

            let h = call.storage_mut::<SharedHandle>()?;
            h.entry_ptr = out_ptr;
            h.type_tag = SharedType::Channel as u8;
            Ok(())
        })
        // ── trySend(value): SendResult ──────────────────────────────
        //   Non-blocking. Returns SendResult::Ok / Full / Closed.
        .method("trySend")
        .param("value", PhpType::Mixed)
        .returns(PhpType::Object)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let buf_owner = serialize_arg(call, 0, "trySend")?;
            let (buf, len) = buf_owner.parts();
            check_value_size(len, registry().config().max_value_size, "trySend")?;
            let mut success: c_int = 0;
            let rc = unsafe { oxphp_shared_channel_try_send(entry_ptr, buf, len, &mut success) };
            drop(buf_owner);

            if rc == SharedError::Closed.code() {
                return results::write_send(call, SendKind::Closed);
            }
            super::counter::counter_rc_to_result(rc)?;
            let kind = if success != 0 {
                SendKind::Ok
            } else {
                SendKind::Full
            };
            results::write_send(call, kind)
        })
        // ── send(value): SendResult ─────────────────────────────────
        //   Forever (or fiber-cancel). Returns SendResult::Ok or Closed.
        //   A fiber cancellation (Async\AsyncException) is propagated as
        //   an exception — it is NOT mapped to SendResult::Closed.
        .method("send")
        .param("value", PhpType::Mixed)
        .returns(PhpType::Object)
        .handler(|call| invoke_channel_send(call, -1, "send"))
        // ── sendTimeout(value, int $ms): SendResult ─────────────────
        //   Bounded. $ms > 0 enforced. Returns Ok / Full / Timeout / Closed.
        .method("sendTimeout")
        .param("value", PhpType::Mixed)
        .param("ms", PhpType::Int)
        .returns(PhpType::Object)
        .handler(|call| {
            let ms = read_positive_ms_arg(call, 1)?;
            invoke_channel_send(call, ms, "sendTimeout")
        })
        // ── tryRecv(): RecvResult ───────────────────────────────────
        //   Non-blocking. Returns RecvResult::Ok(value) / Empty / Closed.
        .method("tryRecv")
        .returns(PhpType::Object)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let mut out_buf: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let mut state: c_int = 0;
            let rc = unsafe {
                oxphp_shared_channel_try_recv(entry_ptr, &mut out_buf, &mut out_len, &mut state)
            };
            super::counter::counter_rc_to_result(rc)?;
            match state {
                0 => {
                    let r = results::write_recv_ok(call, out_buf, out_len);
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    r
                }
                1 => {
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    results::write_recv(call, RecvKind::Empty)
                }
                2 => {
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    results::write_recv(call, RecvKind::Closed)
                }
                other => {
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    Err(PhpError::Custom(format!(
                        "tryRecv: unexpected state {other}"
                    )))
                }
            }
        })
        // ── recv(): RecvResult ──────────────────────────────────────
        //   Forever (or fiber-cancel). Returns RecvResult::Ok(value) or
        //   Closed. A fiber cancellation (Async\AsyncException) is
        //   propagated as an exception — it is NOT mapped to
        //   RecvResult::Closed.
        .method("recv")
        .returns(PhpType::Object)
        .handler(|call| invoke_channel_recv(call, -1))
        // ── recvTimeout(int $ms): RecvResult ────────────────────────
        //   Bounded. $ms > 0 enforced. Returns Ok(value) / Empty /
        //   Timeout / Closed.
        .method("recvTimeout")
        .param("ms", PhpType::Int)
        .returns(PhpType::Object)
        .handler(|call| {
            let ms = read_positive_ms_arg(call, 0)?;
            invoke_channel_recv(call, ms)
        })
        // ── close(): void ──────────────────────────────────────────
        .method("close")
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let rc = unsafe { oxphp_shared_channel_close(entry_ptr) };
            super::counter::counter_rc_to_result(rc)?;
            Ok(())
        })
        // ── isClosed(): bool ───────────────────────────────────────
        .method("isClosed")
        .returns(PhpType::Bool)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let mut out: c_int = 0;
            let rc = unsafe { oxphp_shared_channel_is_closed(entry_ptr, &mut out) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(out != 0);
            Ok(())
        })
        // ── count(): int ───────────────────────────────────────────
        .method("count")
        .returns(PhpType::Int)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let mut out: u64 = 0;
            let rc = unsafe { oxphp_shared_channel_pending(entry_ptr, &mut out) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_long(out as i64);
            Ok(())
        })
        // ── sendMany(values, int $ms): int ─────────────────────────
        //
        // Serializes each array element to its own portbuf via the C
        // helper `oxphp_iter_array_to_portbufs`, then deposits them
        // one-by-one through the `oxphp_shared_channel_send_many` FFI.
        // `$ms` must be `> 0` (bounded wait per batch). Returns the
        // count actually sent — closed/timeout end the batch early
        // and surface as a partial count, never as an exception.
        .method("sendMany")
        .param("values", PhpType::Array)
        .param("ms", PhpType::Int)
        .returns(PhpType::Int)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let timeout_ms: i64 = read_positive_ms_arg(call, 1)?;

            // Fast path: empty array → 0 sent, no FFI round-trip.
            let count = call.arg_array_count(0).unwrap_or(0);
            if count == 0 {
                call.ret_long(0);
                return Ok(());
            }

            // Split the PHP array into N portbuf payloads via the C helper.
            let arr_ptr = unsafe { call.raw_arg_ptr(0) };
            let mut concat: *mut u8 = std::ptr::null_mut();
            let mut concat_len: usize = 0;
            let mut offsets: *mut usize = std::ptr::null_mut();
            let mut n: usize = 0;
            let iter_rc = unsafe {
                bridge_ffi::oxphp_iter_array_to_portbufs(
                    arr_ptr as *const _,
                    &mut concat,
                    &mut concat_len,
                    &mut offsets,
                    &mut n,
                )
            };
            if iter_rc != 0 {
                // Defensive cleanup — C helper zeroes outs on failure,
                // but free is null-safe so an extra call is harmless.
                unsafe {
                    if !concat.is_null() {
                        bridge_ffi::oxphp_portable_free(concat);
                    }
                    if !offsets.is_null() {
                        bridge_ffi::oxphp_portable_free(offsets as *mut u8);
                    }
                }
                let msg = if iter_rc == -3 {
                    "sendMany: first argument must be an array"
                } else {
                    "sendMany: failed to serialize one or more values (e.g. closure, resource)"
                };
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\TypeException".into(),
                    message: msg.into(),
                    code: 0,
                });
            }

            // Pre-flight per-value cap: reject the whole batch before any
            // deposit if any element exceeds the cap.
            let cap = registry().config().max_value_size;
            let sizes: &[usize] = unsafe { std::slice::from_raw_parts(offsets, n) };
            if let Some((idx, size)) = first_oversized_element(sizes, concat_len, cap) {
                unsafe {
                    if !concat.is_null() {
                        bridge_ffi::oxphp_portable_free(concat);
                    }
                    if !offsets.is_null() {
                        bridge_ffi::oxphp_portable_free(offsets as *mut u8);
                    }
                }
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\ValueTooLargeException".into(),
                    message: format!(
                        "sendMany: element {idx} of {size} bytes exceeds the \
                         per-value cap of {cap} bytes (SHARED_MAX_VALUE_SIZE)"
                    ),
                    code: 0,
                });
            }

            let mut sent: u64 = 0;
            let rc = unsafe {
                oxphp_shared_channel_send_many(entry_ptr, concat, offsets, n, timeout_ms, &mut sent)
            };
            unsafe {
                if !concat.is_null() {
                    bridge_ffi::oxphp_portable_free(concat);
                }
                if !offsets.is_null() {
                    bridge_ffi::oxphp_portable_free(offsets as *mut u8);
                }
            }
            if rc == SharedError::Timeout.code() || rc == SharedError::Closed.code() {
                call.ret_long(sent as i64);
                return Ok(());
            }
            super::counter::counter_rc_to_result(rc)?;
            call.ret_long(sent as i64);
            Ok(())
        })
        // ── recvMany(int $max, int $ms): array ─────────────────────
        //
        // `$max == 0` → drain all currently-buffered items at once.
        // `$ms > 0`   → block up to that many milliseconds collecting at most `$max`.
        //
        // Returns a (possibly partial / possibly empty) array on
        // timeout or channel close — never throws for those cases.
        .method("recvMany")
        .param("max", PhpType::Int)
        .param("ms", PhpType::Int)
        .returns(PhpType::Array)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let max_raw = call.arg_long(0).unwrap_or(0);
            let max: u64 = if max_raw < 0 { 0 } else { max_raw as u64 };
            let timeout_ms: i64 = read_positive_ms_arg(call, 1)?;

            let mut concat: *mut u8 = std::ptr::null_mut();
            let mut concat_len: usize = 0;
            let mut offsets: *mut usize = std::ptr::null_mut();
            let mut n: u64 = 0;
            let rc = unsafe {
                oxphp_shared_channel_recv_many(
                    entry_ptr,
                    max,
                    timeout_ms,
                    &mut concat,
                    &mut concat_len,
                    &mut offsets,
                    &mut n,
                )
            };
            if let Err(e) = super::counter::counter_rc_to_result(rc) {
                unsafe {
                    if !concat.is_null() {
                        bridge_ffi::oxphp_portable_free(concat);
                    }
                    if !offsets.is_null() {
                        bridge_ffi::oxphp_portable_free(offsets as *mut u8);
                    }
                }
                return Err(e);
            }

            let n_usize = n as usize;
            let retval = call.retval_ptr();
            unsafe {
                bridge_ffi::oxphp_ret_array_init(retval, n_usize as u32);
            }

            if n_usize > 0 {
                // offsets must be non-null when n > 0 per FFI contract.
                let offsets_slice = unsafe { std::slice::from_raw_parts(offsets, n_usize + 1) };
                for i in 0..n_usize {
                    let start = offsets_slice[i];
                    let end = offsets_slice[i + 1];
                    let len = end - start;
                    let buf_ptr: *const u8 = if len == 0 || concat.is_null() {
                        std::ptr::null()
                    } else {
                        unsafe { concat.add(start) as *const u8 }
                    };
                    let push_rc =
                        unsafe { bridge_ffi::oxphp_arr_push_portbuf(retval, buf_ptr, len) };
                    if push_rc != 0 {
                        unsafe {
                            if !concat.is_null() {
                                bridge_ffi::oxphp_portable_free(concat);
                            }
                            if !offsets.is_null() {
                                bridge_ffi::oxphp_portable_free(offsets as *mut u8);
                            }
                        }
                        return Err(PhpError::Custom(format!(
                            "recvMany: deserialize of payload {i} failed"
                        )));
                    }
                }
            }

            unsafe {
                if !concat.is_null() {
                    bridge_ffi::oxphp_portable_free(concat);
                }
                if !offsets.is_null() {
                    bridge_ffi::oxphp_portable_free(offsets as *mut u8);
                }
            }
            Ok(())
        })
        // ── id(): int ──────────────────────────────────────────────
        .method("id")
        .returns(PhpType::Int)
        .handler(|call| {
            let h = call.storage::<SharedHandle>()?;
            if !h.is_initialized() {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\UninitializedException".into(),
                    message: "uninitialised Shared wrapper".into(),
                    code: 0,
                });
            }
            let id =
                unsafe { crate::plugins::ox_shared::registry::oxphp_shared_entry_id(h.entry_ptr) };
            call.ret_long(id as i64);
            Ok(())
        })
        .build()?;

    Ok(())
}

/// Map a Channel FFI rc → a PhpError exception. Mirrors
/// `counter_rc_to_result` but adds `Closed` / `Timeout` mapping.
#[allow(dead_code)]
fn map_channel_rc(rc: c_int) -> crate::plugin::PhpError {
    use crate::plugin::PhpError;
    let class = match rc {
        -2 => "OxPHP\\Shared\\StaleHandleException",
        -3 => "OxPHP\\Shared\\TypeException",
        -4 => "OxPHP\\Shared\\CapacityException",
        -6 => "OxPHP\\Shared\\ClosedException",
        -7 => "OxPHP\\Shared\\OperationTimeoutException",
        -9 => "OxPHP\\Shared\\CycleException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        _ => "OxPHP\\Shared\\SharedException",
    };
    PhpError::Exception {
        class: class.to_string(),
        message: String::new(),
        code: 0,
    }
}

/// RAII wrapper for a portbuf serialized from a single PHP zval. Owns
/// the malloc'd buffer; `Drop` calls `oxphp_portable_free` so every
/// early return path in send/recv stays leak-free without manual
/// bookkeeping.
struct PortbufOwner(*mut u8, usize);

impl PortbufOwner {
    fn parts(&self) -> (*mut u8, usize) {
        (self.0, self.1)
    }
}

impl Drop for PortbufOwner {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { crate::bridge::ffi::oxphp_portable_free(self.0) };
        }
    }
}

/// Serialize one PHP argument (at index `idx`) into a portbuf. On
/// serialization failure (closure / resource / circular) returns a
/// `TypeException` whose message embeds the method name for clarity.
fn serialize_arg(
    call: &crate::bridge::call::NativeCall,
    idx: u32,
    method_name: &str,
) -> Result<PortbufOwner, crate::plugin::PhpError> {
    let arg_ptr = unsafe { call.raw_arg_ptr(idx) };
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let ser_rc = unsafe {
        crate::bridge::ffi::oxphp_portable_serialize(arg_ptr as *const _, 1, &mut buf, &mut len)
    };
    if ser_rc != 0 {
        if !buf.is_null() {
            unsafe { crate::bridge::ffi::oxphp_portable_free(buf) };
        }
        return Err(crate::plugin::PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".into(),
            message: format!("{method_name}: value is not serializable (e.g. closure, resource)"),
            code: 0,
        });
    }
    Ok(PortbufOwner(buf, len))
}

/// Body shared by `send()` and `sendTimeout()`. `timeout_ms` is the
/// wire-format timeout: `-1` = forever (fiber-cancel only), `> 0` =
/// bounded. The non-blocking case is a separate `trySend` handler and
/// is not reachable here.
///
/// Returns a `SendResult` written into the retval slot. Errors only
/// surface when an FFI call fails for non-Closed/Timeout reasons or
/// when the fiber waker raised an unexpected exception (e.g. a
/// non-`Async\AsyncException`) — those propagate as `PhpError` so the
/// plugin framework surfaces the pending EG(exception).
#[allow(clippy::too_many_lines)]
fn invoke_channel_send(
    call: &mut crate::bridge::call::NativeCall,
    timeout_ms: i64,
    method: &str,
) -> Result<(), crate::plugin::PhpError> {
    use crate::bridge::ffi as bridge_ffi;
    use crate::plugin::PhpError;
    use crate::plugins::ox_shared::handle::SharedHandle;
    use crate::plugins::ox_shared::results::{self, SendKind};

    let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
    let buf_owner = serialize_arg(call, 0, method)?;
    let (buf, len) = buf_owner.parts();
    check_value_size(len, registry().config().max_value_size, method)?;

    // The fiber-suspend path uses synthetic-promise FFI that is gated
    // behind `feature = "php"` (PROMISE_MAP lives on PHP worker
    // threads). Without `php`, force `in_fiber = false` so we always
    // take the thread-blocking branch — matches the mock
    // `oxphp_bridge_in_fiber` returning 0 anyway.
    #[cfg(feature = "php")]
    let in_fiber = unsafe { bridge_ffi::oxphp_bridge_in_fiber() } != 0;
    #[cfg(not(feature = "php"))]
    let in_fiber = false;

    if in_fiber {
        // Fiber path: try_send → on full, register send-waiter and
        // suspend via fiber_await. Waker resolves with:
        //   - Value(empty) → "slot free; retry try_send"   (fiber_rc == 0)
        //   - Cancelled    → Async\AsyncException          (fiber_rc == -1)
        //   - Closed       → ClosedException propagated    (fiber_rc == -1)
        let deadline = match parse_timeout(timeout_ms) {
            Wait::Forever => None,
            Wait::Bounded(d) => Some(std::time::Instant::now() + d),
            // Wait::Try is unreachable: trySend is a separate handler.
            Wait::Try => unreachable!("Wait::Try unreachable from send/sendTimeout"),
        };

        loop {
            // Fast attempt.
            let mut success: c_int = 0;
            let rc = unsafe { oxphp_shared_channel_try_send(entry_ptr, buf, len, &mut success) };
            if rc == SharedError::Closed.code() {
                return results::write_send(call, SendKind::Closed);
            }
            if rc == 0 && success != 0 {
                return results::write_send(call, SendKind::Ok);
            }
            if rc != 0 {
                // Some other FFI error (e.g. stale handle).
                return Err(map_channel_rc(rc));
            }

            // Park until a slot frees.
            let remaining_ms: u64 = if let Some(d) = deadline {
                let r = d.saturating_duration_since(std::time::Instant::now());
                if r.is_zero() {
                    return results::write_send(call, SendKind::Timeout);
                }
                // Clamp to ≥1ms. `as_millis()` floors, and after the first
                // retry the residual is sub-millisecond (the timer was armed
                // with this same floored value and fires just before the
                // deadline), so an unclamped value rounds to 0 — which
                // `spawn_fiber_timeout` treats as "no timeout, park forever".
                // The loop would then never re-check the deadline and the
                // bounded send would hang on a stuck channel. A 1ms floor
                // keeps the timer armed so each retry re-evaluates
                // `r.is_zero()` and converges to Timeout.
                (r.as_millis() as u64).max(1)
            } else {
                0
            };

            let mut promise_id: i64 = 0;
            // `oxphp_shared_channel_send_fiber_register` is gated by
            // `feature = "php"`. Without `php`, `in_fiber` is forced to
            // false above so this branch is unreachable at runtime, but
            // rustc still compiles it — call through a small shim that
            // has a non-php fallback returning -1.
            let reg_rc =
                unsafe { send_fiber_register_shim(entry_ptr, remaining_ms, &mut promise_id) };
            if reg_rc != 0 {
                super::counter::counter_rc_to_result(reg_rc)?;
            }

            // Suspend fiber. Timeout is handled by spawn_fiber_timeout
            // in the FFI register path (via synthetic::cancel), so we
            // pass 0.0 here — the SAPI fiber_await ignores this arg
            // anyway (see oxphp_fiber_suspend_for_await in ext/oxphp_sapi.c).
            let retval = call.retval_ptr();
            let fiber_rc = unsafe { bridge_ffi::oxphp_bridge_fiber_await(promise_id, 0.0, retval) };

            match fiber_rc {
                // Waker resolved with Value(empty) → slot free, retry.
                0 => continue,
                // Exception pending — inspect.
                -1 => {
                    // As on the recv path, the waker dispatcher
                    // (`await_dispatch_callback`) reports through the bridge
                    // async-exception TLS, NOT `EG(exception)`. Reading
                    // `EG(exception)` here always saw null, so every waker
                    // outcome (timeout, cancel, close) fell through to a
                    // generic fatal. Read+clear the correct channel.
                    let cls_ptr = unsafe { bridge_ffi::oxphp_bridge_get_async_exc_class() };
                    let msg_ptr = unsafe { bridge_ffi::oxphp_bridge_get_async_exc_message() };
                    let class = if cls_ptr.is_null() {
                        None
                    } else {
                        Some(
                            unsafe { std::ffi::CStr::from_ptr(cls_ptr) }
                                .to_string_lossy()
                                .into_owned(),
                        )
                    };
                    let message = if msg_ptr.is_null() {
                        String::new()
                    } else {
                        unsafe { std::ffi::CStr::from_ptr(msg_ptr) }
                            .to_string_lossy()
                            .into_owned()
                    };
                    unsafe { bridge_ffi::oxphp_bridge_clear_async_exception() };

                    match class.as_deref() {
                        Some("OxPHP\\Shared\\ClosedException") => {
                            // Channel closed while the send was parked.
                            // Mirror the fast-path (above) and the recv
                            // close handling: a SendResult::Closed, not a
                            // thrown exception — close is timing-independent.
                            return results::write_send(call, SendKind::Closed);
                        }
                        Some("OxPHP\\Async\\AsyncException") => {
                            // Bounded send whose deadline has elapsed → Timeout.
                            if timeout_ms > 0
                                && deadline
                                    .map(|d| std::time::Instant::now() >= d)
                                    .unwrap_or(false)
                            {
                                return results::write_send(call, SendKind::Timeout);
                            }
                            // Forever send (timeout_ms < 0): the cancel is
                            // always external — propagate the AsyncException.
                            // (No busy-loop risk: we do not retry here.)
                            if timeout_ms < 0 {
                                return Err(PhpError::Exception {
                                    class: "OxPHP\\Async\\AsyncException".into(),
                                    message: if message.is_empty() {
                                        "send: fiber cancelled".into()
                                    } else {
                                        message
                                    },
                                    code: 0,
                                });
                            }
                            // Bounded send woken just BEFORE the Rust deadline:
                            // the fiber timeout timer is armed with `remaining_ms`
                            // (floored, then clamped to ≥1ms; see its
                            // computation), re-computed each retry, so it can
                            // fire up to ~1ms early — unlike the recv path, whose
                            // deadline is captured before the timer is armed.
                            // Retry: the loop recomputes `remaining` at the top
                            // and, once it reaches zero, returns Timeout. (A
                            // pre-deadline external cancel of a bounded send is
                            // thereby surfaced as Timeout, matching the recv
                            // path's approximate `deadline_hit`.)
                            continue;
                        }
                        Some(other) => {
                            // A real application exception delivered through
                            // the waker — surface its true class.
                            return Err(PhpError::Exception {
                                class: other.to_string(),
                                message,
                                code: 0,
                            });
                        }
                        None => {
                            // No error channel set (e.g. internal failure) —
                            // surface honestly, not the generic "fiber waker"
                            // string.
                            return Err(PhpError::Custom("send: waker resolution failed".into()));
                        }
                    }
                }
                // Direct fiber-layer timeout (should be rare given the
                // synthetic::cancel path, but handle it).
                -2 => {
                    return results::write_send(call, SendKind::Timeout);
                }
                other => {
                    // rc=1 means "not in oxphp fiber"; we only land here
                    // when in_fiber == true, so the SAPI predicate and
                    // `oxphp_current_fiber` disagree — a logic bug worth
                    // crashing on in dev. Release builds degrade to a
                    // Custom error so the worker stays up.
                    debug_assert!(
                        other != 1,
                        "fiber_await rc=1 in fiber path — oxphp_bridge_in_fiber lied",
                    );
                    return Err(PhpError::Custom(format!("send: fiber_await rc={other}")));
                }
            }
        }
    } else {
        // Non-fiber path: thread-block via send_blocking.
        let rc = unsafe { oxphp_shared_channel_send_blocking(entry_ptr, buf, len, timeout_ms) };

        if rc == SharedError::Closed.code() {
            return results::write_send(call, SendKind::Closed);
        }
        if rc == SharedError::Timeout.code() {
            return results::write_send(call, SendKind::Timeout);
        }
        super::counter::counter_rc_to_result(rc)?;
        results::write_send(call, SendKind::Ok)
    }
}

/// Body shared by `recv()` and `recvTimeout()`. `timeout_ms` is the
/// wire-format timeout: `-1` = forever (fiber-cancel only), `> 0` =
/// bounded. The non-blocking case is a separate `tryRecv` handler.
///
/// Returns a `RecvResult` written into the retval slot.
#[allow(clippy::too_many_lines)]
fn invoke_channel_recv(
    call: &mut crate::bridge::call::NativeCall,
    timeout_ms: i64,
) -> Result<(), crate::plugin::PhpError> {
    use crate::bridge::ffi as bridge_ffi;
    use crate::plugin::PhpError;
    use crate::plugins::ox_shared::handle::SharedHandle;
    use crate::plugins::ox_shared::results::{self, RecvKind};

    let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;

    // The fiber-suspend path uses synthetic-promise FFI that is gated
    // behind `feature = "php"` (PROMISE_MAP lives on PHP worker
    // threads). Without `php`, force `in_fiber = false` so we always
    // take the thread-blocking branch — matches the mock
    // `oxphp_bridge_in_fiber` returning 0 anyway.
    #[cfg(feature = "php")]
    let in_fiber = unsafe { bridge_ffi::oxphp_bridge_in_fiber() } != 0;
    #[cfg(not(feature = "php"))]
    let in_fiber = false;

    if in_fiber {
        // Fiber path: try_recv once first, then register recv-waiter and
        // suspend via fiber_await if the channel is empty. Matches the
        // blocking-path FFI which always attempts once before returning
        // Timeout, so recvTimeout($ms) on a non-empty channel succeeds
        // rather than spuriously timing out.
        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 0;
        let try_rc = unsafe {
            oxphp_shared_channel_try_recv(entry_ptr, &mut out_buf, &mut out_len, &mut state)
        };
        super::counter::counter_rc_to_result(try_rc)?;
        match state {
            0 => {
                let r = results::write_recv_ok(call, out_buf, out_len);
                if !out_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                }
                return r;
            }
            2 => {
                if !out_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                }
                return results::write_recv(call, RecvKind::Closed);
            }
            _ => {
                // state == 1: WouldBlockEmpty. Free defensively before
                // parking; Wait::Try is unreachable (tryRecv is its own
                // handler).
                if !out_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                }
                if matches!(parse_timeout(timeout_ms), Wait::Try) {
                    unreachable!("Wait::Try unreachable from recv/recvTimeout");
                }
            }
        }

        let mut promise_id: i64 = 0;
        // Shim takes u64: -1 (forever) → 0 (no timeout spawned).
        let fiber_timeout_ms: u64 = if timeout_ms < 0 { 0 } else { timeout_ms as u64 };
        // Capture the deadline so the timeout branch below can tell a real
        // deadline expiry from an external fiber cancel that arrived early
        // — the latter must propagate AsyncException, not masquerade as
        // Timeout. Mirrors the send path's `deadline` check.
        let deadline = (timeout_ms > 0)
            .then(|| std::time::Instant::now() + Duration::from_millis(fiber_timeout_ms));
        let reg_rc =
            unsafe { recv_fiber_register_shim(entry_ptr, fiber_timeout_ms, &mut promise_id) };
        super::counter::counter_rc_to_result(reg_rc)?;

        // The await may write a value-class object directly via the
        // waker dispatcher; we capture that into a temporary zval and
        // then transcribe into a RecvResult::Ok payload below.
        let retval = call.retval_ptr();
        let fiber_rc = unsafe { bridge_ffi::oxphp_bridge_fiber_await(promise_id, 0.0, retval) };

        // True only when the recvTimeout deadline has actually elapsed —
        // distinguishes the timeout timer firing from a pre-deadline
        // external cancel.
        let deadline_hit = deadline
            .map(|d| std::time::Instant::now() >= d)
            .unwrap_or(false);

        match fiber_rc {
            0 => {
                // Waker resolved with Value → retval holds the materialized
                // payload zval (oxphp_bridge_fiber_await already deserialised
                // it). Wrap it into RecvResult::Ok in place — no portbuf
                // serialize/deserialize round-trip. (The blocking and
                // buffered-hit paths still go through write_recv_ok because
                // their payload arrives pre-serialised.)
                results::write_recv_ok_inplace(call)
            }
            -1 => {
                // The waker dispatcher (`await_dispatch_callback`) reports
                // failures through the bridge async-exception TLS — NOT
                // `EG(exception)` — exactly like the channel that
                // `oxphp_async_await` drains. Read and clear it here:
                // reading `EG(exception)` would see null and misclassify
                // every waker timeout as a fatal.
                let cls_ptr = unsafe { bridge_ffi::oxphp_bridge_get_async_exc_class() };
                let msg_ptr = unsafe { bridge_ffi::oxphp_bridge_get_async_exc_message() };
                let class = if cls_ptr.is_null() {
                    None
                } else {
                    Some(
                        unsafe { std::ffi::CStr::from_ptr(cls_ptr) }
                            .to_string_lossy()
                            .into_owned(),
                    )
                };
                let message = if msg_ptr.is_null() {
                    String::new()
                } else {
                    unsafe { std::ffi::CStr::from_ptr(msg_ptr) }
                        .to_string_lossy()
                        .into_owned()
                };
                unsafe { bridge_ffi::oxphp_bridge_clear_async_exception() };

                match class.as_deref() {
                    Some("OxPHP\\Async\\AsyncException") => {
                        // Timeout, forever-cancel and close-while-parked all
                        // arrive here as AsyncException (synthetic::cancel).
                        // Disambiguate with one non-blocking probe: a value
                        // may have landed just before the cancel, or the
                        // channel may have closed under the parked recv.
                        let mut p_buf: *mut u8 = std::ptr::null_mut();
                        let mut p_len: usize = 0;
                        let mut p_state: c_int = 0;
                        let probe_rc = unsafe {
                            oxphp_shared_channel_try_recv(
                                entry_ptr,
                                &mut p_buf,
                                &mut p_len,
                                &mut p_state,
                            )
                        };
                        super::counter::counter_rc_to_result(probe_rc)?;
                        match p_state {
                            0 => {
                                let r = results::write_recv_ok(call, p_buf, p_len);
                                if !p_buf.is_null() {
                                    unsafe { bridge_ffi::oxphp_portable_free(p_buf) };
                                }
                                r
                            }
                            2 => {
                                if !p_buf.is_null() {
                                    unsafe { bridge_ffi::oxphp_portable_free(p_buf) };
                                }
                                results::write_recv(call, RecvKind::Closed)
                            }
                            _ => {
                                // Still empty + open.
                                if !p_buf.is_null() {
                                    unsafe { bridge_ffi::oxphp_portable_free(p_buf) };
                                }
                                if deadline_hit {
                                    // Bounded recv: the deadline fired →
                                    // RecvResult::Timeout, not a fatal.
                                    results::write_recv(call, RecvKind::Timeout)
                                } else {
                                    // Forever recv: caller cancelled the
                                    // fiber. Per spec, propagate the
                                    // AsyncException — do NOT map to Closed.
                                    Err(PhpError::Exception {
                                        class: "OxPHP\\Async\\AsyncException".into(),
                                        message: if message.is_empty() {
                                            "recv: fiber cancelled".into()
                                        } else {
                                            message
                                        },
                                        code: 0,
                                    })
                                }
                            }
                        }
                    }
                    Some(other) => {
                        // A real application exception delivered through the
                        // waker (defensive: recv-waiters only resolve
                        // Value/Cancelled today). Surface its true class
                        // instead of the generic "fiber waker" string.
                        Err(PhpError::Exception {
                            class: other.to_string(),
                            message,
                            code: 0,
                        })
                    }
                    None => {
                        // Neither bridge TLS nor a thrown exception was set
                        // (e.g. a result deserialize failed without setting
                        // any error channel) — a genuine internal failure.
                        Err(PhpError::Custom("recv: waker resolution failed".into()))
                    }
                }
            }
            -2 => {
                // Direct fiber-layer timeout.
                results::write_recv(call, RecvKind::Timeout)
            }
            other => {
                debug_assert!(
                    other != 1,
                    "fiber_await rc=1 in fiber path — oxphp_bridge_in_fiber lied",
                );
                Err(PhpError::Custom(format!("recv: fiber_await rc={other}")))
            }
        }
    } else {
        // Thread-block path.
        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_recv_blocking(
                entry_ptr,
                timeout_ms,
                &mut out_buf,
                &mut out_len,
                &mut state,
            )
        };
        if rc == SharedError::Timeout.code() {
            if !out_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
            }
            return results::write_recv(call, RecvKind::Timeout);
        }
        super::counter::counter_rc_to_result(rc)?;
        match state {
            0 => {
                let r = results::write_recv_ok(call, out_buf, out_len);
                if !out_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                }
                r
            }
            2 => {
                if !out_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                }
                results::write_recv(call, RecvKind::Closed)
            }
            other => {
                if !out_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                }
                Err(PhpError::Custom(format!("recv: unexpected state {other}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_shared::error::SharedError;
    use crate::plugins::ox_shared::types::once::OnceInner;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    // Test-only equality on `Payload`: compares wire bytes and ignores
    // `keepalive` (which carries non-comparable `SharedRefOwned`). Lets the
    // existing assertions compare a popped `Payload` against the bytes a
    // test sent.
    impl PartialEq for Payload {
        fn eq(&self, other: &Self) -> bool {
            self.bytes == other.bytes
        }
    }
    impl PartialEq<Vec<u8>> for Payload {
        fn eq(&self, other: &Vec<u8>) -> bool {
            self.bytes == *other
        }
    }

    /// Encode a tag-7 (nested `Shared\*` ref) portbuf record for `id`/`tag`.
    fn tag7(tag: SharedType, id: u64) -> Vec<u8> {
        let mut b = vec![7u8, tag as u8];
        b.extend_from_slice(&id.to_le_bytes());
        b
    }

    #[test]
    fn buffered_send_holds_keepalive_for_shared_value() {
        ensure_test_registry();
        let reg = registry();
        // A nested Shared\* value (another Channel entry stands in for one).
        let value = reg
            .insert(SharedType::Channel, Arc::new(ChannelInner::new(1)))
            .unwrap();
        let vid = value.id;
        // The channel under test, bound to the registry.
        let typed = Arc::new(ChannelInner::new(4));
        let ch_entry = reg.insert(SharedType::Channel, typed.clone()).unwrap();
        typed.bind_entry(Arc::downgrade(&ch_entry));

        let payload = typed
            .build_payload(tag7(SharedType::Channel, vid))
            .expect("build ok");
        assert_eq!(
            payload.keepalive.len(),
            1,
            "keepalive resolved for nested ref"
        );
        typed.try_send(payload).expect("send ok");

        // Drop the producer's external strong ref; the buffered payload's
        // keepalive must keep the entry alive in transit.
        let weak = Arc::downgrade(&value);
        drop(value);
        assert!(
            weak.upgrade().is_some(),
            "buffered payload should keep the in-transit entry alive"
        );

        // Consume it; the returned payload still carries its keepalive.
        let got = typed.try_recv().expect("recv ok").expect("some");
        assert_eq!(got.bytes[0], 7);
        drop(got);
    }

    #[test]
    fn build_payload_rejects_direct_self_reference() {
        ensure_test_registry();
        let reg = registry();
        let typed = Arc::new(ChannelInner::new(4));
        let ch_entry = reg.insert(SharedType::Channel, typed.clone()).unwrap();
        typed.bind_entry(Arc::downgrade(&ch_entry));
        let bytes = tag7(SharedType::Channel, ch_entry.id);
        assert_eq!(typed.build_payload(bytes).err(), Some(SharedError::Cycle));
    }

    #[test]
    fn build_payload_no_shared_refs_is_empty_keepalive() {
        ensure_test_registry();
        let typed = ChannelInner::new(4);
        // a plain Long(7) portbuf: tag 3 + 8 LE bytes — no nested refs.
        let mut bytes = vec![3u8];
        bytes.extend_from_slice(&7i64.to_le_bytes());
        let payload = typed.build_payload(bytes).expect("build ok");
        assert!(payload.keepalive.is_empty());
    }

    #[tokio::test]
    async fn waker_handoff_carries_keepalive_into_result() {
        ensure_test_registry();
        let reg = registry();
        // A nested Shared\* value that must stay alive in transit.
        let target = reg
            .insert(SharedType::Channel, Arc::new(ChannelInner::new(1)))
            .unwrap();
        let tid = target.id;
        // The channel under test.
        let typed = Arc::new(ChannelInner::new(1));
        let ch_entry = reg.insert(SharedType::Channel, typed.clone()).unwrap();
        typed.bind_entry(Arc::downgrade(&ch_entry));
        // Park a recv-waiter, then send a tag-7 payload referencing target so
        // try_send hands it straight to the waiter (the waker path).
        let (id, rx) = synthetic::alloc();
        typed.register_recv_waiter(id);
        let payload = typed.build_payload(tag7(SharedType::Channel, tid)).unwrap();
        typed.try_send(payload).unwrap();
        // Drop the producer's external strong ref; the AsyncResult's keepalive
        // must pin the entry until the fiber deserializes.
        let weak = Arc::downgrade(&target);
        drop(target);
        let result = rx.await.unwrap();
        assert!(
            result.keepalive.is_some(),
            "keepalive rode into AsyncResult"
        );
        assert!(
            weak.upgrade().is_some(),
            "AsyncResult keepalive should pin the in-transit entry"
        );
    }

    #[test]
    fn children_reports_in_flight_refs_until_consumed() {
        ensure_test_registry();
        let reg = registry();
        let inner = Arc::new(ChannelInner::new(4));
        let entry = reg.insert(SharedType::Channel, inner.clone()).unwrap();
        inner.bind_entry(Arc::downgrade(&entry));
        // A nested Shared\* value to send into the channel.
        let other = reg
            .insert(SharedType::Channel, Arc::new(ChannelInner::new(1)))
            .unwrap();
        let oid = other.id;

        let payload = inner.build_payload(tag7(SharedType::Channel, oid)).unwrap();
        inner.try_send(payload).unwrap();

        let mut out = Vec::new();
        inner.children(&mut out);
        assert!(
            out.iter().any(|r| r.id == oid),
            "in-flight ref must be visible to the cycle walker"
        );

        // Consume → the ref leaves the index.
        let _ = inner.try_recv().unwrap().unwrap();
        let mut out2 = Vec::new();
        inner.children(&mut out2);
        assert!(
            !out2.iter().any(|r| r.id == oid),
            "consumed ref must leave children()"
        );
    }

    #[test]
    fn cycle_through_two_channels_is_rejected() {
        ensure_test_registry();
        let reg = registry();
        let a = Arc::new(ChannelInner::new(2));
        let ea = reg.insert(SharedType::Channel, a.clone()).unwrap();
        a.bind_entry(Arc::downgrade(&ea));
        let b = Arc::new(ChannelInner::new(2));
        let eb = reg.insert(SharedType::Channel, b.clone()).unwrap();
        b.bind_entry(Arc::downgrade(&eb));

        // a.send(b): no cycle yet; b becomes in-flight in a (visible via
        // a.children()).
        let pa = a.build_payload(tag7(SharedType::Channel, eb.id)).unwrap();
        a.try_send(pa).unwrap();

        // b.send(a) would close an a<->b strong-ref cycle — must be rejected,
        // which only works because a.children() now reports b.
        assert_eq!(
            b.build_payload(tag7(SharedType::Channel, ea.id)).err(),
            Some(SharedError::Cycle)
        );
    }

    // Concurrent blocking producer/consumer on a cap=1 ChannelInner from two
    // OS threads: the consumer must receive every sent item with none lost or
    // duplicated. Exercises the bounded-queue park/wake handoff under the
    // tightest capacity, where each send must wait for a recv and vice versa.
    #[test]
    fn stress_concurrent_blocking_send_recv_cap1() {
        let ch = Arc::new(ChannelInner::new(1));
        let n: usize = 200_000;

        let prod = {
            let ch = ch.clone();
            std::thread::spawn(move || {
                for _ in 0..n {
                    ch.send_blocking(Payload::bytes_only(vec![7u8]), Wait::Forever)
                        .expect("send");
                }
                ch.close();
            })
        };
        let cons = {
            let ch = ch.clone();
            std::thread::spawn(move || {
                let mut got = 0usize;
                loop {
                    match ch.recv_blocking(Wait::Forever) {
                        Ok(Some(_)) => got += 1,
                        Ok(None) => break,
                        Err(e) => panic!("recv err: {e:?}"),
                    }
                }
                got
            })
        };
        prod.join().expect("producer thread");
        let got = cons.join().expect("consumer thread");
        assert_eq!(got, n, "consumer must receive every sent item");
    }

    #[test]
    fn capacity_fits_within_budget() {
        // 100 slots * SLOT_BYTES is far under a 1 MiB budget.
        assert!(channel_capacity_fits(100, 1 << 20));
    }

    #[test]
    fn capacity_rejected_over_budget() {
        // budget allows exactly 10 slots; 11 must be rejected.
        let budget = 10 * SLOT_BYTES;
        assert!(channel_capacity_fits(10, budget));
        assert!(!channel_capacity_fits(11, budget));
    }

    #[test]
    fn capacity_rejected_on_overflow() {
        // capacity * SLOT_BYTES overflows u64 -> treated as over-budget,
        // never panics.
        assert!(!channel_capacity_fits(u64::MAX, u64::MAX));
    }

    #[test]
    fn tiny_budget_clamped_to_one_slot() {
        // A zero / sub-slot configured budget is clamped up to one slot, so a
        // minimal capacity-1 channel always succeeds; larger capacities still
        // need a real budget.
        assert_eq!(effective_channel_budget(0), SLOT_BYTES);
        assert_eq!(effective_channel_budget(SLOT_BYTES - 1), SLOT_BYTES);
        assert_eq!(effective_channel_budget(100 * SLOT_BYTES), 100 * SLOT_BYTES);
        assert!(channel_capacity_fits(1, effective_channel_budget(0)));
        assert!(!channel_capacity_fits(2, effective_channel_budget(0)));
    }

    #[test]
    fn value_within_cap_is_accepted() {
        // exactly at the cap is allowed (boundary off-by-one guard).
        assert!(check_value_size(100, 100, "send").is_ok());
        assert!(check_value_size(0, 100, "send").is_ok());
    }

    #[test]
    fn value_over_cap_throws_value_too_large() {
        let err = check_value_size(101, 100, "send").unwrap_err();
        match err {
            crate::plugin::PhpError::Exception { class, message, .. } => {
                assert_eq!(class, "OxPHP\\Shared\\ValueTooLargeException");
                assert!(message.contains("101"), "message names the size: {message}");
                assert!(
                    message.contains("send"),
                    "message names the method: {message}"
                );
            }
            other => panic!("expected ValueTooLargeException, got {other:?}"),
        }
    }

    #[test]
    fn send_many_first_oversized_offset_is_detected() {
        // offsets = element start positions; concat_len = total bytes.
        // Three elements: [0..10), [10..210), [210..215) -> sizes 10, 200, 5.
        let offsets = [0usize, 10, 210];
        let concat_len = 215usize;
        let cap = 100usize;
        // Element 1 (size 200) is the first to exceed cap=100.
        assert_eq!(
            first_oversized_element(&offsets, concat_len, cap),
            Some((1, 200))
        );
    }

    #[test]
    fn send_many_all_within_cap_is_none() {
        let offsets = [0usize, 10, 30];
        let concat_len = 35usize; // sizes 10, 20, 5
        assert_eq!(first_oversized_element(&offsets, concat_len, 100), None);
    }

    #[test]
    fn new_channel_empty() {
        let ch = ChannelInner::new(4);
        assert_eq!(ch.pending(), 0);
        assert!(!ch.is_closed());
        assert_eq!(ch.capacity(), 4);
    }

    #[test]
    fn try_send_then_try_recv() {
        let ch = ChannelInner::new(4);
        ch.try_send(Payload::bytes_only(vec![1, 2, 3]))
            .expect("send ok");
        assert_eq!(ch.pending(), 1);
        match ch.try_recv() {
            Ok(Some(p)) => assert_eq!(p, vec![1, 2, 3]),
            other => panic!("expected Ok(Some([1,2,3])), got {other:?}"),
        }
        assert_eq!(ch.pending(), 0);
    }

    #[test]
    fn try_send_full_returns_full() {
        let ch = ChannelInner::new(1);
        ch.try_send(Payload::bytes_only(vec![0xAA]))
            .expect("first send ok");
        match ch.try_send(Payload::bytes_only(vec![0xBB])) {
            Err(TrySendErr::Full(p)) => assert_eq!(p, vec![0xBB]),
            other => panic!("expected Full([0xBB]), got {other:?}"),
        }
    }

    #[test]
    fn try_recv_empty_not_closed_returns_would_block() {
        let ch = ChannelInner::new(4);
        assert_eq!(ch.try_recv(), Err(TryRecvErr::WouldBlockEmpty));
    }

    #[test]
    fn try_recv_empty_closed_returns_none() {
        let ch = ChannelInner::new(4);
        ch.close();
        match ch.try_recv() {
            Ok(None) => {}
            other => panic!("expected Ok(None), got {other:?}"),
        }
    }

    #[test]
    fn close_is_idempotent() {
        let ch = ChannelInner::new(4);
        assert!(ch.close());
        assert!(!ch.close());
        assert!(ch.is_closed());
    }

    #[test]
    fn try_send_after_close_errors() {
        let ch = ChannelInner::new(4);
        ch.close();
        match ch.try_send(Payload::bytes_only(vec![9])) {
            Err(TrySendErr::Closed(p)) => assert_eq!(p, vec![9]),
            other => panic!("expected Closed([9]), got {other:?}"),
        }
    }

    #[test]
    fn try_recv_drains_after_close() {
        let ch = ChannelInner::new(4);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        ch.try_send(Payload::bytes_only(vec![2])).unwrap();
        ch.close();
        match ch.try_recv() {
            Ok(Some(p)) => assert_eq!(p, vec![1]),
            other => panic!("expected Ok(Some([1])), got {other:?}"),
        }
        match ch.try_recv() {
            Ok(Some(p)) => assert_eq!(p, vec![2]),
            other => panic!("expected Ok(Some([2])), got {other:?}"),
        }
        match ch.try_recv() {
            Ok(None) => {}
            other => panic!("expected Ok(None), got {other:?}"),
        }
    }

    #[test]
    fn capacity_zero_clamped_to_one() {
        let ch = ChannelInner::new(0);
        assert_eq!(ch.capacity(), 1);
        ch.try_send(Payload::bytes_only(vec![1]))
            .expect("first send ok");
        match ch.try_send(Payload::bytes_only(vec![2])) {
            Err(TrySendErr::Full(_)) => {}
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn pending_matches_try_send_counts() {
        let ch = ChannelInner::new(8);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        ch.try_send(Payload::bytes_only(vec![2])).unwrap();
        ch.try_send(Payload::bytes_only(vec![3])).unwrap();
        assert_eq!(ch.pending(), 3);
        let _ = ch.try_recv().unwrap();
        let _ = ch.try_recv().unwrap();
        assert_eq!(ch.pending(), 1);
    }

    #[test]
    fn debug_snapshot_returns_pending_as_long() {
        let ch = ChannelInner::new(4);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        ch.try_send(Payload::bytes_only(vec![2])).unwrap();
        match ch.debug_snapshot() {
            SharedValue::Long(2) => {}
            other => panic!("expected Long(2), got {other:?}"),
        }
    }

    #[test]
    fn shared_inner_downcast_channel() {
        let ch: Arc<dyn SharedInner> = Arc::new(ChannelInner::new(4));
        assert!(ch.as_any_channel().is_some());

        let once: Arc<dyn SharedInner> = Arc::new(OnceInner::new());
        assert!(once.as_any_channel().is_none());
    }

    // ─── blocking send / recv ─────────────────────────────────

    #[test]
    fn send_blocking_fast_path() {
        let ch = ChannelInner::new(4);
        ch.send_blocking(
            Payload::bytes_only(vec![1]),
            Wait::Bounded(Duration::from_millis(100)),
        )
        .expect("send ok");
        assert_eq!(ch.pending(), 1);
    }

    #[test]
    fn send_blocking_on_full_waits_until_recv() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                ch.send_blocking(
                    Payload::bytes_only(vec![2]),
                    Wait::Bounded(Duration::from_secs(2)),
                )
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        let first = ch.try_recv().expect("drain first");
        assert_eq!(first.map(|p| p.bytes), Some(vec![1]));
        let res = sender.join().expect("sender thread");
        assert!(res.is_ok(), "send_blocking got {res:?}");
        let second = ch.try_recv().expect("drain second");
        assert_eq!(second.map(|p| p.bytes), Some(vec![2]));
    }

    #[test]
    fn send_blocking_timeout_returns_timeout() {
        let ch = ChannelInner::new(1);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let res = ch.send_blocking(
            Payload::bytes_only(vec![2]),
            Wait::Bounded(Duration::from_millis(50)),
        );
        assert!(matches!(res, Err(SharedError::Timeout)), "got {res:?}");
    }

    #[test]
    fn send_blocking_on_closed_returns_closed() {
        let ch = ChannelInner::new(4);
        ch.close();
        let res = ch.send_blocking(Payload::bytes_only(vec![1]), Wait::Forever);
        assert!(matches!(res, Err(SharedError::Closed)), "got {res:?}");
    }

    #[test]
    fn send_blocking_wakes_when_closed_mid_wait() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                ch.send_blocking(
                    Payload::bytes_only(vec![2]),
                    Wait::Bounded(Duration::from_secs(5)),
                )
            })
        };
        // Let the blocker arm.
        std::thread::sleep(Duration::from_millis(50));
        let close_at = Instant::now();
        ch.close();
        let res = sender.join().expect("sender thread");
        let wake_latency = close_at.elapsed();
        assert!(
            wake_latency < Duration::from_millis(100),
            "wake latency {wake_latency:?} exceeded 100ms budget (POLL_QUANTUM = 20ms)"
        );
        assert!(matches!(res, Err(SharedError::Closed)), "got {res:?}");
    }

    #[test]
    fn recv_blocking_fast_path() {
        let ch = ChannelInner::new(4);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let res = ch.recv_blocking(Wait::Bounded(Duration::from_millis(100)));
        assert_eq!(res.unwrap().map(|p| p.bytes), Some(vec![1]));
    }

    #[test]
    fn recv_blocking_on_empty_waits_until_send() {
        let ch = Arc::new(ChannelInner::new(4));
        let receiver = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || ch.recv_blocking(Wait::Bounded(Duration::from_secs(2))))
        };
        std::thread::sleep(Duration::from_millis(50));
        ch.try_send(Payload::bytes_only(vec![7])).unwrap();
        let res = receiver.join().expect("receiver thread");
        assert_eq!(res.unwrap().map(|p| p.bytes), Some(vec![7]));
    }

    #[test]
    fn recv_blocking_timeout_returns_timeout() {
        let ch = ChannelInner::new(4);
        let res = ch.recv_blocking(Wait::Bounded(Duration::from_millis(50)));
        assert!(matches!(res, Err(SharedError::Timeout)), "got {res:?}");
    }

    #[test]
    fn recv_blocking_on_closed_empty_returns_none() {
        let ch = ChannelInner::new(4);
        ch.close();
        let res = ch.recv_blocking(Wait::Forever).expect("ok");
        assert!(res.is_none());
    }

    #[test]
    fn recv_blocking_wakes_when_closed_mid_wait() {
        let ch = Arc::new(ChannelInner::new(4));
        let receiver = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || ch.recv_blocking(Wait::Bounded(Duration::from_secs(5))))
        };
        // Let the blocker arm.
        std::thread::sleep(Duration::from_millis(50));
        let close_at = Instant::now();
        ch.close();
        let res = receiver.join().expect("receiver thread");
        let wake_latency = close_at.elapsed();
        assert!(
            wake_latency < Duration::from_millis(100),
            "wake latency {wake_latency:?} exceeded 100ms budget (POLL_QUANTUM = 20ms)"
        );
        assert_eq!(res.unwrap(), None);
    }

    #[test]
    fn recv_blocking_drains_before_returning_none() {
        let ch = ChannelInner::new(4);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        ch.try_send(Payload::bytes_only(vec![2])).unwrap();
        ch.close();
        assert_eq!(
            ch.recv_blocking(Wait::Forever).unwrap().map(|p| p.bytes),
            Some(vec![1])
        );
        assert_eq!(
            ch.recv_blocking(Wait::Forever).unwrap().map(|p| p.bytes),
            Some(vec![2])
        );
        assert_eq!(ch.recv_blocking(Wait::Forever).unwrap(), None);
    }

    #[test]
    fn senders_blocked_gauge_increments_while_blocked() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                ch.send_blocking(
                    Payload::bytes_only(vec![2]),
                    Wait::Bounded(Duration::from_secs(2)),
                )
            })
        };
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(ch.senders_blocked().load(Ordering::Relaxed), 1);
        let _ = ch.try_recv().unwrap();
        let res = sender.join().expect("sender thread");
        assert!(res.is_ok());
        assert_eq!(ch.senders_blocked().load(Ordering::Relaxed), 0);
    }

    #[test]
    fn receivers_blocked_gauge_increments_while_blocked() {
        let ch = Arc::new(ChannelInner::new(4));
        let receiver = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || ch.recv_blocking(Wait::Bounded(Duration::from_secs(2))))
        };
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(ch.receivers_blocked().load(Ordering::Relaxed), 1);
        ch.try_send(Payload::bytes_only(vec![9])).unwrap();
        let res = receiver.join().expect("receiver thread");
        assert_eq!(res.unwrap().map(|p| p.bytes), Some(vec![9]));
        assert_eq!(ch.receivers_blocked().load(Ordering::Relaxed), 0);
    }

    // ─── synthetic-promise waker lists ────────────────────────

    #[tokio::test]
    async fn register_recv_waiter_fires_on_try_send() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id, rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        let ch2 = ch.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            ch2.try_send(Payload::bytes_only(vec![7, 7])).unwrap();
        });
        let got = rx.await.unwrap();
        assert!(got.success);
        assert_eq!(got.serialized_value_len, 2);
    }

    #[tokio::test]
    async fn register_recv_waiter_cancelled_on_close() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id, rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        ch.close();
        let got = rx.await.unwrap();
        assert!(!got.success);
        assert_eq!(
            got.exception_class.as_deref(),
            Some("OxPHP\\Async\\AsyncException")
        );
    }

    #[tokio::test]
    async fn register_recv_waiter_immediately_cancels_if_closed_empty() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        ch.close();
        let (id, rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        let got = rx.await.unwrap();
        assert!(!got.success);
        assert_eq!(
            got.exception_class.as_deref(),
            Some("OxPHP\\Async\\AsyncException")
        );
    }

    #[tokio::test]
    async fn register_recv_waiter_drains_buffered_items() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        ch.try_send(Payload::bytes_only(vec![1, 2, 3])).unwrap();
        assert_eq!(ch.pending(), 1);
        let (id, rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        let got = rx.await.unwrap();
        assert!(got.success);
        assert_eq!(got.serialized_value_len, 3);
        assert_eq!(ch.pending(), 0);
    }

    #[tokio::test]
    async fn register_send_waiter_fires_on_try_recv() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(1));
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let (id, rx) = synthetic::alloc();
        ch.register_send_waiter(id);
        let ch2 = ch.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let _ = ch2.try_recv().unwrap();
        });
        let got = rx.await.unwrap();
        assert!(got.success);
        assert_eq!(got.serialized_value_len, 0);
    }

    #[tokio::test]
    async fn register_send_waiter_cancelled_with_closed_on_close() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(1));
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let (id, rx) = synthetic::alloc();
        ch.register_send_waiter(id);
        ch.close();
        let got = rx.await.unwrap();
        assert!(!got.success);
        assert_eq!(
            got.exception_class.as_deref(),
            Some("OxPHP\\Shared\\ClosedException")
        );
    }

    #[tokio::test]
    async fn try_send_prefers_waiter_over_buffer() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id, rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        ch.try_send(Payload::bytes_only(vec![99])).unwrap();
        let got = rx.await.unwrap();
        assert!(got.success);
        assert_eq!(got.serialized_value_len, 1);
        // Waiter received directly; nothing landed in the buffer.
        assert_eq!(ch.pending(), 0);
    }

    #[tokio::test]
    async fn close_resolves_multiple_waiters() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id1, rx1) = synthetic::alloc();
        let (id2, rx2) = synthetic::alloc();
        let (id3, rx3) = synthetic::alloc();
        ch.register_recv_waiter(id1);
        ch.register_recv_waiter(id2);
        ch.register_recv_waiter(id3);
        ch.close();
        for rx in [rx1, rx2, rx3] {
            let got = rx.await.unwrap();
            assert!(!got.success);
            assert_eq!(
                got.exception_class.as_deref(),
                Some("OxPHP\\Async\\AsyncException")
            );
        }
    }

    #[tokio::test]
    async fn dead_waiter_skipped_on_drain() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id1, rx1) = synthetic::alloc();
        let (id2, rx2) = synthetic::alloc();
        ch.register_recv_waiter(id1);
        ch.register_recv_waiter(id2);
        // Cancel the first waiter BEFORE the send — makes it "dead"
        // from the draining thread's POV.
        assert!(synthetic::cancel(id1));
        ch.try_send(Payload::bytes_only(vec![42])).unwrap();
        let got1 = rx1.await.unwrap();
        assert!(!got1.success); // first got Cancelled
        let got2 = rx2.await.unwrap();
        assert!(got2.success);
        assert_eq!(got2.serialized_value_len, 1);
        assert_eq!(ch.pending(), 0);
    }

    /// Regression: when `drain_one_recv_waiter_with` is called with a
    /// single-entry waiter list and that waiter is live, the happy
    /// path must NOT clone the payload — but that's a non-observable
    /// perf detail. The observable invariant is: the waiter receives
    /// the payload intact. This test exercises exactly that path
    /// (`has_more == false` + live waiter) to guard against
    /// regressions in the optimised branch.
    #[tokio::test]
    async fn single_live_waiter_receives_payload_intact() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id, rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        // Sanity: exactly one waiter parked.
        let expected = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        ch.try_send(Payload::bytes_only(expected.clone())).unwrap();
        let got = rx.await.unwrap();
        assert!(got.success);
        assert_eq!(got.serialized_value_len, expected.len());
        // Buffer was bypassed — nothing landed in the crossbeam queue.
        assert_eq!(ch.pending(), 0);
    }

    /// Regression: dead-last-waiter race (single parked waiter,
    /// cancelled before `try_send` reaches `resolve`). The payload must
    /// NOT be lost — `resolve_value` hands it back when the waiter was
    /// taken, so `try_send` re-deposits it into the buffer and a later
    /// `try_recv` still returns it (FIFO before subsequent sends). No
    /// panic, no deadlock, no data loss.
    #[tokio::test]
    async fn single_dead_waiter_redeposits_payload() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id, _rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        // Kill the waiter before send reaches it.
        assert!(synthetic::cancel(id));
        // Send must not panic and must not drop the payload.
        ch.try_send(Payload::bytes_only(vec![0xDE, 0xAD])).unwrap();
        // Channel is still operational.
        ch.try_send(Payload::bytes_only(vec![0xBE, 0xEF])).unwrap();
        // The dead-waiter payload was preserved in the buffer (FIFO).
        assert_eq!(
            ch.try_recv().unwrap().map(|p| p.bytes),
            Some(vec![0xDE, 0xAD])
        );
        assert_eq!(
            ch.try_recv().unwrap().map(|p| p.bytes),
            Some(vec![0xBE, 0xEF])
        );
    }

    /// Regression: `drain_buffered_to_waiters` must re-park a buffered item
    /// at the front (loss-free, non-blocking) when the only parked waiter
    /// turns out dead — not drop it (losing the item) nor block (starving
    /// the runtime). A later `try_recv` returns it intact, before the
    /// crossbeam buffer.
    #[tokio::test]
    async fn drain_bounce_restashes_at_front_no_loss() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(2));
        // Park a waiter while the buffer is empty (register's own drain is
        // a no-op), then kill it so it stays dead in the list.
        let (id, _rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        assert!(synthetic::cancel(id));
        // Deposit straight into the crossbeam buffer, bypassing `try_send`
        // (which would reap the dead waiter itself). This reproduces an
        // item buffered while a dead waiter is still parked.
        ch.tx.try_send(Payload::bytes_only(vec![0xAA])).unwrap();
        ch.bump_pending();
        assert_eq!(ch.pending(), 1);
        // Drain: item is popped for the dead waiter and must be re-parked
        // at the front, still counted.
        ch.drain_buffered_to_waiters();
        assert_eq!(ch.pending(), 1, "re-parked item must stay counted");
        // Recovered intact; channel then empty+open.
        assert_eq!(ch.try_recv().unwrap().map(|p| p.bytes), Some(vec![0xAA]));
        assert_eq!(ch.pending(), 0);
        assert_eq!(ch.try_recv(), Err(TryRecvErr::WouldBlockEmpty));
    }

    /// Regression: a bounce (buffer item re-parked to the front-stash for a
    /// dead recv-waiter) must NOT wake a parked send-waiter — the item is
    /// still in the channel, so waking a sender to refill the freed slot
    /// would overshoot capacity (cap in buffer + 1 in stash). The sender is
    /// woken only once the stashed item is actually consumed.
    #[test]
    fn bounce_does_not_overshoot_capacity_with_parked_sender() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(1)); // cap=1
                                                            // Dead recv-waiter parked while empty (so register's own
                                                            // drain is a no-op), then cancelled — stays dead in the list.
        let (rid, _rrx) = synthetic::alloc();
        ch.register_recv_waiter(rid);
        assert!(synthetic::cancel(rid));
        // A fiber send-waiter parked, waiting for a slot.
        let (sid, mut srx) = synthetic::alloc();
        ch.register_send_waiter(sid);
        // Now fill the buffer directly (bypass `try_send`, which would reap
        // the dead waiter / drain itself), reproducing a buffered item with
        // a dead recv-waiter still parked ahead of it.
        ch.tx.try_send(Payload::bytes_only(vec![0xAA])).unwrap();
        ch.bump_pending();
        // Drain: the item bounces to the front-stash. The send-waiter must
        // NOT be woken — the channel still holds exactly one item.
        ch.drain_buffered_to_waiters();
        assert_eq!(ch.pending(), 1, "exactly one item (now in the front-stash)");
        assert!(
            srx.try_recv().is_err(),
            "send-waiter must NOT be woken: no real capacity freed"
        );
        // Consume the stashed item → occupancy truly drops → sender wakes.
        assert_eq!(ch.try_recv().unwrap().map(|p| p.bytes), Some(vec![0xAA]));
        assert_eq!(ch.pending(), 0);
        assert!(
            srx.try_recv().is_ok(),
            "send-waiter woken once a slot genuinely freed"
        );
    }

    /// Regression: the bounce overshoot guard must also hold when the
    /// re-delivered item is sourced from the front-stash (not the crossbeam
    /// buffer). `drain_buffered_to_waiters` pops the stash via the
    /// non-waking `take_front_stash`; if it instead used the waking
    /// `pop_front_stash`, a sender would be woken even though the item
    /// bounces straight back to the stash (dead waiter) and never leaves the
    /// channel → cap+1.
    #[test]
    fn bounce_from_stash_does_not_overshoot_capacity_with_parked_sender() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(1)); // cap=1
                                                            // Dead recv-waiter parked while empty (register's own drain is a
                                                            // no-op), then cancelled — stays dead in the list.
        let (rid, _rrx) = synthetic::alloc();
        ch.register_recv_waiter(rid);
        assert!(synthetic::cancel(rid));
        // A fiber send-waiter parked, waiting for a slot.
        let (sid, mut srx) = synthetic::alloc();
        ch.register_send_waiter(sid);
        // Seed the item directly in the FRONT-STASH (not the buffer), so the
        // drain sources it from the stash and must bounce it back unchanged.
        ch.push_front_stash(Payload::bytes_only(vec![0xAA]));
        assert_eq!(ch.pending(), 1);
        // Drain: stash item is taken for the dead waiter and re-stashed. The
        // send-waiter must NOT be woken — occupancy is unchanged.
        ch.drain_buffered_to_waiters();
        assert_eq!(ch.pending(), 1, "exactly one item (still in the stash)");
        assert!(
            srx.try_recv().is_err(),
            "send-waiter must NOT be woken: stash bounce frees no real capacity"
        );
        // Consume the stashed item → occupancy truly drops → sender wakes.
        assert_eq!(ch.try_recv().unwrap().map(|p| p.bytes), Some(vec![0xAA]));
        assert_eq!(ch.pending(), 0);
        assert!(
            srx.try_recv().is_ok(),
            "send-waiter woken once the stashed item is genuinely consumed"
        );
    }

    /// Regression: a recv-waiter whose receiver was dropped (parked fiber
    /// torn down, sender still lingering) ahead of a LIVE sibling must not
    /// swallow the message — delivery falls through to the live sibling.
    /// Without `resolve_value`'s `is_closed` check the item was lost and
    /// the sibling starved.
    #[test]
    fn dropped_receiver_falls_through_to_live_sibling() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(2));
        let (id_dead, rx_dead) = synthetic::alloc();
        let (id_live, mut rx_live) = synthetic::alloc();
        ch.register_recv_waiter(id_dead);
        ch.register_recv_waiter(id_live);
        // Receiver torn down while its synthetic sender lingers, parked
        // ahead of the live sibling.
        drop(rx_dead);
        // Delivery must skip the dead id and reach the live sibling.
        ch.try_send(Payload::bytes_only(vec![0x11])).unwrap();
        let got = rx_live
            .try_recv()
            .expect("live sibling must receive the item, not lose it");
        assert!(got.success);
        assert_eq!(got.serialized_value_len, 1);
    }

    #[test]
    fn send_blocking_forever_wait_is_indefinite() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                ch.send_blocking(Payload::bytes_only(vec![2]), Wait::Forever)
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        let first = ch.try_recv().expect("drain first");
        assert_eq!(first.map(|p| p.bytes), Some(vec![1]));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if sender.is_finished() {
                break;
            }
            assert!(Instant::now() < deadline, "sender did not finish in time");
            std::thread::sleep(Duration::from_millis(20));
        }
        let res = sender.join().expect("sender thread");
        assert!(res.is_ok(), "got {res:?}");
    }

    // ─── FFI surface ──────────────────────────────────────────
    //
    // These tests drive the C-ABI entry points directly. The registry
    // is process-global (OnceLock), so `ensure_test_registry` is
    // idempotent and safe to call from every test — only the first
    // caller sets the config.

    fn ensure_test_registry() {
        use crate::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
        use crate::plugins::ox_shared::registry::init_registry;
        init_registry(SharedConfig {
            enabled: true,
            max_entries: 10_000,
            max_bytes: 1 << 30,
            soft_limit_ratio: 0.7,
            metrics_enabled: true,
            introspection_enabled: true,
            introspection_preview_enabled: true,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            max_value_size: 1 << 20,
            max_channel_bytes: 64 << 20,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        });
    }

    /// Free a malloc'd output buffer if non-null. Mirrors what the C
    /// side does via `oxphp_portable_free`.
    unsafe fn free_out(buf: *mut u8) {
        if !buf.is_null() {
            unsafe { libc::free(buf as *mut libc::c_void) };
        }
    }

    /// RAII guard that releases a test channel's registry entry on drop.
    /// Without this, the shared process-global registry can accumulate
    /// entries across tests and hit `max_entries` / `max_bytes` caps —
    /// `registry.rs` tests init with `max_bytes = 1024` (16 channels
    /// worth), and test ordering is non-deterministic.
    struct TestChannel(*const Entry);

    impl TestChannel {
        fn new(capacity: u64) -> Self {
            ensure_test_registry();
            let mut ptr: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_channel_create(capacity, &mut ptr) };
            assert_eq!(rc, 0, "create failed with rc={rc}");
            assert!(!ptr.is_null());
            Self(ptr)
        }

        fn entry(&self) -> *const Entry {
            self.0
        }

        fn id(&self) -> u64 {
            unsafe { crate::plugins::ox_shared::registry::oxphp_shared_entry_id(self.0) }
        }
    }

    impl Drop for TestChannel {
        fn drop(&mut self) {
            // SAFETY: self.0 was produced by oxphp_shared_channel_create
            // (= Arc::into_raw) and is dropped exactly once here.
            unsafe { crate::plugins::ox_shared::registry::oxphp_shared_handle_drop(self.0) };
        }
    }

    #[test]
    fn ffi_create_valid_id() {
        let ch = TestChannel::new(4);
        // Registry lookup must succeed and downcast to ChannelInner.
        let reg = crate::plugins::ox_shared::registry::registry();
        let entry = reg.lookup(ch.id()).expect("entry present");
        assert!(entry.inner.as_any_channel().is_some());
    }

    #[test]
    fn ffi_create_zero_capacity_errors() {
        ensure_test_registry();
        let mut ptr: *const Entry = std::ptr::null();
        let rc = unsafe { oxphp_shared_channel_create(0, &mut ptr) };
        assert_eq!(rc, SharedError::Type.code());
        // Registry must not have a fresh entry bound to this pointer —
        // create() left it NULL on the error path.
        assert!(ptr.is_null());
    }

    #[test]
    fn ffi_try_send_and_recv_roundtrip() {
        let ch = TestChannel::new(4);

        let payload = [1u8, 2, 3];
        let mut success: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_try_send(ch.entry(), payload.as_ptr(), payload.len(), &mut success)
        };
        assert_eq!(rc, 0);
        assert_eq!(success, 1);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 1;
        let rc = unsafe {
            oxphp_shared_channel_try_recv(ch.entry(), &mut out_buf, &mut out_len, &mut state)
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 0);
        assert_eq!(out_len, 3);
        let slice = unsafe { std::slice::from_raw_parts(out_buf, out_len) };
        assert_eq!(slice, &[1, 2, 3]);
        unsafe { free_out(out_buf) };
    }

    #[test]
    fn ffi_try_send_full_reports_zero_success() {
        let ch = TestChannel::new(1);

        let first = [0xAAu8];
        let mut success: c_int = 0;
        let rc =
            unsafe { oxphp_shared_channel_try_send(ch.entry(), first.as_ptr(), 1, &mut success) };
        assert_eq!(rc, 0);
        assert_eq!(success, 1);

        let second = [0xBBu8];
        let mut success2: c_int = 99;
        let rc =
            unsafe { oxphp_shared_channel_try_send(ch.entry(), second.as_ptr(), 1, &mut success2) };
        assert_eq!(rc, 0, "Full is not an error — rc must stay 0");
        assert_eq!(success2, 0);
    }

    #[test]
    fn ffi_try_send_on_closed_returns_closed() {
        let ch = TestChannel::new(4);
        assert_eq!(unsafe { oxphp_shared_channel_close(ch.entry()) }, 0);

        let payload = [9u8];
        let mut success: c_int = 1;
        let rc =
            unsafe { oxphp_shared_channel_try_send(ch.entry(), payload.as_ptr(), 1, &mut success) };
        assert_eq!(rc, SharedError::Closed.code());
    }

    #[test]
    fn ffi_try_recv_empty_open_state_1() {
        let ch = TestChannel::new(4);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 99;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_try_recv(ch.entry(), &mut out_buf, &mut out_len, &mut state)
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 1);
        assert!(out_buf.is_null());
        assert_eq!(out_len, 0);
    }

    #[test]
    fn ffi_try_recv_closed_empty_state_2() {
        let ch = TestChannel::new(4);
        assert_eq!(unsafe { oxphp_shared_channel_close(ch.entry()) }, 0);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 99;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_try_recv(ch.entry(), &mut out_buf, &mut out_len, &mut state)
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 2);
        assert!(out_buf.is_null());
    }

    #[test]
    fn ffi_close_is_idempotent() {
        let ch = TestChannel::new(4);
        assert_eq!(unsafe { oxphp_shared_channel_close(ch.entry()) }, 0);
        assert_eq!(unsafe { oxphp_shared_channel_close(ch.entry()) }, 0);
    }

    #[test]
    fn ffi_is_closed_reports_state() {
        let ch = TestChannel::new(4);

        let mut out: c_int = 99;
        assert_eq!(
            unsafe { oxphp_shared_channel_is_closed(ch.entry(), &mut out) },
            0
        );
        assert_eq!(out, 0);

        assert_eq!(unsafe { oxphp_shared_channel_close(ch.entry()) }, 0);
        assert_eq!(
            unsafe { oxphp_shared_channel_is_closed(ch.entry(), &mut out) },
            0
        );
        assert_eq!(out, 1);
    }

    #[test]
    fn ffi_pending_returns_count() {
        let ch = TestChannel::new(4);

        for b in 0u8..2 {
            let payload = [b];
            let mut success: c_int = 0;
            assert_eq!(
                unsafe {
                    oxphp_shared_channel_try_send(ch.entry(), payload.as_ptr(), 1, &mut success)
                },
                0
            );
            assert_eq!(success, 1);
        }

        let mut out: u64 = 99;
        assert_eq!(
            unsafe { oxphp_shared_channel_pending(ch.entry(), &mut out) },
            0
        );
        assert_eq!(out, 2);
    }

    #[test]
    fn ffi_send_blocking_succeeds_on_vacancy() {
        let ch = TestChannel::new(4);

        let payload = [42u8];
        let rc =
            unsafe { oxphp_shared_channel_send_blocking(ch.entry(), payload.as_ptr(), 1, 100) };
        assert_eq!(rc, 0);
    }

    #[test]
    fn ffi_send_blocking_times_out() {
        let ch = TestChannel::new(1);

        let first = [1u8];
        let mut success: c_int = 0;
        assert_eq!(
            unsafe { oxphp_shared_channel_try_send(ch.entry(), first.as_ptr(), 1, &mut success) },
            0
        );

        let second = [2u8];
        let rc = unsafe { oxphp_shared_channel_send_blocking(ch.entry(), second.as_ptr(), 1, 50) };
        assert_eq!(rc, SharedError::Timeout.code());
    }

    #[test]
    fn ffi_recv_blocking_returns_item() {
        let ch = TestChannel::new(4);

        let payload = [7u8, 8, 9];
        let mut success: c_int = 0;
        assert_eq!(
            unsafe { oxphp_shared_channel_try_send(ch.entry(), payload.as_ptr(), 3, &mut success) },
            0
        );

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 2;
        let rc = unsafe {
            oxphp_shared_channel_recv_blocking(
                ch.entry(),
                100,
                &mut out_buf,
                &mut out_len,
                &mut state,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 0);
        assert_eq!(out_len, 3);
        let slice = unsafe { std::slice::from_raw_parts(out_buf, out_len) };
        assert_eq!(slice, &[7, 8, 9]);
        unsafe { free_out(out_buf) };
    }

    #[test]
    fn ffi_recv_blocking_closed_empty_state_2() {
        let ch = TestChannel::new(4);
        assert_eq!(unsafe { oxphp_shared_channel_close(ch.entry()) }, 0);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 99;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_recv_blocking(
                ch.entry(),
                -1,
                &mut out_buf,
                &mut out_len,
                &mut state,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 2);
        assert!(out_buf.is_null());
    }

    #[test]
    fn ffi_recv_blocking_times_out() {
        let ch = TestChannel::new(4);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_recv_blocking(
                ch.entry(),
                50,
                &mut out_buf,
                &mut out_len,
                &mut state,
            )
        };
        assert_eq!(rc, SharedError::Timeout.code());
    }

    // ─── Wait::Try coverage ──────────────────────────────────

    #[test]
    fn send_blocking_try_returns_timeout_when_full() {
        let ch = ChannelInner::new(1);
        ch.try_send(Payload::bytes_only(vec![0])).unwrap();
        let res = ch.send_blocking(Payload::bytes_only(vec![1]), Wait::Try);
        assert!(matches!(res, Err(SharedError::Timeout)), "got {res:?}");
        // Gauge must not have been incremented — Try short-circuits before the guard.
        assert_eq!(ch.senders_blocked().load(Ordering::Relaxed), 0);
    }

    #[test]
    fn recv_blocking_try_returns_timeout_when_empty() {
        let ch = ChannelInner::new(4);
        let res = ch.recv_blocking(Wait::Try);
        assert!(matches!(res, Err(SharedError::Timeout)), "got {res:?}");
        assert_eq!(ch.receivers_blocked().load(Ordering::Relaxed), 0);
    }

    #[test]
    fn ffi_send_blocking_try_returns_timeout_when_full() {
        let ch = TestChannel::new(1);
        let first = [1u8];
        let mut success: c_int = 0;
        let _ =
            unsafe { oxphp_shared_channel_try_send(ch.entry(), first.as_ptr(), 1, &mut success) };
        let second = [2u8];
        let rc = unsafe { oxphp_shared_channel_send_blocking(ch.entry(), second.as_ptr(), 1, 0) };
        assert_eq!(rc, SharedError::Timeout.code());
    }

    // Verify the symmetric success paths: Wait::Try (timeout_ms == 0) on a
    // non-full channel must succeed, and on a non-empty channel must return
    // the item. These guard the fix for the fiber-path bug where the bail ran
    // BEFORE any try_send/try_recv attempt, causing spurious Timeout/null on
    // channels that had capacity or items available.
    #[test]
    fn send_blocking_try_succeeds_when_not_full() {
        let ch = ChannelInner::new(2);
        // Channel is empty — Wait::Try must succeed, not timeout.
        let res = ch.send_blocking(Payload::bytes_only(vec![0xAA]), Wait::Try);
        assert!(res.is_ok(), "expected Ok, got {res:?}");
        assert_eq!(ch.pending(), 1);
    }

    #[test]
    fn recv_blocking_try_succeeds_when_not_empty() {
        let ch = ChannelInner::new(2);
        ch.try_send(Payload::bytes_only(vec![0xBB])).unwrap();
        // Channel has an item — Wait::Try must return it, not Timeout.
        let res = ch.recv_blocking(Wait::Try);
        assert!(
            matches!(res, Ok(Some(_))),
            "expected Ok(Some(_)), got {res:?}"
        );
        if let Ok(Some(payload)) = res {
            assert_eq!(payload, vec![0xBB]);
        }
    }

    #[test]
    fn ffi_send_blocking_try_succeeds_when_not_full() {
        let ch = TestChannel::new(4);
        let payload = [0xCCu8];
        // timeout_ms == 0 → Wait::Try; channel has vacancy → must succeed.
        let rc = unsafe { oxphp_shared_channel_send_blocking(ch.entry(), payload.as_ptr(), 1, 0) };
        assert_eq!(rc, 0, "expected 0 (success), got {rc}");
    }

    #[test]
    fn ffi_recv_blocking_try_succeeds_when_not_empty() {
        let ch = TestChannel::new(4);
        let payload = [0xDDu8];
        let mut success: c_int = 0;
        let _ =
            unsafe { oxphp_shared_channel_try_send(ch.entry(), payload.as_ptr(), 1, &mut success) };
        assert_eq!(success, 1);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 1;
        // timeout_ms == 0 → Wait::Try; channel has an item → must return it.
        let rc = unsafe {
            oxphp_shared_channel_recv_blocking(
                ch.entry(),
                0,
                &mut out_buf,
                &mut out_len,
                &mut state,
            )
        };
        assert_eq!(rc, 0, "expected 0, got {rc}");
        assert_eq!(state, 0, "expected state=0 (item), got {state}");
        assert_eq!(out_len, 1);
        let slice = unsafe { std::slice::from_raw_parts(out_buf, out_len) };
        assert_eq!(slice, &[0xDDu8]);
        unsafe { free_out(out_buf) };
    }

    // ─── batched send_many / recv_many ───────────────────────

    #[test]
    fn send_many_fills_channel_and_returns_count() {
        let ch = ChannelInner::new(10);
        let payloads: Vec<Payload> = (0u8..5).map(|i| Payload::bytes_only(vec![i])).collect();
        let sent = ch.send_many(payloads, Wait::Forever);
        assert_eq!(sent, 5);
        assert_eq!(ch.pending(), 5);
    }

    #[test]
    fn send_many_stops_on_closed() {
        let ch = ChannelInner::new(10);
        ch.close();
        let payloads: Vec<Payload> = (0u8..5).map(|i| Payload::bytes_only(vec![i])).collect();
        let sent = ch.send_many(payloads, Wait::Forever);
        assert_eq!(sent, 0);
    }

    #[test]
    fn send_many_partial_on_timeout() {
        // Capacity 2; push 5 with 50ms timeout → only first 2 fit, rest
        // time out and send_many returns the running count.
        let ch = ChannelInner::new(2);
        let payloads: Vec<Payload> = (0u8..5).map(|i| Payload::bytes_only(vec![i])).collect();
        let sent = ch.send_many(payloads, Wait::Bounded(Duration::from_millis(50)));
        assert_eq!(sent, 2);
        assert_eq!(ch.pending(), 2);
    }

    #[test]
    fn recv_many_drain_max_zero() {
        let ch = ChannelInner::new(10);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        ch.try_send(Payload::bytes_only(vec![2])).unwrap();
        ch.try_send(Payload::bytes_only(vec![3])).unwrap();
        let got = ch.recv_many(0, Wait::Forever);
        assert_eq!(got, vec![vec![1], vec![2], vec![3]]);
        assert_eq!(ch.pending(), 0);
    }

    #[test]
    fn recv_many_drain_on_empty_returns_empty() {
        let ch = ChannelInner::new(10);
        let got = ch.recv_many(0, Wait::Forever);
        assert!(got.is_empty());
    }

    #[test]
    fn recv_many_respects_max() {
        let ch = ChannelInner::new(10);
        for i in 0u8..5 {
            ch.try_send(Payload::bytes_only(vec![i])).unwrap();
        }
        let got = ch.recv_many(3, Wait::Bounded(Duration::from_millis(50)));
        assert_eq!(got, vec![vec![0], vec![1], vec![2]]);
        assert_eq!(ch.pending(), 2);
    }

    #[test]
    fn recv_many_stops_on_closed_empty() {
        let ch = ChannelInner::new(10);
        ch.try_send(Payload::bytes_only(vec![7])).unwrap();
        ch.close();
        let got = ch.recv_many(10, Wait::Bounded(Duration::from_millis(50)));
        // Got the buffered item and then stopped on closed+empty; no
        // timeout wait because recv_blocking returned Ok(None).
        assert_eq!(got, vec![vec![7]]);
    }

    #[test]
    fn recv_many_timeout_returns_partial() {
        let ch = ChannelInner::new(10);
        ch.try_send(Payload::bytes_only(vec![1])).unwrap();
        let start = Instant::now();
        let got = ch.recv_many(5, Wait::Bounded(Duration::from_millis(50)));
        let elapsed = start.elapsed();
        assert_eq!(got, vec![vec![1]]);
        // Must have waited ~50ms for the remaining 4 slots.
        assert!(
            elapsed >= Duration::from_millis(40),
            "recv_many returned too fast: {elapsed:?}"
        );
    }

    // ─── fiber-register FFI (tokio) ───────────────────────────
    //
    // `alloc_and_register` requires a PHP thread context that unit
    // tests lack, so these tests exercise the waker-list wiring
    // directly by parking an `alloc()`-allocated promise. The FFI-level
    // `spawn_fiber_timeout` / `*out_promise_id` bookkeeping around that
    // is validated in the PHP-side integration tests.

    #[tokio::test]
    async fn ffi_recv_fiber_register_resolves_on_send() {
        let ch_handle = TestChannel::new(4);
        let id = ch_handle.id();
        let entry_ptr = ch_handle.entry();

        let (pid, rx) = synthetic::alloc();
        // Scope the entry lookup so the Arc<Entry> is not held across
        // the await below.
        {
            let reg = crate::plugins::ox_shared::registry::registry();
            let entry = reg.lookup(id).expect("entry");
            let ch = entry.inner.as_any_channel().expect("channel");
            ch.register_recv_waiter(pid);
        }

        // Send from the main task — FFI path, exercises try_send under
        // a production-shaped entry point.
        let send_payload = [0xABu8, 0xCD];
        let mut success: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_try_send(entry_ptr, send_payload.as_ptr(), 2, &mut success)
        };
        assert_eq!(rc, 0);
        assert_eq!(success, 1);

        let got = rx.await.expect("receiver should fire");
        assert!(got.success);
        assert_eq!(got.serialized_value_len, 2);
        // NOTE: `AsyncResult::Drop` frees `got.serialized_value` via
        // `libc::free`. Do NOT free it here — double-free trips the
        // SIGTRAP that earlier versions of this test hit.
    }

    #[tokio::test]
    async fn ffi_send_fiber_register_resolves_on_slot_free() {
        let ch_handle = TestChannel::new(1);
        let id = ch_handle.id();
        let entry_ptr = ch_handle.entry();

        // Fill the channel so a send-waiter can park.
        let payload = [1u8];
        let mut success: c_int = 0;
        assert_eq!(
            unsafe { oxphp_shared_channel_try_send(entry_ptr, payload.as_ptr(), 1, &mut success) },
            0
        );
        assert_eq!(success, 1);

        let (pid, rx) = synthetic::alloc();
        let reg = crate::plugins::ox_shared::registry::registry();
        let entry = reg.lookup(id).expect("entry");
        let ch = entry.inner.as_any_channel().expect("channel");
        ch.register_send_waiter(pid);

        // Free a slot via try_recv — this wakes the parked sender.
        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 1;
        let rc = unsafe {
            oxphp_shared_channel_try_recv(entry_ptr, &mut out_buf, &mut out_len, &mut state)
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 0);
        unsafe { free_out(out_buf) };

        let got = rx.await.expect("receiver should fire");
        assert!(got.success);
        // Empty-Value ack = "slot free, retry your send".
        assert_eq!(got.serialized_value_len, 0);
        assert!(got.serialized_value.is_null());
    }

    /// Pins the contract that `try_send` / `try_recv` propagate per-
    /// payload growth through `adjust_mem_bytes` via the Channel's
    /// `bump_pending` / `drop_pending` helpers. Counterpart to
    /// `map_set_grows_registry_entry_bytes` in map.rs.
    ///
    /// Asserts against `Entry::mem_bytes`, not registry-global
    /// `total_bytes` — see `SharedRegistry::total_bytes` for why.
    #[test]
    fn channel_send_recv_track_registry_entry_bytes() {
        ensure_test_registry();
        let reg = registry();

        let inner: Arc<dyn SharedInner> = Arc::new(ChannelInner::new(8));
        let entry = reg
            .insert(SharedType::Channel, Arc::clone(&inner))
            .expect("registry insert should succeed");
        let id = entry.id;
        let ch = (*inner).as_any_channel().expect("downcast");
        ch.bind_id(id);

        let baseline = entry.mem_bytes.load(Ordering::Relaxed);
        for i in 0..4u8 {
            ch.try_send(Payload::bytes_only(vec![i]))
                .expect("buffer has room");
        }
        let grown = entry.mem_bytes.load(Ordering::Relaxed);
        assert_eq!(
            grown - baseline,
            (4 * CHANNEL_PER_PAYLOAD_BYTES) as usize,
            "entry mem_bytes must grow by 4 × CHANNEL_PER_PAYLOAD_BYTES"
        );

        for _ in 0..4 {
            ch.try_recv()
                .expect("not empty")
                .expect("not closed-and-empty");
        }
        assert_eq!(
            entry.mem_bytes.load(Ordering::Relaxed),
            baseline,
            "entry mem_bytes must return to baseline after symmetric recvs"
        );

        drop(entry);
    }
}
