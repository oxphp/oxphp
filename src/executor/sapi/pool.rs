//! Shared worker-pool machinery: slot tracking, channel alias, monitor
//! (static mode) and scale manager (dynamic mode). Does not know about
//! traditional vs worker-mode lifecycle — spawn decisions live in the
//! caller today (Task 5 will introduce `SpawnStrategy`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

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

pub(super) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Health monitor for static mode: detects dead workers and respawns to target count.
pub(super) async fn run_worker_monitor(
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    target: usize,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    worker_mode_config: Option<Arc<WorkerModeConfig>>,
    worker_metrics: Option<Arc<WorkerMetrics>>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut next_id = target; // IDs above initial range

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

        while guard.len() < target {
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(AtomicU64::new(now_millis()));
            let handle = if let Some(ref wmc) = worker_mode_config {
                let wm = worker_metrics.as_ref().unwrap();
                let slot_idx = next_id % wm.slots.len();
                let stats = Arc::clone(&wm.slots[slot_idx]);
                spawn_worker_mode(
                    next_id,
                    request_rx.clone(),
                    Arc::clone(&shutdown),
                    Arc::clone(&last_active),
                    wmc.clone(),
                    stats,
                    Arc::clone(wm),
                )
            } else {
                spawn_worker(
                    next_id,
                    request_rx.clone(),
                    Arc::clone(&shutdown),
                    Arc::clone(&last_active),
                    WorkerLoopMode::Static,
                )
            };
            guard.push(ManagedWorker {
                id: next_id,
                handle,
                shutdown,
                last_active,
            });
            next_id += 1;
            metrics.worker_spawned();
        }

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
    worker_mode_config: Option<Arc<WorkerModeConfig>>,
    worker_metrics: Option<Arc<WorkerMetrics>>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut last_scale_up = Instant::now();
    let mut last_scale_down = Instant::now();
    let idle_timeout_ms = idle_timeout_seconds * 1000;
    let mut next_id = max; // start IDs above initial range to avoid collisions

    loop {
        interval.tick().await;
        if global_shutdown.load(Ordering::Relaxed) {
            break;
        }

        let mut workers_guard = workers.lock().unwrap();
        let now = now_millis();

        // Clean up exited workers
        let before = workers_guard.len();
        workers_guard.retain(|w| !w.handle.is_finished());
        let dead = before - workers_guard.len();
        let total = workers_guard.len();

        if dead > 0 {
            tracing::warn!(dead, remaining = total, "Dead workers detected, respawning");
        }

        // Respawn to maintain minimum (unconditional — dead worker recovery)
        while workers_guard.len() < min {
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(AtomicU64::new(now));
            let handle = if let Some(ref wmc) = worker_mode_config {
                let wm = worker_metrics.as_ref().unwrap();
                let slot_idx = next_id % wm.slots.len();
                let stats = Arc::clone(&wm.slots[slot_idx]);
                spawn_worker_mode(
                    next_id,
                    request_rx.clone(),
                    Arc::clone(&shutdown),
                    Arc::clone(&last_active),
                    wmc.clone(),
                    stats,
                    Arc::clone(wm),
                )
            } else {
                spawn_worker(
                    next_id,
                    request_rx.clone(),
                    Arc::clone(&shutdown),
                    Arc::clone(&last_active),
                    WorkerLoopMode::Dynamic,
                )
            };
            workers_guard.push(ManagedWorker {
                id: next_id,
                handle,
                shutdown,
                last_active,
            });
            next_id += 1;
            metrics.worker_spawned();
        }
        let total = workers_guard.len();

        // Count idle workers (last_active > 200ms ago)
        let idle_count = workers_guard
            .iter()
            .filter(|w| now.saturating_sub(w.last_active.load(Ordering::Relaxed)) > 200)
            .count();

        // Update metrics
        metrics.set_workers_current(total);

        // Scale-up: no idle workers and under max
        let needs_scale_up =
            idle_count == 0 && total < max && last_scale_up.elapsed() >= Duration::from_millis(500);

        if needs_scale_up {
            // Prepare Arcs inside lock, but drop lock before spawning the OS thread
            // to avoid blocking the Tokio runtime during thread creation (~10-50μs).
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(AtomicU64::new(now));
            let spawn_shutdown = Arc::clone(&shutdown);
            let spawn_last_active = Arc::clone(&last_active);
            let spawn_rx = request_rx.clone();
            let spawn_id = next_id;
            let spawn_wmc = worker_mode_config.clone();
            let spawn_wm = worker_metrics.clone();
            drop(workers_guard);

            let handle = if let Some(wmc) = spawn_wmc {
                let wm = spawn_wm.as_ref().unwrap();
                let slot_idx = spawn_id % wm.slots.len();
                let stats = Arc::clone(&wm.slots[slot_idx]);
                spawn_worker_mode(
                    spawn_id,
                    spawn_rx,
                    spawn_shutdown,
                    spawn_last_active,
                    wmc,
                    stats,
                    Arc::clone(wm),
                )
            } else {
                spawn_worker(
                    spawn_id,
                    spawn_rx,
                    spawn_shutdown,
                    spawn_last_active,
                    WorkerLoopMode::Dynamic,
                )
            };

            // Re-lock to insert
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

        // Scale-down: retire workers idle longer than threshold
        if total > min && last_scale_down.elapsed() >= Duration::from_secs(5) {
            if let Some(pos) = workers_guard.iter().position(|w| {
                now.saturating_sub(w.last_active.load(Ordering::Relaxed)) > idle_timeout_ms
            }) {
                let worker = workers_guard.remove(pos);
                worker.shutdown.store(true, Ordering::Relaxed);
                // Join in background thread to avoid blocking the async runtime
                std::thread::spawn(move || {
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
