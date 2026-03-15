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
    use std::os::raw::c_char;

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
            b"GET\0".as_ptr() as *const c_char,
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

    // 3. Task loop
    let mut tasks_executed: u64 = 0;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let task = match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(t) => t,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        // Check if the task was cancelled before we even start
        if task.cancelled.load(Ordering::Relaxed) {
            free_task_args(&task);
            let _ = task.result_tx.send(AsyncResult {
                success: false,
                serialized_value: std::ptr::null_mut(),
                serialized_value_len: 0,
                exception_class: Some("OxPHP\\AsyncException".into()),
                exception_message: Some("Task cancelled before execution".into()),
                exception_trace: None,
            });
            if let Some(ref m) = metrics {
                m.async_task_cancelled();
            }
            continue;
        }

        // Reset PHP state between tasks (clear errors, output buffers, etc.)
        unsafe { ffi::oxphp_async_reset() };

        // Deserialize args on THIS thread's heap (correct emalloc)
        let zval_size = unsafe { ffi::oxphp_zval_size() };
        let local_args = if task.argc > 0 && !task.serialized_args.is_null() {
            let layout = std::alloc::Layout::from_size_align(zval_size * task.argc as usize, 8)
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
                    exception_trace: None,
                });
                continue;
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
                    exception_trace: None,
                });
                continue;
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
        let local_static_vars =
            if !task.serialized_static_vars.is_null() && task.serialized_static_vars_len > 0 {
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
                        exception_trace: None,
                    });
                    continue;
                }
                ht_ptr
            } else {
                if !task.serialized_static_vars.is_null() {
                    unsafe { ffi::oxphp_portable_free(task.serialized_static_vars) };
                }
                std::ptr::null_mut()
            };

        // Execute the closure via the C bridge (zend_try protected)
        let mut exc_class: *mut c_char = std::ptr::null_mut();
        let mut exc_message: *mut c_char = std::ptr::null_mut();
        let mut exc_trace: *mut c_char = std::ptr::null_mut();

        // Allocate a zval for the return value
        let retval_layout =
            std::alloc::Layout::from_size_align(zval_size, 8).expect("invalid zval layout");
        let retval_buf = unsafe { std::alloc::alloc_zeroed(retval_layout) };
        if retval_buf.is_null() {
            // Free local args (on this thread's heap — safe)
            free_local_args(local_args, task.argc, zval_size);
            let _ = task.result_tx.send(AsyncResult {
                success: false,
                serialized_value: std::ptr::null_mut(),
                serialized_value_len: 0,
                exception_class: Some("RuntimeException".into()),
                exception_message: Some("Failed to allocate return value buffer".into()),
                exception_trace: None,
            });
            continue;
        }

        let rc = unsafe {
            ffi::oxphp_execute_async_task(
                task.op_array,
                local_static_vars as *const c_void,
                task.this_ptr,
                task.argc,
                local_args,
                retval_buf as *mut c_void,
                &mut exc_class,
                &mut exc_message,
                &mut exc_trace,
            )
        };

        let result = if rc == 0 {
            // Success — portable-serialize the retval for safe cross-thread transfer
            let mut ser_buf: *mut u8 = std::ptr::null_mut();
            let mut ser_len: usize = 0;
            let ser_rc = unsafe {
                ffi::oxphp_portable_serialize(
                    retval_buf as *const c_void,
                    1, // single return value
                    &mut ser_buf,
                    &mut ser_len,
                )
            };
            // Free the original retval contents (on this thread's heap — safe)
            unsafe { ffi::oxphp_deep_free_zval(retval_buf as *mut c_void) };
            // Free the Rust-allocated retval container
            unsafe { std::alloc::dealloc(retval_buf, retval_layout) };

            if ser_rc != 0 || ser_buf.is_null() {
                AsyncResult {
                    success: true,
                    serialized_value: std::ptr::null_mut(),
                    serialized_value_len: 0,
                    exception_class: None,
                    exception_message: None,
                    exception_trace: None,
                }
            } else {
                AsyncResult {
                    success: true,
                    serialized_value: ser_buf,
                    serialized_value_len: ser_len,
                    exception_class: None,
                    exception_message: None,
                    exception_trace: None,
                }
            }
        } else {
            // Failure — extract exception details from C-allocated strings
            let class_str = unsafe { cstr_to_string_free(exc_class) };
            let message_str = unsafe { cstr_to_string_free(exc_message) };
            let trace_str = unsafe { cstr_to_string_free(exc_trace) };

            // Free the retval contents (may have been partially initialized)
            unsafe { ffi::oxphp_deep_free_zval(retval_buf as *mut c_void) };
            // Free the Rust-allocated retval container
            unsafe { std::alloc::dealloc(retval_buf, retval_layout) };

            AsyncResult {
                success: false,
                serialized_value: std::ptr::null_mut(),
                serialized_value_len: 0,
                exception_class: class_str,
                exception_message: message_str,
                exception_trace: trace_str,
            }
        };

        // Free local deserialized data (on this thread's heap — safe)
        free_local_args(local_args, task.argc, zval_size);
        if !local_static_vars.is_null() {
            unsafe { ffi::oxphp_portable_free_ht(local_static_vars) };
        }

        // Track metrics
        if let Some(ref m) = metrics {
            if result.success {
                m.async_task_completed();
            } else {
                m.async_task_failed();
            }
        }

        // Send result to awaiting thread
        let _ = task.result_tx.send(result);

        tasks_executed += 1;
    }

    // 4. Shutdown
    unsafe { bindings::php_request_shutdown(std::ptr::null_mut()) };

    tracing::info!(
        worker = %thread_name,
        tasks_executed,
        "Async worker thread exiting"
    );
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

/// Convert a C `strdup`'d string to a Rust `Option<String>`, freeing the C allocation.
///
/// # Safety
/// `ptr` must be null or a valid C string allocated with malloc/strdup.
#[cfg(feature = "php")]
unsafe fn cstr_to_string_free(ptr: *mut std::os::raw::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
    libc::free(ptr as *mut std::ffi::c_void);
    Some(s)
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
