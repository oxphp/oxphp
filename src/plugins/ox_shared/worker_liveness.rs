//! Per-thread liveness registry for Shared\Pool cross-thread release.
//!
//! The pool uses `pthread_self() as u64` (`ThreadKey`) to route slots
//! back to their owner's idle deque. When a slot is released from a
//! non-owner thread we need to answer one question: is the owner
//! still alive? If it is, we park normally. If it is gone (panic +
//! respawn, scale-down) we destroy the resource inline to avoid
//! orphaning the slot in a deque nobody will ever read.
//!
//! `pthread_kill(tid, 0)` is NOT a reliable answer. Both glibc
//! NPTL and macOS recycle the `struct pthread` storage after
//! `pthread_join` / `pthread_detach`, and `pthread_self()` for a
//! freshly-spawned thread frequently hands back the same value the
//! just-exited thread used. The probe then reports "alive" for a
//! completely unrelated thread and we park into its (stale) deque.
//!
//! This module side-steps the hazard with an explicit
//! `DashSet<ThreadKey>`: each SAPI worker thread calls
//! `register_worker()` on entry and `unregister_worker()` on exit
//! (both clean and panic paths — see `src/executor/sapi/traditional.rs`).
//! `is_thread_alive(k)` is an O(1) set lookup with zero syscalls.

use std::sync::OnceLock;

use dashmap::DashSet;

use crate::plugins::ox_shared::types::pool::{current_thread_key, ThreadKey};

static LIVE_WORKERS: OnceLock<DashSet<ThreadKey>> = OnceLock::new();

fn registry() -> &'static DashSet<ThreadKey> {
    LIVE_WORKERS.get_or_init(DashSet::new)
}

/// Mark the calling thread as a live SAPI worker. Idempotent — a
/// second call from the same thread is a no-op (DashSet dedupes).
///
/// Called from `run_worker_loop` right after the per-thread TSRM /
/// bridge TLS setup and before the first request is pulled. Missing
/// this call is a correctness hazard: cross-thread release on any
/// slot this worker produces will be misclassified as "owner dead"
/// and the resource will be destroyed prematurely.
pub fn register_worker() {
    registry().insert(current_thread_key());
}

/// Remove the calling thread from the live-worker set. Idempotent.
///
/// MUST be called on every exit path the worker takes — the normal
/// channel-closed tail, the panic-break, and any future early-return
/// branch. Miss an unregister and the slot will park forever in a
/// deque whose owner never comes back.
///
/// Before removing, this invokes the chaos-reclaim hook:
/// `types::pool::reclaim_all_pools_for_dead_worker` walks every Pool
/// in the SharedRegistry and refunds budget for any slots the dying
/// worker was holding in-flight (invariant: mid-acquire panic must
/// not leak budget).
pub fn unregister_worker() {
    let me = current_thread_key();
    crate::plugins::ox_shared::types::pool::reclaim_all_pools_for_dead_worker(me);
    if let Some(reg) = LIVE_WORKERS.get() {
        reg.remove(&me);
    }
}

/// `true` iff `key` is currently registered as a live worker.
///
/// Called by `PoolInner::release` on the cross-thread branch. The
/// same-thread branch in `release` short-circuits before this check,
/// so the hot-path release cost is unchanged.
#[inline]
pub fn is_thread_alive(key: ThreadKey) -> bool {
    match LIVE_WORKERS.get() {
        Some(reg) => reg.contains(&key),
        // No registry means no workers registered yet — safest answer
        // is "not alive", which falls back to the inline-destroy path.
        // In practice this only fires during host unit tests that
        // never spin up a SAPI worker.
        None => false,
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn clear_for_test() {
    if let Some(reg) = LIVE_WORKERS.get() {
        reg.clear();
    }
}

/// Test helper: mark an arbitrary `ThreadKey` as alive without
/// requiring that the caller run on that thread. Used by pool tests
/// to simulate "owner A is still alive and parked on its worker
/// loop" from an unrelated test thread.
#[cfg(test)]
pub(crate) fn force_insert(key: ThreadKey) {
    registry().insert(key);
}

/// Test helper: counterpart to `force_insert`. Simulates an owner
/// thread exiting (clean or panic path) so subsequent cross-thread
/// releases see the dead-owner branch.
#[cfg(test)]
pub(crate) fn force_remove(key: ThreadKey) {
    if let Some(reg) = LIVE_WORKERS.get() {
        reg.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    // The registry is process-wide, so tests that mutate it must not
    // assume a clean state. Each test either uses a unique synthesised
    // key or asserts before/after deltas rather than absolutes.

    #[test]
    fn self_registration_is_visible() {
        let me = current_thread_key();
        // Pre-condition: may or may not be registered depending on
        // test ordering. Force a known state, then verify round-trip.
        unregister_worker();
        assert!(!is_thread_alive(me));

        register_worker();
        assert!(is_thread_alive(me));

        unregister_worker();
        assert!(!is_thread_alive(me));
    }

    #[test]
    fn registration_is_per_thread() {
        let main_key = current_thread_key();
        register_worker();

        let spawned_key = thread::spawn(|| {
            let k = current_thread_key();
            register_worker();
            k
        })
        .join()
        .unwrap();

        // Main's registration is independent of the spawned thread's
        // lifecycle. Spawned thread exited without unregistering — its
        // key remains in the set (this is exactly the hazard we want
        // the explicit unregister call in the worker loop to avoid).
        assert!(is_thread_alive(main_key));
        assert!(is_thread_alive(spawned_key));

        // Cleanup so other tests don't observe leftover state. In
        // production the worker loop's tail handles this.
        registry().remove(&spawned_key);
        unregister_worker();
    }

    #[test]
    fn register_is_idempotent() {
        let me = current_thread_key();
        register_worker();
        register_worker();
        register_worker();
        assert!(is_thread_alive(me));

        // Single unregister clears it regardless of how many times
        // register ran — matches the set's dedup semantics.
        unregister_worker();
        assert!(!is_thread_alive(me));
    }

    #[test]
    fn unknown_key_is_not_alive() {
        const GHOST: ThreadKey = 0xDEAD_BEEF_DEAD_BEEF;
        assert!(!is_thread_alive(GHOST));
    }
}
