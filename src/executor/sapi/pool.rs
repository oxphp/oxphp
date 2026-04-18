//! Shared worker-pool machinery: slot tracking, channel alias, monitor
//! (static mode) and scale manager (dynamic mode). Spawn decisions are
//! centralized through the `SpawnStrategy` enum so monitor / scale-up /
//! respawn paths never branch on worker-mode configuration.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::WorkerMode;
use crate::metrics::{Metrics, WorkerMetrics};
use crate::php::sapi::WorkerIncomingRequest;

use super::traditional::{spawn_worker, WorkerLoopMode};
use super::worker_mode::{spawn_worker_mode, WorkerModeConfig};

/// Alias for the channel message type used in both traditional and worker modes.
pub(super) type WorkerRequest = WorkerIncomingRequest;

/// Per-worker state for the managed worker pool.
pub(super) struct ManagedWorker {
    /// Not read at runtime; kept so debug-printing `workers` shows which ID each slot holds.
    #[allow(dead_code)]
    pub id: usize,
    pub handle: std::thread::JoinHandle<()>,
    pub shutdown: Arc<AtomicBool>,
    pub last_active: Arc<AtomicU64>,
}

/// Monotonic ms-since-process-start. Used for `last_active` stamps and
/// idle-timeout math — never for user-visible timestamps. Monotonic clock
/// avoids false idle detection if the system wall clock jumps backwards.
pub(super) fn now_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Bundles the parameters every spawn call needs, regardless of model.
pub(super) struct SpawnArgs {
    pub id: usize,
    pub rx: crossbeam_channel::Receiver<WorkerRequest>,
    pub shutdown: Arc<AtomicBool>,
    pub last_active: Arc<AtomicU64>,
}

/// How to spawn a worker thread. Built once in `SapiExecutor::new()` and
/// cloned into the monitor / scale manager so respawn / scale-up paths
/// don't branch on worker-mode configuration at every call site.
#[derive(Clone)]
pub(super) enum SpawnStrategy {
    Traditional {
        loop_mode: WorkerLoopMode,
    },
    WorkerMode {
        config: Arc<WorkerModeConfig>,
        metrics: Arc<WorkerMetrics>,
    },
}

impl SpawnStrategy {
    pub(super) fn spawn(&self, args: SpawnArgs) -> std::thread::JoinHandle<()> {
        match self {
            Self::Traditional { loop_mode } => spawn_worker(
                args.id,
                args.rx,
                args.shutdown,
                args.last_active,
                *loop_mode,
            ),
            Self::WorkerMode { config, metrics } => {
                let slot = args.id % metrics.slots.len();
                let stats = Arc::clone(&metrics.slots[slot]);
                spawn_worker_mode(
                    args.id,
                    args.rx,
                    args.shutdown,
                    args.last_active,
                    config.clone(),
                    stats,
                    Arc::clone(metrics),
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
        let last_active = Arc::new(AtomicU64::new(now_millis()));
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
    strategy: SpawnStrategy,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut next_id = target;

    loop {
        interval.tick().await;
        if global_shutdown.load(Ordering::Relaxed) {
            break;
        }

        let mut guard = workers.lock().unwrap();
        let before = guard.len();
        guard.retain(|w| !w.handle.is_finished());
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
            let last_active = Arc::new(AtomicU64::new(now_millis()));
            let id = next_id;
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
            next_id += 1;
            metrics.worker_spawned();
        }

        let mut guard = workers.lock().unwrap();
        guard.extend(new);
        metrics.set_workers_current(guard.len());
    }
}

pub(super) async fn run_scale_manager(
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    min: usize,
    max: usize,
    idle_timeout_seconds: u64,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    strategy: SpawnStrategy,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut last_scale_up = Instant::now();
    let mut last_scale_down = Instant::now();
    let idle_timeout_ms = idle_timeout_seconds * 1000;
    let mut next_id = max;

    loop {
        interval.tick().await;
        if global_shutdown.load(Ordering::Relaxed) {
            break;
        }

        let mut workers_guard = workers.lock().unwrap();
        let now = now_millis();

        let before = workers_guard.len();
        workers_guard.retain(|w| !w.handle.is_finished());
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
            let last_active = Arc::new(AtomicU64::new(now));
            let id = next_id;
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
            next_id += 1;
            metrics.worker_spawned();
        }

        let mut workers_guard = workers.lock().unwrap();
        workers_guard.extend(new);
        let total = workers_guard.len();

        // Count idle workers (last_active > 200ms ago).
        let idle_count = workers_guard
            .iter()
            .filter(|w| now.saturating_sub(w.last_active.load(Ordering::Relaxed)) > 200)
            .count();

        metrics.set_workers_current(total);

        let needs_scale_up =
            idle_count == 0 && total < max && last_scale_up.elapsed() >= Duration::from_millis(500);

        if needs_scale_up {
            // Prepare Arcs inside the lock, then drop it before spawning the OS thread —
            // otherwise thread creation (~10-50μs) blocks the Tokio runtime.
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(AtomicU64::new(now));
            let spawn_id = next_id;
            drop(workers_guard);

            let handle = strategy.spawn(SpawnArgs {
                id: spawn_id,
                rx: request_rx.clone(),
                shutdown: Arc::clone(&shutdown),
                last_active: Arc::clone(&last_active),
            });

            let mut workers_guard = workers.lock().unwrap();
            workers_guard.push(ManagedWorker {
                id: next_id,
                handle,
                shutdown,
                last_active,
            });
            next_id += 1;
            last_scale_up = Instant::now();
            metrics.worker_spawned();
            metrics.set_workers_current(workers_guard.len());
            tracing::info!(
                id = next_id - 1,
                total = workers_guard.len(),
                "Scale-up: spawned worker"
            );
            continue;
        }

        // Scale-down: retire one worker idle longer than the threshold.
        if total > min && last_scale_down.elapsed() >= Duration::from_secs(5) {
            if let Some(pos) = workers_guard.iter().position(|w| {
                now.saturating_sub(w.last_active.load(Ordering::Relaxed)) > idle_timeout_ms
            }) {
                let worker = workers_guard.remove(pos);
                worker.shutdown.store(true, Ordering::Relaxed);
                // Join on Tokio's blocking pool so the async runtime isn't blocked
                // and we don't pay pthread_create latency per scale-down event.
                tokio::task::spawn_blocking(move || {
                    let _ = worker.handle.join();
                });
                last_scale_down = Instant::now();
                metrics.worker_retired();
                metrics.set_workers_current(total - 1);
                tracing::info!(total = total - 1, "Scale-down: retired worker");
            }
        }
    }
}
