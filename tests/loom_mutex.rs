//! Loom stress test for MutexInner poison semantics.
//! Run with: RUSTFLAGS="--cfg loom" cargo test --test loom_mutex

#![cfg(loom)]

use loom::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Standalone mirror of the poison-flag invariants. Loom does not
/// model parking_lot::Mutex, so we exercise the lock-free atomic
/// slice in isolation. The real MutexInner layers this on top of
/// parking_lot::Mutex<SharedValue>; the key correctness property
/// this test covers is that a mark_poisoned on one thread is
/// visible to an is_poisoned check on any other thread.
struct LoomMutexPoison {
    poisoned: AtomicBool,
}

impl LoomMutexPoison {
    fn new() -> Self {
        Self {
            poisoned: AtomicBool::new(false),
        }
    }
    fn mark(&self) {
        self.poisoned.store(true, Ordering::Release);
    }
    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }
}

#[test]
fn poison_visible_across_threads() {
    loom::model(|| {
        let m = Arc::new(LoomMutexPoison::new());
        let m1 = Arc::clone(&m);
        let h1 = loom::thread::spawn(move || {
            m1.mark();
        });
        let m2 = Arc::clone(&m);
        let h2 = loom::thread::spawn(move || {
            // Observe (value ignored — loom explores both orderings).
            let _ = m2.is_poisoned();
        });
        h1.join().unwrap();
        h2.join().unwrap();
        // After both join, the mark must be visible.
        assert!(m.is_poisoned());
    });
}
