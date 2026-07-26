use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use crossbeam_channel::{self, TrySendError};
use http::{HeaderName, HeaderValue};

use crate::config::{Config, WorkerMode};
use crate::executor::admission::{Admission, Admitted, ShedReason};
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

/// Build the spawn strategy once based on `WORKER_MODE_ENABLED`.
///
/// Side effects (worker-mode branch only):
/// - registers `oxphp_bridge_set_worker_callbacks`,
/// - creates `WorkerMetrics` and publishes it via `metrics.set_worker_metrics`.
fn build_spawn_strategy(config: &Config, metrics: &Arc<Metrics>) -> SpawnStrategy {
    if config.worker_mode_enabled {
        // Validated at startup: worker mode requires a `.php` entry file.
        let entry_file = config
            .entry_file
            .as_ref()
            .expect("WORKER_MODE_ENABLED=true requires ENTRY_FILE — should have been caught by Config::validate");
        let wmc = Arc::new(WorkerModeConfig {
            entry_file: entry_file.clone(),
            document_root: config.server.document_root.clone(),
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
            server_metrics: Arc::clone(metrics),
        }
    } else {
        let loop_mode = match &config.worker_mode {
            WorkerMode::Static(_) => WorkerLoopMode::Static,
            WorkerMode::Dynamic { .. } => WorkerLoopMode::Dynamic,
        };
        SpawnStrategy::Traditional {
            loop_mode,
            server_metrics: Arc::clone(metrics),
        }
    }
}

fn log_startup(
    mode: &WorkerMode,
    strategy: &SpawnStrategy,
    initial_count: usize,
    queue_capacity: usize,
    queue_wait_timeout_ms: u64,
    queue_max_waiting: usize,
    idle_timeout_seconds: u64,
) {
    if let SpawnStrategy::WorkerMode { config, .. } = strategy {
        tracing::info!(
            mode = ?mode,
            workers = initial_count,
            queue_capacity,
            queue_wait_timeout_ms,
            queue_max_waiting,
            idle_timeout_seconds,
            entry_file = %config.entry_file.display(),
            "PHP worker pool started (worker mode)"
        );
    } else {
        tracing::info!(
            mode = ?mode,
            workers = initial_count,
            queue_capacity,
            queue_wait_timeout_ms,
            queue_max_waiting,
            idle_timeout_seconds,
            "PHP worker pool started"
        );
    }
}

/// 529 body shared by every path that refuses for overload — the fail-fast
/// shed, the expired wait budget, and the worker-side pickup check — so they
/// cannot drift apart.
pub(crate) fn overloaded_response() -> ScriptResponse {
    ScriptResponse {
        status: 529,
        headers: vec![
            (
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                HeaderName::from_static("retry-after"),
                HeaderValue::from_static("3"),
            ),
        ],
        body: Bytes::from_static(b"Site is overloaded"),
        refused: true,
        ..Default::default()
    }
}

/// Hand a request that already holds a queue-slot permit to the workers.
///
/// Holding a permit means a slot was free, so `Full` here is a broken
/// invariant rather than backpressure — it is still mapped to the overload
/// response because the request cannot run either way.
fn send_admitted(
    tx: &crossbeam_channel::Sender<WorkerRequest>,
    metrics: &Metrics,
    script: ScriptRequest,
    response_tx: tokio::sync::oneshot::Sender<ScriptResponse>,
    permit: tokio::sync::OwnedSemaphorePermit,
    deadline: Option<std::time::Instant>,
) -> Result<(), ScriptResponse> {
    let worker_request = WorkerRequest {
        script,
        response_tx,
        permit,
        deadline,
    };
    match tx.try_send(worker_request) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => {
            // Unreachable by construction: the permit count equals the queue
            // capacity, so a permit holder always has a slot. Reaching this
            // means the two have drifted apart and the queue is silently
            // shedding at the wrong depth — loud, because no metric would
            // distinguish it from ordinary overload.
            tracing::error!(
                "queue reported full while holding an admission permit — permit \
                 count and queue capacity have diverged; shedding this request"
            );
            metrics.request_admission_refused(ShedReason::QueueFull);
            Err(shed_response(ShedReason::QueueFull))
        }
        Err(TrySendError::Disconnected(_)) => {
            // No receiver left: every worker thread is gone, which is a dead
            // pool rather than a busy one. Counted because the response is an
            // ordinary 500 — without a series of its own, "the pool died" is
            // indistinguishable from "the application errored" and shows up
            // only in the logs.
            tracing::error!("PHP worker queue has no receivers — the pool is gone");
            metrics.request_admission_refused(ShedReason::PoolUnavailable);
            Err(shed_response(ShedReason::PoolUnavailable))
        }
    }
}

/// The response for a refusal, by reason. Single mapping so the fail-fast
/// path and the wait path cannot answer the same condition differently.
fn shed_response(reason: ShedReason) -> ScriptResponse {
    match reason {
        ShedReason::ShuttingDown => shutting_down_response(),
        ShedReason::PoolUnavailable => pool_unavailable_response(),
        ShedReason::QueueFull | ShedReason::WaitTimeout | ShedReason::WaitingFull => {
            overloaded_response()
        }
    }
}

/// Emitted when admission is refused because the gate is closing.
///
/// Not the 529: teardown is not overload, and a client told "overloaded, retry
/// in 3" learns the wrong thing about an instance that is going away. 503 with
/// the drain path's own retry window is what the rest of shutdown already
/// answers, and what a load balancer reads as "take this instance out".
fn shutting_down_response() -> ScriptResponse {
    ScriptResponse {
        status: 503,
        headers: vec![
            (
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                HeaderName::from_static("retry-after"),
                HeaderValue::from_static("5"),
            ),
        ],
        body: Bytes::from_static(b"Server is shutting down"),
        refused: true,
        ..Default::default()
    }
}

/// Emitted when the channel is closed — a dead pool, not backpressure.
fn pool_unavailable_response() -> ScriptResponse {
    ScriptResponse {
        status: 500,
        headers: vec![(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )],
        body: Bytes::from_static(b"PHP worker pool unavailable"),
        refused: true,
        ..Default::default()
    }
}

pub struct SapiExecutor {
    request_tx: Option<crossbeam_channel::Sender<WorkerRequest>>,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    /// One permit per queue slot. A request only enters the channel once it
    /// holds one, so `try_send` can no longer legitimately report a full
    /// queue. `Arc` because the wait path moves it into a `'static` future.
    admission: Arc<Admission>,
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
        let max_waiting = config.queue_max_waiting;

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
            config.queue_wait_timeout_ms,
            max_waiting,
            idle_timeout_seconds,
        );

        Self {
            request_tx: Some(request_tx),
            request_rx,
            admission: Arc::new(Admission::new(
                queue_capacity,
                config.queue_wait_timeout_ms,
                max_waiting,
            )),
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
        use crate::executor::ExecuteResult;

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let tx = self.request_tx.as_ref().unwrap();

        // One deadline, stamped on arrival and carried through both waits —
        // for a slot here, and for a worker once in the queue. Taking the
        // budget fresh at each stage would let a request spend it twice, which
        // is how a wait budget of a second turns into a queue wait of half a
        // minute on a pool whose handlers are slow. Costs the admitted path one
        // clock read, and nothing at all in fail-fast mode.
        let deadline = self
            .admission
            .budget()
            .map(|budget| std::time::Instant::now() + budget);

        // Fast path: a free queue slot is available right now. Stays
        // synchronous and allocation-free — the overwhelming majority of
        // requests never touch the wait path below.
        let refused = match self.admission.try_admit() {
            Ok(permit) => {
                return match send_admitted(
                    tx,
                    &self.metrics,
                    request,
                    response_tx,
                    permit,
                    deadline,
                ) {
                    Ok(()) => ExecuteResult::Deferred(response_rx),
                    Err(resp) => ExecuteResult::Rejected(resp),
                }
            }
            Err(reason) => reason,
        };

        // No slot right now. A closed gate never becomes open, so waiting on it
        // is pointless whatever the budget says; a full queue is worth waiting
        // on only if there is a budget to wait with. Everything else is the
        // historical fail-fast shed, where the trip point is the instantaneous
        // queue depth.
        let Some(deadline) = deadline.filter(|_| refused != ShedReason::ShuttingDown) else {
            self.metrics.request_admission_refused(refused);
            return ExecuteResult::Rejected(shed_response(refused));
        };

        // Claim a place in the waiting set before committing to anything.
        // Refusal here is what a sustained overload settles into, so it is
        // answered synchronously — allocating a future only to refuse would put
        // the allocation on the shed path and leave the fast path the only one
        // free of it.
        let parked = match self.admission.try_park() {
            Ok(parked) => parked,
            Err(reason) => {
                self.metrics.request_admission_refused(reason);
                return ExecuteResult::Rejected(shed_response(reason));
            }
        };

        // With a budget, wait for a slot and shed only if the request can no
        // longer make it. Awaiting the future in the connection task rather
        // than a detached one keeps the wait tied to the request it belongs to.
        let tx = tx.clone();
        let admission = Arc::clone(&self.admission);
        let metrics = Arc::clone(&self.metrics);
        ExecuteResult::Admitting(Box::pin(async move {
            match admission.admit(parked, deadline).await {
                Admitted::Slot(permit) => {
                    // A client that leaves during the wait needs nothing from
                    // this future: hyper drops the request future, which drops
                    // the wait and releases its place in the waiting set on the
                    // spot. This check covers the remainder — a departure in
                    // the gap between the slot being granted and the request
                    // being sent. Handing a dead request to a worker costs a
                    // queue slot and a pickup that the 499 fast path throws
                    // away, ahead of requests someone is still waiting on.
                    //
                    // Neither covers an HTTP/1.1 close mid-handler: hyper does
                    // not report it, so there is nothing to drop and nothing to
                    // check.
                    if request.cancel_state.get() != crate::bridge::cancel::CancelReason::None {
                        return Err(ScriptResponse::client_closed());
                    }
                    send_admitted(&tx, &metrics, request, response_tx, permit, Some(deadline))
                        .map(|()| response_rx)
                }
                Admitted::Shed(reason) => {
                    metrics.request_admission_refused(reason);
                    Err(shed_response(reason))
                }
            }
        }))
    }

    fn shutdown(&self) {
        // No-op: cleanup handled in Drop
    }

    fn close_admission(&self) {
        // Requests parked here are invisible to the worker registry's hard
        // cancel — they have no worker yet — so this is the only thing that
        // turns them into a response instead of a dropped connection. `close`
        // wakes every waiter at once and they shed as `shutting_down`, which
        // answers 503 like the rest of the drain rather than "overloaded".
        self.admission.close();
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

        // 2. Close the admission gate before dropping our sender. A request
        //    still parked waiting for a queue slot holds its own clone of the
        //    sender, so leaving it parked would keep the channel open and the
        //    workers blocked in `recv` for the join below. The server drops
        //    its Tokio runtime before the executor, so no waiter is normally
        //    alive by this point — closing first is what makes that ordering
        //    a safety margin rather than the only thing preventing a hang.
        self.admission.close();

        // 3. Drop sender to close channel — workers will exit their recv loop
        self.request_tx.take();

        // 4. Signal each worker to shut down and join
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.drain(..) {
                worker.shutdown.store(true, Ordering::Relaxed);
                let _ = worker.handle.join();
            }
        }

        // 5. PHP shutdown after all workers are done
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

    /// Executor whose queue holds `capacity` requests and whose admission gate
    /// waits `wait_timeout_ms` for a slot. No workers are spawned, so nothing
    /// ever drains the queue — capacity is exhausted by sending into it.
    fn test_executor(capacity: usize, wait_timeout_ms: u64) -> SapiExecutor {
        let (tx, rx) = crossbeam_channel::bounded::<WorkerRequest>(capacity);
        SapiExecutor {
            request_tx: Some(tx),
            request_rx: rx,
            admission: Arc::new(Admission::new(capacity, wait_timeout_ms, 64)),
            workers: Arc::new(Mutex::new(Vec::new())),
            mode: WorkerMode::Static(1),
            strategy: Arc::new(SpawnStrategy::Traditional {
                loop_mode: WorkerLoopMode::Static,
                server_metrics: Arc::new(Metrics::new()),
            }),
            global_shutdown: Arc::new(AtomicBool::new(false)),
            metrics: Arc::new(Metrics::new()),
            idle_timeout_seconds: 30,
        }
    }

    /// PHP was never initialized in these tests, so the real `Drop` impl would
    /// call `php_module_shutdown` / `sapi_shutdown` / `tsrm_shutdown` against
    /// uninitialized state — undefined behaviour under the `php` feature.
    /// Leak the executor; the test process exits immediately after.
    fn forget_executor(executor: SapiExecutor) {
        std::mem::forget(executor);
    }

    fn assert_overloaded(resp: &ScriptResponse) {
        assert_eq!(resp.status, 529, "backpressure should return 529");
        assert_eq!(resp.body, Bytes::from_static(b"Site is overloaded"));
        let retry_after = resp
            .headers
            .iter()
            .find(|(n, _)| n.as_str() == "retry-after");
        assert!(retry_after.is_some(), "should include Retry-After header");
        assert_eq!(retry_after.unwrap().1, "3");
    }

    #[test]
    fn test_backpressure_returns_529_with_retry_after() {
        use crate::executor::ExecuteResult;

        // Zero wait budget = fail fast: the queue is full, so the request is
        // shed on the spot rather than waiting for a slot.
        let executor = test_executor(0, 0);

        match executor.execute(make_request()) {
            ExecuteResult::Rejected(resp) => assert_overloaded(&resp),
            _ => panic!("expected Rejected 529"),
        }
        // The shed must be countable — it is otherwise invisible server-side.
        assert!(
            executor
                .metrics
                .to_prometheus()
                .contains("oxphp_admission_refused_total{reason=\"queue_full\"} 1"),
            "fail-fast shed must be counted, and counted as queue_full"
        );

        forget_executor(executor);
    }

    #[tokio::test]
    async fn test_admission_wait_sheds_after_budget() {
        use crate::executor::ExecuteResult;

        // Capacity 1, already occupied, and nothing drains it — the second
        // request must wait for the budget and only then be shed.
        let executor = test_executor(1, 150);
        assert!(
            matches!(executor.execute(make_request()), ExecuteResult::Deferred(_)),
            "first request takes the only slot"
        );

        let start = std::time::Instant::now();
        match executor.execute(make_request()) {
            ExecuteResult::Admitting(fut) => match fut.await {
                Err(resp) => assert_overloaded(&resp),
                Ok(_) => panic!("no slot ever freed — must shed"),
            },
            _ => panic!("expected Admitting once the queue is full"),
        }
        assert!(
            start.elapsed() >= std::time::Duration::from_millis(150),
            "must wait the budget before shedding, waited {:?}",
            start.elapsed()
        );

        forget_executor(executor);
    }

    #[tokio::test]
    async fn test_admission_wait_admits_when_a_slot_frees() {
        use crate::executor::ExecuteResult;

        let executor = test_executor(1, 5_000);
        assert!(
            matches!(executor.execute(make_request()), ExecuteResult::Deferred(_)),
            "first request takes the only slot"
        );

        let admitting = match executor.execute(make_request()) {
            ExecuteResult::Admitting(fut) => fut,
            _ => panic!("expected Admitting once the queue is full"),
        };

        // Stand in for a worker picking the queued request up: drain it and
        // release its permit, exactly as the worker loop does.
        let queued = executor.request_rx.recv().expect("queued request");
        drop(queued.permit);

        assert!(
            admitting.await.is_ok(),
            "a freed slot inside the budget must admit, not shed"
        );

        forget_executor(executor);
    }

    fn make_request() -> ScriptRequest {
        use http::{HeaderMap, Method, Uri};
        use std::path::PathBuf;

        ScriptRequest {
            request_id: String::new(),
            script_path: PathBuf::from("/var/www/public/index.php"),
            method: Method::GET,
            uri: Uri::from_static("/"),
            query_string: String::new(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
            remote_addr: "127.0.0.1:0".parse().unwrap(),
            document_root: Arc::new(PathBuf::from("/var/www/public")),
            cancel_state: std::sync::Arc::new(crate::bridge::cancel::CancellationState::new()),
            trace_id: String::new(),
            span_id: String::new(),
            parent_span_id: String::new(),
            is_tls: false,
            version: http::Version::HTTP_11,
            path_info: None,
            forwarded_proto: None,
            forwarded_host: None,
            forwarded_port: None,
            denied_meta: None,
            profiling_mode: crate::profiling::ProfilingMode::Off,
            profiling_run_id: None,
        }
    }
}
