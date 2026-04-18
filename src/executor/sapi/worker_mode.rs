//! Long-lived PHP worker: single `php_request_startup` for the lifetime of
//! the thread, PHP-land `oxphp_worker()` loops on bridge worker callbacks.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;

use crate::metrics::{WorkerMetrics, WorkerStats};
use crate::php::{bindings, sapi};
use crate::types::ScriptResponse;

use super::pool::WorkerRequest;

/// Configuration for worker mode threads.
/// Wrapped in Arc to avoid PathBuf heap clones on every worker spawn/respawn.
pub(super) struct WorkerModeConfig {
    pub worker_file: std::path::PathBuf,
    pub document_root: std::path::PathBuf,
    pub max_requests: u64,
    pub max_memory_mib: u64,
}

pub(super) fn spawn_worker_mode(
    id: usize,
    rx: crossbeam_channel::Receiver<WorkerRequest>,
    _shutdown: Arc<AtomicBool>, // kept for ManagedWorker interface; worker mode shuts down via channel closure
    last_active: Arc<AtomicU64>,
    config: Arc<WorkerModeConfig>,
    stats: Arc<WorkerStats>,
    worker_metrics: Arc<WorkerMetrics>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("php-worker-{id}"))
        .spawn(move || {
            worker_mode_thread(id, rx, last_active, config, stats, worker_metrics);
        })
        .expect("failed to spawn PHP worker mode thread")
}

/// Worker mode thread: runs a single PHP request context for the lifetime of the thread.
/// The worker file calls `oxphp_worker()` which loops internally, receiving requests
/// via the crossbeam channel and calling the PHP handler for each one.
fn worker_mode_thread(
    worker_id: usize,
    request_rx: crossbeam_channel::Receiver<WorkerRequest>,
    last_active: Arc<AtomicU64>,
    config: Arc<WorkerModeConfig>,
    stats: Arc<WorkerStats>,
    worker_metrics: Arc<WorkerMetrics>,
) {
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("php-worker")
        .to_string();

    // 1. Initialize TSRM thread-local storage (required for ZTS)
    unsafe {
        let _ = bindings::ts_resource_ex(0, std::ptr::null_mut());
        bindings::oxphp_bridge_tsrm_update();
        bindings::oxphp_bridge_init_ctx();
        bindings::oxphp_bridge_set_worker_id(worker_id as i32);
    }

    // 2. Set worker mode TLS flags
    unsafe {
        bindings::oxphp_bridge_set_worker_mode(config.max_requests, config.max_memory_mib);
    }

    // 3. Store channel receiver, last_active, and metrics in thread-local
    sapi::set_worker_rx(request_rx);
    sapi::set_worker_last_active(last_active);
    sapi::set_worker_stats(Arc::clone(&stats));
    sapi::set_worker_metrics(Arc::clone(&worker_metrics));

    // Mark worker as active with spawn time
    let spawn_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    stats.spawn_time_ms.store(spawn_ms, Ordering::Relaxed);
    stats.active.store(true, Ordering::Relaxed);

    tracing::info!(worker = %thread_name, file = %config.worker_file.display(), "Worker mode thread started");

    // 4. Single php_request_startup for the entire worker lifetime.
    //    Do NOT call oxphp_bridge_init_ctx() again — it would wipe worker_mode.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    unsafe {
        bindings::oxphp_bridge_set_request_time(now.as_secs_f64());
        // Set minimal request info for the boot request
        bindings::oxphp_bridge_set_request_info(
            b"GET\0".as_ptr() as *const std::os::raw::c_char,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
    }

    // Populate $_SERVER with boot-phase values so the worker script
    // can access SCRIPT_FILENAME, DOCUMENT_ROOT, etc. during bootstrap.
    // Without this, frameworks like Symfony abort early on empty $_SERVER.
    sapi::set_boot_server_vars(&config.worker_file, &config.document_root);

    if unsafe { bindings::php_request_startup() } != 0 {
        tracing::error!(worker = %thread_name, "php_request_startup() failed in worker mode");
        return;
    }

    // 5. Execute worker file — this enters oxphp_worker() loop.
    //    The loop blocks on recv() inside worker_wait_callback.
    //    Returns when shutdown or limits reached.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let script_path_str = config.worker_file.to_str().unwrap_or("");
        let script_path = CString::new(script_path_str).unwrap_or_default();

        let mut file_handle: bindings::zend_file_handle = unsafe { std::mem::zeroed() };
        unsafe {
            bindings::zend_stream_init_filename(&mut file_handle, script_path.as_ptr());
        }
        file_handle.primary_script = true;

        let script_ok = unsafe {
            bindings::oxphp_execute_script_safe(
                &mut file_handle as *mut _ as *mut std::os::raw::c_void,
            )
        };
        if script_ok == 0 {
            tracing::warn!(
                worker = %thread_name,
                path = %config.worker_file.display(),
                "Worker script aborted via zend_bailout"
            );
        }

        unsafe {
            bindings::zend_destroy_file_handle(&mut file_handle);
        }
    }));

    if result.is_err() {
        tracing::error!(worker = %thread_name, "Worker mode thread panicked");
        // Try to send 500 for any pending request
        if let Some((_start, tx)) = sapi::take_early_tx() {
            let _ = tx.send(ScriptResponse {
                status: 500,
                body: Bytes::from_static(b"Internal Server Error"),
                ..Default::default()
            });
        }
    }

    // Read exit reason and record recycle metrics
    let exit_reason = unsafe { bindings::oxphp_bridge_get_exit_reason() };
    if exit_reason > 0 {
        // Non-shutdown exit = recycle
        worker_metrics.record_recycle(exit_reason);
    }
    stats.active.store(false, Ordering::Relaxed);

    // 6. Single php_request_shutdown for the entire worker lifetime
    unsafe {
        bindings::php_request_shutdown(std::ptr::null_mut());
    }
    sapi::clear_request_data();

    tracing::info!(
        worker = %thread_name,
        exit_reason = exit_reason,
        "Worker mode thread stopped"
    );
}
