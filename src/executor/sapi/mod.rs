use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use crossbeam_channel::{self, TrySendError};
use http::{HeaderName, HeaderValue};

use crate::config::{Config, WorkerMode};
use crate::executor::ScriptExecutor;
use crate::metrics::{Metrics, WorkerMetrics};
use crate::php::bindings;
use crate::php::sapi;
use crate::php::sapi::WorkerIncomingRequest;
use crate::types::{ScriptRequest, ScriptResponse};

mod traditional;
mod worker_mode;

use traditional::{spawn_worker, WorkerLoopMode};
use worker_mode::{spawn_worker_mode, WorkerModeConfig};

/// Alias for the channel message type used in both traditional and worker modes.
type WorkerRequest = WorkerIncomingRequest;

/// Per-worker state for the managed worker pool.
struct ManagedWorker {
    /// Not read at runtime; kept so debug-printing `workers` shows which ID each slot holds.
    #[allow(dead_code)]
    id: usize,
    handle: std::thread::JoinHandle<()>,
    shutdown: Arc<AtomicBool>,
    last_active: Arc<AtomicU64>,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct SapiExecutor {
    request_tx: Option<crossbeam_channel::Sender<WorkerRequest>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    mode: WorkerMode,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    idle_timeout_seconds: u64,
    worker_mode_config: Option<Arc<WorkerModeConfig>>,
    worker_metrics: Option<Arc<WorkerMetrics>>,
}

impl SapiExecutor {
    pub fn new(config: &Config, metrics: Arc<Metrics>) -> Self {
        let mode = config.worker_mode.clone();
        let idle_timeout_seconds = config.worker_idle_timeout_seconds;
        let initial_count = mode.worker_count();

        // 1. TSRM must be initialized first for ZTS builds
        if !unsafe { bindings::php_tsrm_startup() } {
            panic!("php_tsrm_startup() failed");
        }

        // 2. Build and register our SAPI module
        let mut module = sapi::build_sapi_module();

        unsafe {
            bindings::sapi_startup(&mut module);
        }

        // 3. Start the PHP engine (PHP 8.4: 2 arguments)
        let startup_result =
            unsafe { bindings::php_module_startup(&mut module, std::ptr::null_mut()) };
        if startup_result != 0 {
            panic!("php_module_startup() failed with code {startup_result}");
        }

        // 4. Install structured error logging callback (must be after php_module_startup)
        unsafe {
            sapi::install_error_cb();
        }

        let queue_capacity = config.queue_capacity;

        let worker_mode_config = config.worker_file.as_ref().map(|path| {
            Arc::new(WorkerModeConfig {
                worker_file: path.clone(),
                document_root: config.server.document_root.clone(),
                max_requests: config.worker_max_requests,
                max_memory_mib: config.worker_max_memory_mib,
            })
        });

        let (request_tx, request_rx) = crossbeam_channel::bounded(queue_capacity);

        let loop_mode = match &mode {
            WorkerMode::Static(_) => WorkerLoopMode::Static,
            WorkerMode::Dynamic { .. } => WorkerLoopMode::Dynamic,
        };

        // Register worker callbacks once before spawning any worker mode threads
        if worker_mode_config.is_some() {
            unsafe {
                bindings::oxphp_bridge_set_worker_callbacks(
                    sapi::get_worker_wait_callback(),
                    sapi::get_worker_send_callback(),
                );
            }
        }

        // Create worker mode metrics if worker mode is active
        let max_workers = match &mode {
            WorkerMode::Static(n) => *n,
            WorkerMode::Dynamic { max, .. } => *max,
        };
        let worker_metrics = worker_mode_config.as_ref().map(|_| {
            let wm = Arc::new(WorkerMetrics::new(max_workers));
            metrics.set_worker_metrics(Arc::clone(&wm));
            wm
        });

        let mut managed_workers = Vec::with_capacity(initial_count);
        for i in 0..initial_count {
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(AtomicU64::new(now_millis()));
            let handle = if let Some(ref wmc) = worker_mode_config {
                let stats = Arc::clone(&worker_metrics.as_ref().unwrap().slots[i]);
                spawn_worker_mode(
                    i,
                    request_rx.clone(),
                    Arc::clone(&shutdown),
                    Arc::clone(&last_active),
                    wmc.clone(),
                    stats,
                    Arc::clone(worker_metrics.as_ref().unwrap()),
                )
            } else {
                spawn_worker(
                    i,
                    request_rx.clone(),
                    Arc::clone(&shutdown),
                    Arc::clone(&last_active),
                    loop_mode,
                )
            };
            managed_workers.push(ManagedWorker {
                id: i,
                handle,
                shutdown,
                last_active,
            });
        }

        // Set initial metrics
        metrics.set_workers_current(initial_count);
        for _ in 0..initial_count {
            metrics.worker_spawned();
        }
        match &mode {
            WorkerMode::Static(n) => {
                metrics.set_workers_min(*n);
                metrics.set_workers_max(*n);
            }
            WorkerMode::Dynamic { min, max } => {
                metrics.set_workers_min(*min);
                metrics.set_workers_max(*max);
            }
        }

        if worker_mode_config.is_some() {
            tracing::info!(
                mode = ?mode,
                workers = initial_count,
                queue_capacity,
                idle_timeout_seconds,
                worker_file = %worker_mode_config.as_ref().unwrap().worker_file.display(),
                "PHP worker pool started (worker mode)"
            );
        } else {
            tracing::info!(
                mode = ?mode,
                workers = initial_count,
                queue_capacity,
                idle_timeout_seconds,
                "PHP worker pool started"
            );
        }

        Self {
            request_tx: Some(request_tx),
            request_rx,
            workers: Arc::new(Mutex::new(managed_workers)),
            mode,
            global_shutdown: Arc::new(AtomicBool::new(false)),
            metrics,
            idle_timeout_seconds,
            worker_mode_config,
            worker_metrics,
        }
    }
}

impl ScriptExecutor for SapiExecutor {
    fn execute(&self, request: ScriptRequest) -> crate::executor::ExecuteResult {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let worker_request = WorkerRequest {
            script: request,
            response_tx,
        };

        if let Err(e) = self.request_tx.as_ref().unwrap().try_send(worker_request) {
            let (status, body) = match e {
                TrySendError::Full(_) => (529, Bytes::from_static(b"Site is overloaded")),
                TrySendError::Disconnected(_) => {
                    (500, Bytes::from_static(b"PHP worker pool unavailable"))
                }
            };
            let mut headers = vec![(
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )];
            if status == 529 {
                headers.push((
                    HeaderName::from_static("retry-after"),
                    HeaderValue::from_static("3"),
                ));
            }
            return crate::executor::ExecuteResult::Immediate(ScriptResponse {
                status,
                headers,
                body,
                ..Default::default()
            });
        }

        crate::executor::ExecuteResult::Deferred(response_rx)
    }

    fn shutdown(&self) {
        // No-op: cleanup handled in Drop
    }

    fn start_scale_manager(&self) {
        let workers = Arc::clone(&self.workers);
        let request_rx = self.request_rx.clone();
        let global_shutdown = Arc::clone(&self.global_shutdown);
        let metrics = Arc::clone(&self.metrics);
        let wmc = self.worker_mode_config.clone();
        let wm = self.worker_metrics.clone();

        match &self.mode {
            WorkerMode::Static(target) => {
                let target = *target;
                tokio::spawn(async move {
                    run_worker_monitor(
                        workers,
                        request_rx,
                        target,
                        global_shutdown,
                        metrics,
                        wmc,
                        wm,
                    )
                    .await;
                });
                tracing::info!(target, "Worker health monitor started");
            }
            WorkerMode::Dynamic { min, max } => {
                let min = *min;
                let max = *max;
                let idle_timeout_seconds = self.idle_timeout_seconds;
                tokio::spawn(async move {
                    run_scale_manager(
                        workers,
                        request_rx,
                        min,
                        max,
                        idle_timeout_seconds,
                        global_shutdown,
                        metrics,
                        wmc,
                        wm,
                    )
                    .await;
                });
                tracing::info!(min, max, "Scale manager started");
            }
        }
    }
}

impl Drop for SapiExecutor {
    fn drop(&mut self) {
        // 1. Signal scale manager to stop
        self.global_shutdown.store(true, Ordering::Relaxed);

        // 2. Drop sender to close channel — workers will exit their recv loop
        self.request_tx.take();

        // 3. Signal each worker to shut down and join
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.drain(..) {
                worker.shutdown.store(true, Ordering::Relaxed);
                let _ = worker.handle.join();
            }
        }

        // 4. PHP shutdown after all workers are done
        unsafe {
            bindings::php_module_shutdown();
            bindings::sapi_shutdown();
            bindings::tsrm_shutdown();
        }
    }
}

/// Health monitor for static mode: detects dead workers and respawns to target count.
async fn run_worker_monitor(
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

async fn run_scale_manager(
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── now_millis test ──

    #[test]
    fn test_now_millis_reasonable() {
        let ms = now_millis();
        // Should be after 2020 and before 2100
        assert!(ms > 1_577_836_800_000); // 2020-01-01
        assert!(ms < 4_102_444_800_000); // 2100-01-01
    }

    #[test]
    fn test_backpressure_returns_529_with_retry_after() {
        use crate::executor::ExecuteResult;
        use http::{HeaderMap, Method, Uri};
        use std::path::PathBuf;

        // Create a zero-capacity channel — any send will fail with Full
        let (tx, rx) = crossbeam_channel::bounded::<WorkerRequest>(0);
        let metrics = Arc::new(Metrics::new());

        let executor = SapiExecutor {
            request_tx: Some(tx),
            request_rx: rx,
            workers: Arc::new(Mutex::new(Vec::new())),
            mode: WorkerMode::Static(1),
            global_shutdown: Arc::new(AtomicBool::new(false)),
            metrics,
            idle_timeout_seconds: 30,
            worker_mode_config: None,
            worker_metrics: None,
        };

        let request = ScriptRequest {
            request_id: String::new(),
            script_path: PathBuf::from("/var/www/public/index.php"),
            method: Method::GET,
            uri: Uri::from_static("/"),
            query_string: String::new(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
            remote_addr: "127.0.0.1:0".parse().unwrap(),
            document_root: Arc::new(PathBuf::from("/var/www/public")),
            timeout_us: 0,
            trace_id: String::new(),
            span_id: String::new(),
            parent_span_id: String::new(),
            is_tls: false,
            version: http::Version::HTTP_11,
            path_info: None,
            forwarded_proto: None,
            forwarded_host: None,
        };

        let result = executor.execute(request);

        match result {
            ExecuteResult::Immediate(resp) => {
                assert_eq!(resp.status, 529, "backpressure should return 529");
                assert_eq!(resp.body, Bytes::from_static(b"Site is overloaded"));
                let retry_after = resp
                    .headers
                    .iter()
                    .find(|(n, _)| n.as_str() == "retry-after");
                assert!(retry_after.is_some(), "should include Retry-After header");
                assert_eq!(retry_after.unwrap().1, "3");
            }
            ExecuteResult::Deferred(_) => panic!("expected Immediate, got Deferred"),
        }
    }
}
