//! Cross-thread deadlock detection via wait-for graph.
//!
//! Spec: .internal/technical-docs/en/features/shared/06-mutex-contract.md
//!       §Cross-thread deadlock detection — 3 levels

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use smallvec::SmallVec;

use crate::plugins::ox_shared::config::LockDiagnosticsLevel;
use crate::plugins::ox_shared::registry::SharedId;

/// Per-mutex wait state: current holder thread + queue of waiting
/// threads (FIFO in insertion order).
#[derive(Default, Debug)]
pub struct WaitState {
    pub holder: Option<ThreadId>,
    pub waiters: SmallVec<[ThreadId; 4]>,
    pub last_updated: Option<Instant>,
}

static WAITER_GRAPH: OnceLock<DashMap<SharedId, WaitState>> = OnceLock::new();
static CYCLES_DETECTED: AtomicU64 = AtomicU64::new(0);
static BREAK_REQUESTS: OnceLock<DashMap<ThreadId, BreakSignal>> = OnceLock::new();
static DETECTOR_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
pub struct BreakSignal {
    pub cycle_id: u64,
}

fn graph() -> &'static DashMap<SharedId, WaitState> {
    WAITER_GRAPH.get_or_init(DashMap::new)
}

fn break_reqs() -> &'static DashMap<ThreadId, BreakSignal> {
    BREAK_REQUESTS.get_or_init(DashMap::new)
}

/// Register this thread as waiting on `mutex_id`. Returns a guard
/// that removes the waiter on drop (covers timeout/panic/normal
/// acquire paths).
pub fn register_waiter(mutex_id: SharedId) -> WaiterGuard {
    let tid = std::thread::current().id();
    graph().entry(mutex_id).or_default().waiters.push(tid);
    WaiterGuard { mutex_id, tid }
}

pub struct WaiterGuard {
    mutex_id: SharedId,
    tid: ThreadId,
}

impl WaiterGuard {
    /// Promote this thread from waiter to holder (called after
    /// try_lock_for success).
    pub fn promote_to_holder(self) -> HolderGuard {
        if let Some(mut e) = graph().get_mut(&self.mutex_id) {
            if let Some(pos) = e.waiters.iter().rposition(|x| *x == self.tid) {
                e.waiters.swap_remove(pos);
            }
            e.holder = Some(self.tid);
            e.last_updated = Some(Instant::now());
        }
        let mutex_id = self.mutex_id;
        let tid = self.tid;
        std::mem::forget(self);
        HolderGuard { mutex_id, tid }
    }
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        if let Some(mut e) = graph().get_mut(&self.mutex_id) {
            if let Some(pos) = e.waiters.iter().rposition(|x| *x == self.tid) {
                e.waiters.swap_remove(pos);
            }
        }
    }
}

/// Guard for the "we hold this lock" phase. Drop clears the holder.
pub struct HolderGuard {
    mutex_id: SharedId,
    tid: ThreadId,
}

impl Drop for HolderGuard {
    fn drop(&mut self) {
        if let Some(mut e) = graph().get_mut(&self.mutex_id) {
            if e.holder == Some(self.tid) {
                e.holder = None;
                e.last_updated = Some(Instant::now());
            }
        }
    }
}

/// Consume a pending break signal for the current thread (set by
/// the detector). Used by Mutex::with to surface DeadlockException
/// instead of TimeoutException when part of a detected cycle.
pub fn consume_break_signal() -> Option<BreakSignal> {
    let tid = std::thread::current().id();
    break_reqs().remove(&tid).map(|(_, v)| v)
}

/// Cumulative cycles observed during this process lifetime.
pub fn cycles_detected_total() -> u64 {
    CYCLES_DETECTED.load(Ordering::Relaxed)
}

/// Start the detector on the tokio runtime. Idempotent. No-op when
/// level == Off or when already started.
pub fn start_detector(level: LockDiagnosticsLevel, interval: Duration) {
    if matches!(level, LockDiagnosticsLevel::Off) {
        return;
    }
    if DETECTOR_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            run_cycle_scan(level);
        }
    });
}

fn run_cycle_scan(level: LockDiagnosticsLevel) {
    // Snapshot the wait-for graph into plain HashMaps for DFS.
    let mut waits_on: HashMap<ThreadId, Vec<SharedId>> = HashMap::new();
    let mut held_by: HashMap<SharedId, ThreadId> = HashMap::new();

    for entry in graph().iter() {
        let state = entry.value();
        if let Some(h) = state.holder {
            held_by.insert(*entry.key(), h);
        }
        for w in &state.waiters {
            waits_on.entry(*w).or_default().push(*entry.key());
        }
    }

    let waiter_threads: Vec<ThreadId> = waits_on.keys().copied().collect();
    for &start in &waiter_threads {
        let mut stack: Vec<ThreadId> = vec![start];
        let mut visited: HashSet<ThreadId> = HashSet::new();
        while let Some(t) = stack.pop() {
            if !visited.insert(t) {
                continue;
            }
            let Some(mutexes) = waits_on.get(&t) else {
                continue;
            };
            for m in mutexes {
                if let Some(&holder) = held_by.get(m) {
                    if holder == start && t != start {
                        CYCLES_DETECTED.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            event = "shared.deadlock_detected",
                            level = ?level,
                            start_thread = ?start,
                            holder_thread = ?holder,
                            "wait-for graph cycle detected"
                        );
                        if matches!(level, LockDiagnosticsLevel::Strict) {
                            let cycle_id = CYCLES_DETECTED.load(Ordering::Relaxed);
                            // Break the cycle by signalling the youngest
                            // (DFS root) waiter.
                            break_reqs().insert(start, BreakSignal { cycle_id });
                        }
                        break;
                    }
                    stack.push(holder);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiter_guard_drops_cleanly() {
        {
            let _g = register_waiter(1000);
            let g_ref = graph().get(&1000).unwrap();
            assert_eq!(g_ref.waiters.len(), 1);
        }
        if let Some(e) = graph().get(&1000) {
            assert_eq!(e.waiters.len(), 0);
        }
    }

    #[test]
    fn holder_guard_clears_on_drop() {
        {
            let wg = register_waiter(1001);
            let _hg = wg.promote_to_holder();
            let g_ref = graph().get(&1001).unwrap();
            assert!(g_ref.holder.is_some());
        }
        if let Some(e) = graph().get(&1001) {
            assert!(e.holder.is_none());
        }
    }
}
