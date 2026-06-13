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

    tracing::info!(worker = %thread_name, "Async worker thread started");

    // 3. Task loop — fiber-driven.
    //
    // Each received task is spawned into a scheduler fiber, which runs to its
    // first suspend (await / sleep / channel) or to completion. The driver
    // then ticks suspended fibers and drains any that completed, serialising
    // their result and releasing the fiber. Several tasks can be in-flight at
    // once when they suspend, so we track each pending result by fiber id.
    use std::collections::HashMap;
    type ResultTx = tokio::sync::oneshot::Sender<AsyncResult>;

    let mut in_flight: HashMap<i64, ResultTx> = HashMap::new();
    let mut tasks_executed: u64 = 0;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Block for new work only when idle; when fibers are in flight, poll
        // non-blocking so we keep driving them to completion.
        let maybe_task = if in_flight.is_empty() {
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

            'task: {
                // Cancelled before we even start?
                if task.cancelled.load(Ordering::Relaxed) {
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

                // Spawn the task into a scheduler fiber. The closure is
                // reconstructed and run to its first suspend or completion
                // inside the call; args / static_vars / op_array are consumed
                // there, so free them immediately afterwards.
                let fiber_id = unsafe {
                    ffi::oxphp_bridge_async_spawn(
                        task.op_array_buf as *const c_void,
                        local_static_vars as *mut c_void,
                        task.this_ptr,
                        task.argc,
                        local_args,
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

                in_flight.insert(fiber_id, task.result_tx);
            }
        }

        // Drive suspended fibers, then drain any that completed.
        if !in_flight.is_empty() {
            unsafe { ffi::oxphp_bridge_async_tick() };
            if drain_completed(&mut in_flight, &metrics, &mut tasks_executed) {
                progressed = true;
            }
        }

        // When fibers are suspended but nothing was ready and no new task
        // arrived, back off briefly to avoid a busy spin (timers are ms-grained).
        if !progressed && !in_flight.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(1));
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
    in_flight: &mut std::collections::HashMap<
        i64,
        tokio::sync::oneshot::Sender<crate::async_types::AsyncResult>,
    >,
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
        if let Some(tx) = in_flight.remove(&fiber_id) {
            let _ = tx.send(result);
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
