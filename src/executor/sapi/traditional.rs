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

    // Register this thread as a live Shared\Pool worker so cross-thread
    // releases routed to our ThreadKey park (rather than destroy-inline
    // under the dead-owner branch). See src/plugins/ox_shared/worker_liveness.rs
    // for the rationale — pthread_kill(tid, 0) is unsafe under pthread_t
    // reuse, so Pool uses an explicit registry instead.
    register_as_live_pool_worker();
    // Allocate the per-worker idle-eviction flag; the central scheduler
    // (src/plugins/ox_shared/eviction.rs) sets it when our idle deques
    // have slots past their idle_timeout.
    register_pool_evict_flag();

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

    // Unregister from the Shared\Pool liveness set before the thread
    // actually exits — any subsequent cross-thread release targeted
    // at our ThreadKey then correctly takes the dead-owner destroy
    // path. Runs on both the normal channel-closed tail and the
    // panic-break branch above.
    unregister_as_live_pool_worker();
    unregister_pool_evict_flag();

    tracing::info!(worker = %thread_name, "PHP worker thread stopped");
}

// ── Shared\Pool liveness hooks ───────────────────────────────────────
// One-line shims so the worker loop doesn't need its own cfg guards.
// When the `plugin-shared` feature is off, both calls compile away.

#[cfg(feature = "plugin-shared")]
#[inline]
fn register_as_live_pool_worker() {
    crate::plugins::ox_shared::worker_liveness::register_worker();
}

#[cfg(not(feature = "plugin-shared"))]
#[inline]
fn register_as_live_pool_worker() {}

#[cfg(feature = "plugin-shared")]
#[inline]
fn unregister_as_live_pool_worker() {
    crate::plugins::ox_shared::worker_liveness::unregister_worker();
}

#[cfg(not(feature = "plugin-shared"))]
#[inline]
fn unregister_as_live_pool_worker() {}

#[cfg(feature = "plugin-shared")]
#[inline]
fn register_pool_evict_flag() {
    crate::plugins::ox_shared::eviction::register(
        crate::plugins::ox_shared::types::pool::current_thread_key(),
    );
}

#[cfg(not(feature = "plugin-shared"))]
#[inline]
fn register_pool_evict_flag() {}

#[cfg(feature = "plugin-shared")]
#[inline]
fn unregister_pool_evict_flag() {
    crate::plugins::ox_shared::eviction::unregister(
        crate::plugins::ox_shared::types::pool::current_thread_key(),
    );
}

#[cfg(not(feature = "plugin-shared"))]
#[inline]
fn unregister_pool_evict_flag() {}

/// Request-frame hook: if the central eviction scheduler raised our
/// flag, drain this thread's stale slots across every Pool. Called
/// from `execute_request` after `php_request_startup` so PHP context
/// is live for `$destroy` invocations.
#[cfg(feature = "plugin-shared")]
#[inline]
fn drain_pool_stale_if_requested() {
    if crate::plugins::ox_shared::eviction::take_evict_request() {
        crate::plugins::ox_shared::eviction::drain_stale_for_current_thread();
    }
}

#[cfg(not(feature = "plugin-shared"))]
#[inline]
fn drain_pool_stale_if_requested() {}

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
    // Bridge ctx is `__thread` and persists across requests on the same PHP
    // worker thread. Without this, stream_mode/headers_sent/finished from a
    // prior streaming request leak into the next, making `oxphp_flush()` skip
    // `send_streaming_headers()` and silently drop the stream channel —
    // observable as the chunked Transfer-Encoding disappearing after the
    // first oxphp_stream_flush call on a worker. Worker-mode does this in
    // `setup_request_tls`; traditional needs the same guarantee.
    unsafe { bindings::oxphp_bridge_reset_request_ctx() };
    sapi::set_request_data(request);
    sapi::set_early_tx(start, response_tx);

    // Capture whether profiling is active for this request. Used both to
    // guard the RINIT setup below and to guard the RSHUTDOWN finalize/attach
    // block so we skip 3 FFI calls + an Arc allocation on the hot path.
    #[cfg(any(feature = "plugin-apm", feature = "plugin-profiler"))]
    let profiling_active = request.profiling_mode != crate::profiling::ProfilingMode::Off;

    // Reset profiling context on the PHP worker thread with the mode selected
    // by the trigger (ox_profiler) or the default (ApmOnly when plugin-apm is
    // on, Off otherwise — both set by dispatch_request). This MUST happen on
    // the worker thread (not Tokio) because PROFILING_CONTEXT is thread-local.
    #[cfg(any(feature = "plugin-apm", feature = "plugin-profiler"))]
    if profiling_active {
        crate::profiling::PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                request.profiling_mode,
                request.trace_id.clone(),
                request.span_id.clone(),
            );
        });
    }
    // Tell the C-side profiler observer which mode to use for this
    // request. Must happen before php_request_startup() so the
    // observer init callback and the first begin() see the right
    // mode. APM does not use this flag (its hooks have their own
    // state), so the gate is plugin-profiler only.
    #[cfg(feature = "plugin-profiler")]
    if profiling_active {
        crate::profiling::set_profiling_mode(request.profiling_mode);
    }
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

    // Shared\Pool idle-timeout eviction check. Runs here — after PHP
    // request startup, before the user script — because `$destroy`
    // needs a live `EG(current_execute_data)` frame to execute
    // bytecode. Compiles away when `plugin-shared` is off.
    drain_pool_stale_if_requested();

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

    // Finalize the profiling context if any mode-aware plugin is compiled in —
    // mirrors the reset() guard above. Without this, `plugin-profiler` alone
    // would reach `reset(ProfileAll, …)` at RINIT but never call `finalize()`,
    // so `ProfilerCompleteHandler` would always see `profile_tree: None`.
    //
    // If profiling was Off at RINIT we skip the flush/finalize trio entirely,
    // UNLESS the PHP SDK (`OxPHP\Profile\start()`) promoted the bridge mode
    // mid-request — in which case `get_profiling_mode()` reports non-Off and
    // we must still flush so spans the C observer captured make it out.
    #[cfg(any(feature = "plugin-apm", feature = "plugin-profiler"))]
    let profile_tree = {
        #[cfg(feature = "plugin-profiler")]
        let do_finalize = profiling_active
            || crate::profiling::get_profiling_mode()
                != crate::profiling::flush::PROFILING_MODE_OFF_RAW;
        #[cfg(not(feature = "plugin-profiler"))]
        let do_finalize = profiling_active;

        if do_finalize {
            // Drain any partial ring buffer left by the C observer so all
            // events make it into PROFILING_CONTEXT before we finalize.
            // Idempotent — a second call when the buffer is empty is a
            // no-op.
            #[cfg(feature = "plugin-profiler")]
            crate::profiling::profiler_rshutdown_flush();

            let tree = crate::profiling::PROFILING_CONTEXT.with(|ctx| ctx.borrow_mut().finalize());

            // Reset the C-side mode so the next request on this worker
            // thread starts cleanly. Also clears the sticky was-active
            // flag and the open_stack mirror.
            #[cfg(feature = "plugin-profiler")]
            crate::profiling::set_profiling_mode(crate::profiling::ProfilingMode::Off);

            if tree.is_empty() {
                None
            } else {
                Some(tree)
            }
        } else {
            None
        }
    };
    #[cfg(not(any(feature = "plugin-apm", feature = "plugin-profiler")))]
    let profile_tree: Option<std::sync::Arc<crate::profiling::SpanTree>> = None;

    Some(ScriptResponse {
        status,
        headers,
        body,
        execution_time_us: start.elapsed().as_micros() as u64,
        stream_rx: None,
        errors: sapi::take_request_errors(),
        profile_tree,
    })
}
