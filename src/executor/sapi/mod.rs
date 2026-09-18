use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use crossbeam_channel::{self, TrySendError};
use http::{HeaderName, HeaderValue};

use crate::config::{Config, WorkerMode};
use crate::executor::admission::{Admission, Admitted, ShedReason, WaitBudget};
use crate::executor::ScriptExecutor;
use crate::metrics::{Metrics, WorkerMetrics};
use crate::php::bindings;
use crate::php::sapi;
use crate::types::{ScriptRequest, ScriptResponse};

/// How long a worker that can be retired sleeps between looks at its shutdown
/// flag. Shared by both worker models on purpose: the interval is the upper
/// bound on how long a retirement takes to be noticed, and having the two
/// loops drift apart would make that bound depend on which model is running.
pub(crate) const WORKER_RETIRE_POLL: std::time::Duration = std::time::Duration::from_millis(200);

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
    // Both models need it: only a dynamic pool ever retires a worker, so only
    // there does a thread have to interrupt its wait to read its own flag.
    let loop_mode = match &config.worker_mode {
        WorkerMode::Static(_) => WorkerLoopMode::Static,
        WorkerMode::Dynamic { .. } => WorkerLoopMode::Dynamic,
    };
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
            loop_mode,
            config: wmc,
            metrics: wm,
            server_metrics: Arc::clone(metrics),
        }
    } else {
        SpawnStrategy::Traditional {
            loop_mode,
            server_metrics: Arc::clone(metrics),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn log_startup(
    mode: &WorkerMode,
    strategy: &SpawnStrategy,
    initial_count: usize,
    queue_capacity: usize,
    queue_wait_timeout_ms: u64,
    queue_max_waiting: usize,
    queue_max_waiting_bytes: usize,
    idle_timeout_seconds: u64,
) {
    if let SpawnStrategy::WorkerMode { config, .. } = strategy {
        tracing::info!(
            mode = ?mode,
            workers = initial_count,
            queue_capacity,
            queue_wait_timeout_ms,
            queue_max_waiting,
            queue_max_waiting_bytes,
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
            queue_max_waiting_bytes,
            idle_timeout_seconds,
            "PHP worker pool started"
        );
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
        ShedReason::QueueFull
        | ShedReason::WaitTimeout
        | ShedReason::WaitingFull
        | ShedReason::WaitingBytes => ScriptResponse::overloaded(),
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
        let max_waiting_bytes = config.queue_max_waiting_bytes;

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
            max_waiting_bytes,
            idle_timeout_seconds,
        );

        let admission = Arc::new(Admission::new(
            queue_capacity,
            config.queue_wait_timeout_ms,
            max_waiting,
            max_waiting_bytes,
        ));

        // Publish the queue itself, so `/metrics` and the supervisor can read
        // where its capacity went rather than infer it. Deliberately a
        // `Receiver` clone and never a `Sender` one: a sender held here would
        // outlive `request_tx` and keep the channel open, and the worker
        // threads' `recv()` would never come back `Disconnected` on shutdown.
        // A receiver costs nothing of the sort — the executor holds one for
        // its whole life already.
        {
            let admission = Arc::clone(&admission);
            let queue = request_rx.clone();
            metrics.set_queue_probe(Box::new(move || crate::metrics::QueueSnapshot {
                depth: queue.len(),
                capacity: admission.capacity(),
                slots_available: admission.slots_available(),
                wait_budget_us: admission.wait_budget().map_or(0, WaitBudget::effective_us),
                wait_budget_ceiling_us: admission
                    .wait_budget()
                    .map_or(0, WaitBudget::configured_us),
            }));
        }

        Self {
            request_tx: Some(request_tx),
            request_rx,
            admission,
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
        // minute on a pool whose handlers are slow. Costs the admitted path
        // this clock read plus the timer the waiting side then arms against
        // the deadline, and nothing at all in fail-fast mode. Measured on a
        // pool serving an empty handler, the pair is indistinguishable from
        // zero: −0.27 %, 95 % CI [−1.19 %, +0.64 %].
        //
        // Stamped together with the answer to what window this request's wait
        // will be measured over, from the one reading of the budget: a request
        // given a shortened wait must not be judged, when that wait runs out,
        // as though it had been given the configured one.
        let (deadline, wait_at_ceiling) = match self.admission.budget() {
            Some((budget, at_ceiling)) => (Some(std::time::Instant::now() + budget), at_ceiling),
            None => (None, false),
        };

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
                    Ok(()) => ExecuteResult::Deferred(crate::executor::Queued {
                        rx: response_rx,
                        deadline,
                        wait_at_ceiling,
                    }),
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

        // And a claim on the memory it will hold while it waits. The body is
        // buffered in full before dispatch, so parking is what turns a
        // transient allocation into one held for the whole budget — a bound in
        // places says how many wait, and nothing about how much they weigh.
        // Refused synchronously for the same reason the place is.
        let charge = match self.admission.try_park_bytes(request.body.len()) {
            Ok(charge) => charge,
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
            // Bound by name, not by `_`: `let _ = charge` would give the bytes
            // back on the spot and leave the budget measuring nothing. Held
            // here, they come back however this future ends — admitted,
            // refused, or dropped along with a client that left.
            let _charge = charge;
            // Paired with the reading below: the gate is the other place a
            // budget can run out, and a wait that ends here has the same
            // question asked of it — did the pool begin anything at all while
            // it waited.
            let starts_on_arrival = crate::metrics::pool_starts();
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
                    // Neither covers a client that stops waiting without
                    // closing, nor — while the wait is on — a close on a
                    // message whose body was never read: no layer is told, so
                    // there is nothing to drop and nothing to check.
                    if request.cancel_state.get() != crate::bridge::cancel::CancelReason::None {
                        return Err(ScriptResponse::client_closed());
                    }
                    send_admitted(&tx, &metrics, request, response_tx, permit, Some(deadline)).map(
                        |()| crate::executor::Queued {
                            rx: response_rx,
                            deadline: Some(deadline),
                            wait_at_ceiling,
                        },
                    )
                }
                Admitted::Shed(reason) => {
                    // The queue deadline's reasoning, for the other budget:
                    // a refusal nobody is waiting for is not one the pool
                    // handed out, and this series is read as the case for
                    // shortening `QUEUE_WAIT_TIMEOUT_MS` — an argument about
                    // the pool, which a client's own patience has no business
                    // making.
                    //
                    // Reachable in spite of the note above about hyper
                    // dropping the future, and by one route rather than any:
                    // the signal the connection raises once `serve_connection`
                    // has returned drops the abort guard and then goes on
                    // awaiting the dispatch on purpose, so this wait runs to
                    // its deadline with the cell already filled. What survives
                    // to hear that signal is an HTTP/2 stream, whose handler
                    // runs as a task of its own rather than inside the
                    // connection. That is the whole of the route: one stream
                    // going away takes its own wait with it on either
                    // protocol, and only the end of the connection under a
                    // request that is still queued gets this far.
                    //
                    // `client_closed()` and not the `_unpicked` variant its
                    // sibling in the queue uses: the flag that one carries
                    // keeps a wait out of `oxphp_queue_wait_us`, and every
                    // answer this gate returns as `Err` is already held out
                    // of it by `rejected` on the dispatch side. Nothing here
                    // was ever in the queue to have a pickup latency.
                    //
                    // `WaitTimeout` and `ClientAbort` and nothing wider: the
                    // other shed reasons are the pool's own state at the
                    // moment of arrival — a full waiting set is full whoever
                    // was asking — and the other cancel reasons are a
                    // worker's, which a request that never reached one cannot
                    // have been given.
                    if reason == ShedReason::WaitTimeout
                        && request.cancel_state.get()
                            == crate::bridge::cancel::CancelReason::ClientAbort
                    {
                        return Err(ScriptResponse::client_closed());
                    }
                    metrics.request_admission_refused(reason);
                    // A budget spent at the gate with no work starting behind
                    // it. This is the shape an application calling back into
                    // itself takes when the queue is short enough that its
                    // inner request never gets into it.
                    //
                    // Only asked over a full-length window. "The pool began
                    // nothing while this request waited" is a statement about
                    // the pool only for as long as the wait was: a healthy
                    // pool of four workers on quarter-second handlers starts
                    // one request every 62 ms, so once the budget is down at
                    // its floor most waits end between two starts and the
                    // counter would climb on a pool that is serving perfectly
                    // well. A shortened budget is also never the diagnosis
                    // this counter leads to — it names a wait that cannot be
                    // bought out at any length, and the server has already
                    // shortened the only thing the operator would be told to
                    // shorten. A pool that starts nothing holds its ceiling on
                    // its own — behind a queue it never fills the controller's
                    // window, so nothing that could shorten it is ever decided
                    // — and this reading is armed there. The self-calling pool
                    // named above is not that pool: it starts about one
                    // request per budget, and once its own clients are less
                    // patient than its handlers, the work it finishes for
                    // clients who have already left is the evidence the
                    // controller halves on. What is left to count is armed by
                    // the budget it arrived on, so the arrivals from before
                    // the drop go on being counted for up to a whole ceiling
                    // after it and then nothing is: measured at two halvings
                    // within seconds, after which this counter took nothing
                    // that arrived — the shape this comment opens with is one
                    // it can miss for the whole of an episode. Where the
                    // callers are patient there is no such evidence, the
                    // ceiling stands, and this is what names the state.
                    // Elsewhere nothing takes over: a budget under its
                    // ceiling with abandoned work climbing is what the
                    // controller decides on, so an ordinary overload of
                    // impatient clients reads the same, and the wedge readings
                    // built on no worker being busy are about a pool starting
                    // nothing.
                    if reason == ShedReason::WaitTimeout
                        && wait_at_ceiling
                        && crate::metrics::pool_starts() == starts_on_arrival
                    {
                        metrics.admission_wait_wasted();
                    }
                    Err(shed_response(reason))
                }
            }
        }))
    }

    fn shutdown(&self) {
        // No-op: cleanup handled in Drop
    }

    /// Whether any worker thread is still running.
    ///
    /// The pool is judged by its threads because that is the part of it a
    /// health probe can be wrong about in a way nothing else would catch: a
    /// pool whose every thread has ended answers nothing at all, and the
    /// monitor that replaces them runs on the Tokio runtime, so a pool left
    /// with no workers and no monitor stays that way. `is_finished` is exactly
    /// the question — the thread has returned or panicked — and it is asked of
    /// the same list the monitor maintains, so a worker it has already
    /// replaced is not counted twice.
    ///
    /// Deliberately not a judgement on whether the pool is *working*: a wedged
    /// worker is a live thread and reads healthy here. That state is watched
    /// for separately, by the supervisor, and reaches the probes through
    /// [`Metrics::pool_stalled`](crate::metrics::Metrics::pool_stalled).
    ///
    /// A pool is never legitimately empty for long: `PHP_WORKERS` never
    /// resolves a minimum below one, so no scale-down reaches this, and the
    /// only other thing that clears the list outright is `Drop`, which joins
    /// every worker on its way out. A worker that ends does leave the pool
    /// short until the monitor reaps and replaces it, and on a single-worker
    /// pool that window reads unhealthy here — which is literally true: until
    /// the replacement lands there is nothing to serve on. What bounds the
    /// window is the monitor's poll rather than the spawn: a finished thread
    /// still in the list already answers `false`, and the monitor looks every
    /// 500 ms.
    fn is_healthy(&self) -> bool {
        // Read through a poisoned lock rather than panicking: the `Vec` is
        // intact whatever panicked while holding it, and a probe that panics
        // answers a connection error, which an orchestrator reads as a failed
        // check for a reason that has nothing to do with the pool.
        let workers = match self.workers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        workers.iter().any(|w| !w.handle.is_finished())
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

        // One controller for either pool model — what it reads is the queue and
        // the pool's own output, and neither depends on how the workers are
        // managed. Spawned before the match for that reason.
        if self.admission.wait_budget().is_some() {
            let admission = Arc::clone(&self.admission);
            let queue = self.request_rx.clone();
            let shutdown = Arc::clone(&self.global_shutdown);
            tokio::spawn(async move {
                run_wait_budget_controller(admission, queue, shutdown).await;
            });
            tracing::info!("Admission wait budget controller started");
        }

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

/// How often the wait budget is recomputed.
///
/// How often the pool is sampled, not how often the budget moves: a tick with
/// a queue behind it and too little work in it is carried forward by
/// [`admission::WaitBudget::observe`] rather than decided on, so how long the
/// budget takes to come down is set by how fast the pool starts requests, not
/// by this period. What the period has to be is short enough that a pool
/// starting work at a healthy rate is not the thing holding the decision up —
/// at four samples a second it is not.
const WAIT_BUDGET_TICK: std::time::Duration = std::time::Duration::from_millis(250);

/// Keeps the admission wait budget matched to what the pool is getting out of
/// the waiting.
///
/// Reads three things per tick and hands them to [`admission::WaitBudget`]:
/// how much the pool started, how much of what it finished went to clients who
/// had already left, and whether anything is queued. Two counters read as
/// deltas and one depth read as a level, because that is what each question
/// is: the first two are rates, the third is a state. How many ticks it takes
/// to move the budget is the budget's own business — this task only feeds it.
async fn run_wait_budget_controller(
    admission: Arc<Admission>,
    queue: crossbeam_channel::Receiver<WorkerRequest>,
    global_shutdown: Arc<AtomicBool>,
) {
    let mut interval = tokio::time::interval(WAIT_BUDGET_TICK);
    // Ticks the task slept through are dropped rather than delivered back to
    // back. They carry no new work — the counters were read once — so a burst
    // of them would land on the recovery arm and walk the budget from its
    // floor to its ceiling inside one scheduling slice, which is the ramp this
    // controller exists to make gradual. A stall is exactly when that is
    // reachable: a small `TOKIO_WORKERS` count under the load the budget is
    // there for.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_started = crate::metrics::pool_starts();
    let mut last_abandoned = crate::metrics::abandoned_work_total();

    loop {
        interval.tick().await;
        if global_shutdown.load(Ordering::Relaxed) {
            break;
        }

        let started = crate::metrics::pool_starts();
        let abandoned = crate::metrics::abandoned_work_total();
        let sample = crate::executor::admission::LoadSample {
            started: started.wrapping_sub(last_started),
            abandoned: abandoned.wrapping_sub(last_abandoned),
            queue_depth: queue.len(),
        };
        last_started = started;
        last_abandoned = abandoned;

        // `wait_budget()` is `Some` for the life of the executor — the task is
        // only spawned when it is — but reading it per tick keeps the fail-fast
        // case a single expression rather than an unwrap justified elsewhere.
        if let Some(budget) = admission.wait_budget() {
            budget.observe(sample);
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
    use super::*;

    /// Executor whose queue holds `capacity` requests and whose admission gate
    /// waits `wait_timeout_ms` for a slot. No workers are spawned, so nothing
    /// ever drains the queue — capacity is exhausted by sending into it.
    fn test_executor(capacity: usize, wait_timeout_ms: u64) -> SapiExecutor {
        let (tx, rx) = crossbeam_channel::bounded::<WorkerRequest>(capacity);
        SapiExecutor {
            request_tx: Some(tx),
            request_rx: rx,
            admission: Arc::new(Admission::new(capacity, wait_timeout_ms, 64, usize::MAX)),
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
    async fn admission_wait_counts_a_shed_nothing_could_have_saved() {
        use crate::executor::ExecuteResult;

        // The same state as the shed test, read for what it records rather
        // than what it answers. There are no workers here, so nothing began a
        // request while the second one waited — which is the whole of what
        // the wasted counter claims, and this pool satisfies it by having no
        // way to start anything at all.
        let executor = test_executor(1, 150);
        assert!(
            matches!(executor.execute(make_request()), ExecuteResult::Deferred(_)),
            "first request takes the only slot"
        );

        match executor.execute(make_request()) {
            ExecuteResult::Admitting(fut) => match fut.await {
                Err(resp) => assert_overloaded(&resp),
                Ok(_) => panic!("no slot ever freed — must shed"),
            },
            _ => panic!("expected Admitting once the queue is full"),
        }

        // Both counters, from the one path that moves them together. The
        // refusal without the wasted wait would be an ordinary overload; the
        // wasted wait without the refusal would be a subset that is not one.
        let out = executor.metrics.to_prometheus();
        assert!(
            out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 1"),
            "a budget spent at the gate is a wait_timeout refusal: {out}"
        );
        assert!(
            out.contains("oxphp_admission_wait_wasted_total 1"),
            "and one no pickup could have saved, so also a wasted wait: {out}"
        );

        // The other arm — the pool did begin something, so do not count it —
        // is not reachable here: the tick only moves from `take_from_queue`,
        // which is compiled under the `php` feature, so a host build has no
        // way to advance it. It is pinned on the waiting side instead, by
        // `await_queued_does_not_call_a_lost_race_a_wasted_wait`.

        forget_executor(executor);
    }

    #[tokio::test]
    async fn admission_wait_charges_nothing_to_a_client_that_has_gone() {
        use crate::bridge::cancel::CancelReason;
        use crate::executor::ExecuteResult;

        // The shed above, on a request whose client left while it waited.
        // Nothing about the pool is different — the budget ran out the same
        // way — but there is nobody the refusal can be handed to, and both
        // series this arm moves are read as statements about the pool.
        //
        // One route reaches it in production: hyper drops the request future
        // when the client goes, which takes this wait with it, except at the
        // end of an HTTP/2 connection under a still-queued stream, where the
        // handler outlives the connection and the dispatch is deliberately
        // kept awaited so a worker's answer can still arrive.
        let executor = test_executor(1, 150);
        assert!(
            matches!(executor.execute(make_request()), ExecuteResult::Deferred(_)),
            "first request takes the only slot"
        );

        let gone = make_request();
        gone.cancel_state.set(CancelReason::ClientAbort);
        match executor.execute(gone) {
            ExecuteResult::Admitting(fut) => match fut.await {
                Err(resp) => {
                    assert_eq!(resp.status, 499, "a departed client is not an overload");
                    assert_eq!(resp.cancel_reason, CancelReason::ClientAbort as u8);
                }
                Ok(_) => panic!("no slot ever freed — must shed"),
            },
            _ => panic!("expected Admitting once the queue is full"),
        }

        let out = executor.metrics.to_prometheus();
        assert!(
            out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 0"),
            "a refusal nobody received is not one the pool handed out: {out}"
        );
        assert!(
            out.contains("oxphp_admission_wait_wasted_total 0"),
            "and it is out of the subset for the same reason: {out}"
        );

        // The conjunct, not just the branch: every other cancel reason is a
        // worker's own, and a request shed at the gate never reached one, so
        // widening this check to "cancelled at all" would answer a drain's
        // own shed with a 499 and drop a refusal the pool really did hand
        // out.
        let draining = make_request();
        draining.cancel_state.set(CancelReason::Shutdown);
        match executor.execute(draining) {
            ExecuteResult::Admitting(fut) => match fut.await {
                Err(resp) => assert_overloaded(&resp),
                Ok(_) => panic!("no slot ever freed — must shed"),
            },
            _ => panic!("expected Admitting once the queue is full"),
        }
        let out = executor.metrics.to_prometheus();
        assert!(
            out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 1"),
            "a cancellation a worker wrote leaves the refusal where it was: {out}"
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

        // Stand in for a worker picking the queued request up, as far as
        // admission is concerned: drain it and release its permit, which is
        // the half of a pickup that frees the slot.
        let queued = executor.request_rx.recv().expect("queued request");
        drop(queued.permit);

        assert!(
            admitting.await.is_ok(),
            "a freed slot inside the budget must admit, not shed"
        );

        forget_executor(executor);
    }

    /// A pool entry whose thread runs until `shutdown` is raised, or one whose
    /// thread has already ended.
    fn managed_worker(id: usize, keep_running: bool) -> ManagedWorker {
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            while keep_running && !flag.load(Ordering::Relaxed) {
                std::thread::sleep(WORKER_RETIRE_POLL);
            }
        });
        if !keep_running {
            // `is_finished` is about the thread's closure having returned, so
            // wait for that rather than for a scheduling window.
            while !handle.is_finished() {
                std::thread::yield_now();
            }
        }
        ManagedWorker {
            id,
            handle,
            shutdown,
            last_active: Arc::new(crate::executor::idle_clock::LastActive::now()),
        }
    }

    #[test]
    fn health_follows_the_worker_threads_and_not_the_gauge() {
        // Readiness and the container health check both go through this, and
        // before it existed the trait's default answered `true` for every
        // pool in every state — including one whose threads had all ended,
        // which answers nothing and, with the monitor gone with the runtime,
        // never gets a replacement.
        let executor = test_executor(4, 0);
        assert!(
            !executor.is_healthy(),
            "a pool with no threads at all serves nothing"
        );

        executor
            .workers
            .lock()
            .unwrap()
            .push(managed_worker(0, false));
        assert!(
            !executor.is_healthy(),
            "a thread that has ended is not a worker"
        );

        executor
            .workers
            .lock()
            .unwrap()
            .push(managed_worker(1, true));
        assert!(
            executor.is_healthy(),
            "one live thread beside a dead one is a pool that can still serve"
        );

        for worker in executor.workers.lock().unwrap().iter() {
            worker.shutdown.store(true, Ordering::Relaxed);
        }
        for worker in executor.workers.lock().unwrap().drain(..) {
            let _ = worker.handle.join();
        }
        forget_executor(executor);
    }

    #[test]
    fn health_answers_through_a_poisoned_pool_lock() {
        // The probe is served by the internal listener, whose whole purpose is
        // to answer when the rest of the server cannot. Unwrapping the lock
        // would turn a panic that happened somewhere else into a connection
        // error on the probe, and an orchestrator would read that as a failed
        // check for a reason that has nothing to do with the pool.
        let executor = test_executor(4, 0);
        executor
            .workers
            .lock()
            .unwrap()
            .push(managed_worker(0, true));

        let workers = Arc::clone(&executor.workers);
        let _ = std::thread::spawn(move || {
            let _guard = workers.lock().unwrap();
            panic!("poisoning the pool lock on purpose");
        })
        .join();
        assert!(
            executor.workers.is_poisoned(),
            "the lock has to be poisoned"
        );

        assert!(
            executor.is_healthy(),
            "a running worker is still a running worker behind a poisoned lock"
        );

        let mut workers = match executor.workers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for worker in workers.drain(..) {
            worker.shutdown.store(true, Ordering::Relaxed);
            let _ = worker.handle.join();
        }
        drop(workers);
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
        }
    }
}
