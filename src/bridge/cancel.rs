//! Per-request cancellation state shared between the tokio dispatch
//! task and the worker thread.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[repr(u8)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CancelReason {
    None = 0,
    ClientAbort = 1,
    Timeout = 2,
    Shutdown = 3,
    Stuck = 4,
    UserCancel = 5,
}

#[derive(Debug)]
#[repr(C, align(64))]
pub struct CancellationState {
    reason: AtomicU8,
    done: AtomicBool,
    /// Won once, by whichever side gets to answer a queued request: the
    /// worker that picks it up, or the dispatch task when the queue deadline
    /// passes first.
    ///
    /// Answering is all the winner takes. A request the dispatch side refuses
    /// stays in the channel, holding its queue slot and its admission permit
    /// until a worker reaches it and drops it — so the claim bounds what the
    /// client waits for, not how deep the queue gets.
    ///
    /// Both can reach it in the same instant, and both would otherwise answer
    /// it — one with the handler's response, the other with a `529` — and the
    /// two refusal paths would count one refusal as two in
    /// `oxphp_admission_refused_total{reason="wait_timeout"}`. The loser must
    /// answer nothing, count nothing and run nothing: from the claim onwards
    /// the request belongs to the winner.
    ///
    /// One flag, not two — a "picked up" mark read separately from a "refused"
    /// mark. Two would have to be read in an order nothing guarantees: a
    /// worker that picks the request up just inside the deadline stores its
    /// mark nanoseconds before the dispatch task loads it, and "earlier in
    /// real time" is not a rule about when a store becomes visible. Reading
    /// the marks the other way round loses the same race from the other end.
    /// A single claim cannot split that way — the CAS orders the two sides
    /// against each other, whatever the clock did.
    claimed_from_queue: AtomicBool,
}

impl CancellationState {
    pub fn new() -> Self {
        Self {
            reason: AtomicU8::new(CancelReason::None as u8),
            done: AtomicBool::new(false),
            claimed_from_queue: AtomicBool::new(false),
        }
    }

    /// Disarms the request's `ClientAbortGuard` — that guard's `Drop`
    /// early-returns on this flag. Keep this single-caller (`disarm()` in
    /// `server/connection.rs`): calling it from anywhere else pre-disarms the
    /// guard, and a client abort then stops cancelling the in-flight request,
    /// silently.
    pub(crate) fn mark_done(&self) {
        // Release pairs with the Acquire load in `is_done()`. Today both run
        // on one thread — `disarm()` stores, the guard's own Drop loads two
        // lines later — so the ordering is not currently load-bearing; it is
        // kept so the pair stays correct if the store and the load ever end
        // up on different tasks.
        self.done.store(true, Ordering::Release);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn set(&self, reason: CancelReason) -> bool {
        self.reason
            .compare_exchange(
                CancelReason::None as u8,
                reason as u8,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    pub fn get(&self) -> CancelReason {
        match self.reason.load(Ordering::Relaxed) {
            1 => CancelReason::ClientAbort,
            2 => CancelReason::Timeout,
            3 => CancelReason::Shutdown,
            4 => CancelReason::Stuck,
            5 => CancelReason::UserCancel,
            _ => CancelReason::None,
        }
    }

    /// Takes a queued request out of the queue, for whatever this caller means
    /// to do with it: run it, or refuse it on its deadline. True for exactly
    /// one caller — the worker that picks the request up, or the waiting side
    /// when the deadline fires first. Everyone else must leave it alone; it is
    /// already answered, or about to be, by whoever won.
    ///
    /// Only reached by a request carrying a deadline: with
    /// `QUEUE_WAIT_TIMEOUT_MS=0` nothing waits on a timer and there is no
    /// second side to race.
    pub fn claim_from_queue(&self) -> bool {
        self.claimed_from_queue
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn as_ptr(&self) -> *const AtomicU8 {
        &self.reason as *const _
    }
}

impl Default for CancellationState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn new_starts_as_none() {
        let s = CancellationState::new();
        assert_eq!(s.get(), CancelReason::None);
    }

    #[test]
    fn first_set_wins_returns_true() {
        let s = CancellationState::new();
        assert!(s.set(CancelReason::ClientAbort));
        assert_eq!(s.get(), CancelReason::ClientAbort);
        assert!(!s.set(CancelReason::Timeout));
        assert_eq!(s.get(), CancelReason::ClientAbort);
    }

    #[test]
    fn concurrent_set_only_one_wins() {
        let s = Arc::new(CancellationState::new());
        let s1 = s.clone();
        let s2 = s.clone();
        let h1 = thread::spawn(move || s1.set(CancelReason::ClientAbort));
        let h2 = thread::spawn(move || s2.set(CancelReason::Timeout));
        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();
        assert_ne!(r1, r2, "exactly one set call must succeed");
        let final_reason = s.get();
        assert!(matches!(
            final_reason,
            CancelReason::ClientAbort | CancelReason::Timeout
        ));
    }

    #[test]
    fn cache_line_alignment() {
        assert_eq!(std::mem::align_of::<CancellationState>(), 64);
    }

    #[test]
    fn done_starts_false() {
        let s = CancellationState::new();
        assert!(!s.is_done());
    }

    #[test]
    fn mark_done_sets_flag() {
        let s = CancellationState::new();
        s.mark_done();
        assert!(s.is_done());
        // done is independent of reason; setting reason still works.
        assert!(s.set(CancelReason::ClientAbort));
        assert_eq!(s.get(), CancelReason::ClientAbort);
        assert!(s.is_done());
    }

    #[test]
    fn queue_claim_is_won_exactly_once_and_moves_nothing_else() {
        let s = CancellationState::new();
        assert!(s.claim_from_queue());
        assert!(!s.claim_from_queue());
        assert!(!s.claim_from_queue());
        // Independent of cancellation: a claimed request is one side's to
        // answer, which says nothing about the client still being there.
        assert!(!s.is_done());
        assert_eq!(s.get(), CancelReason::None);
    }

    #[test]
    fn concurrent_queue_claim_has_one_winner() {
        // The dispatch task's timer and a worker's pickup can reach a request
        // in the same instant. Two winners would answer it twice, count one
        // refusal as two, and run a handler for a client already sent a 529.
        for _ in 0..256 {
            let s = Arc::new(CancellationState::new());
            let a = Arc::clone(&s);
            let b = Arc::clone(&s);
            let h1 = thread::spawn(move || a.claim_from_queue());
            let h2 = thread::spawn(move || b.claim_from_queue());
            let won = [h1.join().unwrap(), h2.join().unwrap()];
            assert_eq!(won.iter().filter(|w| **w).count(), 1);
        }
    }
}
