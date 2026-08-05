//! The clock the worker pool measures idleness on, and the stamp the worker
//! loops write to it.
//!
//! Deliberately outside the PHP-gated executor: nothing here touches the
//! engine, and a host without `libphp.so` — which is every CI run of the test
//! suite — would otherwise never execute the tests below.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// Monotonic ms-since-process-start. Used for `last_active` stamps and
/// idle-timeout math — never for user-visible timestamps. Monotonic clock
/// avoids false idle detection if the system wall clock jumps backwards.
pub(crate) fn now_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// When a worker last took a request, on the clock the scale manager reads.
///
/// The stamp is private so the clock cannot be chosen at the call site. It used
/// to be a bare `AtomicU64` handed down through four layers to the worker
/// loops, and one of those loops stamped the Unix epoch into it while the
/// manager measured ages against `now_millis()`. Twelve orders of magnitude
/// apart, so the saturating subtraction below returned zero for every worker
/// that had ever served a request: none of them could look idle again, which
/// left the pool spawning until it hit its ceiling and made the idle timeout
/// unreachable for anyone actually serving traffic.
pub(crate) struct LastActive(AtomicU64);

// The worker pool that stamps and reads these is compiled only with the PHP
// engine linked in; the type lives out here so its tests run on a host without
// one, which is where the clock mismatch this guards against would surface.
#[cfg_attr(not(feature = "php"), allow(dead_code))]
impl LastActive {
    /// A worker counts as just-active the moment it is spawned: it must not be
    /// retired before it has had the chance to receive anything.
    pub(crate) fn now() -> Self {
        Self(AtomicU64::new(now_millis()))
    }

    /// Record that this worker has taken a request.
    pub(crate) fn touch(&self) {
        self.0.store(now_millis(), Ordering::Relaxed);
    }

    /// Milliseconds since the last `touch`, against a `now` the caller read
    /// from `now_millis()` — one reading per tick, so every worker in a pass
    /// is judged against the same instant.
    pub(crate) fn idle_ms(&self, now: u64) -> u64 {
        now.saturating_sub(self.0.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_millis_is_monotonic() {
        let a = now_millis();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = now_millis();
        assert!(b >= a, "now_millis must be non-decreasing");
        // Sanity bound: a single sleep+call shouldn't exceed 10s even on slow CI.
        assert!(b - a < 10_000);
    }

    // The pool's scale decisions are a subtraction between what a worker loop
    // writes and what the manager reads, and the interesting failure is not a
    // wrong number but two different clocks: a stamp taken from the wall clock
    // is ~1.7e12 against a `now` of ~1e4, so every age saturates to zero and no
    // worker that has served can ever look idle again. These assert the two
    // sides agree, which is why one type owns both of them.

    #[test]
    fn touch_reads_as_just_active() {
        let la = LastActive::now();
        la.touch();
        let age = la.idle_ms(now_millis());
        assert!(
            age < 200,
            "a worker that just took a request must not be idle by the scale-up threshold, got {age}ms"
        );
    }

    #[test]
    fn stamp_ages_past_the_idle_threshold() {
        let la = LastActive::now();
        la.touch();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let age = la.idle_ms(now_millis());
        assert!(
            age > 200,
            "a worker quiet for 300ms must cross a 200ms threshold, got {age}ms"
        );
        // Upper bound catches the mirror-image mismatch — a `now` on a
        // different origin than the stamp reads as an implausible age rather
        // than as zero.
        assert!(age < 10_000, "age of a 300ms-old stamp read as {age}ms");
    }
}
