//! Idle-timeout eviction for `Shared\Pool`.
//!
//! Architecture: **central scheduler + per-thread flag + owner-driven
//! drain while the owner has no request in flight.**
//!
//! A background Tokio task wakes every `scan_interval` ms, walks the
//! `SharedRegistry`, and for every `PoolInner` entry collects the set
//! of owner `ThreadKey`s whose deques have a stale front slot
//! (`front.last_active.elapsed() >= pool.idle_timeout`). For each such
//! owner it sets an atomic flag in `EVICT_FLAGS[owner_key]` and wakes the
//! owner through its `PhpThreadWaker`.
//!
//! PHP threads register their flag as they start, next to
//! `worker_liveness::register_worker`, with the waker of the thread when
//! it has one. A woken thread raises `thread_idle` once it has no request
//! in flight — at once if it is waiting for one — and the handler calls
//! `take_evict_request()`: an atomic swap that clears the flag and tells
//! us whether to drain. On a positive read it calls
//! `drain_stale_for_current_thread()`, which walks the registry again and
//! pops stale slots from every `PoolInner`'s *own* idle deque, destroying
//! each with a bailout held back for the serve loop that raised the event.
//!
//! Only the HTTP workers in worker mode have a waker. On the other PHP
//! threads the flag is set and never taken: without worker mode a pool's
//! slots end with the request that created it, and `oxphp run` and the
//! `oxphp_async()` pool have no point between requests to drain at.
//!
//! Why drain on the PHP thread and not from the scheduler? Because
//! `$destroy` runs Zend bytecode, which needs that thread's engine, and
//! calling into PHP from the Tokio thread would break the per-thread
//! affinity invariant the pool's whole design rests on.
//!
//! Dead owners: a thread unregisters its flag as it stops, so the
//! scheduler signals only live ones, and `worker_liveness::unregister_worker`
//! forgets the slots it had parked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::bridge::ffi;
use crate::plugin::PhpThreadWaker;
use crate::plugins::ox_shared::registry::{self, Entry};
use crate::plugins::ox_shared::types::pool::{
    current_thread_key, EvictReason, SharedInnerPoolExt, ThreadKey,
};

/// A thread's request to evict, and what wakes the thread to look at it.
struct EvictFlag {
    requested: AtomicBool,
    waker: Option<PhpThreadWaker>,
}

static EVICT_FLAGS: OnceLock<DashMap<ThreadKey, EvictFlag>> = OnceLock::new();
static SCHEDULER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Default scheduler cadence. Picked to balance responsiveness (users
/// expect idle eviction within ~1s of the timeout elapsing) against
/// the cost of the scan (one lock-peek per idle deque per Pool).
pub const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_millis(500);

fn flags() -> &'static DashMap<ThreadKey, EvictFlag> {
    EVICT_FLAGS.get_or_init(DashMap::new)
}

/// Allocate an evict flag for the calling thread, woken through `waker`
/// when it has one. Idempotent — subsequent calls re-use the existing
/// flag so an in-flight `request_evict` is never stomped.
pub fn register(key: ThreadKey, waker: Option<PhpThreadWaker>) {
    flags()
        .entry(key)
        .and_modify(|f| f.waker.clone_from(&waker))
        .or_insert_with(|| EvictFlag {
            requested: AtomicBool::new(false),
            waker,
        });
}

/// Remove the calling thread's evict flag. Any subsequent
/// `request_evict` for this key is a no-op (live workers only).
pub fn unregister(key: ThreadKey) {
    if let Some(m) = EVICT_FLAGS.get() {
        m.remove(&key);
    }
}

/// Raise the evict flag for `key` and wake the thread, if it is still a
/// live worker. Returns `true` when a flag was actually set, `false` when
/// the owner has already unregistered.
pub fn request_evict(key: ThreadKey) -> bool {
    if let Some(m) = EVICT_FLAGS.get() {
        if let Some(f) = m.get(&key) {
            f.requested.store(true, Ordering::Release);
            if let Some(waker) = &f.waker {
                waker.wake();
            }
            return true;
        }
    }
    false
}

/// Atomic-swap the current thread's flag to `false`. Returns the
/// prior value — `true` means the scheduler asked us to drain.
pub fn take_evict_request() -> bool {
    if let Some(m) = EVICT_FLAGS.get() {
        if let Some(f) = m.get(&current_thread_key()) {
            return f.requested.swap(false, Ordering::AcqRel);
        }
    }
    false
}

/// True once `start_scheduler` has claimed the one-shot latch. Tests that
/// rest on "the scheduler has not been started yet" read it to check that
/// premise instead of assuming it.
#[cfg(test)]
pub(crate) fn scheduler_running() -> bool {
    SCHEDULER_RUNNING.load(Ordering::Acquire)
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

/// Owner-side drain: walk every Pool and evict this thread's own stale
/// idle slots. Called from the `thread_idle` handler after a positive
/// `take_evict_request`.
///
/// Stops at the first pool whose `$destroy` a bailout ended: while that
/// bailout is held back, every `$destroy` called after it is refused
/// without running, and the slot handed to it would be counted as
/// destroyed without having been. The pools not reached keep their stale
/// slots until the thread is asked again, which on a worker the bailout
/// retires is never — they go with it. A pool not reached that the walk
/// held the last reference to is torn down without any `$destroy`.
pub fn drain_stale_for_current_thread() {
    let Some(reg) = registry::REGISTRY.get() else {
        return;
    };
    drain_stale_in(reg.iter_entries());
}

/// The walk of `drain_stale_for_current_thread` over `entries`, in their
/// order.
pub(crate) fn drain_stale_in(entries: impl IntoIterator<Item = Arc<Entry>>) {
    for entry in entries {
        if let Some(pool) = entry.inner.as_any_pool() {
            pool.evict_stale_on_current_thread(EvictReason::IdleTimeout);
        }
        // Let go of it before asking: this can be the last reference to the
        // pool — another thread let go of its own meanwhile — or to a map
        // or channel holding the last one, and then that pool's teardown
        // runs `$destroy` on this thread's idle slots, which can bail as
        // well.
        drop(entry);
        // SAFETY: reads a thread-local of the bridge; no arguments.
        if unsafe { ffi::oxphp_bridge_deferred_bailout_pending() } != 0 {
            break;
        }
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
        register(k, None);
        assert!(!take_evict_request(), "freshly registered flag is clear");
        unregister(k);
    }

    #[test]
    fn request_then_take_round_trip() {
        let k = current_thread_key();
        unregister(k);
        register(k, None);
        assert!(request_evict(k));
        assert!(take_evict_request(), "flag must read back true once");
        assert!(
            !take_evict_request(),
            "second take after swap must return false"
        );
        unregister(k);
    }

    #[test]
    fn request_evict_wakes_the_thread_it_flags() {
        let k = current_thread_key();
        unregister(k);
        let woken = crate::plugin::php_thread::install_waker();
        register(k, PhpThreadWaker::current());
        assert!(woken.try_recv().is_err());
        assert!(request_evict(k));
        assert!(woken.try_recv().is_ok(), "the flagged thread is woken");
        assert!(take_evict_request());
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
        register(main_key, None);
        assert!(!take_evict_request());

        let spawned_key = thread::spawn(|| {
            let k = current_thread_key();
            register(k, None);
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
