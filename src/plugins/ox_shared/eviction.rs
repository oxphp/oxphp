//! Idle-timeout eviction for `Shared\Pool`.
//!
//! Architecture: **central scheduler + per-worker flag + worker-driven
//! drain inside the request frame.**
//!
//! A background Tokio task wakes every `scan_interval` ms, walks the
//! `SharedRegistry`, and for every `PoolInner` entry collects the set
//! of owner `ThreadKey`s whose deques have a stale front slot
//! (`front.last_active.elapsed() >= pool.idle_timeout`). For each such
//! owner it sets an atomic flag in `EVICT_FLAGS[owner_key]`.
//!
//! SAPI worker threads register their flag on entry (next to
//! `worker_liveness::register_worker`). At the top of every request —
//! *after* `php_request_startup` so `EG(current_execute_data)` is
//! active — the worker calls `take_evict_request()`: an atomic swap
//! that clears the flag and tells us whether to drain. On a positive
//! read the worker calls `drain_stale_for_current_thread()`, which
//! walks the registry again and pops stale slots from every
//! `PoolInner`'s *own* idle deque, destroying each through the usual
//! `destroy_slot` path.
//!
//! Why drain inside a request and not at the top of the recv loop?
//! Because `$destroy` runs Zend bytecode. Between requests the VM's
//! `EG(current_execute_data)` is NULL and `zend_call_known_function`
//! would misbehave. Inside the request frame Zend context is live and
//! the cost is a single atomic load per request on the happy path.
//!
//! Why not destroy directly from the scheduler? Because the scheduler
//! runs on the Tokio thread. Calling into PHP from there would break
//! the per-thread affinity invariant the pool's whole design rests on.
//!
//! Dead owners: the scheduler only signals owners that are currently
//! in the `worker_liveness` set. An orphan deque whose owner has
//! terminated is left untouched until `on_shutdown_notify` drains it
//! during graceful shutdown. See `99-deferred.md` for the rationale
//! and the associated RSS bound.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::plugins::ox_shared::registry;
use crate::plugins::ox_shared::types::pool::{
    current_thread_key, EvictReason, SharedInnerPoolExt, ThreadKey,
};

static EVICT_FLAGS: OnceLock<DashMap<ThreadKey, Arc<AtomicBool>>> = OnceLock::new();
static SCHEDULER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Default scheduler cadence. Picked to balance responsiveness (users
/// expect idle eviction within ~1s of the timeout elapsing) against
/// the cost of the scan (one lock-peek per idle deque per Pool).
pub const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_millis(500);

fn flags() -> &'static DashMap<ThreadKey, Arc<AtomicBool>> {
    EVICT_FLAGS.get_or_init(DashMap::new)
}

/// Allocate an evict flag for the calling thread. Idempotent —
/// subsequent calls re-use the existing `Arc<AtomicBool>` so an
/// in-flight `request_evict` is never stomped.
pub fn register(key: ThreadKey) {
    flags()
        .entry(key)
        .or_insert_with(|| Arc::new(AtomicBool::new(false)));
}

/// Remove the calling thread's evict flag. Any subsequent
/// `request_evict` for this key is a no-op (live workers only).
pub fn unregister(key: ThreadKey) {
    if let Some(m) = EVICT_FLAGS.get() {
        m.remove(&key);
    }
}

/// Raise the evict flag for `key` if the thread is still a live
/// worker. Returns `true` when a flag was actually set, `false` when
/// the owner has already unregistered.
pub fn request_evict(key: ThreadKey) -> bool {
    if let Some(m) = EVICT_FLAGS.get() {
        if let Some(flag) = m.get(&key) {
            flag.store(true, Ordering::Release);
            return true;
        }
    }
    false
}

/// Atomic-swap the current thread's flag to `false`. Returns the
/// prior value — `true` means the scheduler asked us to drain.
pub fn take_evict_request() -> bool {
    if let Some(m) = EVICT_FLAGS.get() {
        if let Some(flag) = m.get(&current_thread_key()) {
            return flag.swap(false, Ordering::AcqRel);
        }
    }
    false
}

/// Start the background scheduler on the current Tokio runtime.
/// Idempotent. Called from `SharedPlugin::on_ready`.
pub fn start_scheduler(interval: Duration) {
    if SCHEDULER_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            run_scan();
        }
    });
}

/// Walk every Pool in the registry and raise evict flags on the
/// owners whose idle deque front is past its pool's idle_timeout.
/// Public so tests can trigger a scan without waiting for the
/// scheduler's sleep interval.
pub fn run_scan() {
    let Some(reg) = registry::REGISTRY.get() else {
        return;
    };
    let now = Instant::now();
    for entry in reg.iter_entries() {
        let Some(pool) = entry.inner.as_any_pool() else {
            continue;
        };
        for owner in pool.stale_owners(now) {
            request_evict(owner);
        }
    }
}

/// Worker-side drain: walk every Pool and evict this thread's own
/// stale idle slots. Called from the request-frame hook after a
/// positive `take_evict_request`.
pub fn drain_stale_for_current_thread() {
    let Some(reg) = registry::REGISTRY.get() else {
        return;
    };
    for entry in reg.iter_entries() {
        let Some(pool) = entry.inner.as_any_pool() else {
            continue;
        };
        pool.evict_stale_on_current_thread(EvictReason::IdleTimeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn register_then_take_returns_false() {
        let k = current_thread_key();
        unregister(k);
        register(k);
        assert!(!take_evict_request(), "freshly registered flag is clear");
        unregister(k);
    }

    #[test]
    fn request_then_take_round_trip() {
        let k = current_thread_key();
        unregister(k);
        register(k);
        assert!(request_evict(k));
        assert!(take_evict_request(), "flag must read back true once");
        assert!(
            !take_evict_request(),
            "second take after swap must return false"
        );
        unregister(k);
    }

    #[test]
    fn request_for_unregistered_key_is_noop() {
        const GHOST: ThreadKey = 0xCAFE_0000_0000_0001;
        unregister(GHOST);
        assert!(!request_evict(GHOST));
    }

    #[test]
    fn flag_is_per_thread() {
        let main_key = current_thread_key();
        unregister(main_key);
        register(main_key);
        assert!(!take_evict_request());

        let spawned_key = thread::spawn(|| {
            let k = current_thread_key();
            register(k);
            // Spawned thread's flag is distinct from main's.
            assert!(!take_evict_request());
            request_evict(k);
            assert!(take_evict_request());
            unregister(k);
            k
        })
        .join()
        .unwrap();

        // Main thread's flag is still clear — spawned's activity did
        // not bleed over.
        assert!(!take_evict_request());
        assert_ne!(main_key, spawned_key);
        unregister(main_key);
    }
}
