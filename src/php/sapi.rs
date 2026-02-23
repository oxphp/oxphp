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

use crate::metrics::{WorkerMetrics, WorkerStats};
use crate::php::bindings::{self, *};
use crate::plugin::php::{PluginNativeFunction, PluginNativeFunctionDef};
use crate::types::{ScriptRequest, ScriptResponse};

/// Per-request response state consolidated in a single thread-local
/// to avoid 3 separate TLS lookups + RefCell borrows on the hot path.
struct ResponseBuffers {
    output: Vec<u8>,
    headers: Vec<(String, String)>,
    status_code: u16,
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
    static RESPONSE: RefCell<ResponseBuffers> = RefCell::new(ResponseBuffers::new());
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

/// Push a server variable, skipping entries with embedded null bytes.
#[inline]
fn push_server_var(vars: &mut Vec<(CString, CString)>, key: &str, val: &str) {
    if let (Ok(k), Ok(v)) = (CString::new(key), CString::new(val)) {
        vars.push((k, v));
    }
}

/// Store a oneshot sender for early response delivery.
/// Called from the worker thread before `execute_request()`.
pub fn set_early_tx(start: Instant, tx: oneshot::Sender<ScriptResponse>) {
    EARLY_TX.with(|slot| {
        *slot.borrow_mut() = Some((start, tx));
    });
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

        // Clear previous values but keep the Vec allocation
        data.server_vars.clear();

        let vars = &mut data.server_vars;

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
        push_server_var(vars, "SERVER_PROTOCOL", "HTTP/1.1");

        // SCRIPT_NAME: URI path without query string
        let path = req.uri.path();
        push_server_var(vars, "SCRIPT_NAME", path);
        push_server_var(vars, "PHP_SELF", path);

        // SCRIPT_FILENAME: absolute filesystem path to the script
        push_server_var(vars, "SCRIPT_FILENAME", &req.script_path.to_string_lossy());

        // DOCUMENT_ROOT
        push_server_var(vars, "DOCUMENT_ROOT", &req.document_root.to_string_lossy());

        // Server identification
        push_server_var(vars, "SERVER_SOFTWARE", "OxPHP/0.1.0");
        push_server_var(vars, "GATEWAY_INTERFACE", "CGI/1.1");

        // Connection info
        push_server_var(vars, "REMOTE_ADDR", &req.remote_addr.ip().to_string());
        push_server_var(vars, "REMOTE_PORT", &req.remote_addr.port().to_string());

        // SERVER_NAME and SERVER_PORT from Host header
        if let Some(host) = req.headers.get(header::HOST) {
            if let Ok(host_str) = host.to_str() {
                if let Some(colon) = host_str.rfind(':') {
                    push_server_var(vars, "SERVER_NAME", &host_str[..colon]);
                    push_server_var(vars, "SERVER_PORT", &host_str[colon + 1..]);
                } else {
                    push_server_var(vars, "SERVER_NAME", host_str);
                    push_server_var(vars, "SERVER_PORT", "80");
                }
            }
        } else {
            push_server_var(vars, "SERVER_NAME", "localhost");
            push_server_var(vars, "SERVER_PORT", "80");
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

        // Extract cookie string for read_cookies callback
        data.cookie_string = req
            .headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| CString::new(s).ok());

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
            push_server_var(&mut data.server_vars, &header_buf, val_str);
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
        phpinfo_as_text: 1,

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

unsafe extern "C" fn oxphp_send_headers(sapi_headers: *mut sapi_headers_struct) -> c_int {
    if !sapi_headers.is_null() {
        let code = (*sapi_headers).http_response_code;
        if code > 0 {
            RESPONSE.with(|r| r.borrow_mut().status_code = code as u16);
        }
    }
    1 // SAPI_HEADER_SENT_SUCCESSFULLY
}

// ─── Logging ────────────────────────────────────────────────

unsafe extern "C" fn oxphp_log_message(message: *const c_char, _syslog_type: c_int) {
    if message.is_null() {
        return;
    }
    let msg = std::ffi::CStr::from_ptr(message);
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
        *request_time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
    }
    0 // SUCCESS
}

// ─── Buffer Access ──────────────────────────────────────────

/// Take output, headers, and status code in a single TLS lookup + borrow.
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
            // Single syscall for both last_active and request_time (#6: avoid double syscall)
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

            // Reset bridge TLS per-request fields (request_id, deadline, cancelled, etc.)
            // before populating new request data.
            bindings::oxphp_bridge_reset_request_ctx();

            // Set up SAPI data for the new request — populates SG(request_info)
            // with method, query_string, content_type, content_length, and stores
            // server vars + cookie/body data in thread-local RequestData.
            // The C-side soft_reset reads cookies and POST data via SAPI callbacks
            // after this point.
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

            // Set request_time BEFORE superglobals are populated
            bindings::oxphp_bridge_set_request_time(now.as_secs_f64());

            // Set execution deadline
            if req.script.timeout_us > 0 {
                let now_us = now.as_micros() as i64;
                let deadline =
                    now_us.saturating_add(req.script.timeout_us.min(i64::MAX as u64) as i64);
                bindings::oxphp_bridge_set_deadline(deadline);
            }

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
}
