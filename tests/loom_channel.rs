//! Loom concurrency stress for Channel lock-free invariants.
//! Run with:
//!   RUSTFLAGS="--cfg loom" cargo test --test loom_channel \
//!     --no-default-features --features plugin-shared \
//!     --release -- --test-threads=1
//!
//! Not part of default CI — nightly job territory. Matches the pattern
//! established by `tests/loom_mutex.rs` and `tests/loom_counter.rs`.
//!
//! Scope limitation (important):
//!   The real `ChannelInner` layers `parking_lot::Mutex`,
//!   `crossbeam_channel`, `tokio::sync::Notify`, and a pile of
//!   `std::sync::atomic` state. Loom cannot model any of those — it
//!   only instruments its own `loom::sync` / `loom::thread` wrappers.
//!   So rather than import the real type (which would compile but not
//!   actually get explored), we mirror the two lock-free sub-invariants
//!   that loom CAN cover:
//!
//!     1. `pending` counter stays consistent across concurrent
//!        increment/decrement pairs (what `try_send` and `try_recv`
//!        do on `self.pending: AtomicUsize`). A lost update here would
//!        desync gauges / observability.
//!
//!     2. `closed` flag ordering: a `close()` on one thread is visible
//!        to an `is_closed()` check on any other thread, with no
//!        acquire/release torn reads. Mirrors the `close()` fast-path.
//!
//!   The MPMC MPSC try_send/try_recv body uses `crossbeam_channel`,
//!   which is already an extensively-loom-tested upstream crate; there
//!   is no useful additional exploration we can bolt on top without
//!   rewriting `ChannelInner` against loom-shims. Known limitation;
//!   nightly integration tests cover the real implementation end-to-end.

#![cfg(loom)]

use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Standalone mirror of the ChannelInner lock-free counters. See the
/// module docstring for why this is a mirror rather than the real type.
struct LoomChannelCounters {
    pending: AtomicUsize,
    closed: AtomicBool,
}

impl LoomChannelCounters {
    fn new() -> Self {
        Self {
            pending: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }
    fn inc_pending(&self) {
        self.pending.fetch_add(1, Ordering::AcqRel);
    }
    fn dec_pending(&self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
    }
    fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// Invariant 1: a producer incrementing `pending` and a consumer
/// decrementing it concurrently never lose or double-apply an update.
/// Net change = +1 (one prod, zero cons), −1 (zero prod, one cons), or
/// 0 (both ran once) depending on interleaving — but the counter never
/// goes wildly out of range.
#[test]
fn pending_counter_survives_concurrent_ops() {
    loom::model(|| {
        let c = Arc::new(LoomChannelCounters::new());

        // Prime with one item so the consumer has something to take.
        c.inc_pending();

        let p = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                c.inc_pending();
            })
        };
        let q = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                c.dec_pending();
            })
        };
        p.join().unwrap();
        q.join().unwrap();

        // Started at 1, +1 and −1 in some order → net 1.
        assert_eq!(c.pending(), 1);
    });
}

/// Invariant 2: `close()` on one thread becomes visible to an
/// `is_closed()` check on any other thread. Mirrors the Mutex
/// `poison_visible_across_threads` loom test.
#[test]
fn closed_visible_across_threads() {
    loom::model(|| {
        let c = Arc::new(LoomChannelCounters::new());

        let closer = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                c.close();
            })
        };
        let observer = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                // Value ignored — loom explores both orderings.
                let _ = c.is_closed();
            })
        };
        closer.join().unwrap();
        observer.join().unwrap();

        // After both joins, the close must be visible.
        assert!(c.is_closed());
    });
}

/// Invariant 3: `close()` races with a concurrent `pending` bump.
/// Neither side should deadlock or produce a torn read. Both orderings
/// must leave: closed == true AND pending observable as either 0 or 1
/// with acquire-load.
#[test]
fn close_races_with_pending_bump() {
    loom::model(|| {
        let c = Arc::new(LoomChannelCounters::new());

        let closer = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                c.close();
            })
        };
        let sender = {
            let c = Arc::clone(&c);
            loom::thread::spawn(move || {
                // Real try_send would check is_closed() first; the key
                // thing we're exercising here is that the two atomic
                // stores don't tear relative to one another.
                if !c.is_closed() {
                    c.inc_pending();
                }
            })
        };
        closer.join().unwrap();
        sender.join().unwrap();

        // After joins: closed is visible; pending is 0 (sender saw the
        // close first) or 1 (sender bumped before close landed).
        assert!(c.is_closed());
        let p = c.pending();
        assert!(p == 0 || p == 1, "unexpected pending: {p}");
    });
}
