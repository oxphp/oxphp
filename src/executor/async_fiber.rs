//! Fiber-mode async worker support: the process-global in-flight bound and the
//! per-task driver bookkeeping shared by the async worker scheduler loop.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Process-global bound on `queued + running` async tasks.
///
/// `limit` is the per-worker `ASYNC_MAX_FIBERS` cap multiplied by the worker
/// count; `0` disables acquisition entirely (a pool with no workers can never
/// admit a task). The counter is incremented at dispatch (`oxphp_async()`
/// enqueue) and decremented at fiber completion, so it bounds both concurrency
/// and task-queue memory. Overflow is a non-blocking reject — never a block —
/// which is what keeps fan-out composition from deadlocking.
pub struct InFlightCounter {
    current: AtomicUsize,
    limit: usize,
}

impl InFlightCounter {
    pub fn new(limit: usize) -> Self {
        Self {
            current: AtomicUsize::new(0),
            limit,
        }
    }

    /// CAS-bounded increment. Returns `false` (reject) when already at or over
    /// `limit`; `true` after a successful reservation.
    pub fn try_acquire(&self) -> bool {
        let mut cur = self.current.load(Ordering::Relaxed);
        loop {
            if cur >= self.limit {
                return false;
            }
            match self.current.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Release one permit reserved by a prior successful `try_acquire`.
    pub fn release(&self) {
        let _ = self.current.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn current(&self) -> usize {
        self.current.load(Ordering::Relaxed)
    }

    /// Configured cap (`ASYNC_MAX_FIBERS` × worker count). Constant for the
    /// process lifetime; exposed so `/metrics` can publish the saturation
    /// ceiling alongside the live in-flight count.
    pub fn limit(&self) -> usize {
        self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_rejects_when_at_limit() {
        let c = InFlightCounter::new(2);
        assert!(c.try_acquire()); // 1
        assert!(c.try_acquire()); // 2
        assert!(!c.try_acquire()); // 3 -> rejected (at cap)
        c.release(); // back to 1
        assert!(c.try_acquire()); // ok again
    }

    #[test]
    fn cap_zero_means_disabled_pool_never_acquires() {
        let c = InFlightCounter::new(0);
        assert!(!c.try_acquire());
    }

    #[test]
    fn limit_reports_configured_cap() {
        let c = InFlightCounter::new(7);
        assert_eq!(c.limit(), 7);
    }
}
