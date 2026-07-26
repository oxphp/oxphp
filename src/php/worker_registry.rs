//! Global registry of per-worker cancellation slots. Each worker
//! registers its EG(vm_interrupt) address here on boot so other
//! threads can interrupt it. Each request stores a Weak back-ref
//! to its CancellationState so cross-thread cancellation can
//! find the right worker.

use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, Weak};

use crate::bridge::cancel::CancellationState;
// Re-exported so the binary crate's shutdown path can name the reason when
// calling `hard_cancel_all` (the `bridge` module itself is crate-private).
pub use crate::bridge::cancel::CancelReason;

pub struct WorkerSlot {
    pub cancel_state: Mutex<Option<Weak<CancellationState>>>,
    pub interrupt_flag_ptr: AtomicPtr<u8>,
    /// Requests currently in flight on this worker. Fiber multiplexing can
    /// hold many at once, and `cancel_state` only ever names the most
    /// recently prepared one — this counter is what `cancel_all` gates the
    /// broadcast vm_interrupt kick on. The gate is an optimization (don't
    /// interrupt an idle worker), not a liveness proof: a worker can die with
    /// the counter still positive — an abnormal exit tears down its scheduler
    /// without running each fiber's terminal cleanup — and `clear_worker_slot`
    /// only zeroes it once the health monitor observes the dead thread. What
    /// keeps the raw write to `interrupt_flag_ptr` sound is that the address
    /// stays mapped for the process lifetime: it points into the worker's TSRM
    /// interpreter block, which is released only by `ts_free_thread()`, and
    /// nothing in PHP or OxPHP ever calls it.
    ///
    /// A second consumer is far less tolerant of staleness: `total_in_flight`
    /// sums this field across slots to decide when the graceful drain may
    /// stop, so a value that stays positive with no real work behind it holds
    /// shutdown until the drain deadline instead of costing one spurious kick.
    /// Weigh both readers before changing how this counter is maintained.
    pub active_requests: AtomicUsize,
    pub heartbeat: crate::php::heartbeat::WorkerHeartbeat,
}

impl WorkerSlot {
    pub fn new() -> Self {
        Self {
            cancel_state: Mutex::new(None),
            interrupt_flag_ptr: AtomicPtr::new(std::ptr::null_mut()),
            active_requests: AtomicUsize::new(0),
            heartbeat: crate::php::heartbeat::WorkerHeartbeat::new(),
        }
    }
}

impl Default for WorkerSlot {
    fn default() -> Self {
        Self::new()
    }
}

pub static WORKERS: OnceLock<Vec<WorkerSlot>> = OnceLock::new();

/// Idempotent — only the first call populates the registry.
pub fn init_workers(count: usize) {
    let _ = WORKERS.get_or_init(|| (0..count).map(|_| WorkerSlot::new()).collect());
}

/// Cross-thread cancellation: set the reason on the request's state
/// (first writer wins) and, if a worker holds it, raise vm_interrupt
/// on that worker. Safe to call when no worker holds it: only the
/// reason is recorded; the worker will observe it on receive (B).
pub fn cancel_request(state: &std::sync::Arc<CancellationState>, reason: CancelReason) {
    if !state.set(reason) {
        return;
    }
    let workers = match WORKERS.get() {
        Some(w) => w,
        None => return,
    };
    for slot in workers.iter() {
        // Resolve the target's interrupt-flag pointer under the lock,
        // then DROP the guard before writing across threads. Holding
        // the Mutex during the cross-thread store would deadlock the
        // disconnect path if the worker panics and poisons it; the
        // Mutex's job is only to keep the Weak<>→Arc<> upgrade
        // race-free, not to serialise the interrupt write itself.
        let interrupt_addr = {
            let guard = slot.cancel_state.lock().unwrap();
            match guard.as_ref().and_then(Weak::upgrade) {
                Some(active) if std::sync::Arc::ptr_eq(&active, state) => {
                    slot.interrupt_flag_ptr.load(Ordering::Acquire)
                }
                _ => continue,
            }
            // guard dropped here at end of scope
        };
        raise_interrupt(interrupt_addr);
        return;
    }
}

/// Register `state` as worker `worker_id`'s most recent in-flight request:
/// publish the Weak for targeted cancellation, count the request for the
/// drain broadcast kick, and stamp the busy heartbeat (capturing the thread
/// id once per worker). Paired with `end_request` at the request's terminal
/// cleanup — keep the two adjacent to any panic-capable code so the counter
/// cannot leak.
pub fn begin_request(worker_id: usize, state: &std::sync::Arc<CancellationState>) {
    let Some(slot) = WORKERS.get().and_then(|w| w.get(worker_id)) else {
        return;
    };
    // Poison-tolerant, like every other slot-lock site: a worker that panicked
    // under its own slot lock must not turn every subsequent request on the
    // recycled slot into a second panic. Losing the Weak only costs targeted
    // cancellation; the counter and the broadcast kick still work.
    if let Ok(mut guard) = slot.cancel_state.lock() {
        *guard = Some(std::sync::Arc::downgrade(state));
    }
    slot.active_requests.fetch_add(1, Ordering::AcqRel);
    slot.heartbeat
        .request_start_us
        .store(crate::php::heartbeat::monotonic_us(), Ordering::Relaxed);
    if slot.heartbeat.tid.load(Ordering::Relaxed) == 0 {
        let tid = crate::php::heartbeat::current_tid();
        if tid != 0 {
            slot.heartbeat.tid.store(tid, Ordering::Relaxed);
        }
    }
}

/// Terminal cleanup for one request on `worker_id`: drop the slot's Weak (so
/// a stale `cancel_request()` walking the registry can't match a finished
/// request), decrement the in-flight counter, and zero the busy heartbeat.
pub fn end_request(worker_id: usize) {
    let Some(slot) = WORKERS.get().and_then(|w| w.get(worker_id)) else {
        return;
    };
    if let Ok(mut guard) = slot.cancel_state.lock() {
        *guard = None;
    }
    // Saturating decrement: if this worker was declared dead and recycled
    // (clear_worker_slot zeroed the counter) before the original thread's
    // late terminal cleanup ran, an unchecked fetch_sub would wrap to
    // usize::MAX and mark the slot busy forever.
    let _ = slot
        .active_requests
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
    slot.heartbeat.request_start_us.store(0, Ordering::Relaxed);
}

/// Raise Zend's `vm_interrupt` at `interrupt_addr` (a worker's
/// `&EG(vm_interrupt)` byte, published in its slot). No-op on null.
fn raise_interrupt(interrupt_addr: *mut u8) {
    if interrupt_addr.is_null() {
        return;
    }
    // SAFETY: cross-thread Zend interrupt pattern. Routed through the bridge
    // so the underlying `zend_atomic_bool` is mutated via
    // `zend_atomic_bool_store_ex`, not aliased as a plain `uint8_t*`. The
    // TLS byte lives for the worker thread's lifetime; callers only pass
    // addresses of workers known to be alive.
    unsafe {
        crate::bridge::ffi::oxphp_bridge_request_interrupt_at(
            interrupt_addr as *mut std::os::raw::c_void,
        );
    }
}

/// Deadline-phase shutdown: latch the bridge's hard-drain flag, then
/// broadcast `reason` + a vm_interrupt kick to every busy worker. The flag
/// must be latched first — the kick lands in whatever request each worker is
/// actually executing, and the interrupt handler self-cancels it only when it
/// observes the hard-drain phase (under fiber multiplexing the registry slot
/// cannot name the running request, so no per-request reason reaches it).
pub fn hard_cancel_all(reason: CancelReason) {
    unsafe { crate::bridge::ffi::oxphp_bridge_set_drain_hard() };
    cancel_all(reason);
}

/// Broadcast cancellation to every worker with requests in flight: set
/// `reason` on the request each slot still names (best effort — under fiber
/// multiplexing that is only the most recently prepared one) and raise
/// vm_interrupt on every worker whose `active_requests` counter is nonzero.
///
/// Works directly on each slot instead of delegating to `cancel_request`
/// (which rescans the whole registry per call): O(workers) rather than
/// O(workers^2), and it never locks another slot's mutex. That is what makes
/// it genuinely poison-tolerant — a worker that panicked while holding its
/// slot lock causes only that slot's state write to be skipped (it still
/// gets the kick); it cannot turn the drain path into a second panic. That
/// matters because this runs in the unawaited SIGTERM task, where a panic
/// would silently abort the whole drain.
fn cancel_all(reason: CancelReason) {
    if let Some(workers) = WORKERS.get() {
        cancel_all_in(workers, reason);
    }
}

fn cancel_all_in(workers: &[WorkerSlot], reason: CancelReason) {
    for slot in workers.iter() {
        // A poisoned slot is skipped, never unwrapped. The guard is dropped
        // before the cross-thread interrupt write (same discipline as
        // `cancel_request`).
        if let Ok(guard) = slot.cancel_state.lock() {
            if let Some(state) = guard.as_ref().and_then(Weak::upgrade) {
                state.set(reason);
            }
        }
        // Kick every worker with work in flight, whether or not its slot
        // still names a live request: the interrupt handler reads the
        // per-fiber cell (or self-cancels in the hard-drain phase), not the
        // registry. Gating on active_requests keeps the raw write sound —
        // see the field's doc comment.
        if slot.active_requests.load(Ordering::Acquire) > 0 {
            raise_interrupt(slot.interrupt_flag_ptr.load(Ordering::Acquire));
        }
    }
}

/// Sum of in-flight requests across every worker slot — the drain loop's
/// second gate, next to live connections. A request that ended its response
/// early via `oxphp_finish_request()` drops its connection while its
/// background work keeps running on the worker, so gating the drain on
/// connections alone lets it exit before the deadline ever applies to that
/// work.
///
/// 0 when nothing counts requests: the stub executor never calls
/// `begin_request`, so every slot stays at zero. The `WORKERS` guard is for
/// callers that run before `init_workers` — unit tests and the one-shot CLI
/// path — not for any server mode, where `main` initializes the registry
/// unconditionally.
pub fn total_in_flight() -> usize {
    WORKERS
        .get()
        .map_or(0, |workers| total_in_flight_in(workers))
}

fn total_in_flight_in(workers: &[WorkerSlot]) -> usize {
    workers
        .iter()
        .map(|slot| slot.active_requests.load(Ordering::Acquire))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn total_in_flight_sums_every_slot() {
        // Local Vec — the process-global WORKERS is shared with the other
        // tests in this binary.
        let workers: Vec<WorkerSlot> = (0..3).map(|_| WorkerSlot::new()).collect();
        assert_eq!(total_in_flight_in(&workers), 0);

        workers[0].active_requests.store(2, Ordering::Release);
        workers[2].active_requests.store(1, Ordering::Release);
        assert_eq!(total_in_flight_in(&workers), 3);
    }

    #[test]
    fn init_is_idempotent() {
        init_workers(4);
        init_workers(8); // ignored — first call wins
        assert_eq!(WORKERS.get().unwrap().len(), 4);
    }

    #[test]
    fn cancel_with_no_active_worker_records_reason() {
        let state = Arc::new(CancellationState::new());
        cancel_request(&state, CancelReason::ClientAbort);
        assert_eq!(state.get(), CancelReason::ClientAbort);
    }

    #[test]
    fn second_cancel_is_noop() {
        let state = Arc::new(CancellationState::new());
        cancel_request(&state, CancelReason::ClientAbort);
        cancel_request(&state, CancelReason::Timeout);
        assert_eq!(state.get(), CancelReason::ClientAbort);
    }

    #[test]
    fn cancel_all_marks_every_registered_state_shutdown() {
        // On graceful shutdown the server must broadcast `Shutdown` to every
        // registered in-flight request, not just the one worker whose slot
        // happens to be targeted. Register a fresh CancellationState into
        // every worker slot, fire cancel_all(Shutdown), and require each
        // state to observe it.
        init_workers(4);
        let slots = WORKERS.get().unwrap();
        let states: Vec<Arc<CancellationState>> = slots
            .iter()
            .map(|_| Arc::new(CancellationState::new()))
            .collect();
        for (slot, state) in slots.iter().zip(&states) {
            *slot.cancel_state.lock().unwrap() = Some(Arc::downgrade(state));
        }

        cancel_all(CancelReason::Shutdown);

        for state in &states {
            assert_eq!(state.get(), CancelReason::Shutdown);
        }

        // Reset the process-global slots we borrowed, so a later test in this
        // binary does not observe stale Weak references from this one.
        for slot in slots.iter() {
            *slot.cancel_state.lock().unwrap() = None;
        }
    }

    #[test]
    fn cancel_all_tolerates_a_poisoned_slot() {
        // A worker that panics while holding its slot lock poisons that slot.
        // The drain broadcast must skip it and still cancel the healthy slots,
        // never re-panic — it runs in the unawaited SIGTERM task, where a
        // panic would silently abort the whole drain. Uses a local Vec so a
        // poisoned mutex cannot leak into the process-global WORKERS shared by
        // the other tests in this binary.
        let workers: Vec<WorkerSlot> = (0..3).map(|_| WorkerSlot::new()).collect();

        // Poison slot 0.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = workers[0].cancel_state.lock().unwrap();
            panic!("simulate worker panic under the slot lock");
        }));
        assert!(
            workers[0].cancel_state.lock().is_err(),
            "slot 0 should be poisoned"
        );

        // Healthy in-flight request in slot 2. The poisoned slot also claims
        // in-flight work with no published interrupt address — the kick path
        // must tolerate both (skip the null pointer, never unwrap the lock).
        workers[0].active_requests.store(1, Ordering::Release);
        let state = Arc::new(CancellationState::new());
        *workers[2].cancel_state.lock().unwrap() = Some(Arc::downgrade(&state));
        workers[2].active_requests.store(1, Ordering::Release);

        // Must not panic, and must still cancel the healthy request.
        cancel_all_in(&workers, CancelReason::Shutdown);
        assert_eq!(state.get(), CancelReason::Shutdown);
    }
}
