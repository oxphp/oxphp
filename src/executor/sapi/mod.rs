use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

use pool::{
    now_millis, run_scale_manager, run_worker_monitor, spawn_initial, ManagedWorker, SpawnStrategy,
    WorkerRequest,
};
use traditional::WorkerLoopMode;
use worker_mode::WorkerModeConfig;

pub struct SapiExecutor {
    request_tx: Option<crossbeam_channel::Sender<WorkerRequest>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    workers: Arc<Mutex<Vec<ManagedWorker>>>,
    mode: WorkerMode,
    strategy: SpawnStrategy,
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

        let strategy = if let Some(ref wmc) = worker_mode_config {
            SpawnStrategy::WorkerMode {
                config: wmc.clone(),
                metrics: Arc::clone(worker_metrics.as_ref().unwrap()),
            }
        } else {
            SpawnStrategy::Traditional { loop_mode }
        };
        let managed_workers = spawn_initial(&strategy, &request_rx, initial_count);

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
            strategy,
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
        let strategy = self.strategy.clone();

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
            strategy: SpawnStrategy::Traditional {
                loop_mode: WorkerLoopMode::Static,
            },
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
