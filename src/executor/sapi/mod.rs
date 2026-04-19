use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use crossbeam_channel::{self, TrySendError};
use http::{HeaderName, HeaderValue};

use crate::config::{Config, WorkerMode};
use crate::executor::ScriptExecutor;
use crate::metrics::{Metrics, WorkerMetrics};
use crate::php::bindings;
use crate::php::sapi;
use crate::types::{ScriptRequest, ScriptResponse};

mod pool;
mod traditional;
mod worker_mode;

use pool::{run_scale_manager, run_worker_monitor, ManagedWorker, SpawnStrategy, WorkerRequest};
use traditional::WorkerLoopMode;
use worker_mode::WorkerModeConfig;

/// TSRM + SAPI + module startup + error callback installation, in order.
/// Panics on unrecoverable failure to match the previous behavior verbatim.
fn php_startup() {
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
    let startup_result = unsafe { bindings::php_module_startup(&mut module, std::ptr::null_mut()) };
    if startup_result != 0 {
        panic!("php_module_startup() failed with code {startup_result}");
    }

    // 4. Install structured error logging callback (must be after php_module_startup)
    unsafe {
        sapi::install_error_cb();
    }
}

/// Build the spawn strategy once based on `WORKER_FILE` presence.
///
/// Side effects (worker-mode branch only):
/// - registers `oxphp_bridge_set_worker_callbacks`,
/// - creates `WorkerMetrics` and publishes it via `metrics.set_worker_metrics`.
fn build_spawn_strategy(config: &Config, metrics: &Metrics) -> SpawnStrategy {
    if let Some(ref worker_file) = config.worker_file {
        let wmc = Arc::new(WorkerModeConfig {
            worker_file: worker_file.clone(),
            document_root: config.server.document_root.clone(),
            max_requests: config.worker_max_requests,
            max_memory_mib: config.worker_max_memory_mib,
        });

        unsafe {
            bindings::oxphp_bridge_set_worker_callbacks(
                sapi::get_worker_wait_callback(),
                sapi::get_worker_send_callback(),
            );
        }

        let max_workers = config.worker_mode.max_worker_count();
        let wm = Arc::new(WorkerMetrics::new(max_workers));
        metrics.set_worker_metrics(Arc::clone(&wm));

        SpawnStrategy::WorkerMode {
            config: wmc,
            metrics: wm,
        }
    } else {
        let loop_mode = match &config.worker_mode {
            WorkerMode::Static(_) => WorkerLoopMode::Static,
            WorkerMode::Dynamic { .. } => WorkerLoopMode::Dynamic,
        };
        SpawnStrategy::Traditional { loop_mode }
    }
}

fn log_startup(
    mode: &WorkerMode,
    strategy: &SpawnStrategy,
    initial_count: usize,
    queue_capacity: usize,
    idle_timeout_seconds: u64,
) {
    if let SpawnStrategy::WorkerMode { config, .. } = strategy {
        tracing::info!(
            mode = ?mode,
            workers = initial_count,
            queue_capacity,
            idle_timeout_seconds,
            worker_file = %config.worker_file.display(),
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
}

pub struct SapiExecutor {
    request_tx: Option<crossbeam_channel::Sender<WorkerRequest>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    mode: WorkerMode,
    strategy: Arc<SpawnStrategy>,
    global_shutdown: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    idle_timeout_seconds: u64,
}

impl SapiExecutor {
    pub fn new(config: &Config, metrics: Arc<Metrics>) -> Self {
        let mode = config.worker_mode.clone();
        let idle_timeout_seconds = config.worker_idle_timeout_seconds;
        let initial_count = mode.worker_count();
        let queue_capacity = config.queue_capacity;

        php_startup();

        let (request_tx, request_rx) = crossbeam_channel::bounded(queue_capacity);

        let strategy = Arc::new(build_spawn_strategy(config, &metrics));
        let managed_workers = pool::spawn_initial(&strategy, &request_rx, initial_count);

        pool::seed_metrics(&metrics, &mode, initial_count);
        log_startup(
            &mode,
            &strategy,
            initial_count,
            queue_capacity,
            idle_timeout_seconds,
        );

        Self {
            request_tx: Some(request_tx),
            request_rx,
            workers: Arc::new(Mutex::new(managed_workers)),
            mode,
            strategy,
            global_shutdown: Arc::new(AtomicBool::new(false)),
            metrics,
            idle_timeout_seconds,
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
        let strategy = Arc::clone(&self.strategy);

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
                        strategy,
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
                        strategy,
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

#[cfg(test)]
mod tests {
    use super::pool::now_millis;
    use super::*;

    // ── now_millis test ──

    #[test]
    fn test_now_millis_monotonic() {
        let a = now_millis();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = now_millis();
        assert!(b >= a, "now_millis must be non-decreasing");
        // Sanity bound: a single sleep+call shouldn't exceed 10s even on slow CI.
        assert!(b - a < 10_000);
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
            strategy: Arc::new(SpawnStrategy::Traditional {
                loop_mode: WorkerLoopMode::Static,
            }),
            global_shutdown: Arc::new(AtomicBool::new(false)),
            metrics,
            idle_timeout_seconds: 30,
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
            denied_meta: None,
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

        // PHP was never initialized in this test, so the real `Drop` impl would
        // call `php_module_shutdown` / `sapi_shutdown` / `tsrm_shutdown` against
        // uninitialized state — undefined behaviour under the `php` feature.
        // Leak the executor; the test process exits immediately after.
        std::mem::forget(executor);
    }
}
