//! Loom stress test for CounterInner under concurrent CAS.
//! Run with: RUSTFLAGS="--cfg loom" cargo test --test loom_counter
//! Not part of default CI; nightly job territory.

#![cfg(loom)]

use loom::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// Standalone mirror of CounterInner for loom (can't use the real one
/// because loom types are cfg-gated into `loom::sync::atomic`).
struct LoomCounter {
    value: AtomicI64,
}

impl LoomCounter {
    fn new(v: i64) -> Self {
        Self {
            value: AtomicI64::new(v),
        }
    }
    fn add(&self, d: i64) -> i64 {
        self.value.fetch_add(d, Ordering::SeqCst) + d
    }
    fn get(&self) -> i64 {
        self.value.load(Ordering::SeqCst)
    }
}

#[test]
fn counter_add_from_two_threads() {
    loom::model(|| {
        let c = Arc::new(LoomCounter::new(0));
        let a = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                c.add(1);
            })
        };
        let b = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                c.add(1);
            })
        };
        a.join().unwrap();
        b.join().unwrap();
        assert_eq!(c.get(), 2);
    });
}
