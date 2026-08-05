//! Shared worker-pool machinery: slot tracking, channel alias, monitor
//! (static mode) and scale manager (dynamic mode). Spawn decisions are
//! centralized through the `SpawnStrategy` enum so monitor / scale-up /
//! respawn paths never branch on worker-mode configuration.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::WorkerMode;
use crate::executor::idle_clock::{now_millis, LastActive};
use crate::metrics::{Metrics, WorkerMetrics};
use crate::php::sapi::WorkerIncomingRequest;

use super::traditional::{spawn_worker, WorkerLoopMode};
use super::worker_mode::{spawn_worker_mode, WorkerModeConfig, WorkerModeMetrics};

/// Alias for the channel message type used in both traditional and worker modes.
pub(super) type WorkerRequest = WorkerIncomingRequest;

/// Per-worker state for the managed worker pool.
pub(super) struct ManagedWorker {
    /// Not read at runtime; kept so debug-printing `workers` shows which ID each slot holds.
    #[allow(dead_code)]
    pub id: usize,
    pub handle: std::thread::JoinHandle<()>,
    pub shutdown: Arc<AtomicBool>,
    pub last_active: Arc<LastActive>,
}

/// Whether this worker's registry slot has requests in flight.
///
/// A busy worker is not a scale-down candidate. What the pool needs to know is
/// whether the thread will act on the flag it is about to raise, and one with
/// work on it will not: the flag is read only while waiting for the next
/// request, which in worker mode means only once the last fiber is gone.
/// Retiring such a worker changes the pool's accounting long before the thread
/// acts on it — the published count drops, the slot id goes back to the free
/// list for the next spawn, and the join sits on the blocking pool that
/// shutdown waits for — while the thread keeps taking requests off the shared
/// queue.
///
/// The counter is a proxy for that question, not an answer to it. It returns
/// to zero when a handler returns, whereas the serve loop stays in the branch
/// that never reads the flag for as long as a fiber or a deferred promise
/// drain outlives that handler; and the check is not atomic with raising the
/// flag, so a request taken in between leaves a retired worker serving. What
/// it removes is the case that the clock fix would otherwise have made
/// ordinary — a worker plainly in the middle of its work being offered up
/// because its last arrival is older than the threshold.
///
/// An absent registry reads as "not busy", which is how the pool behaved
/// before this check existed.
fn worker_busy(id: usize) -> bool {
    crate::php::worker_registry::WORKERS
        .get()
        .and_then(|workers| workers.get(id))
        .is_some_and(|slot| slot.active_requests.load(Ordering::Relaxed) > 0)
}

/// Best-effort wipe of a recycled WORKERS slot: drops the request's
/// Weak<CancellationState> back-ref and nulls the interrupt-flag pointer
/// so a `cancel_request()` for this slot's previous occupant can't write
/// into now-defunct TLS memory before the replacement worker re-publishes
/// its own `EG(vm_interrupt)` address. Normal Drop already does the
/// per-request half of this; the slot-level wipe defends against
/// abnormal worker exit (abort, segfault) where Drop never ran.
fn clear_worker_slot(id: usize) {
    if let Some(workers) = crate::php::worker_registry::WORKERS.get() {
        if let Some(slot) = workers.get(id) {
            if let Ok(mut g) = slot.cancel_state.lock() {
                *g = None;
            }
            slot.interrupt_flag_ptr
                .store(std::ptr::null_mut(), Ordering::Release);
            slot.active_requests.store(0, Ordering::Release);
            slot.heartbeat.request_start_us.store(0, Ordering::Relaxed);
            slot.heartbeat.tid.store(0, Ordering::Relaxed);
        }
    }
}

/// Bundles the parameters every spawn call needs, regardless of model.
pub(super) struct SpawnArgs {
    pub id: usize,
    pub rx: crossbeam_channel::Receiver<WorkerRequest>,
    pub shutdown: Arc<AtomicBool>,
    pub last_active: Arc<LastActive>,
}

/// How to spawn a worker thread. Built once in `SapiExecutor::new()` and
/// shared (`Arc<SpawnStrategy>`) with the monitor / scale manager so the
/// respawn / scale-up paths don't branch on worker-mode configuration
/// at every call site.
pub(super) enum SpawnStrategy {
    Traditional {
        loop_mode: WorkerLoopMode,
        /// Worker threads answer a request whose queue budget ran out before
        /// they reached it, and that refusal has to be counted where every
        /// other one is.
        server_metrics: Arc<Metrics>,
    },
    WorkerMode {
        /// Same meaning as in `Traditional`: whether the thread's wait for the
        /// next request has to come up for air to read its shutdown flag.
        loop_mode: WorkerLoopMode,
        config: Arc<WorkerModeConfig>,
        metrics: Arc<WorkerMetrics>,
        server_metrics: Arc<Metrics>,
    },
}

impl SpawnStrategy {
    pub(super) fn spawn(&self, args: SpawnArgs) -> std::thread::JoinHandle<()> {
        match self {
            Self::Traditional {
                loop_mode,
                server_metrics,
            } => spawn_worker(
                args.id,
                args.rx,
                args.shutdown,
                args.last_active,
                *loop_mode,
                Arc::clone(server_metrics),
            ),
            Self::WorkerMode {
                loop_mode,
                config,
                metrics,
                server_metrics,
            } => {
                let slot = args.id % metrics.slots.len();
                let stats = Arc::clone(&metrics.slots[slot]);
                spawn_worker_mode(
                    args.id,
                    args.rx,
                    args.shutdown,
                    args.last_active,
                    *loop_mode,
                    config.clone(),
                    WorkerModeMetrics {
                        stats,
                        worker: Arc::clone(metrics),
                        server: Arc::clone(server_metrics),
                    },
                )
            }
        }
    }
}

/// Spawn `count` workers and return the initial `ManagedWorker` vector.
pub(super) fn spawn_initial(
    strategy: &SpawnStrategy,
    request_rx: &crossbeam_channel::Receiver<WorkerRequest>,
    count: usize,
) -> Vec<ManagedWorker> {
    let mut managed = Vec::with_capacity(count);
    for id in 0..count {
        let shutdown = Arc::new(AtomicBool::new(false));
        let last_active = Arc::new(LastActive::now());
        let handle = strategy.spawn(SpawnArgs {
            id,
            rx: request_rx.clone(),
            shutdown: Arc::clone(&shutdown),
            last_active: Arc::clone(&last_active),
        });
        managed.push(ManagedWorker {
            id,
            handle,
            shutdown,
            last_active,
        });
    }
    managed
}

/// Seed the worker-count gauges and spawn counters from the initial pool size.
pub(super) fn seed_metrics(metrics: &Metrics, mode: &WorkerMode, initial_count: usize) {
    metrics.set_workers_current(initial_count);
    for _ in 0..initial_count {
        metrics.worker_spawned();
    }
    match mode {
        WorkerMode::Static(n) => {
            metrics.set_workers_min(*n);
            metrics.set_workers_max(*n);
        }
        WorkerMode::Dynamic { min, max } => {
            metrics.set_workers_min(*min);
            metrics.set_workers_max(*max);
        }
    }
}

/// Health monitor for static mode: detects dead workers and respawns to target count.
pub(super) async fn run_worker_monitor(
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    target: usize,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    strategy: Arc<SpawnStrategy>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    // IDs 0..target are taken by the initial pool; respawned workers
    // pull from this free-list so the WORKERS / metrics slot tables
    // (sized to `target` on startup) always cover the live worker set.
    let mut free_ids: VecDeque<usize> = VecDeque::new();

    loop {
        interval.tick().await;
        if global_shutdown.load(Ordering::Relaxed) {
            break;
        }

        let mut guard = workers.lock().unwrap();
        let before = guard.len();
        guard.retain(|w| {
            let alive = !w.handle.is_finished();
            if !alive {
                free_ids.push_back(w.id);
                clear_worker_slot(w.id);
            }
            alive
        });
        let dead = before - guard.len();

        if dead > 0 {
            tracing::warn!(
                dead,
                remaining = guard.len(),
                target,
                "Dead workers detected, respawning"
            );
        }

        // Compute how many to spawn, then drop the Mutex — pthread_create
        // (~10-50μs per thread) must not run under the lock.
        let to_spawn = target.saturating_sub(guard.len());
        drop(guard);

        let mut new = Vec::with_capacity(to_spawn);
        for _ in 0..to_spawn {
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(LastActive::now());
            // Free-list invariant: every spawn matches a prior death,
            // and the initial pool occupies 0..target — so on the first
            // respawn iteration, free_ids holds exactly `dead` IDs.
            let id = free_ids
                .pop_front()
                .expect("free-id underflow: spawning more workers than have died");
            let handle = strategy.spawn(SpawnArgs {
                id,
                rx: request_rx.clone(),
                shutdown: Arc::clone(&shutdown),
                last_active: Arc::clone(&last_active),
            });
            new.push(ManagedWorker {
                id,
                handle,
                shutdown,
                last_active,
            });
            metrics.worker_spawned();
        }

        let mut guard = workers.lock().unwrap();
        guard.extend(new);
        metrics.set_workers_current(guard.len());
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_scale_manager(
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    min: usize,
    max: usize,
    idle_timeout_seconds: u64,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    strategy: Arc<SpawnStrategy>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut last_scale_up = Instant::now();
    let mut last_scale_down = Instant::now();
    let idle_timeout_ms = idle_timeout_seconds * 1000;
    // Free-list of slot IDs in `0..max`. Initial pool occupied 0..min,
    // so the headroom IDs `min..max` are immediately available for
    // scale-up; respawn / scale-down feed dead IDs back to the front.
    // Bounded to max workers ⇒ slot tables (WORKERS / metrics) always
    // index into a real slot.
    let mut free_ids: VecDeque<usize> = (min..max).collect();

    loop {
        interval.tick().await;
        if global_shutdown.load(Ordering::Relaxed) {
            break;
        }

        let mut workers_guard = workers.lock().unwrap();
        let now = now_millis();

        let before = workers_guard.len();
        workers_guard.retain(|w| {
            let alive = !w.handle.is_finished();
            if !alive {
                free_ids.push_back(w.id);
                clear_worker_slot(w.id);
            }
            alive
        });
        let dead = before - workers_guard.len();
        let total = workers_guard.len();

        if dead > 0 {
            tracing::warn!(dead, remaining = total, "Dead workers detected, respawning");
        }

        // Respawn to maintain minimum (unconditional — dead worker recovery).
        // Compute the count, drop the Mutex, spawn OS threads outside the lock,
        // then re-acquire — pthread_create must not run under `workers`.
        let to_spawn_min = min.saturating_sub(workers_guard.len());
        drop(workers_guard);

        let mut new = Vec::with_capacity(to_spawn_min);
        for _ in 0..to_spawn_min {
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(LastActive::now());
            let id = free_ids
                .pop_front()
                .expect("free-id underflow in scale-manager respawn");
            let handle = strategy.spawn(SpawnArgs {
                id,
                rx: request_rx.clone(),
                shutdown: Arc::clone(&shutdown),
                last_active: Arc::clone(&last_active),
            });
            new.push(ManagedWorker {
                id,
                handle,
                shutdown,
                last_active,
            });
            metrics.worker_spawned();
        }

        let mut workers_guard = workers.lock().unwrap();
        workers_guard.extend(new);
        let mut total = workers_guard.len();

        // Count idle workers (last_active > 200ms ago).
        let idle_count = workers_guard
            .iter()
            .filter(|w| w.last_active.idle_ms(now) > 200)
            .count();

        let needs_scale_up =
            idle_count == 0 && total < max && last_scale_up.elapsed() >= Duration::from_millis(500);

        if needs_scale_up {
            // Prepare Arcs inside the lock, then drop it before spawning the OS thread —
            // pthread_create must not run under `workers`.
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(LastActive::now());
            // total<max plus the invariant `len(alive)+len(free_ids)==max`
            // guarantees free_ids has at least one entry here.
            let spawn_id = free_ids.pop_front().expect("free-id underflow in scale-up");
            drop(workers_guard);

            let handle = strategy.spawn(SpawnArgs {
                id: spawn_id,
                rx: request_rx.clone(),
                shutdown: Arc::clone(&shutdown),
                last_active: Arc::clone(&last_active),
            });

            let mut g = workers.lock().unwrap();
            g.push(ManagedWorker {
                id: spawn_id,
                handle,
                shutdown,
                last_active,
            });
            total = g.len();
            last_scale_up = Instant::now();
            metrics.worker_spawned();
            tracing::info!(id = spawn_id, total, "Scale-up: spawned worker");
        } else if total > min && last_scale_down.elapsed() >= Duration::from_secs(5) {
            // Scale-down: retire one worker idle longer than the threshold and
            // holding nothing — an idle stamp says when work last arrived, not
            // whether it has finished, so on its own it would offer up a worker
            // still serving a request that outlasts the threshold.
            if let Some(pos) = workers_guard
                .iter()
                .position(|w| w.last_active.idle_ms(now) > idle_timeout_ms && !worker_busy(w.id))
            {
                let worker = workers_guard.remove(pos);
                let retired_id = worker.id;
                worker.shutdown.store(true, Ordering::Relaxed);
                // Recycle the slot before the thread has finished joining;
                // the slot's per-thread state is re-published the next time
                // a worker spawns into it, so an early reuse is safe.
                free_ids.push_back(retired_id);
                // Join on Tokio's blocking pool so the async runtime isn't blocked
                // and we don't pay pthread_create latency per scale-down event.
                // The log line on the far side is the only trace a retirement
                // that never completes leaves behind: the join itself is silent,
                // and a thread that outlives its retirement keeps taking work
                // off the shared queue as if nothing had happened.
                // `retired_id` names the slot this worker held when it was
                // retired, not one it still owns: the id went back to the free
                // list above, so by the time this prints a replacement may
                // already be serving under it.
                tokio::task::spawn_blocking(move || {
                    let _ = worker.handle.join();
                    tracing::info!(retired_id, "Scale-down: retired worker thread stopped");
                });
                total -= 1;
                last_scale_down = Instant::now();
                metrics.worker_retired();
                tracing::info!(total, "Scale-down: retired worker");
            }
        }

        // Single gauge publish per tick — scale-up/down/no-op all funnel here.
        metrics.set_workers_current(total);
    }
}
