//! Traditional per-request PHP worker: each request does full
//! `php_request_startup` / `php_request_shutdown`. Single `execute_request`
//! per channel message, driven by `worker_thread`.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use crossbeam_channel::RecvTimeoutError;

use crate::php::{bindings, sapi};
use crate::types::{ScriptRequest, ScriptResponse};

use super::pool::WorkerRequest;

/// Controls whether the worker loop uses blocking `recv()` or `recv_timeout()`.
/// Static mode workers sleep via futex with zero CPU cost; dynamic mode workers
/// must wake periodically to check their per-worker shutdown flag.
#[derive(Clone, Copy)]
pub(super) enum WorkerLoopMode {
    /// Blocking recv — exits only when channel closes (sender dropped).
    Static,
    /// Timeout-based recv — checks `shutdown` flag between timeouts.
    Dynamic,
}

pub(super) fn spawn_worker(
    id: usize,
    rx: crossbeam_channel::Receiver<WorkerRequest>,
    shutdown: Arc<AtomicBool>,
    last_active: Arc<AtomicU64>,
    loop_mode: WorkerLoopMode,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("php-worker-{id}"))
        .spawn(move || {
            worker_thread(id, rx, shutdown, last_active, loop_mode);
        })
        .expect("failed to spawn PHP worker thread")
}

fn worker_thread(
    worker_id: usize,
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
        // Update the bridge library's TSRM cache so SG()/CG()/EG() macros work
        // from liboxphp_bridge.so on this thread (each .so has its own _tsrm_ls_cache).
        bindings::oxphp_bridge_tsrm_update();
        // Initialize bridge TLS context and set the worker ID (once per thread).
        bindings::oxphp_bridge_init_ctx();
        bindings::oxphp_bridge_set_worker_id(worker_id as i32);
    }

    tracing::info!(worker = %thread_name, "PHP worker thread started");

    match loop_mode {
        WorkerLoopMode::Static => {
            // Blocking recv — zero CPU while idle, exits when channel closes.
            while let Ok(wr) = request_rx.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_request(&wr.script, wr.response_tx)
                }));
                match result {
                    Ok(Some(response)) => {
                        // Response not sent early — recover the sender from TLS.
                        if let Some((_start, tx)) = sapi::take_early_tx() {
                            let _ = tx.send(response);
                        }
                    }
                    Ok(None) => {
                        // Early send already happened via oxphp_finish_request().
                    }
                    Err(_) => {
                        tracing::error!(worker = %thread_name, "Worker panicked, exiting for respawn");
                        // Try to recover the sender from TLS for 500 response.
                        if let Some((_start, tx)) = sapi::take_early_tx() {
                            let _ = tx.send(ScriptResponse {
                                status: 500,
                                body: Bytes::from_static(b"Internal Server Error"),
                                ..Default::default()
                            });
                        }
                        // If tx was already consumed by early send, response was already sent.
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
                        last_active.store(super::pool::now_millis(), Ordering::Relaxed);
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            execute_request(&wr.script, wr.response_tx)
                        }));
                        match result {
                            Ok(Some(response)) => {
                                if let Some((_start, tx)) = sapi::take_early_tx() {
                                    let _ = tx.send(response);
                                }
                            }
                            Ok(None) => {
                                // Early send already happened.
                            }
                            Err(_) => {
                                tracing::error!(worker = %thread_name, "Worker panicked, exiting for respawn");
                                if let Some((_start, tx)) = sapi::take_early_tx() {
                                    let _ = tx.send(ScriptResponse {
                                        status: 500,
                                        body: Bytes::from_static(b"Internal Server Error"),
                                        ..Default::default()
                                    });
                                }
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

/// RAII guard that clears SAPI request data on drop (even on panic).
struct RequestDataGuard;

impl Drop for RequestDataGuard {
    fn drop(&mut self) {
        sapi::clear_request_data();
    }
}

/// Execute a PHP script. If `oxphp_finish_request()` was called during execution,
/// the response is sent early via the `response_tx` oneshot and `None` is returned.
/// Otherwise, the full `ScriptResponse` is returned for the caller to send.
fn execute_request(
    request: &ScriptRequest,
    response_tx: tokio::sync::oneshot::Sender<ScriptResponse>,
) -> Option<ScriptResponse> {
    let start = Instant::now();

    sapi::clear_buffers();
    sapi::set_request_data(request);
    sapi::set_early_tx(start, response_tx);

    // Reset APM span stack on the PHP worker thread with trace context from the request.
    // This MUST happen on the worker thread (not Tokio) because SPAN_STACK is thread-local.
    #[cfg(feature = "plugin-apm")]
    crate::plugins::ox_apm::spans::SPAN_STACK.with(|s| {
        s.borrow_mut()
            .reset(request.trace_id.clone(), request.span_id.clone());
    });
    #[cfg(feature = "plugin-apm")]
    crate::plugins::ox_apm::connection_meta::clear();

    // Streaming channel is created lazily in send_streaming_headers() — no alloc
    // for the vast majority of non-streaming requests.

    let _guard = RequestDataGuard;

    // Set request_time BEFORE php_request_startup() — OPcache reads it during RINIT.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    unsafe {
        bindings::oxphp_bridge_set_request_time(now.as_secs_f64());
    }

    // Set execution deadline for the cooperative watchdog.
    if request.timeout_us > 0 {
        let now_us = now.as_micros() as i64;
        let deadline = now_us.saturating_add(request.timeout_us.min(i64::MAX as u64) as i64);
        unsafe {
            bindings::oxphp_bridge_set_deadline(deadline);
        }
    }

    if unsafe { bindings::php_request_startup() } != 0 {
        return Some(ScriptResponse {
            status: 500,
            body: Bytes::from_static(b"php_request_startup() failed"),
            ..Default::default()
        });
    }

    let script_path_str = request.script_path.to_str().unwrap_or("");
    let script_path = CString::new(script_path_str).unwrap_or_default();

    let mut file_handle: bindings::zend_file_handle = unsafe { std::mem::zeroed() };
    unsafe {
        bindings::zend_stream_init_filename(&mut file_handle, script_path.as_ptr());
    }

    file_handle.primary_script = true;

    let script_ok = unsafe {
        bindings::oxphp_execute_script_safe(&mut file_handle as *mut _ as *mut std::os::raw::c_void)
    };
    if script_ok == 0 {
        tracing::warn!(path = %request.script_path.display(), "PHP script aborted via zend_bailout");
        // Force HTTP 500 for fatal errors — the error callback may have already
        // set this, but if not (e.g. syntax error before callback runs), ensure it.
        sapi::set_fatal_error_status_if_default();
    }

    // Collect the HTTP response code from PHP's SG(sapi_headers) via the C bridge.
    // Must happen before php_request_shutdown() clears PHP state.
    sapi::collect_response_code();

    unsafe {
        bindings::zend_destroy_file_handle(&mut file_handle);
    }

    unsafe {
        bindings::php_request_shutdown(std::ptr::null_mut());
    }

    // If the response was already sent early (finish_request or streaming), we're done.
    // Clear buffers to drop STREAM_TX (closes channel → ends stream on client side).
    if sapi::was_early_sent() {
        sapi::clear_buffers();
        return None;
    }

    // Single batched TLS lookup for all response data.
    let (raw_output, raw_headers, status) = sapi::take_response();
    let body = Bytes::from(raw_output);

    // Parse header strings into typed HeaderName/HeaderValue on the worker thread,
    // so the single-threaded Tokio runtime doesn't pay the parsing cost.
    let headers = sapi::parse_raw_headers(raw_headers);

    #[cfg(feature = "plugin-apm")]
    let apm_spans_json = crate::plugins::ox_apm::spans::drain_and_serialize();
    #[cfg(not(feature = "plugin-apm"))]
    let apm_spans_json: Option<String> = None;

    Some(ScriptResponse {
        status,
        headers,
        body,
        execution_time_us: start.elapsed().as_micros() as u64,
        stream_rx: None,
        errors: sapi::take_request_errors(),
        apm_spans_json,
    })
}
