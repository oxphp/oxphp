use crate::async_types::AsyncTask;
use crate::metrics::Metrics;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Dedicated worker pool for async promise task execution.
/// Separate from the HTTP worker pool to prevent deadlocks.
pub struct AsyncWorkerPool {
    task_tx: crossbeam_channel::Sender<AsyncTask>,
    // Only read in #[cfg(feature = "php")] start() — suppress dead_code on non-php builds.
    #[allow(dead_code)]
    task_rx: crossbeam_channel::Receiver<AsyncTask>,
    workers: Vec<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    worker_count: usize,
    #[allow(dead_code)]
    metrics: Option<Arc<Metrics>>,
}

impl AsyncWorkerPool {
    /// Create a new async worker pool. Returns None if worker_count is 0 (disabled).
    pub fn new(
        worker_count: usize,
        queue_capacity: usize,
        metrics: Option<Arc<Metrics>>,
    ) -> Option<Self> {
        if worker_count == 0 {
            return None;
        }
        let capacity = if queue_capacity == 0 {
            worker_count * 64
        } else {
            queue_capacity
        };
        let (task_tx, task_rx) = crossbeam_channel::bounded(capacity);
        Some(Self {
            task_tx,
            task_rx,
            workers: Vec::with_capacity(worker_count),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker_count,
            metrics,
        })
    }

    /// Get a clone of the task sender for dispatching async tasks.
    pub fn task_sender(&self) -> crossbeam_channel::Sender<AsyncTask> {
        self.task_tx.clone()
    }

    /// Number of configured workers.
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// Start all async worker threads. Must be called AFTER php_module_startup().
    /// For now, workers are stub implementations (actual PHP task execution will be added later).
    #[cfg(feature = "php")]
    pub fn start(&mut self) {
        for id in 0..self.worker_count {
            let rx = self.task_rx.clone();
            let shutdown = self.shutdown.clone();
            let metrics = self.metrics.clone();

            let handle = std::thread::Builder::new()
                .name(format!("async-worker-{id}"))
                .spawn(move || {
                    async_worker_thread(id, rx, shutdown, metrics);
                })
                .unwrap_or_else(|e| panic!("Failed to spawn async worker {id}: {e}"));

            self.workers.push(handle);
        }
        tracing::info!(workers = self.worker_count, "Async worker pool started");
    }

    /// Signal all workers to shut down and join threads.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
        tracing::info!("Async worker pool shut down");
    }

    /// Check if the pool is shut down.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
}

impl Drop for AsyncWorkerPool {
    fn drop(&mut self) {
        if !self.workers.is_empty() {
            self.shutdown();
        }
    }
}

/// A task fiber currently in flight (suspended at an await/sleep or completing)
/// in an async worker. Tracked by fiber id so the driver can deliver the result
/// and propagate cancellation.
#[cfg(feature = "php")]
struct InFlight {
    result_tx: tokio::sync::oneshot::Sender<crate::async_types::AsyncResult>,
    /// Shared with the promise: its `cancelled` flag is flipped when the awaiter
    /// gives up (await timeout). The driver then asks the scheduler to unwind the
    /// suspended fiber instead of letting it run to completion unobserved. Held
    /// here so the allocation (and the fiber's borrowed pointer into its
    /// `cancelled` cell) stays alive until the fiber is released and drained.
    cancelled: Arc<crate::async_types::CancelShared>,
}

/// Returns the in-flight permit a dequeued task holds (reserved at dispatch) on
/// drop unless `committed`. The worker creates one per task; every early-exit
/// path from task handling gives the permit back, while the spawn-and-insert
/// path commits it so the `InFlight` entry owns it and `drain_completed`
/// releases it at completion. Mirrors the dispatch-side guard so the
/// process-global counter stays balanced across both halves of a task's life.
#[cfg(feature = "php")]
struct WorkerPermitGuard {
    committed: bool,
}

#[cfg(feature = "php")]
impl Drop for WorkerPermitGuard {
    fn drop(&mut self) {
        if !self.committed {
            crate::php::sapi::async_inflight_release();
        }
    }
}

/// Main loop for each async worker thread.
///
/// Each worker initialises TSRM, performs a single `php_request_startup()`,
/// then loops: recv task → reset state → execute closure → deep-copy result → send → free args.
/// On exit (shutdown or channel disconnect), calls `php_request_shutdown()`.
#[cfg(feature = "php")]
fn async_worker_thread(
    id: usize,
    rx: crossbeam_channel::Receiver<AsyncTask>,
    shutdown: Arc<AtomicBool>,
    metrics: Option<Arc<Metrics>>,
) {
    use crate::async_types::AsyncResult;
    use crate::bridge::ffi;
    use crate::php::{bindings, sapi};
    use std::ffi::c_void;

    let thread_name = std::thread::current()
        .name()
        .unwrap_or("async-worker")
        .to_string();

    // Mark this thread as async worker in Rust TLS
    sapi::set_is_async_worker(true);

    // 1. Initialize TSRM thread-local storage (required for ZTS)
    unsafe {
        let _ = bindings::ts_resource_ex(0, std::ptr::null_mut());
        bindings::oxphp_bridge_tsrm_update();
        bindings::oxphp_bridge_init_ctx();
        bindings::oxphp_bridge_set_worker_id(id as i32);
        ffi::oxphp_bridge_set_async_worker(1);
    }

    // 2. php_request_startup — a single long-lived request for the worker lifetime.
    //    Set minimal request info so PHP can initialize properly.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    unsafe {
        bindings::oxphp_bridge_set_request_time(now.as_secs_f64());
        bindings::oxphp_bridge_set_request_info(
            c"GET".as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
    }

    if unsafe { bindings::php_request_startup() } != 0 {
        tracing::error!(id, "php_request_startup() failed in async worker");
        return;
    }

    // Capture this worker thread's &EG(vm_interrupt) so a CPU-bound task fiber
    // can be interrupted cross-thread by an awaiter that times out (Path B).
    // The address is stable for the thread's lifetime once the request is up.
    let worker_interrupt_addr = unsafe {
        ffi::oxphp_capture_vm_interrupt();
        ffi::oxphp_bridge_vm_interrupt_addr() as usize
    };

    tracing::info!(worker = %thread_name, "Async worker thread started");

    // 3. Task loop — fiber-driven.
    //
    // Each received task is spawned into a scheduler fiber, which runs to its
    // first suspend (await / sleep / channel) or to completion. The driver
    // then ticks suspended fibers and drains any that completed, serialising
    // their result and releasing the fiber. Several tasks can be in-flight at
    // once when they suspend, so we track each pending result by fiber id.
    use std::collections::HashMap;

    let mut in_flight: HashMap<i64, InFlight> = HashMap::new();
    let mut tasks_executed: u64 = 0;
    let mut warned_output = false;
    // Set once shutdown is observed with fibers still in flight: the drain has
    // until this instant to finish before stragglers are abandoned.
    let mut shutdown_drain_deadline: Option<std::time::Instant> = None;

    // How long the driver waits when a turn of the loop moved nothing. It
    // starts short, so a fiber that another thread made ready is picked up
    // close to the moment it became ready, and doubles up to a ceiling, so a
    // fiber parked on a half-second timer does not cost a wakeup every
    // millisecond for the whole park. Any progress puts it back at the floor.
    const BACKOFF_MIN: std::time::Duration = std::time::Duration::from_micros(50);
    const BACKOFF_MAX: std::time::Duration = std::time::Duration::from_millis(10);
    // The ceiling is what the driver is prepared to wait before looking again
    // for something only another thread can tell it: a promise settled on
    // another worker, or an awaiter that has given up and wants its task
    // unwound. Neither announces itself, so both are found by looking, and the
    // ceiling is how late they can be found. Deadlines this thread set itself
    // are not in that class — a sleep timer, a per-call await timeout, a hooked
    // socket's read deadline — so the wait below is cut short at the earliest
    // of them and the ceiling never applies to any of them.
    //
    // The shutdown drain keeps a fixed interval: it is bounded by a deadline of
    // its own, and widening the gap between its cancel-and-tick rounds would
    // only spend that budget on waiting.
    const BACKOFF_SHUTDOWN: std::time::Duration = std::time::Duration::from_millis(1);
    let mut backoff = BACKOFF_MIN;

    // A task the idle wait below took off the queue, to be run on the next
    // turn of the loop.
    let mut carried: Option<AsyncTask> = None;

    loop {
        let shutting_down = shutdown.load(Ordering::Relaxed);

        // Exit only once every in-flight fiber is drained. On shutdown we stop
        // accepting new work but keep ticking and cancelling suspended fibers so
        // their PHP stacks unwind (finally / destructors run), awaiters receive a
        // result, and in-flight permits are released — instead of abandoning the
        // fibers. Bounded by a short deadline so a task that refuses to unwind
        // (e.g. heavy work in a finally block) can't hang shutdown forever.
        if shutting_down {
            // A task the idle wait carried over has already left the queue, so
            // it leaves through the drain like any other: spawned below and
            // unwound by the cancellation this block issues. Breaking with it
            // still in hand would drop its result channel under an awaiter that
            // is still waiting, and lose the in-flight permit it holds.
            if in_flight.is_empty() && carried.is_none() {
                break;
            }
            let deadline = *shutdown_drain_deadline.get_or_insert_with(|| {
                std::time::Instant::now() + std::time::Duration::from_secs(2)
            });
            if std::time::Instant::now() >= deadline {
                tracing::warn!(
                    worker = id,
                    abandoned = in_flight.len(),
                    "async shutdown drain timed out; abandoning in-flight fibers"
                );
                break;
            }
            // Cancel every still-running fiber so the scheduler unwinds it on the
            // next tick. Re-issued each iteration to catch a fiber that re-parks
            // (the resume clears its cancel flag).
            for &fiber_id in in_flight.keys() {
                unsafe { ffi::oxphp_bridge_async_cancel(fiber_id) };
            }
        }

        // Block for new work only when idle; when fibers are in flight, poll
        // non-blocking so we keep driving them to completion.
        let maybe_task = if let Some(task) = carried.take() {
            // Already off the queue, so it is this worker's to run even if
            // shutdown was signalled in between: dropping it here would close
            // its result channel and answer an awaiter that is still waiting.
            Some(task)
        } else if shutting_down {
            None
        } else if in_flight.is_empty() {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(t) => Some(t),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.try_recv() {
                Ok(t) => Some(t),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                // No new tasks will arrive, but keep draining in-flight fibers.
                Err(crossbeam_channel::TryRecvError::Disconnected) => None,
            }
        };

        let mut progressed = false;

        if let Some(task) = maybe_task {
            progressed = true;
            let zval_size = unsafe { ffi::oxphp_zval_size() };

            // The dequeued task holds one in-flight permit (reserved at
            // dispatch). Return it on any early exit from the block below; on
            // the spawn-and-insert path we commit so the InFlight entry owns it
            // and drain_completed releases it once the task completes.
            let mut task_permit = WorkerPermitGuard { committed: false };

            'task: {
                // Cancelled before we even start? (Acquire pairs with the
                // awaiter's Release store of the flag.)
                if task.cancelled.cancelled.load(Ordering::Acquire) {
                    free_task_args(&task);
                    free_op_array_buf(&task);
                    let _ = task.result_tx.send(AsyncResult {
                        success: false,
                        serialized_value: std::ptr::null_mut(),
                        serialized_value_len: 0,
                        exception_class: Some("OxPHP\\Async\\AsyncException".into()),
                        exception_message: Some("Task cancelled before execution".into()),
                        keepalive: None,
                    });
                    if let Some(ref m) = metrics {
                        m.async_task_cancelled();
                    }
                    break 'task;
                }

                // Reset PHP state before a task only when the worker is idle —
                // doing it while other fibers are suspended would clobber their
                // output buffers / error state. Concurrent tasks share the
                // worker's superglobals, as in the previous synchronous model.
                if in_flight.is_empty() {
                    unsafe { ffi::oxphp_async_reset() };
                }

                // Deserialize args on THIS thread's heap (correct emalloc)
                let local_args = if task.argc > 0 && !task.serialized_args.is_null() {
                    let layout =
                        std::alloc::Layout::from_size_align(zval_size * task.argc as usize, 8)
                            .expect("invalid layout for args");
                    let buf = unsafe { std::alloc::alloc_zeroed(layout) };
                    if buf.is_null() {
                        free_task_args(&task);
                        let _ = task.result_tx.send(AsyncResult {
                            success: false,
                            serialized_value: std::ptr::null_mut(),
                            serialized_value_len: 0,
                            exception_class: Some("RuntimeException".into()),
                            exception_message: Some("Failed to allocate args buffer".into()),
                            keepalive: None,
                        });
                        break 'task;
                    }
                    let rc = unsafe {
                        ffi::oxphp_portable_deserialize(
                            task.serialized_args,
                            task.serialized_args_len,
                            task.argc,
                            buf as *mut c_void,
                        )
                    };
                    if rc != 0 {
                        unsafe { std::alloc::dealloc(buf, layout) };
                        free_task_args(&task);
                        let _ = task.result_tx.send(AsyncResult {
                            success: false,
                            serialized_value: std::ptr::null_mut(),
                            serialized_value_len: 0,
                            exception_class: Some("RuntimeException".into()),
                            exception_message: Some("Failed to deserialize arguments".into()),
                            keepalive: None,
                        });
                        break 'task;
                    }
                    // Free the serialized buffer now that we've deserialized
                    unsafe { ffi::oxphp_portable_free(task.serialized_args) };
                    buf as *mut c_void
                } else {
                    // Free empty serialized buffer if present
                    if !task.serialized_args.is_null() {
                        unsafe { ffi::oxphp_portable_free(task.serialized_args) };
                    }
                    std::ptr::null_mut()
                };

                // Deserialize static_vars (closure use-vars) on THIS thread's heap
                let local_static_vars = if !task.serialized_static_vars.is_null()
                    && task.serialized_static_vars_len > 0
                {
                    let mut ht_ptr: *mut c_void = std::ptr::null_mut();
                    let rc = unsafe {
                        ffi::oxphp_portable_deserialize_ht(
                            task.serialized_static_vars,
                            task.serialized_static_vars_len,
                            &mut ht_ptr,
                        )
                    };
                    unsafe { ffi::oxphp_portable_free(task.serialized_static_vars) };
                    if rc != 0 || ht_ptr.is_null() {
                        free_local_args(local_args, task.argc, zval_size);
                        let _ = task.result_tx.send(AsyncResult {
                            success: false,
                            serialized_value: std::ptr::null_mut(),
                            serialized_value_len: 0,
                            exception_class: Some("RuntimeException".into()),
                            exception_message: Some("Failed to deserialize static vars".into()),
                            keepalive: None,
                        });
                        break 'task;
                    }
                    ht_ptr
                } else {
                    if !task.serialized_static_vars.is_null() {
                        unsafe { ffi::oxphp_portable_free(task.serialized_static_vars) };
                    }
                    std::ptr::null_mut()
                };

                // Publish this worker's interrupt address into the shared cancel
                // state so a timed-out awaiter can break into this fiber if it
                // goes CPU-bound (Path B). `cancel_cell` is a stable pointer into
                // that same allocation — the scheduler stores it on the fiber and
                // the interrupt handler reads it to decide whether to unwind. The
                // allocation outlives the fiber via InFlight (inserted below).
                // Release: publishes the address to the awaiter's Acquire load
                // in kick_worker_interrupt, so a timed-out awaiter never reads a
                // stale 0 and skips the kick on weakly-ordered hardware.
                task.cancelled
                    .worker_interrupt
                    .store(worker_interrupt_addr, Ordering::Release);
                let cancel_cell = &task.cancelled.cancelled as *const std::sync::atomic::AtomicBool
                    as *mut c_void;

                // Spawn the task into a scheduler fiber. The closure is
                // reconstructed and run to its first suspend or completion
                // inside the call; args / static_vars / op_array are consumed
                // there, so free them immediately afterwards.
                let fiber_id = unsafe {
                    ffi::oxphp_bridge_async_spawn(
                        task.op_array_buf as *const c_void,
                        local_static_vars,
                        task.this_ptr,
                        task.argc,
                        local_args,
                        cancel_cell,
                    )
                };

                free_local_args(local_args, task.argc, zval_size);
                if !local_static_vars.is_null() {
                    unsafe { ffi::oxphp_portable_free_ht(local_static_vars) };
                }
                free_op_array_buf(&task);

                if fiber_id < 0 {
                    // At per-worker fiber capacity (or allocation failure).
                    let _ = task.result_tx.send(AsyncResult {
                        success: false,
                        serialized_value: std::ptr::null_mut(),
                        serialized_value_len: 0,
                        exception_class: Some("OxPHP\\Async\\AsyncException".into()),
                        exception_message: Some("Async worker at fiber capacity".into()),
                        keepalive: None,
                    });
                    if let Some(ref m) = metrics {
                        m.async_task_failed();
                    }
                    break 'task;
                }

                in_flight.insert(
                    fiber_id,
                    InFlight {
                        result_tx: task.result_tx,
                        cancelled: task.cancelled.clone(),
                    },
                );
                // The InFlight entry now owns the permit; drain releases it.
                task_permit.committed = true;
            }
        }

        // Drive suspended fibers, then drain any that completed.
        let had_fibers = !in_flight.is_empty();
        if had_fibers {
            // Propagate cancellation: a promise whose awaiter has given up
            // (await timeout) flips its cancel flag. Ask the scheduler to
            // unwind the still-suspended fiber rather than run it to
            // completion with no one waiting for the result.
            for (&fiber_id, entry) in in_flight.iter() {
                // Acquire pairs with the awaiter's Release store of the flag.
                if entry.cancelled.cancelled.load(Ordering::Acquire) {
                    unsafe { ffi::oxphp_bridge_async_cancel(fiber_id) };
                }
            }

            // A tick that resumed someone is work done, not an empty turn:
            // without it the driver would follow a cross-thread wakeup with a
            // wait that has nothing left to wait for. `> 0` and not `!= 0`
            // because -1 is the no-scheduler-registered sentinel.
            if unsafe { ffi::oxphp_bridge_async_tick() } > 0 {
                progressed = true;
            }
            if drain_completed(&mut in_flight, &metrics, &mut tasks_executed) {
                progressed = true;
            }
        }

        // The worker just went idle: a background task may have left output in
        // the shared PHP output buffer (and the Rust RESPONSE buffer). No fiber
        // is running now, so it is safe to discard it and reclaim the memory.
        if had_fibers && in_flight.is_empty() {
            let discarded = unsafe { ffi::oxphp_bridge_async_drain_output() }
                + crate::php::sapi::clear_response_output() as u64;
            if discarded > 0 {
                if let Some(ref m) = metrics {
                    m.async_output_discarded(discarded);
                }
                if !warned_output {
                    warned_output = true;
                    tracing::warn!(
                        worker = id,
                        bytes = discarded,
                        "async task wrote output that has no client; discarded \
                         (remove echo/print from async task bodies)"
                    );
                }
            }
        }

        // When fibers are suspended but nothing was ready and no new task
        // arrived, back off to avoid a busy spin (timers are ms-grained).
        //
        // A fiber parked on a socket is different from one parked on a timer:
        // its wake-up time is chosen by the peer, so a blind sleep adds up to a
        // full interval of latency to every round trip. When the extension
        // reports parked descriptors it spends the same interval waiting on
        // them and returns non-zero, and this wait is skipped — the wait
        // already happened, and it ended the moment the peer replied.
        if !progressed && !in_flight.is_empty() {
            let wait = if shutting_down {
                BACKOFF_SHUTDOWN
            } else {
                // Never past a deadline this thread itself set. Two places hold
                // them: the timer registry, which is this thread's and answers
                // for `oxphp_sleep()`, and the fibers themselves, which carry a
                // per-call await timeout or a hooked socket's read/write
                // deadline. Both are read here so the wait ends when the
                // earliest of them does. Kept at the floor at minimum — a zero
                // wait would spin, and the extension rejects a zero budget.
                let mut wait = backoff;
                if let Some(deadline) = crate::php::fiber::next_timer_deadline() {
                    wait = wait.min(deadline.saturating_duration_since(std::time::Instant::now()));
                }
                let sched_ns = unsafe { ffi::oxphp_bridge_async_next_deadline_ns() };
                if sched_ns > 0 {
                    wait = wait.min(std::time::Duration::from_nanos(sched_ns));
                }
                wait.max(BACKOFF_MIN)
            };
            let waited_on_descriptors =
                unsafe { ffi::oxphp_bridge_async_io_backoff(wait.as_nanos() as u64) } != 0;
            if !waited_on_descriptors {
                if shutting_down {
                    // No new work is accepted during the drain, so there is
                    // nothing to wait on the queue for.
                    std::thread::sleep(wait);
                } else {
                    // Wait on the queue rather than sleeping blind: a worker
                    // whose fibers are all parked is still a worker a new task
                    // can be handed to, and with the ceiling above, a sleep
                    // would leave that task queued for the rest of the interval.
                    match rx.recv_timeout(wait) {
                        Ok(t) => carried = Some(t),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        // Every sender is gone, so recv_timeout returns at once
                        // and would turn this wait into the spin it exists to
                        // prevent — the in-flight fibers still need driving.
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            std::thread::sleep(wait)
                        }
                    }
                }
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        } else {
            backoff = BACKOFF_MIN;
        }
    }

    // 4. Shutdown
    unsafe { bindings::php_request_shutdown(std::ptr::null_mut()) };

    tracing::info!(
        worker = %thread_name,
        tasks_executed,
        "Async worker thread exiting"
    );
}

/// Drain every task fiber that has completed: serialize its result, send it to
/// the awaiting thread, and release the fiber. Returns true if at least one
/// fiber was drained.
#[cfg(feature = "php")]
fn drain_completed(
    in_flight: &mut std::collections::HashMap<i64, InFlight>,
    metrics: &Option<Arc<Metrics>>,
    tasks_executed: &mut u64,
) -> bool {
    use crate::async_types::AsyncResult;
    use crate::bridge::ffi;
    use std::ffi::c_void;
    use std::os::raw::c_char;

    let mut drained = false;
    loop {
        let mut retval_ptr: *mut c_void = std::ptr::null_mut();
        let mut exc_class: *const c_char = std::ptr::null();
        let mut exc_message: *const c_char = std::ptr::null();
        let fiber_id = unsafe {
            ffi::oxphp_bridge_async_poll_completed(
                &mut retval_ptr,
                &mut exc_class,
                &mut exc_message,
            )
        };
        if fiber_id < 0 {
            break;
        }

        let result = if !exc_class.is_null() {
            // Task threw — copy the (fiber-owned) exception strings; the
            // extension frees them on release.
            let class_str = unsafe { cstr_ptr_to_string(exc_class) };
            let message_str = unsafe { cstr_ptr_to_string(exc_message) };
            AsyncResult {
                success: false,
                serialized_value: std::ptr::null_mut(),
                serialized_value_len: 0,
                exception_class: class_str,
                exception_message: message_str,
                keepalive: None,
            }
        } else {
            // Success — portable-serialize the return value (a *zval into the
            // fiber's storage) for safe cross-thread transfer. Pin any nested
            // `Shared\*` entries the value references BEFORE release: the
            // serialized bytes carry only tag-7 ids and the retval zval is the
            // entries' last strong ref, so the resolve must run while release
            // has not yet dtor'd it. The keepalive rides the result and drops
            // only after the awaiting fiber deserializes it.
            let mut ser_buf: *mut u8 = std::ptr::null_mut();
            let mut ser_len: usize = 0;
            let ser_rc = unsafe {
                ffi::oxphp_portable_serialize(
                    retval_ptr as *const c_void,
                    1,
                    &mut ser_buf,
                    &mut ser_len,
                )
            };
            let keepalive: Option<Box<dyn std::any::Any + Send>> =
                if ser_rc == 0 && !ser_buf.is_null() {
                    #[cfg(feature = "plugin-shared")]
                    {
                        let bytes = unsafe { std::slice::from_raw_parts(ser_buf, ser_len) };
                        crate::plugins::ox_shared::value::resolve_transit_keepalive(bytes)
                    }
                    #[cfg(not(feature = "plugin-shared"))]
                    {
                        None
                    }
                } else {
                    None
                };
            if ser_rc != 0 || ser_buf.is_null() {
                AsyncResult {
                    success: true,
                    serialized_value: std::ptr::null_mut(),
                    serialized_value_len: 0,
                    exception_class: None,
                    exception_message: None,
                    keepalive: None,
                }
            } else {
                AsyncResult {
                    success: true,
                    serialized_value: ser_buf,
                    serialized_value_len: ser_len,
                    exception_class: None,
                    exception_message: None,
                    keepalive,
                }
            }
        };

        // Release the fiber AFTER serialising (release dtors task_retval and
        // the closure and recycles the fiber).
        unsafe { ffi::oxphp_bridge_async_release(fiber_id) };

        if let Some(ref m) = metrics {
            if result.success {
                m.async_task_completed();
            } else {
                m.async_task_failed();
            }
        }
        if let Some(entry) = in_flight.remove(&fiber_id) {
            let _ = entry.result_tx.send(result);
            // Task is done — return the in-flight permit it held since dispatch.
            crate::php::sapi::async_inflight_release();
        }
        *tasks_executed += 1;
        drained = true;
    }
    drained
}

/// Copy a NUL-terminated C string (owned by the extension) into an owned
/// String without freeing it — the extension frees it on fiber release.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated C string.
#[cfg(feature = "php")]
unsafe fn cstr_ptr_to_string(ptr: *const std::os::raw::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned())
}

/// Free the system-malloc'd op_array copy owned by an AsyncTask.
#[cfg(feature = "php")]
fn free_op_array_buf(task: &AsyncTask) {
    if !task.op_array_buf.is_null() {
        unsafe { libc::free(task.op_array_buf as *mut std::ffi::c_void) };
    }
}

/// Free the portable-serialized buffers owned by an AsyncTask.
#[cfg(feature = "php")]
fn free_task_args(task: &AsyncTask) {
    if !task.serialized_args.is_null() {
        unsafe { crate::bridge::ffi::oxphp_portable_free(task.serialized_args) };
    }
    if !task.serialized_static_vars.is_null() {
        unsafe { crate::bridge::ffi::oxphp_portable_free(task.serialized_static_vars) };
    }
}

/// Free deserialized (local) argument zvals that live on the current thread's heap.
#[cfg(feature = "php")]
fn free_local_args(args: *mut std::ffi::c_void, argc: u32, zval_size: usize) {
    if !args.is_null() && argc > 0 {
        for i in 0..argc {
            let p =
                unsafe { (args as *mut u8).add(i as usize * zval_size) as *mut std::ffi::c_void };
            unsafe { crate::bridge::ffi::oxphp_deep_free_zval(p) };
        }
        let layout = std::alloc::Layout::from_size_align(zval_size * argc as usize, 8)
            .expect("invalid layout");
        unsafe { std::alloc::dealloc(args as *mut u8, layout) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_disabled_when_zero_workers() {
        let pool = AsyncWorkerPool::new(0, 0, None);
        assert!(pool.is_none());
    }

    #[test]
    fn test_pool_created_with_workers() {
        let pool = AsyncWorkerPool::new(2, 128, None);
        assert!(pool.is_some());
        let pool = pool.unwrap();
        assert_eq!(pool.worker_count(), 2);
    }

    #[test]
    fn test_pool_task_sender_cloneable() {
        let pool = AsyncWorkerPool::new(1, 64, None).unwrap();
        let tx1 = pool.task_sender();
        let tx2 = pool.task_sender();
        drop(tx1);
        drop(tx2);
    }

    #[test]
    fn test_pool_default_capacity() {
        let pool = AsyncWorkerPool::new(2, 0, None);
        assert!(pool.is_some());
    }
}
