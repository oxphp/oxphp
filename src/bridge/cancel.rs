//! Per-request cancellation state shared between the tokio dispatch
//! task and the worker thread.

use std::sync::atomic::{AtomicU8, Ordering};

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

#[repr(C, align(64))]
pub struct CancellationState {
    reason: AtomicU8,
}

impl CancellationState {
    pub fn new() -> Self {
        Self {
            reason: AtomicU8::new(CancelReason::None as u8),
        }
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
}
