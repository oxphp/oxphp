use std::ffi::CString;
use std::time::Instant;

use bytes::Bytes;
use crossbeam_channel::{self, TrySendError};
use http::{HeaderName, HeaderValue};

use crate::executor::ScriptExecutor;
use crate::php::{bindings, sapi};
use crate::types::{ScriptRequest, ScriptResponse};

struct WorkerRequest {
    script: ScriptRequest,
    response_tx: tokio::sync::oneshot::Sender<ScriptResponse>,
}

pub struct SapiExecutor {
    request_tx: Option<crossbeam_channel::Sender<WorkerRequest>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl SapiExecutor {
    pub fn new() -> Self {
        let worker_count = std::env::var("PHP_WORKERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
            });

        // 1. TSRM must be initialized first for ZTS builds
        if !unsafe { bindings::php_tsrm_startup() } {
            panic!("php_tsrm_startup() failed");
        }

        // 2. Build and register our SAPI module
        let mut module = sapi::build_sapi_module();

        unsafe {
            bindings::sapi_startup(&mut module);
        }

        // sapi_startup() sets ini_entries = NULL — restore on both local and global
        // so php_module_startup() sees them regardless of whether it re-copies module
        unsafe {
            sapi::restore_ini_entries_on(&mut module);
        }

        // 3. Start the PHP engine (PHP 8.4: 2 arguments)
        let startup_result =
            unsafe { bindings::php_module_startup(&mut module, std::ptr::null_mut()) };
        if startup_result != 0 {
            panic!("php_module_startup() failed with code {startup_result}");
        }

        // 4. Override zend_write to route PHP output directly to our ub_write callback.
        // PHP's default php_output_write() buffers output internally and doesn't reliably
        // flush through sapi_module.ub_write on ZTS Alpine builds.
        unsafe {
            bindings::zend_write = sapi::oxphp_ub_write_export();
        }

        // 5. Override zend_error_cb to capture error messages in our output buffer.
        // PHP's error display path (php_error_cb → php_printf → php_output_write) uses
        // the same broken output layer. Our callback writes errors directly to the buffer.
        unsafe {
            sapi::install_error_cb();
        }

        let queue_capacity = std::env::var("QUEUE_CAPACITY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(worker_count * 128);

        let (request_tx, request_rx) = crossbeam_channel::bounded(queue_capacity);

        let mut workers = Vec::with_capacity(worker_count);
        for i in 0..worker_count {
            let rx = request_rx.clone();

            let handle = std::thread::Builder::new()
                .name(format!("php-worker-{i}"))
                .spawn(move || {
                    worker_thread(rx);
                })
                .expect("failed to spawn PHP worker thread");

            workers.push(handle);
        }

        tracing::info!(
            workers = worker_count,
            queue_capacity,
            "PHP worker pool started"
        );

        Self {
            request_tx: Some(request_tx),
            workers,
        }
    }
}

impl ScriptExecutor for SapiExecutor {
    fn execute(&self, request: ScriptRequest) -> tokio::sync::oneshot::Receiver<ScriptResponse> {
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
            let (error_tx, error_rx) = tokio::sync::oneshot::channel();
            let mut headers = Vec::new();
            if status == 503 {
                headers.push((
                    HeaderName::from_static("retry-after"),
                    HeaderValue::from_static("1"),
                ));
            }
            let _ = error_tx.send(ScriptResponse {
                status,
                headers,
                body,
                ..Default::default()
            });
            return error_rx;
        }

        response_rx
    }

    fn shutdown(&self) {
        // No-op: cleanup handled in Drop
    }
}

impl Drop for SapiExecutor {
    fn drop(&mut self) {
        // 1. Drop sender to close channel — workers will exit their recv loop
        self.request_tx.take();

        // 2. Join all worker threads
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }

        // 3. PHP shutdown after all workers are done
        unsafe {
            bindings::php_module_shutdown();
            bindings::sapi_shutdown();
            bindings::tsrm_shutdown();
        }
    }
}

fn worker_thread(request_rx: crossbeam_channel::Receiver<WorkerRequest>) {
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("php-worker")
        .to_string();

    // Initialize TSRM thread-local storage for this worker thread (required for ZTS)
    unsafe {
        let _ = bindings::ts_resource_ex(0, std::ptr::null_mut());
    }

    tracing::info!(worker = %thread_name, "PHP worker thread started");

    while let Ok(wr) = request_rx.recv() {
        let response = execute_request(&wr.script);
        let _ = wr.response_tx.send(response);
    }

    tracing::info!(worker = %thread_name, "PHP worker thread stopped");
}

/// RAII guard that clears SAPI request data on drop (even on panic).
struct RequestDataGuard;

impl Drop for RequestDataGuard {
    fn drop(&mut self) {
        sapi::clear_request_data();
    }
}

fn execute_request(request: &ScriptRequest) -> ScriptResponse {
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
