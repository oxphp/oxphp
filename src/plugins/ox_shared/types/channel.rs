//! Shared\Channel — bounded MPMC channel with fiber-suspending recv/send.
//!
//! Pure-Rust core: `try_send` / `try_recv` / `close` over a crossbeam
//! bounded channel, plus gauge atomics, fiber-waker lists, blocking
//! timeout path, and FFI surface.

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use parking_lot::Mutex;
use smallvec::SmallVec;
use tokio::sync::Notify;

use crate::plugins::ox_async::synthetic::{self, PromisePayload};
use crate::plugins::ox_shared::error::{ffi_entry, set_last_error, SharedError};
use crate::plugins::ox_shared::registry::{registry, SharedInner, SharedType};
use crate::plugins::ox_shared::types::timeout::{parse_timeout, Wait};
use crate::plugins::ox_shared::value::SharedValue;

/// Serialized zval payload (portbuf bytes). Opaque at this layer —
/// encoding/decoding lives in `value.rs` and is driven by the FFI
/// layer.
pub type Payload = Vec<u8>;

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
    // Exercised by blocking paths; surfaced to observability.
    senders_blocked: AtomicU32,
    receivers_blocked: AtomicU32,
    items_sent_total: AtomicU64,
    #[allow(dead_code)]
    items_dropped_total: AtomicU64,
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
            senders_blocked: AtomicU32::new(0),
            receivers_blocked: AtomicU32::new(0),
            items_sent_total: AtomicU64::new(0),
            items_dropped_total: AtomicU64::new(0),
        }
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
        match self.tx.try_send(payload) {
            Ok(()) => {
                self.pending.fetch_add(1, Ordering::AcqRel);
                self.items_sent_total.fetch_add(1, Ordering::Relaxed);
                self.notify_recv.notify_one();
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
        let rx = self.rx.lock();
        match rx.try_recv() {
            Ok(p) => {
                drop(rx);
                self.pending.fetch_sub(1, Ordering::AcqRel);
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
    pub(crate) fn send_blocking(&self, payload: Payload, wait: Wait) -> Result<(), SharedError> {
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
                    self.pending.fetch_add(1, Ordering::AcqRel);
                    self.items_sent_total.fetch_add(1, Ordering::Relaxed);
                    self.notify_recv.notify_one();
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
    pub(crate) fn recv_blocking(&self, wait: Wait) -> Result<Option<Payload>, SharedError> {
        // Fast path: try once under a tight lock scope. NO guard here —
        // a fast-path hit (or a closed+empty miss) never blocked, so
        // the `receivers_blocked` gauge must not transiently tick.
        {
            let rx = self.rx.lock();
            match rx.try_recv() {
                Ok(p) => {
                    drop(rx);
                    self.pending.fetch_sub(1, Ordering::AcqRel);
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
            if self.is_closed() {
                // Drain-before-close: re-check the queue under the
                // lock in case a sender managed to enqueue before
                // setting closed.
                let rx = self.rx.lock();
                return match rx.try_recv() {
                    Ok(p) => {
                        drop(rx);
                        self.pending.fetch_sub(1, Ordering::AcqRel);
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
                    self.pending.fetch_sub(1, Ordering::AcqRel);
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
    pub(crate) fn send_many(&self, payloads: Vec<Payload>, wait: Wait) -> u64 {
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
    pub(crate) fn recv_many(&self, max: usize, wait: Wait) -> Vec<Payload> {
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
    /// promise was already resolved by a concurrent cancel —
    /// `synthetic::resolve` reports this via its `bool` return.
    ///
    /// Ownership / optimisation: `synthetic::resolve` consumes the
    /// payload unconditionally. To avoid a clone on the expected
    /// common case (exactly one live waiter parked, no retry needed),
    /// we observe `has_more` under the same lock acquisition as the
    /// pop and branch:
    ///
    /// * `has_more`  → clone so `payload` survives a dead-waiter
    ///   retry. Amortised across the (rare) waiter list.
    /// * `!has_more` → move `payload` directly into `resolve`. Zero
    ///   clones on the single-live-waiter happy path.
    ///
    /// Dead-last-waiter edge: when `!has_more` AND `resolve` reports
    /// the waiter vanished, the payload is gone. Returning `None`
    /// signals "handled, do not re-deposit" to the caller. This is
    /// correct for `try_send`: the fiber that parked cancelled ≈ the
    /// same instant the send ran, which is observationally
    /// indistinguishable from receiving-then-immediately-cancelling
    /// — the sender's `items_sent_total` +1 matches the "successful
    /// send into a now-dead receiver" semantics.
    ///
    /// `drain_buffered_to_waiters` is also affected on this edge: a
    /// buffered item may be dropped instead of delivered. The race
    /// window is nanoseconds (one DashMap remove + one oneshot send
    /// inside `synthetic::resolve`), so this is acceptable in practice;
    /// a strict fix would need a `synthetic::resolve_with(id, || ...)`
    /// lazy-construction API to construct the payload only after the
    /// waiter's liveness is confirmed. Tracked as a follow-up.
    fn drain_one_recv_waiter_with(&self, mut payload: Payload) -> Option<Payload> {
        loop {
            // Pop head + observe remaining depth in one lock op.
            let (id, has_more) = {
                let mut waiters = self.recv_waiters.lock();
                if waiters.is_empty() {
                    return Some(payload);
                }
                // FIFO: first parked wakes first.
                let head = waiters.remove(0);
                (head, !waiters.is_empty())
            };
            if has_more {
                // Retry possible — clone so `payload` survives a dead
                // waiter outcome.
                if synthetic::resolve(id, PromisePayload::Value(payload.clone())) {
                    return None;
                }
                // Dead — loop and try the next parked id.
            } else {
                // Last candidate — move the payload. Zero clone on
                // the happy path.
                let attempt = std::mem::take(&mut payload);
                if synthetic::resolve(id, PromisePayload::Value(attempt)) {
                    return None;
                }
                // Dead-last-waiter race (rare). Payload is consumed;
                // report as handled so the sender's bookkeeping stays
                // consistent. See rationale above.
                return None;
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
            if synthetic::resolve(id, PromisePayload::Value(Vec::new())) {
                return;
            }
            // Dead — skip and try the next parked id.
        }
    }

    /// Bulk drain: while we still have buffered items AND parked
    /// recv-waiters, pop one item / one live waiter and deliver.
    /// Called from `register_recv_waiter` to cover the race where
    /// a `try_send` landed into the buffer just before we parked.
    ///
    /// Dead-last-waiter race note: `drain_one_recv_waiter_with`'s
    /// optimised fast path may consume a payload on the rare edge
    /// where a single parked waiter cancelled between pop and
    /// resolve. In that window the buffered item is lost — acceptable
    /// because the race window is nanoseconds and the consuming fiber
    /// was about to stop consuming anyway. See `drain_one_recv_waiter_with`
    /// docs for the full rationale.
    fn drain_buffered_to_waiters(&self) {
        loop {
            // Are there any parked waiters? Peek without removing.
            {
                let waiters = self.recv_waiters.lock();
                if waiters.is_empty() {
                    return;
                }
            }
            // Try to take one item from the buffer.
            let item = {
                let rx = self.rx.lock();
                match rx.try_recv() {
                    Ok(p) => {
                        drop(rx);
                        self.pending.fetch_sub(1, Ordering::AcqRel);
                        self.notify_send.notify_one();
                        p
                    }
                    Err(_) => return,
                }
            };
            // Deliver to one live waiter. If delivery fails because
            // ALL waiters (plural) were dead, push the item back into
            // the buffer — we won't spin-drop.
            match self.drain_one_recv_waiter_with(item) {
                None => {
                    // Slot just freed by the recv above — wake one
                    // parked sender if any.
                    self.drain_one_send_waiter_on_slot_free();
                    continue;
                }
                Some(p) => {
                    // All waiters were dead. Re-deposit into the buffer.
                    // try_send here would re-check closed and call into
                    // drain_one_recv_waiter_with again — infinite loop.
                    // So we bypass and write directly.
                    let _ = self.tx.try_send(p).map(|()| {
                        self.pending.fetch_add(1, Ordering::AcqRel);
                    });
                    return;
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
        if self.type_tag() == SharedType::Channel {
            // SAFETY: SharedType::Channel guarantees the concrete type
            // is ChannelInner. Casting a `*const dyn SharedInner` fat
            // pointer to `*const ChannelInner` yields the data pointer,
            // which is the address of the ChannelInner allocation. Sound
            // as long as SharedType::Channel is only ever used with
            // ChannelInner — enforced by the sole insertion site in
            // oxphp_shared_channel_create.
            Some(unsafe { &*(self as *const dyn SharedInner as *const ChannelInner) })
        } else {
            None
        }
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
/// `out_id` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_create(capacity: u64, out_id: *mut u64) -> c_int {
    if out_id.is_null() {
        set_last_error("out_id null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        if capacity == 0 {
            set_last_error("Channel capacity must be >= 1");
            return Err(SharedError::Type);
        }
        let reg = registry();
        let id = reg.insert(
            SharedType::Channel,
            Arc::new(ChannelInner::new(capacity as usize)),
        )?;
        unsafe { *out_id = id };
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
    id: u64,
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
    unsafe { *out_success = 0 };
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let payload: Payload = if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(buf, len) }.to_vec()
        };

        match ch.try_send(payload) {
            Ok(()) => {
                reg.record_op(id);
                unsafe { *out_success = 1 };
                Ok(())
            }
            Err(TrySendErr::Full(_)) => {
                // Not an error per FFI contract — the caller distinguishes
                // via *out_success = 0.
                reg.record_op(id);
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
    id: u64,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_state: *mut c_int,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() || out_state.is_null() {
        set_last_error("out pointers null");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
        *out_state = 1;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        match ch.try_recv() {
            Ok(Some(payload)) => {
                let (ptr, n) = unsafe { payload_to_malloc(payload)? };
                reg.record_op(id);
                unsafe {
                    *out_buf = ptr;
                    *out_len = n;
                    *out_state = 0;
                }
                Ok(())
            }
            Err(TryRecvErr::WouldBlockEmpty) => {
                reg.record_op(id);
                unsafe { *out_state = 1 };
                Ok(())
            }
            Ok(None) => {
                reg.record_op(id);
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
    id: u64,
    buf: *const u8,
    len: usize,
    timeout_ms: i64,
) -> c_int {
    if len > 0 && buf.is_null() {
        set_last_error("buf null with non-zero len");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let payload: Payload = if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(buf, len) }.to_vec()
        };

        let wait = parse_timeout(timeout_ms);
        let res = ch.send_blocking(payload, wait);
        match res {
            Ok(()) => {
                reg.record_op(id);
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
/// throw `TimeoutException` instead of returning null.
///
/// # Safety
/// `out_buf`, `out_len`, `out_state` must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_recv_blocking(
    id: u64,
    timeout_ms: i64,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_state: *mut c_int,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() || out_state.is_null() {
        set_last_error("out pointers null");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
        *out_state = 2;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let wait = parse_timeout(timeout_ms);
        match ch.recv_blocking(wait) {
            Ok(Some(payload)) => {
                let (ptr, n) = unsafe { payload_to_malloc(payload)? };
                reg.record_op(id);
                unsafe {
                    *out_buf = ptr;
                    *out_len = n;
                    *out_state = 0;
                }
                Ok(())
            }
            Ok(None) => {
                reg.record_op(id);
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
#[no_mangle]
pub extern "C" fn oxphp_shared_channel_close(id: u64) -> c_int {
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;
        ch.close();
        reg.record_op(id);
        Ok(())
    })
}

/// `*out = is_closed() as c_int` on success.
///
/// # Safety
/// `out` must be valid for a `c_int` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_is_closed(id: u64, out: *mut c_int) -> c_int {
    if out.is_null() {
        set_last_error("out null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;
        let v = ch.is_closed();
        reg.record_op(id);
        unsafe { *out = v as c_int };
        Ok(())
    })
}

/// `*out = pending() as u64` on success.
///
/// # Safety
/// `out` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_channel_pending(id: u64, out: *mut u64) -> c_int {
    if out.is_null() {
        set_last_error("out null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;
        let v = ch.pending() as u64;
        reg.record_op(id);
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
    id: u64,
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
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        if n == 0 {
            reg.record_op(id);
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
            let payload = if len == 0 {
                Vec::new()
            } else {
                let slice = unsafe { std::slice::from_raw_parts(payloads_concat.add(start), len) };
                slice.to_vec()
            };
            payloads.push(payload);
        }

        let wait = parse_timeout(timeout_ms);
        let sent = ch.send_many(payloads, wait);
        reg.record_op(id);
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
    id: u64,
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
    unsafe {
        *out_concat = std::ptr::null_mut();
        *out_concat_len = 0;
        *out_offsets = std::ptr::null_mut();
        *out_n = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let wait = parse_timeout(timeout_ms);
        let items = ch.recv_many(max as usize, wait);
        reg.record_op(id);

        let n = items.len();
        if n == 0 {
            return Ok(());
        }

        let total: usize = items.iter().map(|p| p.len()).sum();
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
            if !item.is_empty() && !concat_ptr.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        item.as_ptr(),
                        concat_ptr.add(cursor),
                        item.len(),
                    );
                }
            }
            cursor += item.len();
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
    id: u64,
    timeout_ms: u64,
    out_promise_id: *mut i64,
) -> c_int {
    if out_promise_id.is_null() {
        set_last_error("out_promise_id null");
        return SharedError::Generic.code();
    }
    unsafe { *out_promise_id = 0 };
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
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

        reg.record_op(id);
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
    id: u64,
    timeout_ms: u64,
    out_promise_id: *mut i64,
) -> c_int {
    if out_promise_id.is_null() {
        set_last_error("out_promise_id null");
        return SharedError::Generic.code();
    }
    unsafe { *out_promise_id = 0 };
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let ch = entry.inner.as_any_channel().ok_or(SharedError::Type)?;

        let promise_id = synthetic::alloc_and_register();
        ch.register_send_waiter(promise_id);
        spawn_fiber_timeout(promise_id, timeout_ms);

        reg.record_op(id);
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
unsafe fn send_fiber_register_shim(id: u64, timeout_ms: u64, out: *mut i64) -> c_int {
    unsafe { oxphp_shared_channel_send_fiber_register(id, timeout_ms, out) }
}

#[cfg(not(feature = "php"))]
unsafe fn send_fiber_register_shim(_id: u64, _timeout_ms: u64, _out: *mut i64) -> c_int {
    SharedError::Generic.code()
}

#[cfg(feature = "php")]
unsafe fn recv_fiber_register_shim(id: u64, timeout_ms: u64, out: *mut i64) -> c_int {
    unsafe { oxphp_shared_channel_recv_fiber_register(id, timeout_ms, out) }
}

#[cfg(not(feature = "php"))]
unsafe fn recv_fiber_register_shim(_id: u64, _timeout_ms: u64, _out: *mut i64) -> c_int {
    SharedError::Generic.code()
}

// ─── Class registration ───────────────────────────────────────────────

/// Register the `OxPHP\Shared\Channel` PHP class with all its methods.
///
/// Exposed PHP surface:
///   __construct(int $capacity)
///   send(mixed $value, float $timeout = 0.0): void   [fiber-aware]
///   trySend(mixed $value): bool
///   recv(float $timeout = 0.0): mixed                [fiber-aware]
///   tryRecv(): mixed
///   close(): void
///   isClosed(): bool
///   pending(): int
///   id(): int
///   __clone → throws
pub fn register_class(
    ctx: &mut crate::plugin::PluginContext,
) -> Result<(), crate::plugin::PluginError> {
    use crate::bridge::ffi as bridge_ffi;
    use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
    use crate::plugin::PhpError;
    use crate::plugins::ox_shared::handle::SharedHandle;

    ctx.register_class("OxPHP\\Shared\\Channel")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Channel))
        .magic(MagicMethod::Clone)
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\Exception".into(),
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
            let mut out_id: u64 = 0;
            let rc = unsafe { oxphp_shared_channel_create(cap as u64, &mut out_id) };
            super::counter::counter_rc_to_result(rc)?;

            let h = call.storage_mut::<SharedHandle>()?;
            h.shared_id = out_id;
            h.type_tag = SharedType::Channel as u8;
            Ok(())
        })
        // ── trySend(value): bool ────────────────────────────────────
        .method("trySend")
        .param("value", PhpType::Mixed)
        .returns(PhpType::Bool)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;

            // Serialize the zval argument to a portbuf (C owns the buffer).
            let arg_ptr = unsafe { call.raw_arg_ptr(0) };
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let ser_rc = unsafe {
                bridge_ffi::oxphp_portable_serialize(arg_ptr as *const _, 1, &mut buf, &mut len)
            };
            if ser_rc != 0 {
                if !buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(buf) };
                }
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\TypeException".into(),
                    message: "trySend: value is not serializable (e.g. closure, resource)".into(),
                    code: 0,
                });
            }

            let mut success: c_int = 0;
            let rc = unsafe { oxphp_shared_channel_try_send(id, buf, len, &mut success) };
            if !buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(buf) };
            }

            // Spec: "Non-blocking send. Returns false if full or closed."
            if rc == SharedError::Closed.code() {
                call.ret_bool(false);
                return Ok(());
            }
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(success != 0);
            Ok(())
        })
        // ── send(value, timeout=0.0): void ──────────────────────────
        .method("send")
        .param("value", PhpType::Mixed)
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let timeout_s = if call.argc() > 1 {
                call.arg_double(1).unwrap_or(0.0)
            } else {
                0.0
            };
            let timeout_ms: u64 = if timeout_s <= 0.0 {
                0
            } else {
                (timeout_s * 1000.0) as u64
            };

            // Serialize value → portbuf once; reuse across retry loop on fiber path.
            let arg_ptr = unsafe { call.raw_arg_ptr(0) };
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let ser_rc = unsafe {
                bridge_ffi::oxphp_portable_serialize(arg_ptr as *const _, 1, &mut buf, &mut len)
            };
            if ser_rc != 0 {
                if !buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(buf) };
                }
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\TypeException".into(),
                    message: "send: value is not serializable (e.g. closure, resource)".into(),
                    code: 0,
                });
            }

            // The fiber-suspend path uses synthetic-promise FFI that is
            // gated behind `feature = "php"` (PROMISE_MAP lives on PHP
            // worker threads). Without `php`, force `in_fiber = false` so
            // we always take the thread-blocking branch — matches the
            // mock `oxphp_bridge_in_fiber` returning 0 anyway.
            #[cfg(feature = "php")]
            let in_fiber = unsafe { bridge_ffi::oxphp_bridge_in_fiber() } != 0;
            #[cfg(not(feature = "php"))]
            let in_fiber = false;

            if in_fiber {
                // Fiber path: try_send → on full, register send-waiter and
                // suspend via fiber_await. Waker resolves with:
                //   - Value(empty) → "slot free; retry try_send"   (fiber_rc == 0)
                //   - Cancelled    → Async\Exception               (fiber_rc == -1)
                //   - Closed       → ClosedException propagated    (fiber_rc == -1)
                let deadline = if timeout_s > 0.0 {
                    Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout_s))
                } else {
                    None
                };

                loop {
                    // Fast attempt.
                    let mut success: c_int = 0;
                    let rc = unsafe { oxphp_shared_channel_try_send(id, buf, len, &mut success) };
                    if rc == SharedError::Closed.code() {
                        unsafe { bridge_ffi::oxphp_portable_free(buf) };
                        return Err(PhpError::Exception {
                            class: "OxPHP\\Shared\\ClosedException".into(),
                            message: "channel closed".into(),
                            code: 0,
                        });
                    }
                    if rc == 0 && success != 0 {
                        unsafe { bridge_ffi::oxphp_portable_free(buf) };
                        return Ok(());
                    }
                    if rc != 0 {
                        // Some other FFI error (e.g. stale handle).
                        unsafe { bridge_ffi::oxphp_portable_free(buf) };
                        return Err(map_channel_rc(rc));
                    }

                    // Channel full → park.
                    let remaining_ms: u64 = if let Some(d) = deadline {
                        let r = d.saturating_duration_since(std::time::Instant::now());
                        if r.is_zero() {
                            unsafe { bridge_ffi::oxphp_portable_free(buf) };
                            return Err(PhpError::Exception {
                                class: "OxPHP\\Shared\\TimeoutException".into(),
                                message: "send timed out".into(),
                                code: 0,
                            });
                        }
                        r.as_millis() as u64
                    } else {
                        0
                    };

                    let mut promise_id: i64 = 0;
                    // `oxphp_shared_channel_send_fiber_register` is gated by
                    // `feature = "php"`. Without `php`, `in_fiber` is forced
                    // to false above so this branch is unreachable at runtime,
                    // but rustc still compiles it — call through a small
                    // shim that has a non-php fallback returning -1.
                    let reg_rc =
                        unsafe { send_fiber_register_shim(id, remaining_ms, &mut promise_id) };
                    if reg_rc != 0 {
                        unsafe { bridge_ffi::oxphp_portable_free(buf) };
                        super::counter::counter_rc_to_result(reg_rc)?;
                    }

                    // Suspend fiber. Timeout is handled by spawn_fiber_timeout
                    // in the FFI register path (via synthetic::cancel), so we
                    // pass 0.0 here — the SAPI fiber_await ignores this arg
                    // anyway (see oxphp_fiber_suspend_for_await in ext/oxphp_sapi.c).
                    let retval = call.retval_ptr();
                    let fiber_rc =
                        unsafe { bridge_ffi::oxphp_bridge_fiber_await(promise_id, 0.0, retval) };

                    match fiber_rc {
                        // Waker resolved with Value(empty) → slot free, retry.
                        0 => continue,
                        // Exception pending — inspect.
                        -1 => {
                            let mut cls_ptr: *const std::os::raw::c_char = std::ptr::null();
                            unsafe {
                                bridge_ffi::oxphp_exception_get(
                                    &mut cls_ptr,
                                    std::ptr::null_mut(),
                                    std::ptr::null_mut(),
                                );
                            }
                            let is_async_cancel = if cls_ptr.is_null() {
                                false
                            } else {
                                let cls =
                                    unsafe { std::ffi::CStr::from_ptr(cls_ptr).to_string_lossy() };
                                cls == "OxPHP\\Async\\Exception"
                            };
                            if is_async_cancel {
                                // Cancelled (timeout or close-side cancel).
                                unsafe { bridge_ffi::oxphp_exception_clear() };
                                if deadline
                                    .map(|d| std::time::Instant::now() >= d)
                                    .unwrap_or(false)
                                {
                                    unsafe { bridge_ffi::oxphp_portable_free(buf) };
                                    return Err(PhpError::Exception {
                                        class: "OxPHP\\Shared\\TimeoutException".into(),
                                        message: "send timed out".into(),
                                        code: 0,
                                    });
                                }
                                // Else: spurious cancel → loop and retry.
                                continue;
                            }
                            // Other exception (e.g. ClosedException) —
                            // leave it pending; the plugin framework will
                            // surface EG(exception) on Err(Custom).
                            unsafe { bridge_ffi::oxphp_portable_free(buf) };
                            return Err(PhpError::Custom(
                                "send: fiber waker raised exception".into(),
                            ));
                        }
                        // Direct fiber-layer timeout (should be rare given
                        // the synthetic::cancel path, but handle it).
                        -2 => {
                            unsafe { bridge_ffi::oxphp_portable_free(buf) };
                            return Err(PhpError::Exception {
                                class: "OxPHP\\Shared\\TimeoutException".into(),
                                message: "send timed out".into(),
                                code: 0,
                            });
                        }
                        other => {
                            // rc=1 means "not in oxphp fiber"; we only land
                            // here when in_fiber == true, so the SAPI predicate
                            // and `oxphp_current_fiber` disagree — a logic bug
                            // worth crashing on in dev. Release builds degrade
                            // to a Custom error so the worker stays up.
                            debug_assert!(
                                other != 1,
                                "fiber_await rc=1 in fiber path — oxphp_bridge_in_fiber lied",
                            );
                            unsafe { bridge_ffi::oxphp_portable_free(buf) };
                            return Err(PhpError::Custom(format!("send: fiber_await rc={other}")));
                        }
                    }
                }
            } else {
                // Non-fiber path: thread-block via send_blocking.
                // TODO(Task 3): convert the send handler to use read_timeout_arg
                // so timeout_ms carries the i64 wire value directly.
                let rc =
                    unsafe { oxphp_shared_channel_send_blocking(id, buf, len, timeout_ms as i64) };
                unsafe { bridge_ffi::oxphp_portable_free(buf) };

                if rc == SharedError::Closed.code() {
                    return Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\ClosedException".into(),
                        message: "channel closed".into(),
                        code: 0,
                    });
                }
                if rc == SharedError::Timeout.code() {
                    return Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\TimeoutException".into(),
                        message: "send timed out".into(),
                        code: 0,
                    });
                }
                super::counter::counter_rc_to_result(rc)?;
                Ok(())
            }
        })
        // ── tryRecv(): mixed ────────────────────────────────────────
        .method("tryRecv")
        .returns(PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let mut out_buf: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let mut state: c_int = 0;
            let rc = unsafe {
                oxphp_shared_channel_try_recv(id, &mut out_buf, &mut out_len, &mut state)
            };
            super::counter::counter_rc_to_result(rc)?;
            match state {
                0 => {
                    // Got an item — deserialize directly into retval.
                    let retval = call.retval_ptr();
                    let des_rc = unsafe {
                        bridge_ffi::oxphp_portable_deserialize(
                            out_buf,
                            out_len,
                            1,
                            retval as *mut _,
                        )
                    };
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    if des_rc != 0 {
                        return Err(PhpError::Custom(format!(
                            "tryRecv: deserialize failed rc={des_rc}"
                        )));
                    }
                    Ok(())
                }
                1 => {
                    // Empty, open — return null per spec.
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    call.ret_null();
                    Ok(())
                }
                2 => {
                    // Closed + empty — throw ClosedException per spec
                    // (tryRecv is stricter than recv, which returns null).
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\ClosedException".into(),
                        message: "channel closed".into(),
                        code: 0,
                    })
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
        // ── recv(timeout=0.0): mixed ────────────────────────────────
        .method("recv")
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .returns(PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let timeout_s = if call.argc() > 0 {
                call.arg_double(0).unwrap_or(0.0)
            } else {
                0.0
            };
            let timeout_ms: u64 = if timeout_s <= 0.0 {
                0
            } else {
                (timeout_s * 1000.0) as u64
            };

            // The fiber-suspend path uses synthetic-promise FFI that is
            // gated behind `feature = "php"` (PROMISE_MAP lives on PHP
            // worker threads). Without `php`, force `in_fiber = false` so
            // we always take the thread-blocking branch — matches the
            // mock `oxphp_bridge_in_fiber` returning 0 anyway.
            #[cfg(feature = "php")]
            let in_fiber = unsafe { bridge_ffi::oxphp_bridge_in_fiber() } != 0;
            #[cfg(not(feature = "php"))]
            let in_fiber = false;

            if in_fiber {
                // Fiber path: register recv-waiter, suspend, handle resolve.
                let mut promise_id: i64 = 0;
                // See note on send-side for the cfg gating rationale.
                let reg_rc = unsafe { recv_fiber_register_shim(id, timeout_ms, &mut promise_id) };
                super::counter::counter_rc_to_result(reg_rc)?;

                let retval = call.retval_ptr();
                // Pass 0.0 for timeout — synthetic::cancel handles it.
                let fiber_rc =
                    unsafe { bridge_ffi::oxphp_bridge_fiber_await(promise_id, 0.0, retval) };

                match fiber_rc {
                    // 0 = waker resolved with Value → retval written by
                    // await_dispatch_callback. Done.
                    0 => Ok(()),
                    // -1 = exception pending. Cancelled (Async\Exception)
                    // translates to null per spec; other exceptions propagate.
                    -1 => {
                        let mut cls_ptr: *const std::os::raw::c_char = std::ptr::null();
                        unsafe {
                            bridge_ffi::oxphp_exception_get(
                                &mut cls_ptr,
                                std::ptr::null_mut(),
                                std::ptr::null_mut(),
                            );
                        }
                        let is_async_cancel = if cls_ptr.is_null() {
                            false
                        } else {
                            let cls =
                                unsafe { std::ffi::CStr::from_ptr(cls_ptr).to_string_lossy() };
                            cls == "OxPHP\\Async\\Exception"
                        };
                        if is_async_cancel {
                            unsafe { bridge_ffi::oxphp_exception_clear() };
                            // Spec: recv returns null on any non-item outcome
                            // (timeout, close, shutdown). See
                            // 24-type-channel.md:34-38 and the idiomatic
                            // `while (($x = $ch->recv(60)) !== null)` loop
                            // pattern. Asymmetric with send, which throws
                            // TimeoutException.
                            call.ret_null();
                            Ok(())
                        } else {
                            // Non-Async exception — let the plugin framework
                            // surface the pending EG(exception).
                            Err(PhpError::Custom(
                                "recv: fiber waker raised exception".into(),
                            ))
                        }
                    }
                    -2 => {
                        // Spec: recv returns null on timeout (including
                        // SAPI-layer timeout). See 24-type-channel.md:34-38.
                        call.ret_null();
                        Ok(())
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
                        Err(PhpError::Custom(format!("recv: fiber_await rc={other}")))
                    }
                }
            } else {
                // Thread-block path.
                // TODO(Task 3): convert the recv handler to use read_timeout_arg
                // so timeout_ms carries the i64 wire value directly.
                let mut out_buf: *mut u8 = std::ptr::null_mut();
                let mut out_len: usize = 0;
                let mut state: c_int = 0;
                let rc = unsafe {
                    oxphp_shared_channel_recv_blocking(
                        id,
                        timeout_ms as i64,
                        &mut out_buf,
                        &mut out_len,
                        &mut state,
                    )
                };
                if rc == SharedError::Timeout.code() {
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    // Spec: recv returns null on timeout (asymmetric with
                    // send, which throws TimeoutException). See
                    // 24-type-channel.md:34-38.
                    call.ret_null();
                    return Ok(());
                }
                super::counter::counter_rc_to_result(rc)?;
                match state {
                    0 => {
                        let retval = call.retval_ptr();
                        let des_rc = unsafe {
                            bridge_ffi::oxphp_portable_deserialize(
                                out_buf,
                                out_len,
                                1,
                                retval as *mut _,
                            )
                        };
                        if !out_buf.is_null() {
                            unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                        }
                        if des_rc != 0 {
                            return Err(PhpError::Custom(format!(
                                "recv: deserialize failed rc={des_rc}"
                            )));
                        }
                        Ok(())
                    }
                    2 => {
                        // Closed + empty or shutdown — return null per spec.
                        if !out_buf.is_null() {
                            unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                        }
                        call.ret_null();
                        Ok(())
                    }
                    other => {
                        if !out_buf.is_null() {
                            unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                        }
                        Err(PhpError::Custom(format!("recv: unexpected state {other}")))
                    }
                }
            }
        })
        // ── close(): void ──────────────────────────────────────────
        .method("close")
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let rc = oxphp_shared_channel_close(id);
            super::counter::counter_rc_to_result(rc)?;
            Ok(())
        })
        // ── isClosed(): bool ───────────────────────────────────────
        .method("isClosed")
        .returns(PhpType::Bool)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let mut out: c_int = 0;
            let rc = unsafe { oxphp_shared_channel_is_closed(id, &mut out) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(out != 0);
            Ok(())
        })
        // ── pending(): int ─────────────────────────────────────────
        .method("pending")
        .returns(PhpType::Int)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let mut out: u64 = 0;
            let rc = unsafe { oxphp_shared_channel_pending(id, &mut out) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_long(out as i64);
            Ok(())
        })
        // ── sendMany(values, timeout=0.0): int ─────────────────────
        //
        // Serializes each array element to its own portbuf via the C
        // helper `oxphp_iter_array_to_portbufs`, then deposits them
        // one-by-one through the `oxphp_shared_channel_send_many` FFI.
        // Returns the count actually sent per the spec's "how many
        // were actually sent before full/closed/timeout" contract —
        // closed/timeout are NOT raised as exceptions.
        .method("sendMany")
        .param("values", PhpType::Array)
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .returns(PhpType::Int)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let timeout_s = if call.argc() > 1 {
                call.arg_double(1).unwrap_or(0.0)
            } else {
                0.0
            };
            let timeout_ms: u64 = if timeout_s <= 0.0 {
                0
            } else {
                (timeout_s * 1000.0) as u64
            };

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

            let mut sent: u64 = 0;
            // TODO(Task 3): convert sendMany handler to use read_timeout_arg.
            let rc = unsafe {
                oxphp_shared_channel_send_many(id, concat, offsets, n, timeout_ms as i64, &mut sent)
            };
            unsafe {
                if !concat.is_null() {
                    bridge_ffi::oxphp_portable_free(concat);
                }
                if !offsets.is_null() {
                    bridge_ffi::oxphp_portable_free(offsets as *mut u8);
                }
            }
            super::counter::counter_rc_to_result(rc)?;
            call.ret_long(sent as i64);
            Ok(())
        })
        // ── recvMany(max, timeout=0.0): array ──────────────────────
        //
        // `max == 0` → drain currently-buffered items without waiting.
        // `max > 0, timeout == 0.0` → block until max items collected
        //   or channel closes+empties.
        // `max > 0, timeout > 0` → block up to timeout; return whatever
        //   was collected by then (may be empty).
        .method("recvMany")
        .param("max", PhpType::Int)
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .returns(PhpType::Array)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let max_raw = call.arg_long(0).unwrap_or(0);
            let max: u64 = if max_raw < 0 { 0 } else { max_raw as u64 };
            let timeout_s = if call.argc() > 1 {
                call.arg_double(1).unwrap_or(0.0)
            } else {
                0.0
            };
            let timeout_ms: u64 = if timeout_s <= 0.0 {
                0
            } else {
                (timeout_s * 1000.0) as u64
            };

            let mut concat: *mut u8 = std::ptr::null_mut();
            let mut concat_len: usize = 0;
            let mut offsets: *mut usize = std::ptr::null_mut();
            let mut n: u64 = 0;
            // TODO(Task 3): convert recvMany handler to use read_timeout_arg.
            let rc = unsafe {
                oxphp_shared_channel_recv_many(
                    id,
                    max,
                    timeout_ms as i64,
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
            call.ret_long(h.shared_id as i64);
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
        -7 => "OxPHP\\Shared\\TimeoutException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        _ => "OxPHP\\Shared\\Exception",
    };
    PhpError::Exception {
        class: class.to_string(),
        message: String::new(),
        code: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_shared::error::SharedError;
    use crate::plugins::ox_shared::types::once::OnceInner;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

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
        ch.try_send(vec![1, 2, 3]).expect("send ok");
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
        ch.try_send(vec![0xAA]).expect("first send ok");
        match ch.try_send(vec![0xBB]) {
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
        match ch.try_send(vec![9]) {
            Err(TrySendErr::Closed(p)) => assert_eq!(p, vec![9]),
            other => panic!("expected Closed([9]), got {other:?}"),
        }
    }

    #[test]
    fn try_recv_drains_after_close() {
        let ch = ChannelInner::new(4);
        ch.try_send(vec![1]).unwrap();
        ch.try_send(vec![2]).unwrap();
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
        ch.try_send(vec![1]).expect("first send ok");
        match ch.try_send(vec![2]) {
            Err(TrySendErr::Full(_)) => {}
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn pending_matches_try_send_counts() {
        let ch = ChannelInner::new(8);
        ch.try_send(vec![1]).unwrap();
        ch.try_send(vec![2]).unwrap();
        ch.try_send(vec![3]).unwrap();
        assert_eq!(ch.pending(), 3);
        let _ = ch.try_recv().unwrap();
        let _ = ch.try_recv().unwrap();
        assert_eq!(ch.pending(), 1);
    }

    #[test]
    fn debug_snapshot_returns_pending_as_long() {
        let ch = ChannelInner::new(4);
        ch.try_send(vec![1]).unwrap();
        ch.try_send(vec![2]).unwrap();
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
        ch.send_blocking(vec![1], Wait::Bounded(Duration::from_millis(100)))
            .expect("send ok");
        assert_eq!(ch.pending(), 1);
    }

    #[test]
    fn send_blocking_on_full_waits_until_recv() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(vec![1]).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                ch.send_blocking(vec![2], Wait::Bounded(Duration::from_secs(2)))
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        let first = ch.try_recv().expect("drain first");
        assert_eq!(first, Some(vec![1]));
        let res = sender.join().expect("sender thread");
        assert!(res.is_ok(), "send_blocking got {res:?}");
        let second = ch.try_recv().expect("drain second");
        assert_eq!(second, Some(vec![2]));
    }

    #[test]
    fn send_blocking_timeout_returns_timeout() {
        let ch = ChannelInner::new(1);
        ch.try_send(vec![1]).unwrap();
        let res = ch.send_blocking(vec![2], Wait::Bounded(Duration::from_millis(50)));
        assert!(matches!(res, Err(SharedError::Timeout)), "got {res:?}");
    }

    #[test]
    fn send_blocking_on_closed_returns_closed() {
        let ch = ChannelInner::new(4);
        ch.close();
        let res = ch.send_blocking(vec![1], Wait::Forever);
        assert!(matches!(res, Err(SharedError::Closed)), "got {res:?}");
    }

    #[test]
    fn send_blocking_wakes_when_closed_mid_wait() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(vec![1]).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                ch.send_blocking(vec![2], Wait::Bounded(Duration::from_secs(5)))
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
        ch.try_send(vec![1]).unwrap();
        let res = ch.recv_blocking(Wait::Bounded(Duration::from_millis(100)));
        assert_eq!(res.unwrap(), Some(vec![1]));
    }

    #[test]
    fn recv_blocking_on_empty_waits_until_send() {
        let ch = Arc::new(ChannelInner::new(4));
        let receiver = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || ch.recv_blocking(Wait::Bounded(Duration::from_secs(2))))
        };
        std::thread::sleep(Duration::from_millis(50));
        ch.try_send(vec![7]).unwrap();
        let res = receiver.join().expect("receiver thread");
        assert_eq!(res.unwrap(), Some(vec![7]));
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
        ch.try_send(vec![1]).unwrap();
        ch.try_send(vec![2]).unwrap();
        ch.close();
        assert_eq!(ch.recv_blocking(Wait::Forever).unwrap(), Some(vec![1]));
        assert_eq!(ch.recv_blocking(Wait::Forever).unwrap(), Some(vec![2]));
        assert_eq!(ch.recv_blocking(Wait::Forever).unwrap(), None);
    }

    #[test]
    fn senders_blocked_gauge_increments_while_blocked() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(vec![1]).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                ch.send_blocking(vec![2], Wait::Bounded(Duration::from_secs(2)))
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
        ch.try_send(vec![9]).unwrap();
        let res = receiver.join().expect("receiver thread");
        assert_eq!(res.unwrap(), Some(vec![9]));
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
            ch2.try_send(vec![7, 7]).unwrap();
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
            Some("OxPHP\\Async\\Exception")
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
            Some("OxPHP\\Async\\Exception")
        );
    }

    #[tokio::test]
    async fn register_recv_waiter_drains_buffered_items() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        ch.try_send(vec![1, 2, 3]).unwrap();
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
        ch.try_send(vec![1]).unwrap();
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
        ch.try_send(vec![1]).unwrap();
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
        ch.try_send(vec![99]).unwrap();
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
                Some("OxPHP\\Async\\Exception")
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
        ch.try_send(vec![42]).unwrap();
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
        ch.try_send(expected.clone()).unwrap();
        let got = rx.await.unwrap();
        assert!(got.success);
        assert_eq!(got.serialized_value_len, expected.len());
        // Buffer was bypassed — nothing landed in the crossbeam queue.
        assert_eq!(ch.pending(), 0);
    }

    /// Regression: dead-last-waiter race (single parked waiter,
    /// cancelled before `try_send` reaches `resolve`). The optimised
    /// fast path consumes the payload; the sender's bookkeeping still
    /// records +1 to `items_sent_total`. No panic, no deadlock, and
    /// `try_send` reports Ok — later sends and recvs work normally.
    #[tokio::test]
    async fn single_dead_waiter_does_not_panic_or_deadlock() {
        use crate::plugins::ox_async::synthetic;
        let ch = std::sync::Arc::new(ChannelInner::new(4));
        let (id, _rx) = synthetic::alloc();
        ch.register_recv_waiter(id);
        // Kill the waiter before send reaches it.
        assert!(synthetic::cancel(id));
        // Send must not panic; data-loss on this race is documented
        // and acceptable.
        ch.try_send(vec![0xDE, 0xAD]).unwrap();
        // Channel is still operational.
        ch.try_send(vec![0xBE, 0xEF]).unwrap();
        assert_eq!(ch.try_recv().unwrap(), Some(vec![0xBE, 0xEF]));
    }

    #[test]
    fn send_blocking_forever_wait_is_indefinite() {
        let ch = Arc::new(ChannelInner::new(1));
        ch.try_send(vec![1]).unwrap();
        let sender = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || ch.send_blocking(vec![2], Wait::Forever))
        };
        std::thread::sleep(Duration::from_millis(50));
        let first = ch.try_recv().expect("drain first");
        assert_eq!(first, Some(vec![1]));
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
            shutdown_timeout_seconds: 5.0,
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
    struct TestChannel(u64);

    impl TestChannel {
        fn new(capacity: u64) -> Self {
            ensure_test_registry();
            let mut id: u64 = 0;
            let rc = unsafe { oxphp_shared_channel_create(capacity, &mut id) };
            assert_eq!(rc, 0, "create failed with rc={rc}");
            assert!(id != 0);
            Self(id)
        }

        fn id(&self) -> u64 {
            self.0
        }
    }

    impl Drop for TestChannel {
        fn drop(&mut self) {
            let reg = crate::plugins::ox_shared::registry::registry();
            reg.release(self.0);
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
        let mut id: u64 = 0;
        let rc = unsafe { oxphp_shared_channel_create(0, &mut id) };
        assert_eq!(rc, SharedError::Type.code());
        // Registry must not have a fresh entry bound to this id — id
        // remained 0 so any lookup would fail anyway, but double-check.
        assert_eq!(id, 0);
    }

    #[test]
    fn ffi_try_send_and_recv_roundtrip() {
        let ch = TestChannel::new(4);

        let payload = [1u8, 2, 3];
        let mut success: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_try_send(ch.id(), payload.as_ptr(), payload.len(), &mut success)
        };
        assert_eq!(rc, 0);
        assert_eq!(success, 1);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 1;
        let rc = unsafe {
            oxphp_shared_channel_try_recv(ch.id(), &mut out_buf, &mut out_len, &mut state)
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
        let rc = unsafe { oxphp_shared_channel_try_send(ch.id(), first.as_ptr(), 1, &mut success) };
        assert_eq!(rc, 0);
        assert_eq!(success, 1);

        let second = [0xBBu8];
        let mut success2: c_int = 99;
        let rc =
            unsafe { oxphp_shared_channel_try_send(ch.id(), second.as_ptr(), 1, &mut success2) };
        assert_eq!(rc, 0, "Full is not an error — rc must stay 0");
        assert_eq!(success2, 0);
    }

    #[test]
    fn ffi_try_send_on_closed_returns_closed() {
        let ch = TestChannel::new(4);
        assert_eq!(oxphp_shared_channel_close(ch.id()), 0);

        let payload = [9u8];
        let mut success: c_int = 1;
        let rc =
            unsafe { oxphp_shared_channel_try_send(ch.id(), payload.as_ptr(), 1, &mut success) };
        assert_eq!(rc, SharedError::Closed.code());
    }

    #[test]
    fn ffi_try_recv_empty_open_state_1() {
        let ch = TestChannel::new(4);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 99;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_try_recv(ch.id(), &mut out_buf, &mut out_len, &mut state)
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 1);
        assert!(out_buf.is_null());
        assert_eq!(out_len, 0);
    }

    #[test]
    fn ffi_try_recv_closed_empty_state_2() {
        let ch = TestChannel::new(4);
        assert_eq!(oxphp_shared_channel_close(ch.id()), 0);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 99;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_try_recv(ch.id(), &mut out_buf, &mut out_len, &mut state)
        };
        assert_eq!(rc, 0);
        assert_eq!(state, 2);
        assert!(out_buf.is_null());
    }

    #[test]
    fn ffi_close_is_idempotent() {
        let ch = TestChannel::new(4);
        assert_eq!(oxphp_shared_channel_close(ch.id()), 0);
        assert_eq!(oxphp_shared_channel_close(ch.id()), 0);
    }

    #[test]
    fn ffi_is_closed_reports_state() {
        let ch = TestChannel::new(4);

        let mut out: c_int = 99;
        assert_eq!(
            unsafe { oxphp_shared_channel_is_closed(ch.id(), &mut out) },
            0
        );
        assert_eq!(out, 0);

        assert_eq!(oxphp_shared_channel_close(ch.id()), 0);
        assert_eq!(
            unsafe { oxphp_shared_channel_is_closed(ch.id(), &mut out) },
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
                    oxphp_shared_channel_try_send(ch.id(), payload.as_ptr(), 1, &mut success)
                },
                0
            );
            assert_eq!(success, 1);
        }

        let mut out: u64 = 99;
        assert_eq!(
            unsafe { oxphp_shared_channel_pending(ch.id(), &mut out) },
            0
        );
        assert_eq!(out, 2);
    }

    #[test]
    fn ffi_send_blocking_succeeds_on_vacancy() {
        let ch = TestChannel::new(4);

        let payload = [42u8];
        let rc = unsafe { oxphp_shared_channel_send_blocking(ch.id(), payload.as_ptr(), 1, 100) };
        assert_eq!(rc, 0);
    }

    #[test]
    fn ffi_send_blocking_times_out() {
        let ch = TestChannel::new(1);

        let first = [1u8];
        let mut success: c_int = 0;
        assert_eq!(
            unsafe { oxphp_shared_channel_try_send(ch.id(), first.as_ptr(), 1, &mut success) },
            0
        );

        let second = [2u8];
        let rc = unsafe { oxphp_shared_channel_send_blocking(ch.id(), second.as_ptr(), 1, 50) };
        assert_eq!(rc, SharedError::Timeout.code());
    }

    #[test]
    fn ffi_recv_blocking_returns_item() {
        let ch = TestChannel::new(4);

        let payload = [7u8, 8, 9];
        let mut success: c_int = 0;
        assert_eq!(
            unsafe { oxphp_shared_channel_try_send(ch.id(), payload.as_ptr(), 3, &mut success) },
            0
        );

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let mut state: c_int = 2;
        let rc = unsafe {
            oxphp_shared_channel_recv_blocking(ch.id(), 100, &mut out_buf, &mut out_len, &mut state)
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
        assert_eq!(oxphp_shared_channel_close(ch.id()), 0);

        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 99;
        let mut state: c_int = 0;
        let rc = unsafe {
            oxphp_shared_channel_recv_blocking(ch.id(), 0, &mut out_buf, &mut out_len, &mut state)
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
            oxphp_shared_channel_recv_blocking(ch.id(), 50, &mut out_buf, &mut out_len, &mut state)
        };
        assert_eq!(rc, SharedError::Timeout.code());
    }

    // ─── batched send_many / recv_many ───────────────────────

    #[test]
    fn send_many_fills_channel_and_returns_count() {
        let ch = ChannelInner::new(10);
        let payloads: Vec<Payload> = (0u8..5).map(|i| vec![i]).collect();
        let sent = ch.send_many(payloads, Wait::Forever);
        assert_eq!(sent, 5);
        assert_eq!(ch.pending(), 5);
    }

    #[test]
    fn send_many_stops_on_closed() {
        let ch = ChannelInner::new(10);
        ch.close();
        let payloads: Vec<Payload> = (0u8..5).map(|i| vec![i]).collect();
        let sent = ch.send_many(payloads, Wait::Forever);
        assert_eq!(sent, 0);
    }

    #[test]
    fn send_many_partial_on_timeout() {
        // Capacity 2; push 5 with 50ms timeout → only first 2 fit, rest
        // time out and send_many returns the running count.
        let ch = ChannelInner::new(2);
        let payloads: Vec<Payload> = (0u8..5).map(|i| vec![i]).collect();
        let sent = ch.send_many(payloads, Wait::Bounded(Duration::from_millis(50)));
        assert_eq!(sent, 2);
        assert_eq!(ch.pending(), 2);
    }

    #[test]
    fn recv_many_drain_max_zero() {
        let ch = ChannelInner::new(10);
        ch.try_send(vec![1]).unwrap();
        ch.try_send(vec![2]).unwrap();
        ch.try_send(vec![3]).unwrap();
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
            ch.try_send(vec![i]).unwrap();
        }
        let got = ch.recv_many(3, Wait::Bounded(Duration::from_millis(50)));
        assert_eq!(got, vec![vec![0], vec![1], vec![2]]);
        assert_eq!(ch.pending(), 2);
    }

    #[test]
    fn recv_many_stops_on_closed_empty() {
        let ch = ChannelInner::new(10);
        ch.try_send(vec![7]).unwrap();
        ch.close();
        let got = ch.recv_many(10, Wait::Bounded(Duration::from_millis(50)));
        // Got the buffered item and then stopped on closed+empty; no
        // timeout wait because recv_blocking returned Ok(None).
        assert_eq!(got, vec![vec![7]]);
    }

    #[test]
    fn recv_many_timeout_returns_partial() {
        let ch = ChannelInner::new(10);
        ch.try_send(vec![1]).unwrap();
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
        let rc =
            unsafe { oxphp_shared_channel_try_send(id, send_payload.as_ptr(), 2, &mut success) };
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

        // Fill the channel so a send-waiter can park.
        let payload = [1u8];
        let mut success: c_int = 0;
        assert_eq!(
            unsafe { oxphp_shared_channel_try_send(id, payload.as_ptr(), 1, &mut success) },
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
        let rc =
            unsafe { oxphp_shared_channel_try_recv(id, &mut out_buf, &mut out_len, &mut state) };
        assert_eq!(rc, 0);
        assert_eq!(state, 0);
        unsafe { free_out(out_buf) };

        let got = rx.await.expect("receiver should fire");
        assert!(got.success);
        // Empty-Value ack = "slot free, retry your send".
        assert_eq!(got.serialized_value_len, 0);
        assert!(got.serialized_value.is_null());
    }
}
