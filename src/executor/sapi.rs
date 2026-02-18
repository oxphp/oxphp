use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use crossbeam_channel::{self, RecvTimeoutError, TrySendError};
use http::{HeaderName, HeaderValue};

use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::php::bindings;
use crate::php::sapi;
use crate::types::{ScriptRequest, ScriptResponse};

struct WorkerRequest {
    script: ScriptRequest,
    response_tx: tokio::sync::oneshot::Sender<ScriptResponse>,
}

/// Worker scaling mode parsed from `PHP_WORKERS` env var.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerMode {
    /// Fixed number of workers.
    Static(usize),
    /// Dynamic scaling between min and max.
    Dynamic { min: usize, max: usize },
}

impl WorkerMode {
    /// Initial worker count: exact for static, min for dynamic.
    pub fn worker_count(&self) -> usize {
        match self {
            WorkerMode::Static(n) => *n,
            WorkerMode::Dynamic { min, .. } => *min,
        }
    }
}

/// Per-worker state for the managed worker pool.
struct ManagedWorker {
    /// Not read at runtime; kept so debug-printing `workers` shows which ID each slot holds.
    #[allow(dead_code)]
    id: usize,
    handle: std::thread::JoinHandle<()>,
    shutdown: Arc<AtomicBool>,
    last_active: Arc<AtomicU64>,
}

/// Controls whether the worker loop uses blocking `recv()` or `recv_timeout()`.
/// Static mode workers sleep via futex with zero CPU cost; dynamic mode workers
/// must wake periodically to check their per-worker shutdown flag.
#[derive(Clone, Copy)]
enum WorkerLoopMode {
    /// Blocking recv — exits only when channel closes (sender dropped).
    Static,
    /// Timeout-based recv — checks `shutdown` flag between timeouts.
    Dynamic,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Parse `PHP_WORKERS` env var into a `WorkerMode`.
///
/// Formats:
/// - `""` or `"0"` → Static(cpu_count * 2)
/// - `"N"` → Static(N)
/// - `"MIN:MAX"` → Dynamic { min, max }
/// - `"0:0"` → Dynamic { min: cpu/2 (min 2), max: cpu*2 }
fn parse_php_workers(val: &str) -> Result<WorkerMode, String> {
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    if let Some((left, right)) = val.split_once(':') {
        if left.is_empty() || right.is_empty() {
            return Err(format!(
                "invalid PHP_WORKERS: '{val}' (both MIN and MAX required)"
            ));
        }
        let min_raw: usize = left.parse().map_err(|_| format!("invalid MIN: '{left}'"))?;
        let max_raw: usize = right
            .parse()
            .map_err(|_| format!("invalid MAX: '{right}'"))?;
        let min = if min_raw == 0 {
            (cpu / 2).max(2)
        } else {
            min_raw
        };
        let max = if max_raw == 0 { cpu * 2 } else { max_raw };
        if min > max {
            return Err(format!("PHP_WORKERS: min ({min}) > max ({max})"));
        }
        Ok(WorkerMode::Dynamic { min, max })
    } else {
        let n: usize = val
            .parse()
            .map_err(|_| format!("invalid PHP_WORKERS: '{val}'"))?;
        let count = if n == 0 { cpu * 2 } else { n };
        Ok(WorkerMode::Static(count))
    }
}

pub struct SapiExecutor {
    request_tx: Option<crossbeam_channel::Sender<WorkerRequest>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    mode: WorkerMode,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    idle_timeout_sec: u64,
}

impl SapiExecutor {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        let mode = std::env::var("PHP_WORKERS")
            .ok()
            .and_then(|v| {
                if v.is_empty() {
                    None
                } else {
                    match parse_php_workers(&v) {
                        Ok(m) => Some(m),
                        Err(e) => {
                            tracing::error!("{e}");
                            None
                        }
                    }
                }
            })
            .unwrap_or_else(|| {
                let cpu = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);
                WorkerMode::Static(cpu * 2)
            });

        let idle_timeout_sec: u64 = std::env::var("PHP_WORKERS_IDLE_SEC")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);

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

        let queue_capacity = std::env::var("QUEUE_CAPACITY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(initial_count * 128);

        let (request_tx, request_rx) = crossbeam_channel::bounded(queue_capacity);

        let loop_mode = match &mode {
            WorkerMode::Static(_) => WorkerLoopMode::Static,
            WorkerMode::Dynamic { .. } => WorkerLoopMode::Dynamic,
        };

        let mut managed_workers = Vec::with_capacity(initial_count);
        for i in 0..initial_count {
            let shutdown = Arc::new(AtomicBool::new(false));
            let last_active = Arc::new(AtomicU64::new(now_millis()));
            let handle = spawn_worker(
                i,
                request_rx.clone(),
                Arc::clone(&shutdown),
                Arc::clone(&last_active),
                loop_mode,
            );
            managed_workers.push(ManagedWorker {
                id: i,
                handle,
                shutdown,
                last_active,
            });
        }

        // Set initial metrics
        metrics.set_workers_current(initial_count);
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

        tracing::info!(
            mode = ?mode,
            workers = initial_count,
            queue_capacity,
            idle_timeout_sec,
            "PHP worker pool started"
        );

        Self {
            request_tx: Some(request_tx),
            request_rx,
            workers: Arc::new(Mutex::new(managed_workers)),
            mode,
            global_shutdown: Arc::new(AtomicBool::new(false)),
            metrics,
            idle_timeout_sec,
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
                TrySendError::Full(_) => {
                    (503, Bytes::from_static(b"Service Unavailable: queue full"))
                }
                TrySendError::Disconnected(_) => {
                    (500, Bytes::from_static(b"PHP worker pool unavailable"))
                }
            };
            let mut headers = Vec::new();
            if status == 503 {
                headers.push((
                    HeaderName::from_static("retry-after"),
                    HeaderValue::from_static("1"),
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

        match &self.mode {
            WorkerMode::Static(target) => {
                let target = *target;
                tokio::spawn(async move {
                    run_worker_monitor(workers, request_rx, target, global_shutdown, metrics).await;
                });
                tracing::info!(target, "Worker health monitor started");
            }
            WorkerMode::Dynamic { min, max } => {
                let min = *min;
                let max = *max;
                let idle_timeout_sec = self.idle_timeout_sec;
                tokio::spawn(async move {
                    run_scale_manager(
                        workers,
                        request_rx,
                        min,
                        max,
                        idle_timeout_sec,
                        global_shutdown,
                        metrics,
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

fn spawn_worker(
    id: usize,
    rx: crossbeam_channel::Receiver<WorkerRequest>,
    shutdown: Arc<AtomicBool>,
    last_active: Arc<AtomicU64>,
    loop_mode: WorkerLoopMode,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("php-worker-{id}"))
        .spawn(move || {
            worker_thread(rx, shutdown, last_active, loop_mode);
        })
        .expect("failed to spawn PHP worker thread")
}

fn worker_thread(
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    shutdown: Arc<AtomicBool>,
    last_active: Arc<AtomicU64>,
    loop_mode: WorkerLoopMode,
) {
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("php-worker")
        .to_string();

    // Initialize TSRM thread-local storage for this worker thread (required for ZTS)
    unsafe {
        let _ = bindings::ts_resource_ex(0, std::ptr::null_mut());
    }

    tracing::info!(worker = %thread_name, "PHP worker thread started");

    match loop_mode {
        WorkerLoopMode::Static => {
            // Blocking recv — zero CPU while idle, exits when channel closes.
            while let Ok(wr) = request_rx.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_request(&wr.script)
                }));
                match result {
                    Ok(response) => {
                        let _ = wr.response_tx.send(response);
                    }
                    Err(_) => {
                        // wr.response_tx dropped → client gets 500
                        tracing::error!(worker = %thread_name, "Worker panicked, exiting for respawn");
                        break;
                    }
                }
            }
        }
        WorkerLoopMode::Dynamic => {
            // Timeout-based recv — wakes every 200ms to check shutdown flag.
            // Stores last_active timestamp for the scale manager's idle detection.
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                match request_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(wr) => {
                        last_active.store(now_millis(), Ordering::Relaxed);
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            execute_request(&wr.script)
                        }));
                        match result {
                            Ok(response) => {
                                let _ = wr.response_tx.send(response);
                            }
                            Err(_) => {
                                tracing::error!(worker = %thread_name, "Worker panicked, exiting for respawn");
                                break;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        }
    }

    tracing::info!(worker = %thread_name, "PHP worker thread stopped");
}

/// Health monitor for static mode: detects dead workers and respawns to target count.
async fn run_worker_monitor(
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    target: usize,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
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
            let handle = spawn_worker(
                next_id,
                request_rx.clone(),
                Arc::clone(&shutdown),
                Arc::clone(&last_active),
                WorkerLoopMode::Static,
            );
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
    idle_timeout_sec: u64,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut last_scale_up = Instant::now();
    let mut last_scale_down = Instant::now();
    let idle_timeout_ms = idle_timeout_sec * 1000;
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
            let handle = spawn_worker(
                next_id,
                request_rx.clone(),
                Arc::clone(&shutdown),
                Arc::clone(&last_active),
                WorkerLoopMode::Dynamic,
            );
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
        metrics.set_workers_idle(idle_count);

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
            drop(workers_guard);

            let handle = spawn_worker(
                spawn_id,
                spawn_rx,
                spawn_shutdown,
                spawn_last_active,
                WorkerLoopMode::Dynamic,
            );

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

/// Initialize the PHP engine on the main thread (TSRM + SAPI + module startup).
/// Must be called once before any worker threads are spawned.
/// After this, worker threads can call `php_thread_init()` + `execute_request()`.
pub fn php_module_init() {
    // 1. TSRM must be initialized first for ZTS builds
    if !unsafe { bindings::php_tsrm_startup() } {
        panic!("php_tsrm_startup() failed");
    }

    // 2. Build and register our SAPI module
    let mut module = sapi::build_sapi_module();
    unsafe {
        bindings::sapi_startup(&mut module);
    }

    // 3. Start the PHP engine
    let startup_result = unsafe { bindings::php_module_startup(&mut module, std::ptr::null_mut()) };
    if startup_result != 0 {
        panic!("php_module_startup() failed with code {startup_result}");
    }

    // 4. Install structured error logging callback (must be after php_module_startup)
    unsafe {
        sapi::install_error_cb();
    }

    tracing::info!("PHP engine initialized");
}

/// Shut down the PHP engine on the main thread.
/// Must be called after all worker threads have exited.
pub fn php_module_shutdown() {
    unsafe {
        bindings::php_module_shutdown();
        bindings::sapi_shutdown();
        bindings::tsrm_shutdown();
    }
    tracing::info!("PHP engine shut down");
}

/// Initialize PHP ZTS thread-local storage for the current thread.
/// Must be called once per worker thread before `execute_request()`.
pub fn php_thread_init() {
    unsafe {
        let _ = bindings::ts_resource_ex(0, std::ptr::null_mut());
    }
}

/// RAII guard that clears SAPI request data on drop (even on panic).
struct RequestDataGuard;

impl Drop for RequestDataGuard {
    fn drop(&mut self) {
        sapi::clear_request_data();
    }
}

pub(crate) fn execute_request(request: &ScriptRequest) -> ScriptResponse {
    let start = Instant::now();

    sapi::clear_buffers();
    sapi::set_request_data(request);
    let _guard = RequestDataGuard;

    if unsafe { bindings::php_request_startup() } != 0 {
        return ScriptResponse {
            status: 500,
            body: Bytes::from_static(b"php_request_startup() failed"),
            ..Default::default()
        };
    }

    let script_path_str = request.script_path.to_str().unwrap_or("");
    let script_path = CString::new(script_path_str).unwrap_or_default();

    let mut file_handle: bindings::zend_file_handle = unsafe { std::mem::zeroed() };
    unsafe {
        bindings::zend_stream_init_filename(&mut file_handle, script_path.as_ptr());
    }

    file_handle.primary_script = true;

    unsafe { bindings::php_execute_script(&mut file_handle) };

    unsafe {
        bindings::zend_destroy_file_handle(&mut file_handle);
    }

    unsafe {
        bindings::php_request_shutdown(std::ptr::null_mut());
    }

    // Single batched TLS lookup for all response data.
    let (raw_output, raw_headers, status) = sapi::take_response();
    let body = Bytes::from(raw_output);

    // Parse header strings into typed HeaderName/HeaderValue on the worker thread,
    // so the single-threaded Tokio runtime doesn't pay the parsing cost.
    let headers = raw_headers
        .into_iter()
        .filter_map(|(name, value)| {
            let hn = HeaderName::from_bytes(name.as_bytes()).ok()?;
            let hv = HeaderValue::from_str(&value).ok()?;
            Some((hn, hv))
        })
        .collect();

    ScriptResponse {
        status,
        headers,
        body,
        execution_time_us: start.elapsed().as_micros() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to get cpu count for assertions
    fn cpu_count() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }

    // ── parse_php_workers tests ──

    #[test]
    fn test_parse_static_explicit() {
        assert_eq!(parse_php_workers("8").unwrap(), WorkerMode::Static(8));
    }

    #[test]
    fn test_parse_static_one() {
        assert_eq!(parse_php_workers("1").unwrap(), WorkerMode::Static(1));
    }

    #[test]
    fn test_parse_static_zero_auto() {
        let cpu = cpu_count();
        assert_eq!(parse_php_workers("0").unwrap(), WorkerMode::Static(cpu * 2));
    }

    #[test]
    fn test_parse_dynamic_explicit() {
        assert_eq!(
            parse_php_workers("2:16").unwrap(),
            WorkerMode::Dynamic { min: 2, max: 16 }
        );
    }

    #[test]
    fn test_parse_dynamic_min_equals_max() {
        assert_eq!(
            parse_php_workers("4:4").unwrap(),
            WorkerMode::Dynamic { min: 4, max: 4 }
        );
    }

    #[test]
    fn test_parse_dynamic_zero_min_auto() {
        let cpu = cpu_count();
        let expected_min = (cpu / 2).max(2);
        assert_eq!(
            parse_php_workers("0:16").unwrap(),
            WorkerMode::Dynamic {
                min: expected_min,
                max: 16
            }
        );
    }

    #[test]
    fn test_parse_dynamic_zero_max_auto() {
        let cpu = cpu_count();
        assert_eq!(
            parse_php_workers("2:0").unwrap(),
            WorkerMode::Dynamic {
                min: 2,
                max: cpu * 2
            }
        );
    }

    #[test]
    fn test_parse_dynamic_both_zero() {
        let cpu = cpu_count();
        let expected_min = (cpu / 2).max(2);
        let expected_max = cpu * 2;
        assert_eq!(
            parse_php_workers("0:0").unwrap(),
            WorkerMode::Dynamic {
                min: expected_min,
                max: expected_max
            }
        );
    }

    #[test]
    fn test_parse_min_greater_than_max_error() {
        let result = parse_php_workers("10:5");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("min (10) > max (5)"));
    }

    #[test]
    fn test_parse_invalid_number_error() {
        assert!(parse_php_workers("abc").is_err());
    }

    #[test]
    fn test_parse_invalid_min_error() {
        let result = parse_php_workers("abc:10");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid MIN"));
    }

    #[test]
    fn test_parse_invalid_max_error() {
        let result = parse_php_workers("2:xyz");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid MAX"));
    }

    #[test]
    fn test_parse_empty_side_error() {
        assert!(parse_php_workers(":10").is_err());
        assert!(parse_php_workers("10:").is_err());
        assert!(parse_php_workers(":").is_err());
    }

    #[test]
    fn test_parse_large_values() {
        assert_eq!(
            parse_php_workers("1:1000").unwrap(),
            WorkerMode::Dynamic { min: 1, max: 1000 }
        );
    }

    // ── WorkerMode tests ──

    #[test]
    fn test_worker_mode_count_static() {
        assert_eq!(WorkerMode::Static(8).worker_count(), 8);
    }

    #[test]
    fn test_worker_mode_count_dynamic() {
        assert_eq!(WorkerMode::Dynamic { min: 2, max: 16 }.worker_count(), 2);
    }

    // ── now_millis test ──

    #[test]
    fn test_now_millis_reasonable() {
        let ms = now_millis();
        // Should be after 2020 and before 2100
        assert!(ms > 1_577_836_800_000); // 2020-01-01
        assert!(ms < 4_102_444_800_000); // 2100-01-01
    }
}
