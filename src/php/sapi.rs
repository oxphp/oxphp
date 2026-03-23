use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime};

use bytes::Bytes;
use http::header;
use http::{HeaderName, HeaderValue};
use tokio::sync::oneshot;

use crate::async_types::{AsyncResult, AsyncTask, PromiseCleanup};
use crate::metrics::{WorkerMetrics, WorkerStats};
use crate::php::bindings::{self, *};
use crate::plugin::php::{PluginNativeFunction, PluginNativeFunctionDef};
use crate::types::{ScriptRequest, ScriptResponse};

/// Per-request response state consolidated in a single thread-local
/// to avoid 3 separate TLS lookups + RefCell borrows on the hot path.
#[derive(Default)]
pub(crate) struct ResponseBuffers {
    pub(crate) output: Vec<u8>,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) status_code: u16,
}

impl ResponseBuffers {
    fn new() -> Self {
        Self {
            output: Vec::with_capacity(8192),
            headers: Vec::new(),
            status_code: 200,
        }
    }
}

/// A pending worker-mode request received from the channel.
pub struct WorkerIncomingRequest {
    pub script: ScriptRequest,
    pub response_tx: oneshot::Sender<ScriptResponse>,
}

thread_local! {
    pub(crate) static RESPONSE: RefCell<ResponseBuffers> = RefCell::new(ResponseBuffers::new());
    static REQUEST_DATA: RefCell<RequestData> = RefCell::new(RequestData::new());
    /// Holds the oneshot sender + request start time for early response delivery
    /// via `oxphp_finish_request()`. Set before script execution, consumed when early send triggers.
    static EARLY_TX: RefCell<Option<(Instant, oneshot::Sender<ScriptResponse>)>> = const { RefCell::new(None) };
    /// Streaming body chunk sender — worker thread sends chunks via `blocking_send()`.
    /// Created lazily in `send_streaming_headers()` to avoid heap alloc for non-streaming requests.
    static STREAM_TX: RefCell<Option<tokio::sync::mpsc::Sender<Bytes>>> = const { RefCell::new(None) };
    /// Worker mode: channel receiver for incoming requests.
    static WORKER_RX: RefCell<Option<crossbeam_channel::Receiver<WorkerIncomingRequest>>> = const { RefCell::new(None) };
    /// Worker mode: shared last_active timestamp for scale manager idle detection.
    static WORKER_LAST_ACTIVE: RefCell<Option<Arc<AtomicU64>>> = const { RefCell::new(None) };
    /// Worker mode: per-worker stats (memory, requests_done, uptime).
    static WORKER_STATS: RefCell<Option<Arc<WorkerStats>>> = const { RefCell::new(None) };
    /// Worker mode: global worker metrics (counters, histogram).
    static WORKER_METRICS_TLS: RefCell<Option<Arc<WorkerMetrics>>> = const { RefCell::new(None) };
    /// Worker mode: request start time for duration histogram.
    static WORKER_REQUEST_START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// Pending request from non-blocking try_recv, awaiting prepare_received_request().
    static PENDING_REQUEST: RefCell<Option<WorkerIncomingRequest>> = const { RefCell::new(None) };
}

thread_local! {
    /// Promise ID -> (oneshot receiver, cancellation flag). HTTP worker threads only.
    static PROMISE_MAP: RefCell<HashMap<u64, (
        tokio::sync::oneshot::Receiver<AsyncResult>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    )>> = RefCell::new(HashMap::new());

    /// Per-thread monotonic promise ID counter.
    static PROMISE_COUNTER: std::cell::Cell<u64> = std::cell::Cell::new(0);

    /// Async task channel sender. Set once per HTTP/worker-mode thread.
    static ASYNC_TX: RefCell<Option<crossbeam_channel::Sender<AsyncTask>>>
        = RefCell::new(None);

    /// Per-promise freeze/borrow cleanup state.
    static PROMISE_CLEANUP: RefCell<HashMap<u64, PromiseCleanup>>
        = RefCell::new(HashMap::new());

    /// True on async worker threads, false on HTTP workers.
    static IS_ASYNC_WORKER: std::cell::Cell<bool> = std::cell::Cell::new(false);

    /// Pre-fetched async results waiting to be consumed by `take_ready_result`.
    /// Populated by `await_is_ready` when a non-blocking poll finds a completed promise.
    static READY_RESULTS: RefCell<HashMap<u64, AsyncResult>>
        = RefCell::new(HashMap::new());
}

/// Per-request data stored in thread-local for SAPI callbacks to access.
/// Reused across requests — `server_vars` Vec capacity is retained.
struct RequestData {
    /// Pre-built $_SERVER key-value pairs as CStrings (must outlive php_request_shutdown).
    server_vars: Vec<(CString, CString)>,
    /// Raw Cookie header string for read_cookies callback (must outlive request).
    cookie_string: Option<CString>,
    /// Query string for SG(request_info) — must outlive php_request_shutdown.
    query_string: Option<CString>,
    /// Content-Type string for SG(request_info) — must outlive php_request_shutdown.
    content_type_string: Option<CString>,
    /// Request ID CString — must outlive php_request_shutdown.
    request_id_cstr: Option<CString>,
    /// Request body for read_post callback.
    body: Bytes,
    /// How many bytes of body have been read so far.
    body_offset: usize,
    /// Whether this slot has been populated for the current request.
    active: bool,
}

impl RequestData {
    fn new() -> Self {
        Self {
            server_vars: Vec::with_capacity(32),
            cookie_string: None,
            query_string: None,
            content_type_string: None,
            request_id_cstr: None,
            body: Bytes::new(),
            body_offset: 0,
            active: false,
        }
    }
}

const SERVER_SOFTWARE: &str = concat!("OxPHP/", env!("CARGO_PKG_VERSION"));

/// Snapshot of process environment variables captured once at startup.
/// Avoids per-request `std::env::vars()` overhead (mutex, UTF-8 validation,
/// allocations) and eliminates a potential data race with PHP `putenv()`.
static ENV_SNAPSHOT: OnceLock<Vec<(CString, CString)>> = OnceLock::new();

fn env_snapshot() -> &'static [(CString, CString)] {
    ENV_SNAPSHOT.get_or_init(|| {
        std::env::vars()
            .filter_map(|(k, v)| Some((CString::new(k).ok()?, CString::new(v).ok()?)))
            .collect()
    })
}

/// Push a server variable, skipping entries with embedded null bytes.
#[inline]
fn push_server_var(vars: &mut Vec<(CString, CString)>, key: &str, val: &str) {
    if let (Ok(k), Ok(v)) = (CString::new(key), CString::new(val)) {
        vars.push((k, v));
    }
}

/// Set minimal $_SERVER variables for the worker mode boot phase.
/// Called once before php_request_startup() so the worker script
/// sees SCRIPT_FILENAME, DOCUMENT_ROOT, etc. during bootstrap.
pub fn set_boot_server_vars(script_path: &std::path::Path, document_root: &std::path::Path) {
    REQUEST_DATA.with(|rd| {
        let mut data = rd.borrow_mut();
        data.server_vars.clear();

        let vars = &mut data.server_vars;

        // Import process environment variables first so CGI vars can override them.
        for (key, val) in env_snapshot() {
            vars.push((key.clone(), val.clone()));
        }

        push_server_var(vars, "SCRIPT_FILENAME", &script_path.to_string_lossy());
        push_server_var(vars, "DOCUMENT_ROOT", &document_root.to_string_lossy());
        push_server_var(vars, "SERVER_SOFTWARE", SERVER_SOFTWARE);
        push_server_var(vars, "SERVER_PROTOCOL", "HTTP/1.1");
        push_server_var(vars, "REQUEST_METHOD", "GET");
        push_server_var(vars, "REQUEST_URI", "/");
        push_server_var(vars, "SCRIPT_NAME", "/");
        push_server_var(vars, "PHP_SELF", "/");
        push_server_var(vars, "SERVER_NAME", "localhost");
        push_server_var(vars, "SERVER_PORT", "80");
        push_server_var(vars, "REMOTE_ADDR", "127.0.0.1");
        push_server_var(vars, "REMOTE_PORT", "0");
        push_server_var(vars, "QUERY_STRING", "");
        push_server_var(vars, "GATEWAY_INTERFACE", "CGI/1.1");

        data.active = true;
    });
}

/// Store a oneshot sender for early response delivery.
/// Called from the worker thread before `execute_request()`.
pub fn set_early_tx(start: Instant, tx: oneshot::Sender<ScriptResponse>) {
    EARLY_TX.with(|slot| {
        *slot.borrow_mut() = Some((start, tx));
    });
}

/// Restore EARLY_TX into TLS (for per-fiber restore).
pub(crate) fn restore_early_tx(val: Option<(Instant, oneshot::Sender<ScriptResponse>)>) {
    EARLY_TX.with(|slot| {
        *slot.borrow_mut() = val;
    });
}

/// Take WORKER_REQUEST_START from TLS (for per-fiber save).
pub(crate) fn take_request_start() -> Option<Instant> {
    WORKER_REQUEST_START.with(|cell| cell.take())
}

/// Restore WORKER_REQUEST_START into TLS (for per-fiber restore).
pub(crate) fn restore_request_start(val: Option<Instant>) {
    WORKER_REQUEST_START.with(|cell| cell.set(val));
}

/// Parse raw header strings into typed `HeaderName`/`HeaderValue` pairs.
/// Shared between early send and normal response paths.
pub fn parse_raw_headers(raw: Vec<(String, String)>) -> Vec<(HeaderName, HeaderValue)> {
    raw.into_iter()
        .filter_map(|(name, value)| {
            let hn = HeaderName::from_bytes(name.as_bytes()).ok()?;
            let hv = HeaderValue::from_str(&value).ok()?;
            Some((hn, hv))
        })
        .collect()
}

/// Attempt to send the response early if `oxphp_bridge_is_finished()` is true
/// and the sender hasn't been consumed yet. Returns true if response was sent.
pub fn try_early_send() -> bool {
    let finished = unsafe { bindings::oxphp_bridge_is_finished() };
    if !finished {
        return false;
    }

    // If streaming is active, finish_request means "end the stream"
    if unsafe { bindings::oxphp_bridge_is_streaming() } {
        // Flush any remaining buffered output as a final chunk
        flush_stream_chunk();
        // Drop the sender to close the body channel → stream ends
        close_stream();
        return true;
    }

    EARLY_TX.with(|slot| {
        let entry = slot.borrow_mut().take();
        if let Some((start, tx)) = entry {
            let (raw_output, raw_headers, status) = take_response();
            let body = Bytes::from(raw_output);
            let headers = parse_raw_headers(raw_headers);
            let _ = tx.send(ScriptResponse {
                status,
                headers,
                body,
                execution_time_us: start.elapsed().as_micros() as u64,
                stream_rx: None,
            });
            true
        } else {
            false
        }
    })
}

/// Returns true if the early sender has been consumed (response already sent).
pub fn was_early_sent() -> bool {
    EARLY_TX.with(|slot| slot.borrow().is_none())
}

/// Take the early sender from TLS (for panic recovery in the worker thread).
pub fn take_early_tx() -> Option<(Instant, oneshot::Sender<ScriptResponse>)> {
    EARLY_TX.with(|slot| slot.borrow_mut().take())
}

/// Send streaming headers via the EARLY_TX oneshot.
/// Creates the streaming channel on-demand (lazy — avoids heap alloc for non-streaming requests).
/// Takes EARLY_TX from TLS, builds a ScriptResponse with stream_rx, and sends it.
/// Stores the chunk sender in STREAM_TX for subsequent flush_stream_chunk() calls.
/// Returns true if headers were sent.
pub fn send_streaming_headers() -> bool {
    EARLY_TX.with(|slot| {
        let entry = slot.borrow_mut().take();
        if let Some((start, tx)) = entry {
            // Create streaming channel on-demand — only when streaming actually starts.
            // This avoids a heap allocation for the vast majority of non-streaming requests.
            let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
            STREAM_TX.with(|s| {
                *s.borrow_mut() = Some(chunk_tx);
            });

            let (raw_output, raw_headers, status) = take_response();
            let body = Bytes::from(raw_output);
            let headers = parse_raw_headers(raw_headers);
            let _ = tx.send(ScriptResponse {
                status,
                headers,
                body,
                execution_time_us: start.elapsed().as_micros() as u64,
                stream_rx: Some(chunk_rx),
            });
            true
        } else {
            false
        }
    })
}

/// Drain the output buffer and send it as a chunk via STREAM_TX.
fn flush_stream_chunk() {
    STREAM_TX.with(|slot| {
        if let Some(tx) = slot.borrow().as_ref() {
            let data = RESPONSE.with(|r| {
                let mut resp = r.borrow_mut();
                if resp.output.is_empty() {
                    return None;
                }
                Some(Bytes::from(std::mem::take(&mut resp.output)))
            });
            if let Some(chunk) = data {
                // blocking_send: blocks if channel full (backpressure)
                let _ = tx.blocking_send(chunk);
            }
        }
    });
}

/// Drop the STREAM_TX sender to close the body channel (signals stream end).
fn close_stream() {
    STREAM_TX.with(|slot| {
        slot.borrow_mut().take();
    });
}

/// Build request data from a ScriptRequest and store in thread-local.
/// Reuses the existing Vec capacity from previous requests.
/// Must be called BEFORE php_request_startup().
pub fn set_request_data(req: &ScriptRequest) {
    REQUEST_DATA.with(|rd| {
        let mut data = rd.borrow_mut();

        // Extract cookie string for read_cookies callback (before borrowing server_vars)
        data.cookie_string = req
            .headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| CString::new(s).ok());

        // Clear previous values but keep the Vec allocation
        data.server_vars.clear();

        let vars = &mut data.server_vars;

        // Import process environment variables first so CGI/HTTP vars can override them.
        for (key, val) in env_snapshot() {
            vars.push((key.clone(), val.clone()));
        }

        // CGI/1.1 standard variables
        push_server_var(vars, "REQUEST_METHOD", req.method.as_str());
        push_server_var(
            vars,
            "REQUEST_URI",
            req.uri
                .path_and_query()
                .map(|pq| pq.as_str())
                .unwrap_or("/"),
        );
        push_server_var(vars, "QUERY_STRING", &req.query_string);
        let protocol = match req.version {
            http::Version::HTTP_10 => "HTTP/1.0",
            http::Version::HTTP_11 => "HTTP/1.1",
            http::Version::HTTP_2 => "HTTP/2",
            http::Version::HTTP_3 => "HTTP/3",
            _ => "HTTP/1.1",
        };
        push_server_var(vars, "SERVER_PROTOCOL", protocol);

        // SCRIPT_NAME: URI path without query string
        let path = req.uri.path();
        push_server_var(vars, "SCRIPT_NAME", path);
        push_server_var(vars, "PHP_SELF", path);
        push_server_var(vars, "DOCUMENT_URI", path);

        // SCRIPT_FILENAME: absolute filesystem path to the script
        push_server_var(vars, "SCRIPT_FILENAME", &req.script_path.to_string_lossy());

        // DOCUMENT_ROOT
        push_server_var(vars, "DOCUMENT_ROOT", &req.document_root.to_string_lossy());

        // Server identification
        push_server_var(vars, "SERVER_SOFTWARE", SERVER_SOFTWARE);
        push_server_var(vars, "GATEWAY_INTERFACE", "CGI/1.1");

        // Connection info
        push_server_var(vars, "REMOTE_ADDR", &req.remote_addr.ip().to_string());
        push_server_var(vars, "REMOTE_PORT", &req.remote_addr.port().to_string());

        // HTTPS indicator (CGI/1.1: "on" when TLS is active)
        if req.is_tls {
            push_server_var(vars, "HTTPS", "on");
        }

        // REQUEST_SCHEME: "http" or "https" (PHP-FPM / nginx convention)
        push_server_var(
            vars,
            "REQUEST_SCHEME",
            if req.is_tls { "https" } else { "http" },
        );

        // SERVER_NAME and SERVER_PORT from Host header
        let default_port = if req.is_tls { "443" } else { "80" };
        if let Some(host) = req.headers.get(header::HOST) {
            if let Ok(host_str) = host.to_str() {
                if let Some(colon) = host_str.rfind(':') {
                    push_server_var(vars, "SERVER_NAME", &host_str[..colon]);
                    push_server_var(vars, "SERVER_PORT", &host_str[colon + 1..]);
                } else {
                    push_server_var(vars, "SERVER_NAME", host_str);
                    push_server_var(vars, "SERVER_PORT", default_port);
                }
            }
        } else {
            push_server_var(vars, "SERVER_NAME", "localhost");
            push_server_var(vars, "SERVER_PORT", default_port);
        }

        // CONTENT_TYPE and CONTENT_LENGTH (no HTTP_ prefix per CGI spec)
        if let Some(ct) = req.headers.get(header::CONTENT_TYPE) {
            if let Ok(ct_str) = ct.to_str() {
                push_server_var(vars, "CONTENT_TYPE", ct_str);
            }
        }
        if let Some(cl) = req.headers.get(header::CONTENT_LENGTH) {
            if let Ok(cl_str) = cl.to_str() {
                push_server_var(vars, "CONTENT_LENGTH", cl_str);
            }
        }

        // All request headers as HTTP_{UPPER_SNAKE_CASE}
        // (except Content-Type and Content-Length which are handled above without HTTP_ prefix)
        let mut header_buf = String::with_capacity(64);
        for (name, value) in req.headers.iter() {
            if name == header::CONTENT_TYPE || name == header::CONTENT_LENGTH {
                continue;
            }
            let Ok(val_str) = value.to_str() else {
                continue;
            };

            header_buf.clear();
            header_buf.push_str("HTTP_");
            for b in name.as_str().bytes() {
                header_buf.push(if b == b'-' {
                    '_'
                } else {
                    b.to_ascii_uppercase() as char
                });
            }
            push_server_var(vars, &header_buf, val_str);
        }

        // REQUEST_TIME and REQUEST_TIME_FLOAT from bridge (set in setup_request_tls)
        let rt = unsafe { bindings::oxphp_bridge_get_request_time() };
        if rt > 0.0 {
            push_server_var(vars, "REQUEST_TIME", &(rt as u64).to_string());
            push_server_var(vars, "REQUEST_TIME_FLOAT", &format!("{rt:.6}"));
        }

        // Trace context variables (when tracing is enabled)
        if !req.trace_id.is_empty() {
            push_server_var(vars, "OXPHP_TRACE_ID", &req.trace_id);
            push_server_var(vars, "OXPHP_SPAN_ID", &req.span_id);
            push_server_var(vars, "OXPHP_PARENT_SPAN_ID", &req.parent_span_id);
        }

        // Strings for SG(request_info) — stored as CStrings so pointers
        // remain valid through php_request_shutdown().
        data.query_string = if req.query_string.is_empty() {
            None
        } else {
            CString::new(req.query_string.as_str()).ok()
        };

        data.content_type_string = req
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| CString::new(s).ok());

        data.body = req.body.clone();
        data.body_offset = 0;
        data.active = true;

        // Set request ID in bridge TLS so oxphp_request_id() returns it.
        let rid_cstr = CString::new(req.request_id.as_str()).unwrap_or_default();
        unsafe {
            bindings::oxphp_bridge_set_request_id(rid_cstr.as_ptr());
        }
        data.request_id_cstr = Some(rid_cstr);

        // Set SG(request_info) so PHP parses $_GET, $_POST, $_FILES, $_COOKIE.
        // This MUST happen before php_request_startup().
        let method_cstr = data
            .server_vars
            .iter()
            .find(|(k, _)| k.as_bytes() == b"REQUEST_METHOD")
            .map(|(_, v)| v.as_ptr())
            .unwrap_or(std::ptr::null());

        let qs_ptr = data
            .query_string
            .as_ref()
            .map(|cs| cs.as_ptr())
            .unwrap_or(std::ptr::null());

        let ct_ptr = data
            .content_type_string
            .as_ref()
            .map(|cs| cs.as_ptr())
            .unwrap_or(std::ptr::null());

        let content_length = req
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        unsafe {
            bindings::oxphp_bridge_set_request_info(
                method_cstr,
                qs_ptr,
                ct_ptr,
                content_length as std::os::raw::c_long,
            );
        }
    });
}

/// Typical number of $_SERVER variables per request (~20 CGI vars + headers).
/// If the Vec grows beyond 2x this, shrink it to avoid monotonic growth from
/// anomalous requests with many headers.
const SERVER_VARS_NORMAL_CAPACITY: usize = 64;

/// Clear request data from thread-local.
/// Retains Vec capacity for reuse by the next request, but shrinks if oversized.
/// Must be called AFTER php_request_shutdown().
pub fn clear_request_data() {
    REQUEST_DATA.with(|rd| {
        let mut data = rd.borrow_mut();
        data.server_vars.clear();
        if data.server_vars.capacity() > SERVER_VARS_NORMAL_CAPACITY {
            data.server_vars.shrink_to(SERVER_VARS_NORMAL_CAPACITY);
        }
        data.cookie_string = None;
        data.query_string = None;
        data.content_type_string = None;
        data.request_id_cstr = None;
        data.body = Bytes::new();
        data.body_offset = 0;
        data.active = false;
    });

    // Clear SG(request_info) and bridge context so PHP doesn't hold stale references.
    unsafe {
        bindings::oxphp_bridge_set_request_info(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        bindings::oxphp_bridge_set_request_id(std::ptr::null());
        // Reset deadline and cancellation flags so the next request on this worker
        // doesn't inherit stale state from a timed-out or cancelled request.
        bindings::oxphp_bridge_set_deadline(0);
        bindings::oxphp_bridge_set_cancelled(false);
    }
}

// FFI declarations for the C bridge wrapper callbacks
extern "C" {
    fn oxphp_bridge_ub_write(str: *const c_char, str_length: usize) -> usize;
    fn oxphp_bridge_flush(server_context: *mut c_void);
}

/// Build the custom SAPI module struct.
///
/// All string pointers use `b"...\0"` byte literals which have `'static` lifetime.
pub fn build_sapi_module() -> sapi_module_struct {
    // Register Rust implementations with the bridge so the C wrappers can call them.
    // The C wrappers check deadline/cancellation BEFORE calling Rust, and bailout
    // from C (longjmp stays within C frames, never crosses Rust FFI).
    unsafe {
        bindings::oxphp_bridge_set_sapi_callbacks(Some(oxphp_ub_write), Some(oxphp_flush));
    }

    sapi_module_struct {
        name: b"cli-server\0".as_ptr() as *mut c_char,
        pretty_name: b"OxPHP\0".as_ptr() as *mut c_char,

        startup: Some(oxphp_startup),
        shutdown: Some(oxphp_shutdown),

        activate: Some(oxphp_activate),
        deactivate: Some(oxphp_deactivate),

        ub_write: Some(oxphp_bridge_ub_write),
        flush: Some(oxphp_bridge_flush),
        get_stat: None,
        getenv: None,

        sapi_error: zend_error as *mut c_void,

        header_handler: Some(oxphp_header_handler),
        send_headers: Some(oxphp_send_headers),
        send_header: None,

        read_post: Some(oxphp_read_post),
        read_cookies: Some(oxphp_read_cookies),

        register_server_variables: Some(oxphp_register_server_variables),
        log_message: Some(oxphp_log_message),
        get_request_time: Some(oxphp_get_request_time),
        terminate_process: None,

        php_ini_path_override: std::ptr::null_mut(),
        default_post_reader: None,
        treat_data: None,
        executable_location: std::ptr::null_mut(),

        php_ini_ignore: 0,
        php_ini_ignore_cwd: 0,

        get_fd: None,
        force_http_10: None,
        get_target_uid: None,
        get_target_gid: None,
        input_filter: None,
        ini_defaults: None,
        phpinfo_as_text: 0,

        ini_entries: std::ptr::null(),

        additional_functions: std::ptr::null(),
        input_filter_init: None,
    }
}

// ─── SAPI Callbacks: Superglobals ────────────────────────────

/// Callback: register $_SERVER variables.
/// Called by PHP during request startup to populate $_SERVER.
unsafe extern "C" fn oxphp_register_server_variables(track_vars_array: *mut c_void) {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if data.active {
            for (key, value) in &data.server_vars {
                php_register_variable_safe(
                    key.as_ptr(),
                    value.as_ptr(),
                    value.to_bytes().len(),
                    track_vars_array,
                );
            }
        }
    });
}

/// Callback: return raw Cookie header string for PHP to parse into $_COOKIE.
/// The returned pointer must remain valid through php_request_shutdown().
/// We store the CString in RequestData, so it lives long enough.
unsafe extern "C" fn oxphp_read_cookies() -> *mut c_char {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        match data.cookie_string.as_ref() {
            Some(cs) if data.active => cs.as_ptr() as *mut c_char,
            _ => std::ptr::null_mut(),
        }
    })
}

/// Callback: read POST body data for PHP to parse into $_POST/$_FILES/php://input.
/// Called repeatedly until it returns 0.
unsafe extern "C" fn oxphp_read_post(buffer: *mut c_char, count_bytes: usize) -> usize {
    REQUEST_DATA.with(|rd| {
        let mut data = rd.borrow_mut();
        if !data.active {
            return 0;
        }

        let remaining = data.body.len().saturating_sub(data.body_offset);
        if remaining == 0 {
            return 0;
        }

        let to_copy = remaining.min(count_bytes);
        let src = &data.body[data.body_offset..data.body_offset + to_copy];
        std::ptr::copy_nonoverlapping(src.as_ptr(), buffer as *mut u8, to_copy);
        data.body_offset += to_copy;
        to_copy
    })
}

// ─── Module Lifecycle ───────────────────────────────────────

unsafe extern "C" fn oxphp_startup(_module: *mut sapi_module_struct) -> c_int {
    0 // SUCCESS
}

unsafe extern "C" fn oxphp_shutdown(_module: *mut sapi_module_struct) -> c_int {
    0 // SUCCESS
}

// ─── Request Lifecycle ──────────────────────────────────────

/// Called by PHP at the start of each request (during php_request_startup).
/// Clears per-request state so the output handler chain starts clean.
unsafe extern "C" fn oxphp_activate() -> c_int {
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        resp.headers.clear();
        resp.status_code = 200;
    });
    0 // SUCCESS
}

/// Called by PHP at the end of each request (during php_request_shutdown).
unsafe extern "C" fn oxphp_deactivate() -> c_int {
    0 // SUCCESS
}

// ─── Output Capture ─────────────────────────────────────────

/// Check if the Tokio receiver was dropped (client disconnected / timeout fired).
/// Sets the bridge cancellation flag so the C-level deadline check triggers bailout.
/// Only logs once per request — subsequent calls are silent.
unsafe fn check_client_disconnected() {
    if bindings::oxphp_bridge_is_cancelled() {
        return; // already flagged, don't log again
    }
    EARLY_TX.with(|slot| {
        if let Some((_, tx)) = slot.borrow().as_ref() {
            if tx.is_closed() {
                tracing::warn!("Client disconnected, requesting PHP cancellation");
                bindings::oxphp_bridge_set_cancelled(true);
            }
        }
    });
}

unsafe extern "C" fn oxphp_ub_write(str: *const c_char, str_length: usize) -> usize {
    if str.is_null() || str_length == 0 {
        return 0;
    }

    // NOTE: no check_client_disconnected() here — it's too expensive for the hot path.
    // Client disconnect detection happens in flush (infrequent) and via the C-level
    // periodic deadline check (every 128 ub_write calls).

    // After finish_request (non-streaming): discard output to avoid memory growth.
    // After finish_request (streaming): also discard — stream is closed.
    if bindings::oxphp_bridge_is_finished() && was_early_sent() {
        return str_length;
    }

    // In both streaming and buffered modes, buffer into RESPONSE.output.
    // For streaming, the actual channel send happens in oxphp_flush (buffered-until-flush).
    let data = std::slice::from_raw_parts(str as *const u8, str_length);

    RESPONSE.with(|r| {
        r.borrow_mut().output.extend_from_slice(data);
    });

    str_length
}

unsafe extern "C" fn oxphp_flush(_server_context: *mut c_void) {
    check_client_disconnected();

    // Streaming mode: send headers on first flush, then send buffered output as chunk.
    if bindings::oxphp_bridge_is_streaming() {
        if !bindings::oxphp_bridge_get_headers_sent() {
            send_streaming_headers();
            bindings::oxphp_bridge_set_headers_sent(true);
        }
        flush_stream_chunk();
        return;
    }

    // When oxphp_finish_request() sets the finished flag and calls sapi_flush(),
    // this triggers the early response send.
    try_early_send();
}

// ─── Header Handling ────────────────────────────────────────

unsafe extern "C" fn oxphp_header_handler(
    sapi_header: *mut sapi_header_struct,
    op: sapi_header_op_enum,
    _sapi_headers: *mut sapi_headers_struct,
) -> c_int {
    RESPONSE.with(|r| {
        match op {
            sapi_header_op_enum::SAPI_HEADER_DELETE_ALL => {
                r.borrow_mut().headers.clear();
                return 0;
            }
            sapi_header_op_enum::SAPI_HEADER_SET_STATUS => {
                let code = sapi_header as usize as u16;
                if (100..600).contains(&code) {
                    r.borrow_mut().status_code = code;
                }
                return 0;
            }
            _ => {}
        }

        if sapi_header.is_null() {
            return 0;
        }

        let header_ptr = (*sapi_header).header;
        let header_len = (*sapi_header).header_len;

        if header_ptr.is_null() || header_len == 0 {
            return 0;
        }

        let header_bytes = std::slice::from_raw_parts(header_ptr as *const u8, header_len);
        let header_str = String::from_utf8_lossy(header_bytes);

        match op {
            sapi_header_op_enum::SAPI_HEADER_DELETE => {
                let name = header_str.trim();
                r.borrow_mut()
                    .headers
                    .retain(|(n, _)| !n.eq_ignore_ascii_case(name));
            }
            sapi_header_op_enum::SAPI_HEADER_REPLACE | sapi_header_op_enum::SAPI_HEADER_ADD => {
                if let Some(colon_pos) = header_str.find(':') {
                    let name = header_str[..colon_pos].trim().to_string();
                    let value = header_str[colon_pos + 1..].trim().to_string();

                    // Auto-detect SSE: enable streaming when PHP sets Content-Type: text/event-stream
                    if name.eq_ignore_ascii_case("content-type")
                        && value.contains("text/event-stream")
                    {
                        bindings::oxphp_bridge_set_stream_mode(true);
                    }

                    let mut resp = r.borrow_mut();
                    if op == sapi_header_op_enum::SAPI_HEADER_REPLACE {
                        resp.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                    }
                    resp.headers.push((name, value));
                }
            }
            _ => {}
        }

        0
    })
}

unsafe extern "C" fn oxphp_send_headers(_sapi_headers: *mut sapi_headers_struct) -> c_int {
    // NOTE: Do NOT read http_response_code from _sapi_headers here.
    // In PHP ZTS, the sapi_headers pointer passed to this callback may reference
    // stale TSRM memory, causing status codes from previous requests to leak.
    // Instead, we read the response code from the C bridge (which has correct TSRM
    // context) after script execution — see collect_response_code().
    1 // SAPI_HEADER_SENT_SUCCESSFULLY
}

// ─── Logging ────────────────────────────────────────────────

unsafe extern "C" fn oxphp_log_message(message: *const c_char, syslog_type: c_int) {
    if message.is_null() {
        return;
    }
    let msg = std::ffi::CStr::from_ptr(message);

    // syslog_type == 1 (LOG_ALERT) indicates fatal errors.
    // Set HTTP 500 as an additional safety net — the error callback should have
    // already done this, but log_message doesn't trigger zend_bailout so it's
    // guaranteed to execute fully.
    if syslog_type == 1 {
        set_fatal_error_status_if_default();
    }

    tracing::warn!(php_message = %msg.to_string_lossy(), "PHP log");
}

// ─── Structured Error Logging via zend_error_cb ─────────────

/// Stores the original zend_error_cb so we can delegate after logging.
static ORIGINAL_ERROR_CB: OnceLock<ZendErrorCbT> = OnceLock::new();

/// Install our structured error callback, saving the original.
/// Must be called AFTER php_module_startup() (which sets the default zend_error_cb).
///
/// # Safety
/// Must only be called once, from the main thread, after PHP module startup.
pub unsafe fn install_error_cb() {
    let original = crate::php::bindings::zend_error_cb;
    if ORIGINAL_ERROR_CB.set(original).is_err() {
        return;
    }
    crate::php::bindings::zend_error_cb = oxphp_error_cb;
}

/// Map PHP error type constant to (tracing level, human-readable name).
/// PHP uses bitmask values; uncaught exceptions may have high bits set (e.g. 0x1000001).
fn error_type_str(error_type: c_int) -> (&'static str, &'static str) {
    // Match the low 15 bits — PHP's standard E_* constants live in 1..16384
    match error_type & 0x7FFF {
        1 => ("error", "E_ERROR"),
        2 => ("warn", "E_WARNING"),
        4 => ("error", "E_PARSE"),
        8 => ("info", "E_NOTICE"),
        16 => ("error", "E_CORE_ERROR"),
        32 => ("warn", "E_CORE_WARNING"),
        64 => ("error", "E_COMPILE_ERROR"),
        128 => ("warn", "E_COMPILE_WARNING"),
        256 => ("error", "E_USER_ERROR"),
        512 => ("warn", "E_USER_WARNING"),
        1024 => ("info", "E_USER_NOTICE"),
        2048 => ("info", "E_STRICT"),
        4096 => ("error", "E_RECOVERABLE_ERROR"),
        8192 => ("info", "E_DEPRECATED"),
        16384 => ("info", "E_USER_DEPRECATED"),
        _ => ("warn", "E_UNKNOWN"),
    }
}

unsafe extern "C" fn oxphp_error_cb(
    type_: c_int,
    error_filename: *const zend_string,
    error_lineno: c_uint,
    message: *const zend_string,
) {
    // Extract strings from zend_string pointers
    let file = if error_filename.is_null() {
        std::borrow::Cow::Borrowed("unknown")
    } else {
        (*error_filename).to_str_lossy()
    };

    let msg = if message.is_null() {
        std::borrow::Cow::Borrowed("(no message)")
    } else {
        (*message).to_str_lossy()
    };

    let (level, type_name) = error_type_str(type_);

    // For fatal error types, set HTTP 500 BEFORE delegating to the original callback.
    // The original callback may trigger zend_bailout (longjmp) which would skip any
    // code placed after the delegation.
    if level == "error" {
        set_fatal_error_status_if_default();

        // On async worker threads, capture the error message for exception propagation.
        // When an uncaught exception triggers zend_exception_error → zend_bailout,
        // EG(exception) is cleared before bailout. The zend_catch block in
        // oxphp_execute_async_task reads this captured message to extract the
        // original exception class and message.
        if is_async_worker() && !message.is_null() {
            let raw = (*message).as_bytes();
            crate::bridge::ffi::oxphp_bridge_capture_fatal(
                raw.as_ptr() as *const std::os::raw::c_char,
                raw.len(),
            );
        }
    }

    match level {
        "error" => {
            tracing::error!(
                php_error_type = type_name,
                php_file = %file,
                php_line = error_lineno,
                "PHP: {msg}"
            );
        }
        "warn" => {
            tracing::warn!(
                php_error_type = type_name,
                php_file = %file,
                php_line = error_lineno,
                "PHP: {msg}"
            );
        }
        _ => {
            tracing::info!(
                php_error_type = type_name,
                php_file = %file,
                php_line = error_lineno,
                "PHP: {msg}"
            );
        }
    }

    // Delegate to original callback for PHP's standard error handling
    // (display_errors output, user error handlers, fatal abort, etc.)
    if let Some(&original) = ORIGINAL_ERROR_CB.get() {
        original(type_, error_filename, error_lineno, message);
    }
}

// ─── Request Time (for OPcache) ─────────────────────────────

unsafe extern "C" fn oxphp_get_request_time(request_time: *mut f64) -> zend_result {
    if !request_time.is_null() {
        *request_time = bindings::oxphp_bridge_get_request_time();
    }
    0 // SUCCESS
}

// ─── Buffer Access ──────────────────────────────────────────

/// Take output, headers, and status code in a single TLS lookup + borrow.
/// Read the HTTP response code from PHP's SG(sapi_headers).http_response_code
/// via the C bridge (which has a correct TSRM context).
/// Must be called after script execution, before php_request_shutdown()
/// destroys the request state.
pub fn collect_response_code() {
    let code = unsafe { bindings::oxphp_bridge_get_response_code() };
    if code > 0 {
        RESPONSE.with(|r| {
            let mut resp = r.borrow_mut();
            // Don't overwrite status set by error handlers (e.g. 500 from fatal)
            if resp.status_code == 200 {
                resp.status_code = code as u16;
            }
        });
    }
}

pub fn take_response() -> (Vec<u8>, Vec<(String, String)>, u16) {
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        let output = std::mem::take(&mut resp.output);
        let headers = std::mem::take(&mut resp.headers);
        let status = resp.status_code;
        resp.status_code = 200;
        (output, headers, status)
    })
}

pub fn clear_buffers() {
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        resp.output.clear();
        resp.headers.clear();
        resp.status_code = 200;
    });
    // Drop any unconsumed early sender from a previous request.
    EARLY_TX.with(|slot| {
        slot.borrow_mut().take();
    });
    // Drop streaming sender from previous request (receiver was consumed by ScriptResponse).
    STREAM_TX.with(|slot| {
        slot.borrow_mut().take();
    });
}

/// Set HTTP 500 status if the current status is still the default 200.
/// Used by error callback for fatal errors and by execute_request on bailout.
pub fn set_fatal_error_status_if_default() {
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        if resp.status_code == 200 {
            resp.status_code = 500;
        }
    });
}

// ─── Native Plugin Function Bridge ──────────────────────────

use std::collections::HashMap;

/// Global registry of native plugin PHP function handlers, keyed by function name.
/// O(1) lookup on every dispatch instead of O(n) linear scan.
/// Set once from main.rs after plugin_manager.init_all().
static NATIVE_DISPATCH_MAP: OnceLock<HashMap<String, Box<dyn PluginNativeFunction>>> =
    OnceLock::new();

/// Register native plugin functions on the bridge and store handlers for dispatch.
/// Called from main.rs after plugin_manager.init_all().
pub fn register_native_plugin_functions(fns: Vec<PluginNativeFunctionDef>) {
    for f in &fns {
        let name = match CString::new(f.name.as_str()) {
            Ok(n) => n,
            Err(_) => {
                tracing::warn!(name = f.name, "Skipping plugin fn with NUL in name");
                continue;
            }
        };
        let required = f.params.iter().filter(|p| p.required).count() as c_int;
        let total = f.params.len() as c_int;
        unsafe {
            bindings::oxphp_bridge_register_plugin_fn(name.as_ptr(), required, total);
        }
    }
    unsafe {
        bindings::oxphp_bridge_set_native_dispatch(Some(native_dispatch_callback));
    }
    let count = fns.len();
    let map: HashMap<String, Box<dyn PluginNativeFunction>> =
        fns.into_iter().map(|f| (f.name, f.handler)).collect();
    NATIVE_DISPATCH_MAP.set(map).ok();
    tracing::info!(count, "Native plugin PHP functions registered on bridge");
}

/// Native dispatch callback invoked from C extension via bridge.
/// Creates a NativeCall wrapper, finds the handler via HashMap, and invokes it.
///
/// # Safety
/// Called from C code. `name` must be a valid C string.
/// `args` and `retval` must be valid zval pointers for the duration of the call.
unsafe extern "C" fn native_dispatch_callback(
    name: *const c_char,
    args: *mut c_void,
    argc: u32,
    retval: *mut c_void,
) -> c_int {
    // Safety: function names are ASCII identifiers registered by our own code
    // during startup — UTF-8 validation is unnecessary overhead on the hot path.
    let name_str = std::str::from_utf8_unchecked(CStr::from_ptr(name).to_bytes());

    let map = match NATIVE_DISPATCH_MAP.get() {
        Some(m) => m,
        None => return -1,
    };

    let handler = match map.get(name_str) {
        Some(h) => h,
        None => return -1,
    };

    // Catch panics — unwinding through extern "C" is an abort on Rust 2021.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut call = crate::bridge::call::NativeCall::new(args, argc, retval, None);
        handler.handle(&mut call)
    }));

    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            tracing::warn!(func = name_str, error = %e, "Plugin function error");
            -1
        }
        Err(_) => {
            tracing::error!(func = name_str, "Plugin function panicked");
            -1
        }
    }
}

// ─── Worker Mode Helpers ────────────────────────────────────

/// Store the crossbeam receiver in thread-local for worker mode.
pub fn set_worker_rx(rx: crossbeam_channel::Receiver<WorkerIncomingRequest>) {
    WORKER_RX.with(|slot| {
        *slot.borrow_mut() = Some(rx);
    });
}

pub fn set_worker_last_active(last_active: Arc<AtomicU64>) {
    WORKER_LAST_ACTIVE.with(|slot| {
        *slot.borrow_mut() = Some(last_active);
    });
}

pub fn set_worker_stats(stats: Arc<WorkerStats>) {
    WORKER_STATS.with(|slot| {
        *slot.borrow_mut() = Some(stats);
    });
}

pub fn set_worker_metrics(wm: Arc<WorkerMetrics>) {
    WORKER_METRICS_TLS.with(|slot| {
        *slot.borrow_mut() = Some(wm);
    });
}

/// Worker wait callback — called from C bridge when PHP calls oxphp_worker().
/// Blocks on the crossbeam channel until a new request arrives.
/// Populates SAPI TLS with the new request data.
/// Returns 0 on success (request ready), -1 on shutdown (channel closed).
///
/// # Safety
/// Called from C code via function pointer. Must only be called from a worker thread
/// with WORKER_RX set.
unsafe extern "C" fn worker_wait_callback() -> std::os::raw::c_int {
    let incoming = WORKER_RX.with(|slot| {
        let rx = slot.borrow();
        match rx.as_ref() {
            Some(rx) => rx.recv().ok(),
            None => None,
        }
    });

    match incoming {
        Some(req) => {
            // Direct call — no PENDING_REQUEST round-trip on the blocking path
            setup_request_tls(req);
            0 // success
        }
        None => -1, // channel closed = shutdown
    }
}

/// Worker send response callback — called from C bridge after each handler invocation.
/// Takes the accumulated response from SAPI output + headers and sends via oneshot.
/// Returns 0 on success.
///
/// # Safety
/// Called from C code via function pointer.
unsafe extern "C" fn worker_send_callback() -> std::os::raw::c_int {
    // Cleanup any outstanding async promises from this request
    cleanup_outstanding_promises_callback();

    // If streaming was active, close the stream
    if bindings::oxphp_bridge_is_streaming() {
        flush_stream_chunk();
        close_stream();
        // If early TX was already consumed by streaming headers, we're done
        if was_early_sent() {
            record_worker_request_metrics();
            clear_buffers();
            return 0;
        }
    }

    // If response was already sent early (finish_request), we're done
    if was_early_sent() {
        record_worker_request_metrics();
        clear_buffers();
        return 0;
    }

    // If the handler failed (bailout/fatal error), force HTTP 500
    if bindings::oxphp_bridge_get_handler_failed() {
        set_fatal_error_status_if_default();
    }

    // Take accumulated response and send via oneshot
    let (raw_output, raw_headers, status) = take_response();
    let body = Bytes::from(raw_output);
    let headers = parse_raw_headers(raw_headers);

    EARLY_TX.with(|slot| {
        if let Some((start, tx)) = slot.borrow_mut().take() {
            let _ = tx.send(ScriptResponse {
                status,
                headers,
                body,
                execution_time_us: start.elapsed().as_micros() as u64,
                stream_rx: None,
            });
        }
    });

    // Record worker mode metrics after response sent
    record_worker_request_metrics();

    // Clean up for next request
    clear_buffers();

    0
}

/// Record per-request worker mode metrics (memory, requests_done, duration histogram).
/// Called from worker_send_callback after each request.
unsafe fn record_worker_request_metrics() {
    // Read memory and requests_done from bridge.
    // requests_done is read BEFORE C-side increment, so add 1.
    let memory = bindings::oxphp_bridge_get_memory_usage();
    let requests_done = bindings::oxphp_bridge_get_requests_done() + 1;

    // Update per-worker stats
    WORKER_STATS.with(|slot| {
        if let Some(ref stats) = *slot.borrow() {
            stats.memory_bytes.store(memory, Ordering::Relaxed);
            stats.requests_done.store(requests_done, Ordering::Relaxed);
        }
    });

    // Compute duration and update global metrics
    let duration_us = WORKER_REQUEST_START
        .with(|slot| slot.take().map(|start| start.elapsed().as_micros() as u64));

    WORKER_METRICS_TLS.with(|slot| {
        if let Some(ref wm) = *slot.borrow() {
            wm.requests_handled_total.fetch_add(1, Ordering::Relaxed);
            if let Some(dur) = duration_us {
                wm.record_duration(dur);
            }
        }
    });
}

/// Get the worker wait callback function pointer for registering with the bridge.
pub fn get_worker_wait_callback() -> Option<unsafe extern "C" fn() -> std::os::raw::c_int> {
    Some(worker_wait_callback)
}

/// Get the worker send callback function pointer for registering with the bridge.
pub fn get_worker_send_callback() -> Option<unsafe extern "C" fn() -> std::os::raw::c_int> {
    Some(worker_send_callback)
}

// ─── Non-Blocking Try-Recv for Fiber Scheduler ──────────────

/// Result of a non-blocking channel receive attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvResult {
    /// A request is available and has been stored in PENDING_REQUEST.
    Ready,
    /// No request available yet (channel empty).
    Empty,
    /// Channel is closed (worker should shut down).
    Disconnected,
}

/// Non-blocking check of the worker channel.
/// On success, stores the request in PENDING_REQUEST for later processing
/// via `prepare_received_request()`.
fn try_recv_inner() -> TryRecvResult {
    WORKER_RX.with(|slot| {
        let rx = slot.borrow();
        match rx.as_ref() {
            None => TryRecvResult::Disconnected,
            Some(rx) => match rx.try_recv() {
                Ok(req) => {
                    PENDING_REQUEST.with(|p| {
                        *p.borrow_mut() = Some(req);
                    });
                    TryRecvResult::Ready
                }
                Err(crossbeam_channel::TryRecvError::Empty) => TryRecvResult::Empty,
                Err(crossbeam_channel::TryRecvError::Disconnected) => TryRecvResult::Disconnected,
            },
        }
    })
}

/// Non-blocking try-recv callback for the fiber scheduler.
/// Returns: 0 = request ready (stored in PENDING_REQUEST),
///          1 = channel empty (no request available),
///         -1 = channel disconnected (shutdown).
///
/// # Safety
/// Called from C code via function pointer. Must only be called from a worker thread
/// with WORKER_RX set.
unsafe extern "C" fn worker_try_recv_callback() -> c_int {
    match try_recv_inner() {
        TryRecvResult::Ready => 0,
        TryRecvResult::Empty => 1,
        TryRecvResult::Disconnected => -1,
    }
}

/// Get the non-blocking try-recv callback function pointer.
pub fn get_worker_try_recv_callback() -> Option<unsafe extern "C" fn() -> c_int> {
    Some(worker_try_recv_callback)
}

/// Prepare a pending request from PENDING_REQUEST for execution.
/// Core TLS setup for a received request. Accepts the request by value to
/// avoid unnecessary round-trips through PENDING_REQUEST on the blocking path.
///
/// Called by both `worker_wait_callback` (direct) and `prepare_received_request`
/// (via PENDING_REQUEST staging slot for the non-blocking fiber path).
fn setup_request_tls(req: WorkerIncomingRequest) {
    // Single syscall for both last_active and request_time
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    // Update last_active timestamp for dynamic scaling
    WORKER_LAST_ACTIVE.with(|slot| {
        if let Some(ref la) = *slot.borrow() {
            la.store(now.as_millis() as u64, Ordering::Relaxed);
        }
    });

    // Clear previous response buffers
    clear_buffers();

    // Reset bridge TLS per-request fields
    unsafe { bindings::oxphp_bridge_reset_request_ctx() };

    // Set request_time BEFORE set_request_data so server vars can read it
    unsafe { bindings::oxphp_bridge_set_request_time(now.as_secs_f64()) };

    // Set up SAPI data for the new request
    set_request_data(&req.script);

    let start = Instant::now();
    set_early_tx(start, req.response_tx);

    // Store request start time for duration histogram
    WORKER_REQUEST_START.with(|slot| slot.set(Some(start)));

    // Increment soft_resets counter
    WORKER_METRICS_TLS.with(|slot| {
        if let Some(ref wm) = *slot.borrow() {
            wm.soft_resets_total.fetch_add(1, Ordering::Relaxed);
        }
    });

    // Set execution deadline
    if req.script.timeout_us > 0 {
        let now_us = now.as_micros() as i64;
        let deadline = now_us.saturating_add(req.script.timeout_us.min(i64::MAX as u64) as i64);
        unsafe { bindings::oxphp_bridge_set_deadline(deadline) };
    }
}

/// Takes a pending request from `PENDING_REQUEST` TLS (deposited by
/// `try_recv_inner`) and runs `setup_request_tls`. Used by the fiber
/// scheduler's non-blocking path.
///
/// Returns `true` if a pending request was found and prepared, `false` if
/// PENDING_REQUEST was empty.
fn prepare_received_request() -> bool {
    let incoming = PENDING_REQUEST.with(|p| p.borrow_mut().take());
    match incoming {
        Some(req) => {
            setup_request_tls(req);
            true
        }
        None => false,
    }
}

/// Prepare-received-request callback for the fiber scheduler.
/// Returns: 1 = request prepared successfully, 0 = nothing pending.
///
/// # Safety
/// Called from C code via function pointer. Must only be called from a worker thread
/// after a successful `worker_try_recv_callback()` call.
unsafe extern "C" fn prepare_received_request_callback() -> c_int {
    if prepare_received_request() {
        1
    } else {
        0
    }
}

/// Get the prepare-received-request callback function pointer.
pub fn get_prepare_received_request_callback() -> Option<unsafe extern "C" fn() -> c_int> {
    Some(prepare_received_request_callback)
}

// ─── Async Promise TLS Accessors ──────────────────────────────

pub fn next_promise_id() -> u64 {
    PROMISE_COUNTER.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1));
        id
    })
}

pub fn store_promise(
    id: u64,
    rx: tokio::sync::oneshot::Receiver<AsyncResult>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    PROMISE_MAP.with(|m| {
        m.borrow_mut().insert(id, (rx, cancelled));
    });
}

pub fn take_promise(
    id: u64,
) -> Option<(
    tokio::sync::oneshot::Receiver<AsyncResult>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
)> {
    PROMISE_MAP.with(|m| m.borrow_mut().remove(&id))
}

pub fn store_promise_cleanup(id: u64, cleanup: PromiseCleanup) {
    PROMISE_CLEANUP.with(|m| {
        m.borrow_mut().insert(id, cleanup);
    });
}

pub fn take_promise_cleanup(id: u64) -> Option<PromiseCleanup> {
    PROMISE_CLEANUP.with(|m| m.borrow_mut().remove(&id))
}

pub fn outstanding_promise_ids() -> Vec<u64> {
    let mut ids: std::collections::HashSet<u64> =
        PROMISE_MAP.with(|m| m.borrow().keys().copied().collect());
    // Also include IDs that only exist in PROMISE_CLEANUP (e.g., timed-out promises
    // where the rx was consumed but cleanup data remains).
    PROMISE_CLEANUP.with(|m| {
        for &id in m.borrow().keys() {
            ids.insert(id);
        }
    });
    ids.into_iter().collect()
}

pub fn set_async_tx(tx: crossbeam_channel::Sender<AsyncTask>) {
    ASYNC_TX.with(|slot| {
        *slot.borrow_mut() = Some(tx);
    });
}

pub fn get_async_tx() -> Option<crossbeam_channel::Sender<AsyncTask>> {
    ASYNC_TX.with(|slot| slot.borrow().clone())
}

pub fn set_is_async_worker(val: bool) {
    IS_ASYNC_WORKER.with(|c| c.set(val));
}

pub fn is_async_worker() -> bool {
    IS_ASYNC_WORKER.with(|c| c.get())
}

// ─── Non-Blocking Await Poll ──────────────────────────────────

/// Non-blocking check if a promise result is ready.
///
/// 1. Check `READY_RESULTS` — if already pre-fetched, return true.
/// 2. Check `PROMISE_MAP`:
///    - Remove entry to get ownership (avoids double-borrow).
///    - `try_recv()` on the oneshot receiver:
///      - `Ok(result)` → move to `READY_RESULTS`, return true.
///      - `TryRecvError::Empty` → put back into `PROMISE_MAP`, return false.
///      - `TryRecvError::Closed` → drop (task was cancelled/dropped), return false.
pub fn await_is_ready(promise_id: u64) -> bool {
    // Fast path: already pre-fetched
    let already = READY_RESULTS.with(|m| m.borrow().contains_key(&promise_id));
    if already {
        return true;
    }

    // Try to poll the oneshot receiver — must remove first to get ownership
    let entry = PROMISE_MAP.with(|m| m.borrow_mut().remove(&promise_id));
    match entry {
        Some((mut rx, cancelled)) => match rx.try_recv() {
            Ok(result) => {
                // Result is ready — store in READY_RESULTS for later take
                READY_RESULTS.with(|m| {
                    m.borrow_mut().insert(promise_id, result);
                });
                // Put cancelled flag back? No — the promise completed, we keep the
                // cancellation Arc in PROMISE_CLEANUP for cleanup_promise.
                // But we need to keep the cancelled flag accessible... actually no,
                // the rx is consumed so there's no need to re-store. The cancelled
                // flag is only relevant for the async worker (which already finished).
                // We do NOT re-insert into PROMISE_MAP since the result is consumed.
                true
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                // Not ready yet — put back
                PROMISE_MAP.with(|m| {
                    m.borrow_mut().insert(promise_id, (rx, cancelled));
                });
                false
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                // Sender dropped (task was cancelled or pool shut down)
                false
            }
        },
        None => false, // Unknown promise ID
    }
}

/// Remove and return a pre-fetched result from `READY_RESULTS`.
///
/// Called after `await_is_ready` returns true to consume the result.
pub fn take_ready_result(promise_id: u64) -> Option<AsyncResult> {
    READY_RESULTS.with(|m| m.borrow_mut().remove(&promise_id))
}

/// C-callable callback for non-blocking await poll.
/// Returns 1 if the promise result is ready, 0 if not.
///
/// # Safety
/// Called from C code via function pointer.
unsafe extern "C" fn await_poll_callback(promise_id: i64) -> c_int {
    if await_is_ready(promise_id as u64) {
        1
    } else {
        0
    }
}

/// Register the await poll callback with the C bridge.
pub fn register_await_poll_callback() {
    unsafe {
        crate::bridge::ffi::oxphp_bridge_set_await_poll(Some(await_poll_callback));
    }
}

/// Process-global async task sender — set once after the AsyncWorkerPool is started,
/// read by PHP worker threads to clone a per-thread sender without needing to pass
/// it through spawn_worker (workers are started before the pool exists).
static GLOBAL_ASYNC_TX: OnceLock<crossbeam_channel::Sender<crate::async_types::AsyncTask>> =
    OnceLock::new();

/// Set the global async task sender. Must be called at most once, after the pool starts.
pub fn set_global_async_tx(tx: crossbeam_channel::Sender<crate::async_types::AsyncTask>) {
    GLOBAL_ASYNC_TX
        .set(tx)
        .expect("GLOBAL_ASYNC_TX already set");
}

/// Get a reference to the global async task sender, if the pool is configured.
pub fn get_global_async_tx(
) -> Option<&'static crossbeam_channel::Sender<crate::async_types::AsyncTask>> {
    GLOBAL_ASYNC_TX.get()
}

// ─── Async Tokio Handle ──────────────────────────────────────

/// Process-global Tokio runtime handle for async promise await operations.
/// Set once in main.rs, read by PHP worker threads for `block_on()` in `await_dispatch_callback`.
static ASYNC_TOKIO_HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// Set the Tokio runtime handle for async await. Must be called once from main.rs.
pub fn set_async_tokio_handle(handle: tokio::runtime::Handle) {
    ASYNC_TOKIO_HANDLE
        .set(handle)
        .expect("ASYNC_TOKIO_HANDLE already set");
}

// ─── Async Metrics ──────────────────────────────────────────────

/// Process-global metrics handle for async task counters.
/// Set once in main.rs alongside async pool setup; read from dispatch/await callbacks.
static ASYNC_METRICS: OnceLock<Arc<crate::metrics::Metrics>> = OnceLock::new();

/// Set the metrics handle for async task tracking. Called once from main.rs.
pub fn set_async_metrics(metrics: Arc<crate::metrics::Metrics>) {
    ASYNC_METRICS.set(metrics).ok();
}

fn get_async_metrics() -> Option<&'static Arc<crate::metrics::Metrics>> {
    ASYNC_METRICS.get()
}

// ─── Async Dispatch Callbacks ──────────────────────────────────

/// Rust-side callback invoked from C when PHP calls `oxphp_async()`.
///
/// Freezes static variables, borrows objects, deep-copies arguments,
/// sends the task to the async worker pool, and returns a promise ID.
///
/// # Safety
/// Called from C FFI. All pointer arguments must be valid PHP zvals/op_arrays.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn async_dispatch_callback(
    op_array: *const c_void,
    static_vars: *mut c_void,
    this_ptr: *mut c_void,
    argc: u32,
    args: *mut c_void,
    closure_zval: *mut c_void,
) -> i64 {
    use crate::async_types::BorrowedZval;
    use crate::bridge::ffi;

    // 1. Get global async task sender
    let tx = match get_global_async_tx() {
        Some(tx) => tx,
        None => return -1,
    };

    // 2. Generate promise ID
    let promise_id = next_promise_id();

    // 3. Prepare cleanup tracker for freeze/borrow state
    let mut cleanup = PromiseCleanup::new();

    // Addref the closure object to prevent GC from freeing the op_array
    // while the async worker still holds a pointer to it. We store the
    // zend_object pointer (stable) rather than the zval pointer (stack-local
    // in PHP_FUNCTION, invalid after the C function returns).
    if !closure_zval.is_null() {
        let obj_ptr = ffi::oxphp_closure_addref(closure_zval);
        if !obj_ptr.is_null() {
            cleanup.closure_zval = obj_ptr;
        }
    }

    // Portable-serialize static_vars (closure use-vars) for safe cross-thread transfer.
    // All data crossing thread boundaries must be serialized — no pointer sharing.
    let (sv_buf, sv_len) = if !static_vars.is_null() {
        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let rc = ffi::oxphp_portable_serialize_ht(
            static_vars as *mut c_void,
            &mut out_buf,
            &mut out_len,
        );
        if rc != 0 || out_buf.is_null() {
            return -1;
        }
        (out_buf, out_len)
    } else {
        (std::ptr::null_mut(), 0usize)
    };

    // 4. Handle this_ptr borrowing
    if !this_ptr.is_null() {
        let zval_size = ffi::oxphp_zval_size();
        let mut original_data = [0u8; 16];
        let copy_len = zval_size.min(16);
        std::ptr::copy_nonoverlapping(this_ptr as *const u8, original_data.as_mut_ptr(), copy_len);
        ffi::oxphp_create_borrow_proxy(this_ptr, promise_id);
        cleanup.borrowed.push(BorrowedZval {
            proxy_zval_ptr: this_ptr,
            original_zval_data: original_data,
        });
    }

    // 5. Portable-serialize args (system malloc buffer, safe to cross threads)
    let (ser_buf, ser_len) = if argc > 0 && !args.is_null() {
        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let rc =
            ffi::oxphp_portable_serialize(args as *const c_void, argc, &mut out_buf, &mut out_len);
        if rc != 0 || out_buf.is_null() {
            return -1;
        }
        (out_buf, out_len)
    } else {
        (std::ptr::null_mut(), 0usize)
    };

    // 6. Create oneshot channel
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // 7. Build and send task
    let task = AsyncTask {
        promise_id,
        op_array,
        serialized_static_vars: sv_buf,
        serialized_static_vars_len: sv_len,
        this_ptr,
        argc,
        serialized_args: ser_buf,
        serialized_args_len: ser_len,
        cancelled: cancelled.clone(),
        result_tx,
    };

    match tx.try_send(task) {
        Ok(()) => {
            store_promise(promise_id, result_rx, cancelled);
            store_promise_cleanup(promise_id, cleanup);
            if let Some(m) = get_async_metrics() {
                m.async_task_dispatched();
            }
            promise_id as i64
        }
        Err(crossbeam_channel::TrySendError::Full(_))
        | Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
            // Rollback: cleanup borrowed (restore original zval bytes)
            for borrowed in &cleanup.borrowed {
                let zval_size = ffi::oxphp_zval_size();
                let copy_len = zval_size.min(16);
                std::ptr::copy_nonoverlapping(
                    borrowed.original_zval_data.as_ptr(),
                    borrowed.proxy_zval_ptr as *mut u8,
                    copy_len,
                );
            }
            // Cleanup frozen (unfreeze)
            for frozen in &cleanup.frozen {
                ffi::oxphp_unfreeze_zval(
                    frozen.zval_ptr,
                    frozen.orig_refcount,
                    frozen.orig_gc_flags,
                    frozen.orig_type_flags,
                );
            }
            // Free serialized buffers (system malloc'd — safe from any thread)
            if !ser_buf.is_null() {
                ffi::oxphp_portable_free(ser_buf);
            }
            if !sv_buf.is_null() {
                ffi::oxphp_portable_free(sv_buf);
            }
            // Release the closure object reference on rollback
            if !cleanup.closure_zval.is_null() {
                ffi::oxphp_closure_release(cleanup.closure_zval);
            }
            if let Some(m) = get_async_metrics() {
                m.async_task_rejected();
            }
            -1
        }
    }
}

/// Rust-side callback invoked from C when PHP calls `oxphp_async_await()`.
///
/// Blocks on the oneshot receiver until the async result arrives,
/// cleans up frozen/borrowed state, and copies the result into retval.
///
/// Returns: 0 = success (retval populated), -1 = error, -2 = timeout.
///
/// # Safety
/// Called from C FFI. `retval` must be a valid zval pointer.
pub unsafe extern "C" fn await_dispatch_callback(
    promise_id: i64,
    timeout: f64,
    retval: *mut c_void,
) -> c_int {
    use crate::bridge::ffi;
    use std::time::Duration;

    let id = promise_id as u64;

    // Fast path: check if result was pre-fetched by the scheduler's poll loop
    // (fiber-aware await resumes after the scheduler detects readiness via await_poll)
    if let Some(mut result) = take_ready_result(id) {
        cleanup_promise(id);

        if result.success {
            if !result.serialized_value.is_null() && result.serialized_value_len > 0 {
                let rc = ffi::oxphp_portable_deserialize(
                    result.serialized_value,
                    result.serialized_value_len,
                    1,
                    retval,
                );
                ffi::oxphp_portable_free(result.serialized_value);
                result.serialized_value = std::ptr::null_mut(); // prevent double-free in Drop
                if rc != 0 {
                    return -1;
                }
            }
            return 0;
        } else {
            // Store exception details in bridge TLS for the C extension
            if let (Some(cls), Some(msg)) = (&result.exception_class, &result.exception_message) {
                let cls_c = CString::new(cls.as_str()).unwrap_or_default();
                let msg_c = CString::new(msg.as_str()).unwrap_or_default();
                let trace_c = result
                    .exception_trace
                    .as_deref()
                    .map(|t| CString::new(t).unwrap_or_default());
                ffi::oxphp_bridge_set_async_exception(
                    cls_c.as_ptr(),
                    msg_c.as_ptr(),
                    trace_c
                        .as_ref()
                        .map(|c| c.as_ptr())
                        .unwrap_or(std::ptr::null()),
                );
            }
            return -1;
        }
    }

    // Take promise from map
    let (rx, cancelled) = match take_promise(id) {
        Some(p) => p,
        None => return -1,
    };

    // Block on result — use tokio::runtime::Handle::block_on for timeout support
    let mut result = if timeout > 0.0 {
        match ASYNC_TOKIO_HANDLE.get() {
            Some(handle) => {
                let dur = Duration::from_secs_f64(timeout);
                match handle.block_on(async { tokio::time::timeout(dur, rx).await }) {
                    Ok(Ok(r)) => r,
                    Ok(Err(_)) => {
                        // Channel closed — task was dropped
                        cleanup_promise(id);
                        return -1;
                    }
                    Err(_) => {
                        // Timeout — signal cancellation so the async worker stops.
                        // Note: rx was consumed by the timeout future, so we can't
                        // re-store it. The PROMISE_CLEANUP data remains and will be
                        // cleaned up by RSHUTDOWN (cleanup_outstanding_promises).
                        // The cancelled flag tells the worker to stop early.
                        cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                        return -2;
                    }
                }
            }
            None => {
                // No tokio handle — fall back to blocking recv
                match rx.blocking_recv() {
                    Ok(r) => r,
                    Err(_) => {
                        cleanup_promise(id);
                        return -1;
                    }
                }
            }
        }
    } else {
        // No timeout — blocking recv
        match rx.blocking_recv() {
            Ok(r) => r,
            Err(_) => {
                cleanup_promise(id);
                return -1;
            }
        }
    };

    // Cleanup frozen/borrowed state
    cleanup_promise(id);

    // Handle result
    if result.success {
        if !result.serialized_value.is_null() && result.serialized_value_len > 0 {
            // Deserialize the return value on THIS thread's heap (correct emalloc)
            let rc = ffi::oxphp_portable_deserialize(
                result.serialized_value,
                result.serialized_value_len,
                1, // single return value
                retval,
            );
            // Free the serialized buffer (system malloc'd) and null the pointer
            // to prevent double-free in AsyncResult::drop
            ffi::oxphp_portable_free(result.serialized_value);
            result.serialized_value = std::ptr::null_mut();
            if rc != 0 {
                return -1;
            }
        }
        0
    } else {
        // Store exception details in bridge TLS so the C extension can
        // create a proper exception with class/message/trace from the worker.
        if let (Some(cls), Some(msg)) = (&result.exception_class, &result.exception_message) {
            let cls_c = CString::new(cls.as_str()).unwrap_or_default();
            let msg_c = CString::new(msg.as_str()).unwrap_or_default();
            let trace_c = result
                .exception_trace
                .as_deref()
                .map(|t| CString::new(t).unwrap_or_default());
            ffi::oxphp_bridge_set_async_exception(
                cls_c.as_ptr(),
                msg_c.as_ptr(),
                trace_c
                    .as_ref()
                    .map(|c| c.as_ptr())
                    .unwrap_or(std::ptr::null()),
            );
        }
        -1
    }
}

/// Rust-side callback invoked from C when PHP calls `oxphp_async_await_any()`.
///
/// Races multiple promise receivers using `futures::select_all`, returning the
/// first to complete. Non-winning receivers are put back into PROMISE_MAP so
/// they can be awaited individually later via `oxphp_async_await()`.
///
/// Returns: 0 = success, -1 = error, -2 = timeout.
/// On success: `*out_winner_id` is the winning promise ID, `retval` is populated.
///
/// **Timeout behavior**: On timeout, all receivers are consumed by `select_all`
/// and cannot be recovered. The corresponding promises are cancelled and will be
/// cleaned up by RSHUTDOWN. Remaining promises cannot be awaited after timeout.
///
/// # Safety
/// Called from C FFI. `promise_ids` must point to `count` valid i64 values.
/// `out_winner_id` and `retval` must be valid writable pointers.
pub unsafe extern "C" fn await_any_dispatch_callback(
    promise_ids: *const i64,
    count: u32,
    timeout: f64,
    out_winner_id: *mut i64,
    retval: *mut c_void,
) -> c_int {
    use crate::bridge::ffi;
    use futures_util::future::select_all;
    use std::time::Duration;

    if count == 0 || promise_ids.is_null() {
        return -1;
    }

    // Collect promise IDs from the C array
    let ids: Vec<u64> = (0..count as usize)
        .map(|i| *promise_ids.add(i) as u64)
        .collect();

    // Take all receivers and cancelled flags from PROMISE_MAP.
    // Track (id, cancelled) in parallel vecs — select_all returns remaining
    // receivers in original order (minus winner), so index-based mapping works.
    let mut id_map: Vec<u64> = Vec::with_capacity(ids.len());
    let mut cancel_map: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>> =
        Vec::with_capacity(ids.len());
    let mut rxs: Vec<tokio::sync::oneshot::Receiver<AsyncResult>> = Vec::with_capacity(ids.len());

    for &id in &ids {
        if let Some((rx, cancelled)) = take_promise(id) {
            id_map.push(id);
            cancel_map.push(cancelled);
            rxs.push(rx);
        }
    }

    if rxs.is_empty() {
        return -1;
    }

    let handle = match ASYNC_TOKIO_HANDLE.get() {
        Some(h) => h,
        None => {
            // No tokio handle — put receivers back and fail
            for (rx, (id, cancelled)) in rxs.into_iter().zip(id_map.iter().zip(cancel_map.iter())) {
                store_promise(*id, rx, cancelled.clone());
            }
            return -1;
        }
    };

    // select_all races all receivers. oneshot::Receiver<T> is Unpin + Future,
    // so select_all operates directly — no Box::pin needed.
    // Returns (winner_output, winner_index, remaining_receivers).
    let race_result = if timeout > 0.0 {
        let dur = Duration::from_secs_f64(timeout);
        handle.block_on(async { tokio::time::timeout(dur, select_all(rxs)).await })
    } else {
        Ok(handle.block_on(select_all(rxs)))
    };

    match race_result {
        Ok((recv_result, winner_idx, remaining)) => {
            let winner_id = id_map[winner_idx];

            // Put remaining (non-winning) receivers back into PROMISE_MAP.
            // select_all returns remaining in original order with the winner removed.
            let mut remaining_iter = remaining.into_iter();
            for orig_idx in 0..id_map.len() {
                if orig_idx == winner_idx {
                    continue;
                }
                if let Some(rx) = remaining_iter.next() {
                    store_promise(id_map[orig_idx], rx, cancel_map[orig_idx].clone());
                }
            }

            // Handle the winning result
            let mut result = match recv_result {
                Ok(r) => r,
                Err(_) => {
                    // Channel closed — task was dropped
                    cleanup_promise(winner_id);
                    return -1;
                }
            };

            // Cleanup the winner's frozen/borrowed state
            cleanup_promise(winner_id);

            if result.success {
                *out_winner_id = winner_id as i64;
                if !result.serialized_value.is_null() && result.serialized_value_len > 0 {
                    let rc = ffi::oxphp_portable_deserialize(
                        result.serialized_value,
                        result.serialized_value_len,
                        1,
                        retval,
                    );
                    ffi::oxphp_portable_free(result.serialized_value);
                    result.serialized_value = std::ptr::null_mut();
                    if rc != 0 {
                        return -1;
                    }
                }
                0
            } else {
                if let (Some(cls), Some(msg)) = (&result.exception_class, &result.exception_message)
                {
                    let cls_c = CString::new(cls.as_str()).unwrap_or_default();
                    let msg_c = CString::new(msg.as_str()).unwrap_or_default();
                    let trace_c = result
                        .exception_trace
                        .as_deref()
                        .map(|t| CString::new(t).unwrap_or_default());
                    ffi::oxphp_bridge_set_async_exception(
                        cls_c.as_ptr(),
                        msg_c.as_ptr(),
                        trace_c
                            .as_ref()
                            .map(|c| c.as_ptr())
                            .unwrap_or(std::ptr::null()),
                    );
                }
                *out_winner_id = winner_id as i64;
                -1
            }
        }
        Err(_timeout) => {
            // Timeout — select_all consumed all receivers. Signal cancellation
            // so async workers stop early. RSHUTDOWN will clean up PromiseCleanup data.
            for cancel in &cancel_map {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            -2
        }
    }
}

/// Restore frozen and borrowed zvals for a completed/timed-out promise.
unsafe fn cleanup_promise(id: u64) {
    use crate::bridge::ffi;

    if let Some(cleanup) = take_promise_cleanup(id) {
        for frozen in &cleanup.frozen {
            ffi::oxphp_unfreeze_zval(
                frozen.zval_ptr,
                frozen.orig_refcount,
                frozen.orig_gc_flags,
                frozen.orig_type_flags,
            );
        }
        for borrowed in &cleanup.borrowed {
            let zval_size = ffi::oxphp_zval_size();
            let copy_len = zval_size.min(16);
            std::ptr::copy_nonoverlapping(
                borrowed.original_zval_data.as_ptr(),
                borrowed.proxy_zval_ptr as *mut u8,
                copy_len,
            );
        }
        // Release the closure object reference (prevents op_array from being
        // freed while the async worker holds a pointer to it).
        if !cleanup.closure_zval.is_null() {
            ffi::oxphp_closure_release(cleanup.closure_zval);
        }
    }
}

/// RSHUTDOWN / worker-mode callback: clean up any async promises that were
/// dispatched but never awaited by user code.  Safe to call when the map is
/// empty (returns immediately).
///
/// # Safety
/// Called from C FFI (RSHUTDOWN) or internally from `worker_send_callback`.
unsafe extern "C" fn cleanup_outstanding_promises_callback() {
    use crate::bridge::ffi;

    let ids = outstanding_promise_ids();
    if ids.is_empty() {
        return;
    }
    tracing::warn!(count = ids.len(), "Cleaning up non-awaited async promises");

    for id in ids {
        if let Some((rx, cancelled)) = take_promise(id) {
            // Signal cancellation
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
            // Block with 5-second timeout per promise to avoid indefinite hang
            if let Some(handle) = ASYNC_TOKIO_HANDLE.get() {
                let _ = handle.block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(5), rx).await
                });
            } else {
                // No handle available, just drop rx
                drop(rx);
            }
        }
        if let Some(cleanup) = take_promise_cleanup(id) {
            // Unfreeze frozen zvals
            for frozen in &cleanup.frozen {
                ffi::oxphp_unfreeze_zval(
                    frozen.zval_ptr,
                    frozen.orig_refcount,
                    frozen.orig_gc_flags,
                    frozen.orig_type_flags,
                );
            }
            // Restore borrowed zvals
            for borrowed in &cleanup.borrowed {
                let zval_size = ffi::oxphp_zval_size();
                let copy_len = zval_size.min(16);
                std::ptr::copy_nonoverlapping(
                    borrowed.original_zval_data.as_ptr(),
                    borrowed.proxy_zval_ptr as *mut u8,
                    copy_len,
                );
            }
        }
    }
}

/// Register all fiber scheduler callbacks with the C bridge.
/// Called once from main.rs before PHP workers start.
///
/// Registers: try_recv, prepare_request, timer service, await poll, fiber ctx save/restore.
pub fn register_fiber_callbacks() {
    unsafe {
        // Non-blocking channel receive + request preparation
        crate::bridge::ffi::oxphp_bridge_set_fiber_callbacks(
            Some(worker_try_recv_callback),
            Some(prepare_received_request_callback),
        );

        // Timer service for oxphp_sleep/oxphp_usleep
        crate::bridge::ffi::oxphp_bridge_set_timer_callbacks(
            Some(crate::php::fiber::timer_register_callback),
            Some(crate::php::fiber::timer_poll_callback),
            Some(crate::php::fiber::timer_remove_callback),
        );

        // Non-blocking await poll for fiber-aware oxphp_async_await
        crate::bridge::ffi::oxphp_bridge_set_await_poll(Some(await_poll_callback));

        // Per-fiber TLS save/restore/drop
        crate::bridge::ffi::oxphp_bridge_set_fiber_ctx_callbacks(
            Some(crate::php::fiber::fiber_save_ctx_callback),
            Some(crate::php::fiber::fiber_restore_ctx_callback),
            Some(crate::php::fiber::fiber_drop_ctx_callback),
        );
    }
}

/// Register the async dispatch callbacks with the C bridge.
///
/// This must be called after the async pool is started and before PHP workers
/// begin processing requests. It wires up the Rust dispatch functions so the
/// C extension's `oxphp_async()` and `oxphp_async_await()` can call into Rust.
pub fn register_async_callbacks() {
    unsafe {
        crate::bridge::ffi::oxphp_bridge_set_async_dispatch(Some(async_dispatch_callback));
        crate::bridge::ffi::oxphp_bridge_set_await_dispatch(Some(await_dispatch_callback));
        crate::bridge::ffi::oxphp_bridge_set_await_any_dispatch(Some(await_any_dispatch_callback));
        crate::bridge::ffi::oxphp_bridge_set_cleanup_promises(Some(
            cleanup_outstanding_promises_callback,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_type_str_known_types() {
        assert_eq!(error_type_str(1), ("error", "E_ERROR"));
        assert_eq!(error_type_str(2), ("warn", "E_WARNING"));
        assert_eq!(error_type_str(4), ("error", "E_PARSE"));
        assert_eq!(error_type_str(8), ("info", "E_NOTICE"));
        assert_eq!(error_type_str(16), ("error", "E_CORE_ERROR"));
        assert_eq!(error_type_str(32), ("warn", "E_CORE_WARNING"));
        assert_eq!(error_type_str(64), ("error", "E_COMPILE_ERROR"));
        assert_eq!(error_type_str(128), ("warn", "E_COMPILE_WARNING"));
        assert_eq!(error_type_str(256), ("error", "E_USER_ERROR"));
        assert_eq!(error_type_str(512), ("warn", "E_USER_WARNING"));
        assert_eq!(error_type_str(1024), ("info", "E_USER_NOTICE"));
        assert_eq!(error_type_str(2048), ("info", "E_STRICT"));
        assert_eq!(error_type_str(4096), ("error", "E_RECOVERABLE_ERROR"));
        assert_eq!(error_type_str(8192), ("info", "E_DEPRECATED"));
        assert_eq!(error_type_str(16384), ("info", "E_USER_DEPRECATED"));
    }

    #[test]
    fn error_type_str_unknown() {
        assert_eq!(error_type_str(0), ("warn", "E_UNKNOWN"));
        assert_eq!(error_type_str(-1), ("warn", "E_UNKNOWN"));
        assert_eq!(error_type_str(99999), ("warn", "E_UNKNOWN"));
    }

    #[test]
    fn error_type_str_bitmask_high_bits() {
        // PHP uncaught exceptions may set high bits (e.g. 0x1000001 = E_ERROR with flag)
        assert_eq!(error_type_str(0x1000001), ("error", "E_ERROR"));
        assert_eq!(error_type_str(0x1000002), ("warn", "E_WARNING"));
    }

    #[test]
    fn fatal_error_status_sets_500_from_default() {
        RESPONSE.with(|r| r.borrow_mut().status_code = 200);
        set_fatal_error_status_if_default();
        RESPONSE.with(|r| assert_eq!(r.borrow().status_code, 500));
    }

    #[test]
    fn fatal_error_status_idempotent() {
        RESPONSE.with(|r| r.borrow_mut().status_code = 200);
        set_fatal_error_status_if_default();
        set_fatal_error_status_if_default();
        RESPONSE.with(|r| assert_eq!(r.borrow().status_code, 500));
    }

    #[test]
    fn fatal_error_status_preserves_non_200() {
        RESPONSE.with(|r| r.borrow_mut().status_code = 404);
        set_fatal_error_status_if_default();
        RESPONSE.with(|r| assert_eq!(r.borrow().status_code, 404));
    }

    #[test]
    fn await_poll_returns_false_for_unknown_id() {
        // Unknown promise ID should return false
        assert!(!await_is_ready(99999));
    }

    #[test]
    fn await_poll_returns_true_when_result_sent() {
        let (tx, rx) = tokio::sync::oneshot::channel::<AsyncResult>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let promise_id = 42424242u64;

        // Store the promise
        store_promise(promise_id, rx, cancelled);

        // Before sending, should not be ready
        assert!(!await_is_ready(promise_id));

        // Send a result
        let result = AsyncResult {
            success: true,
            serialized_value: std::ptr::null_mut(),
            serialized_value_len: 0,
            exception_class: None,
            exception_message: None,
            exception_trace: None,
        };
        tx.send(result).unwrap();

        // Now it should be ready
        assert!(await_is_ready(promise_id));

        // Calling again should still return true (cached in READY_RESULTS)
        assert!(await_is_ready(promise_id));

        // Consume the result
        let taken = take_ready_result(promise_id);
        assert!(taken.is_some());
        assert!(taken.unwrap().success);

        // After take, should no longer be ready
        assert!(!await_is_ready(promise_id));
    }

    #[test]
    fn await_poll_returns_false_when_sender_dropped() {
        let (tx, rx) = tokio::sync::oneshot::channel::<AsyncResult>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let promise_id = 77777777u64;

        store_promise(promise_id, rx, cancelled);

        // Drop the sender without sending
        drop(tx);

        // Should return false (channel closed)
        assert!(!await_is_ready(promise_id));

        // Promise should be removed from map (consumed by try_recv)
        assert!(take_promise(promise_id).is_none());
    }

    #[test]
    fn try_recv_returns_disconnected_when_no_rx() {
        // Ensure WORKER_RX is None (default state for test threads)
        WORKER_RX.with(|slot| {
            *slot.borrow_mut() = None;
        });
        assert_eq!(try_recv_inner(), TryRecvResult::Disconnected);
    }

    #[test]
    fn try_recv_returns_empty_on_empty_channel() {
        let (tx, rx) = crossbeam_channel::bounded::<WorkerIncomingRequest>(8);
        WORKER_RX.with(|slot| {
            *slot.borrow_mut() = Some(rx);
        });

        // Channel is empty — should return Empty
        assert_eq!(try_recv_inner(), TryRecvResult::Empty);

        // Clean up: drop tx so channel closes, then clear WORKER_RX
        drop(tx);
        WORKER_RX.with(|slot| {
            *slot.borrow_mut() = None;
        });
    }
}
