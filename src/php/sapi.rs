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

use crate::async_types::{AsyncResult, AsyncTask, CancelShared, PromiseCleanup};
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
    /// Queue-slot permit taken before the request entered the channel. Dropped
    /// when a worker picks the request up, which is what frees the slot for
    /// the next waiter — so it must not be held for the duration of script
    /// execution. Worker mode gets that for free (this struct dies inside
    /// `setup_request_tls`); the traditional loop drops it explicitly.
    pub permit: tokio::sync::OwnedSemaphorePermit,
    /// When the request stops being worth starting, or `None` in fail-fast
    /// mode. Set once on arrival from `QUEUE_WAIT_TIMEOUT_MS` and shared with
    /// the wait for a queue slot, so it bounds the whole time the request
    /// spends not executing rather than the admission half of it.
    pub deadline: Option<Instant>,
}

impl WorkerIncomingRequest {
    /// Whether this request sat in the queue past its budget.
    ///
    /// Checked at pickup, where refusing costs nothing and executing costs a
    /// worker: with a queue `worker_count × 128` deep, a pool serving 200 ms
    /// handlers takes some 25 seconds to reach the tail, and every request in
    /// it is answered later than any budget the operator set. Without this the
    /// budget would bound only the wait for a queue slot — the smaller of the
    /// two waits, and the one that is short precisely when the other is long.
    pub fn queue_budget_expired(&self) -> bool {
        self.deadline.is_some_and(|at| Instant::now() > at)
    }
}

thread_local! {
    pub(crate) static RESPONSE: RefCell<ResponseBuffers> = RefCell::new(ResponseBuffers::new());
    static REQUEST_DATA: RefCell<RequestData> = RefCell::new(RequestData::new());
    /// Holds the oneshot sender + request start time for early response delivery
    /// via `oxphp_finish_request()`. Set before script execution, consumed when early send triggers.
    static EARLY_TX: RefCell<Option<(Instant, oneshot::Sender<ScriptResponse>)>> = const { RefCell::new(None) };
    /// Strong Arc to the request's CancellationState held for the worker's
    /// view of the request. Set in setup_request_tls, cleared in
    /// worker_send_callback's terminal cleanup, so the bridge's raw pointer
    /// stays valid even if the tokio dispatch future is dropped early.
    static WORKER_CANCEL_STATE: RefCell<Option<std::sync::Arc<crate::bridge::cancel::CancellationState>>> = const { RefCell::new(None) };
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
    /// Worker mode: server-wide metrics, for refusals decided on this thread.
    static SERVER_METRICS: RefCell<Option<Arc<crate::metrics::Metrics>>> = const { RefCell::new(None) };
    /// Worker mode: request start time for duration histogram.
    static WORKER_REQUEST_START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// Sub-design A: whether &EG(vm_interrupt) has been captured for this worker thread.
    static VM_INTERRUPT_CAPTURED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Pending request from non-blocking try_recv, awaiting prepare_received_request().
    static PENDING_REQUEST: RefCell<Option<WorkerIncomingRequest>> = const { RefCell::new(None) };
    /// Worker mode: whether the current request started with profiling enabled
    /// (mode != Off at RINIT). Captured by `setup_request_tls` and read by
    /// `worker_send_callback` to drive the `do_finalize` formula — mirrors
    /// the `profiling_active` local in `traditional.rs` across the split
    /// RINIT/RSHUTDOWN callbacks that worker mode uses.
    static PROFILING_WAS_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Per-promise pending state: oneshot receiver paired with a cancellation flag.
type PromiseEntry = (
    tokio::sync::oneshot::Receiver<AsyncResult>,
    std::sync::Arc<CancelShared>,
);

/// A finished fiber's orphaned promise awaiting non-blocking drain. Moved out of
/// the promise maps by `cleanup_promises_for_fiber_callback` so the worker's
/// finalize path never blocks in `block_on`; the scheduler tick polls `rx` and,
/// once the task settles (or `deadline` passes), runs `cleanup` to release the
/// frozen/borrowed captures. Worker mode does not free request memory at the
/// handler boundary, so deferring the unfreeze past the response send is safe.
struct DeferredDrain {
    rx: tokio::sync::oneshot::Receiver<AsyncResult>,
    cleanup: Option<PromiseCleanup>,
    deadline: Instant,
}

thread_local! {
    /// Promise ID -> (oneshot receiver, cancellation flag). HTTP worker threads only.
    static PROMISE_MAP: RefCell<HashMap<u64, PromiseEntry>> = RefCell::new(HashMap::new());

    /// Promise ID -> fiber_id of the request fiber that created it (0 = created
    /// outside fiber context: traditional mode, CLI, worker boot code). The
    /// promise maps above are shared by every request fiber multiplexed on
    /// this worker thread, so the per-request cleanup must drain only the ids
    /// owned by the finishing fiber — draining thread-wide would cancel
    /// sibling fibers' live promises and stall their awaits to timeout.
    /// First-writer-wins: ownership is fixed at creation and never reassigned
    /// while the promise lives (a re-registration into CLEANUP/STRANDED runs on
    /// the same fiber anyway). Entries are removed on cleanup, and IDs are
    /// monotonic, so a live entry is never a stale record for a reused id.
    static PROMISE_OWNER: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());

    /// Per-thread monotonic promise ID counter.
    static PROMISE_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };

    /// Async task channel sender. Set once per HTTP/worker-mode thread.
    static ASYNC_TX: RefCell<Option<crossbeam_channel::Sender<AsyncTask>>>
        = const { RefCell::new(None) };

    /// Per-promise freeze/borrow cleanup state.
    static PROMISE_CLEANUP: RefCell<HashMap<u64, PromiseCleanup>>
        = RefCell::new(HashMap::new());

    /// Receivers for promises stranded by an `await_race` / `await_any`
    /// timeout. The original `select_all` / poll-loop future was dropped
    /// when the timeout fired, so these rxs are no longer in PROMISE_MAP
    /// — but the workers may still be running and still touching frozen
    /// captures. RSHUTDOWN drains this map first, block_on'ing each rx
    /// (5 s budget) before the matching PROMISE_CLEANUP entry unfreezes
    /// the zvals — closing the UAF window between cancel-flag signal and
    /// vm_interrupt observation.
    static PROMISE_STRANDED: RefCell<HashMap<u64, PromiseEntry>>
        = RefCell::new(HashMap::new());

    /// Orphaned promises of finished request fibers, awaiting non-blocking drain
    /// by the scheduler tick (see `DeferredDrain`). HTTP worker threads only.
    static DEFERRED_DRAIN: RefCell<Vec<DeferredDrain>> = const { RefCell::new(Vec::new()) };

    /// True on async worker threads, false on HTTP workers.
    static IS_ASYNC_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// Pre-fetched async results waiting to be consumed by `take_ready_result`.
    /// Populated by `await_is_ready` when a non-blocking poll finds a completed promise.
    static READY_RESULTS: RefCell<HashMap<u64, AsyncResult>>
        = RefCell::new(HashMap::new());
}

thread_local! {
    /// PHP errors captured during the current request's script execution.
    /// Drained into ScriptResponse at request completion; cleared at request boundaries.
    pub(crate) static REQUEST_ERRORS: RefCell<Vec<crate::types::PhpScriptError>> = const { RefCell::new(Vec::new()) };
}

/// Take all captured PHP errors for the current request, leaving the Vec empty.
pub fn take_request_errors() -> Vec<crate::types::PhpScriptError> {
    REQUEST_ERRORS.with(|errors| std::mem::take(&mut *errors.borrow_mut()))
}

/// Restore a request's captured PHP errors (the counterpart of
/// `take_request_errors`, used by the per-fiber TLS save/restore so a suspended
/// worker request's errors are not lost to — or mixed with — the errors of
/// another request that runs on the same worker thread while it is parked).
pub fn restore_request_errors(errors: Vec<crate::types::PhpScriptError>) {
    REQUEST_ERRORS.with(|slot| *slot.borrow_mut() = errors);
}

/// Take the streaming chunk sender out of TLS (per-fiber save). A streaming
/// worker request that suspends mid-stream must not leave `STREAM_TX` in the
/// thread-global slot: another fiber's request running on the same worker thread
/// would overwrite it, and this request's later chunks would then flow to the
/// other client's body channel (and vice versa).
pub fn take_stream_tx() -> Option<tokio::sync::mpsc::Sender<Bytes>> {
    STREAM_TX.with(|s| s.borrow_mut().take())
}

/// Restore a fiber's streaming chunk sender (counterpart of `take_stream_tx`).
pub fn restore_stream_tx(tx: Option<tokio::sync::mpsc::Sender<Bytes>>) {
    STREAM_TX.with(|s| *s.borrow_mut() = tx);
}

/// Pop a worker-mode unhandled-exception capture (set by the fiber catch site in
/// `oxphp_fiber.c`) into a synthetic terminal `PhpScriptError`. `None` when no
/// capture is pending. The `exception_class` field lets the downstream parser use
/// the real class without re-parsing a formatted message.
unsafe fn pop_worker_unhandled_exception() -> Option<crate::types::PhpScriptError> {
    let mut cls_len: usize = 0;
    let cls_ptr = bindings::oxphp_bridge_pop_unhandled_class(&mut cls_len);
    if cls_ptr.is_null() {
        return None;
    }

    // Copy a length-delimited C buffer into an owned String (lossy for non-UTF-8)
    // and free the C allocation. NULL/empty → None.
    let take = |ptr: *mut std::os::raw::c_char, len: usize| -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let s = if len == 0 {
            None
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
            Some(String::from_utf8_lossy(bytes).into_owned())
        };
        unsafe { libc::free(ptr as *mut std::os::raw::c_void) };
        s
    };

    let class = take(cls_ptr, cls_len).unwrap_or_default();
    let mut m_len: usize = 0;
    let message = take(
        bindings::oxphp_bridge_pop_unhandled_message(&mut m_len),
        m_len,
    );
    let mut t_len: usize = 0;
    let stacktrace = take(
        bindings::oxphp_bridge_pop_unhandled_trace(&mut t_len),
        t_len,
    );
    let mut f_len: usize = 0;
    let file = take(bindings::oxphp_bridge_pop_unhandled_file(&mut f_len), f_len);
    let line = bindings::oxphp_bridge_get_unhandled_line();

    Some(crate::types::PhpScriptError {
        level: "error",
        error_type: "E_ERROR",
        message: message.unwrap_or_default(),
        file: file.unwrap_or_default(),
        line,
        stacktrace,
        exception_class: Some(class),
    })
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
    /// Whether $_SERVER superglobal population is enabled for this request.
    sg_enabled: bool,

    // ── Object API: lazily accessed via bridge callbacks ──
    /// HTTP method string (e.g. "GET", "POST").
    method_str: String,
    /// URI path (e.g. "/users/42").
    path_str: String,
    /// Full URI (e.g. "https://example.com/users/42?page=2").
    full_uri_str: String,
    /// Scheme ("http" or "https").
    scheme_str: String,
    /// Host from Host header, or empty string.
    host_str: String,
    /// Port number (from Host header or scheme default).
    port_val: u16,
    /// Raw query string (without leading '?').
    query_string_raw: String,
    /// Remote address string (IP).
    remote_addr_str: String,
    /// HTTP protocol version (e.g. "1.1").
    protocol_version_str: String,
    /// Whether TLS is active.
    is_secure: bool,
    /// Raw headers as (lowercase_name, value) pairs.
    headers_raw: Vec<(String, String)>,
    /// Parsed cookies as (name, value) pairs.
    cookies_parsed: Vec<(String, String)>,
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
            sg_enabled: true,
            method_str: String::new(),
            path_str: String::new(),
            full_uri_str: String::new(),
            scheme_str: String::new(),
            host_str: String::new(),
            port_val: 0,
            query_string_raw: String::new(),
            remote_addr_str: String::new(),
            protocol_version_str: String::new(),
            is_secure: false,
            headers_raw: Vec::with_capacity(16),
            cookies_parsed: Vec::new(),
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

/// Parse a Host header value into (server_name, server_port).
/// Handles IPv6 literals per RFC 9110 §7.2 / RFC 3986 §3.2.2.
fn parse_host<'a>(host: &'a str, default_port: &'a str) -> (&'a str, &'a str) {
    if host.starts_with('[') {
        // IPv6 literal: find closing bracket, then optional :port
        if let Some(bracket_end) = host.find(']') {
            if host.get(bracket_end + 1..bracket_end + 2) == Some(":") {
                (&host[..bracket_end + 1], &host[bracket_end + 2..])
            } else {
                (host, default_port)
            }
        } else {
            (host, default_port)
        }
    } else if let Some(colon) = host.rfind(':') {
        (&host[..colon], &host[colon + 1..])
    } else {
        (host, default_port)
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

        // NOTE: Process environment variables are registered directly from the
        // static snapshot in oxphp_register_server_variables() (zero-clone path).

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

/// Populate `$_SERVER` for the one-shot CLI role (`oxphp run`). Mirrors the
/// variable set of PHP's own CLI SAPI: PHP_SELF / SCRIPT_NAME /
/// SCRIPT_FILENAME / PATH_TRANSLATED point at the script, DOCUMENT_ROOT is
/// empty. This script-identity skeleton is registered unconditionally; the
/// process-environment fold is gated by `sg_enabled` (the shared
/// `oxphp_register_server_variables` callback folds env only when set). The
/// flag is read back from the bridge — the single source of truth that the
/// CLI frontend already populated from `SUPERGLOBALS_ENABLED` before MINIT —
/// so the request side and the bridge always agree. `$argv` / `$argc` are NOT
/// set here, and are unaffected by the flag: PHP builds those from
/// `SG(request_info).argc/argv` during `php_request_startup`. Must be called
/// before `php_request_startup()`.
pub fn set_cli_request_data(script_path: &std::path::Path) {
    REQUEST_DATA.with(|rd| {
        let mut data = rd.borrow_mut();
        data.server_vars.clear();

        let script = script_path.to_string_lossy();
        let vars = &mut data.server_vars;
        push_server_var(vars, "PHP_SELF", &script);
        push_server_var(vars, "SCRIPT_NAME", &script);
        push_server_var(vars, "SCRIPT_FILENAME", &script);
        push_server_var(vars, "PATH_TRANSLATED", &script);
        push_server_var(vars, "DOCUMENT_ROOT", "");

        data.sg_enabled = unsafe { bindings::oxphp_bridge_get_superglobals_enabled() };
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

/// Take the worker's strong ref to the current request's CancellationState
/// (for per-fiber save). Parking the Arc in the fiber's TLS slot keeps the
/// cell the C side points at (`fiber->request_cancel_ptr`) alive across the
/// suspension: the per-thread slot below only ever protects the most recently
/// prepared request.
pub(crate) fn take_worker_cancel_state(
) -> Option<std::sync::Arc<crate::bridge::cancel::CancellationState>> {
    WORKER_CANCEL_STATE.with(|slot| slot.borrow_mut().take())
}

/// Restore the request's CancellationState strong ref into TLS (for per-fiber
/// restore).
pub(crate) fn restore_worker_cancel_state(
    val: Option<std::sync::Arc<crate::bridge::cancel::CancellationState>>,
) {
    WORKER_CANCEL_STATE.with(|slot| *slot.borrow_mut() = val);
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
                errors: take_request_errors(),
                profile_tree: None, // early response — spans not finished yet
                cancel_reason: unsafe { bindings::oxphp_bridge_get_cancel_reason() },
                refused: false,
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

            // Sync the status a script set via `http_response_code()` before it
            // started streaming — that call updates `SG(sapi_headers)` only, and
            // for a streaming response the headers go out now (there is no later
            // `collect_response_code()` at completion to pick it up). Without this
            // a streamed `http_response_code(500)` would ship as 200.
            collect_response_code();

            let (raw_output, raw_headers, status) = take_response();
            let body = Bytes::from(raw_output);
            let headers = parse_raw_headers(raw_headers);
            let _ = tx.send(ScriptResponse {
                status,
                headers,
                body,
                execution_time_us: start.elapsed().as_micros() as u64,
                stream_rx: Some(chunk_rx),
                // Streaming: errors accumulate on the worker thread for the rest
                // of the stream and are not delivered to the dispatch side — a
                // fatal thrown after these headers is a documented streaming
                // boundary, not attached to the root span.
                errors: Vec::new(),
                profile_tree: None, // streaming — spans not finished yet
                cancel_reason: 0,
                refused: false,
            });
            true
        } else {
            false
        }
    })
}

/// Drain the output buffer and send it as a chunk via STREAM_TX.
/// If `blocking_send` errors (receiver dropped → client disconnected),
/// mark cancellation + `PG(connection_status) |= PHP_CONNECTION_ABORTED`
/// so the next deadline check bails out and `connection_aborted()` returns true.
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
            // blocking_send: blocks if channel full (backpressure).
            // Err means the receiver was dropped — client gone. A None chunk
            // (empty flush) is not an error but must still see the drain check.
            let send_failed = match data {
                Some(chunk) => tx.blocking_send(chunk).is_err(),
                None => false,
            };
            unsafe {
                if send_failed {
                    if bindings::oxphp_bridge_get_cancel_reason() == 0 {
                        tracing::warn!("Stream client disconnected during flush");
                        let _ = bindings::oxphp_bridge_set_cancel_reason(1 /* CLIENT_ABORT */);
                        bindings::oxphp_bridge_request_interrupt();
                    }
                } else if bindings::oxphp_bridge_is_draining()
                    && bindings::oxphp_bridge_get_cancel_reason() == 0
                    && !bindings::oxphp_bridge_is_finished()
                {
                    // Server is draining: end the stream on the next opcode even
                    // if this flush carried no output (heartbeat comment, or the
                    // buffer was already drained). Catches tight flush loops that
                    // never suspend, so the fiber scheduler's drain sweep — which
                    // only reaches suspended fibers — cannot see them.
                    //
                    // The `is_finished` guard excludes the closing flush that
                    // oxphp_finish_request() performs on a streaming request:
                    // the response is complete at that point, and the script's
                    // post-response work is an ordinary request for drain
                    // purposes — it gets the window, not an immediate cancel.
                    let _ = bindings::oxphp_bridge_set_cancel_reason(3 /* SHUTDOWN */);
                    bindings::oxphp_bridge_request_interrupt();
                }
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

        let sg_enabled = unsafe { bindings::oxphp_bridge_get_superglobals_enabled() };
        data.sg_enabled = sg_enabled;

        // Clear previous values but keep the Vec allocation
        data.server_vars.clear();

        // Format remote IP once — reused for both server_vars and object API
        let remote_ip = req.remote_addr.ip().to_string();

        if !sg_enabled {
            // Skip server_vars population — object API fields below are always populated.
        } else {
            let vars = &mut data.server_vars;

            // NOTE: Process environment variables are no longer cloned here.
            // They are registered directly from the static snapshot in
            // oxphp_register_server_variables() (zero-clone path).

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

            // SCRIPT_NAME, PHP_SELF, PATH_INFO, DOCUMENT_URI
            if let Some(meta) = &req.denied_meta {
                // Deny-fallback: a different script runs than the URI requested.
                // SCRIPT_NAME must identify the fallback script (CGI contract),
                // not the attacker-requested URI. PATH_INFO carries the original
                // sanitized URI (with leading `/` per CGI/1.1 §4.1.6) so the
                // fallback can route on it. OXPHP_DENIED_* expose matcher
                // metadata for logging / SIEM integration.
                //
                // `fallback_script_uri` is precomputed at config load using
                // the canonical DOCUMENT_ROOT — deriving it here via
                // `strip_prefix(&req.document_root)` would be wrong because
                // the raw `DOCUMENT_ROOT` may differ from its canonical form
                // (e.g. `/tmp` vs `/private/tmp` on macOS, or any symlinked
                // deployment), causing a silent fallback to the attacker URI.
                let script_name = meta.fallback_script_uri.as_str();

                // `original_path` = `/` + sanitized URI. `meta.path` is stored
                // without the leading slash, so we prepend it once into a
                // pre-sized buffer instead of paying for `format!`'s
                // intermediate `Arguments` machinery.
                let mut original_path = String::with_capacity(meta.path.len() + 1);
                original_path.push('/');
                original_path.push_str(&meta.path);

                // `php_self` = `script_name` ++ `original_path`. Same trick.
                let mut php_self = String::with_capacity(script_name.len() + original_path.len());
                php_self.push_str(script_name);
                php_self.push_str(&original_path);

                push_server_var(vars, "SCRIPT_NAME", script_name);
                push_server_var(vars, "PHP_SELF", &php_self);
                push_server_var(vars, "DOCUMENT_URI", script_name);
                push_server_var(vars, "PATH_INFO", &original_path);
                push_server_var(vars, "OXPHP_DENIED_PATH", &original_path);
                push_server_var(vars, "OXPHP_DENIED_PATTERN", &meta.pattern);
            } else {
                // SCRIPT_NAME is the resolved script path relative to the
                // document root — both decoded, since `script_path` is built by
                // joining the *decoded* sanitized URI onto `document_root`. The
                // old code subtracted the decoded PATH_INFO length from the raw
                // percent-encoded URI, which produced an empty SCRIPT_NAME in
                // Framework mode and mis-sliced under percent-encoding.
                let script_name = req
                    .script_path
                    .strip_prefix(&*req.document_root)
                    .ok()
                    .map(|rel| {
                        let rel = rel.to_string_lossy();
                        let mut s = String::with_capacity(rel.len() + 1);
                        s.push('/');
                        // On Unix the path separator is already `/`, so push the
                        // borrowed slice directly — no extra scan/allocation on
                        // the hot path. Only Windows needs separator translation.
                        #[cfg(windows)]
                        s.push_str(&rel.replace('\\', "/"));
                        #[cfg(not(windows))]
                        s.push_str(&rel);
                        s
                    })
                    .unwrap_or_else(|| {
                        // Worker script configured outside the document root —
                        // fall back to its basename.
                        let base = req
                            .script_path
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let mut s = String::with_capacity(base.len() + 1);
                        s.push('/');
                        s.push_str(&base);
                        s
                    });

                push_server_var(vars, "SCRIPT_NAME", &script_name);
                push_server_var(vars, "DOCUMENT_URI", &script_name);

                if let Some(ref path_info) = req.path_info {
                    let mut php_self = String::with_capacity(script_name.len() + path_info.len());
                    php_self.push_str(&script_name);
                    php_self.push_str(path_info);
                    push_server_var(vars, "PHP_SELF", &php_self);
                    push_server_var(vars, "PATH_INFO", path_info);
                } else {
                    push_server_var(vars, "PHP_SELF", &script_name);
                }
            }

            // SCRIPT_FILENAME: absolute filesystem path to the script
            push_server_var(vars, "SCRIPT_FILENAME", &req.script_path.to_string_lossy());

            // DOCUMENT_ROOT
            push_server_var(vars, "DOCUMENT_ROOT", &req.document_root.to_string_lossy());

            // Server identification
            push_server_var(vars, "SERVER_SOFTWARE", SERVER_SOFTWARE);
            push_server_var(vars, "GATEWAY_INTERFACE", "CGI/1.1");

            // Connection info
            push_server_var(vars, "REMOTE_ADDR", &remote_ip);
            push_server_var(vars, "REMOTE_PORT", &req.remote_addr.port().to_string());

            // HTTPS indicator (CGI/1.1: "on" when TLS is active)
            // Check forwarded proto first, then direct TLS
            let effective_tls = req
                .forwarded_proto
                .as_deref()
                .map(|p| p.eq_ignore_ascii_case("https"))
                .unwrap_or(req.is_tls);

            if effective_tls {
                push_server_var(vars, "HTTPS", "on");
            }

            // REQUEST_SCHEME: "http" or "https" (PHP-FPM / nginx convention)
            push_server_var(
                vars,
                "REQUEST_SCHEME",
                if effective_tls { "https" } else { "http" },
            );

            // SERVER_NAME and SERVER_PORT. Port priority: X-Forwarded-Port >
            // port suffix of forwarded/Host header > scheme default.
            let default_port = if effective_tls { "443" } else { "80" };
            let fwd_port = req.forwarded_port.map(|p| p.to_string());
            if let Some(ref fwd_host) = req.forwarded_host {
                let (name, port) = parse_host(fwd_host, default_port);
                push_server_var(vars, "SERVER_NAME", name);
                push_server_var(vars, "SERVER_PORT", fwd_port.as_deref().unwrap_or(port));
            } else if let Some(host) = req.headers.get(header::HOST) {
                if let Ok(host_str) = host.to_str() {
                    let (name, port) = parse_host(host_str, default_port);
                    push_server_var(vars, "SERVER_NAME", name);
                    push_server_var(vars, "SERVER_PORT", fwd_port.as_deref().unwrap_or(port));
                }
            } else {
                push_server_var(vars, "SERVER_NAME", "localhost");
                push_server_var(
                    vars,
                    "SERVER_PORT",
                    fwd_port.as_deref().unwrap_or(default_port),
                );
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
        } // end if sg_enabled

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

        // ── Object API fields ──
        data.method_str.clear();
        data.method_str.push_str(req.method.as_str());

        data.path_str.clear();
        data.path_str.push_str(req.uri.path());

        // is_secure: forwarded proto takes priority
        let effective_tls = req
            .forwarded_proto
            .as_deref()
            .map(|p| p.eq_ignore_ascii_case("https"))
            .unwrap_or(req.is_tls);

        data.is_secure = effective_tls;
        data.scheme_str.clear();
        data.scheme_str
            .push_str(if effective_tls { "https" } else { "http" });

        // Parse host and port: forwarded host takes priority
        data.host_str.clear();
        data.port_val = if effective_tls { 443 } else { 80 };

        let host_source = req
            .forwarded_host
            .as_deref()
            .or_else(|| req.headers.get(header::HOST).and_then(|v| v.to_str().ok()));

        if let Some(host_val) = host_source {
            // Handle IPv6: [::1]:8080
            if let Some(bracket_end) = host_val.find(']') {
                data.host_str.push_str(&host_val[..=bracket_end]);
                if let Some(port_str) = host_val.get(bracket_end + 2..) {
                    if let Ok(p) = port_str.parse::<u16>() {
                        data.port_val = p;
                    }
                }
            } else if let Some(colon) = host_val.rfind(':') {
                data.host_str.push_str(&host_val[..colon]);
                if let Ok(p) = host_val[colon + 1..].parse::<u16>() {
                    data.port_val = p;
                }
            } else {
                data.host_str.push_str(host_val);
            }
        }

        // X-Forwarded-Port overrides any port derived from the host above.
        if let Some(p) = req.forwarded_port {
            data.port_val = p;
        }

        data.query_string_raw.clear();
        data.query_string_raw.push_str(&req.query_string);

        data.remote_addr_str.clear();
        data.remote_addr_str.push_str(&remote_ip);

        data.protocol_version_str.clear();
        data.protocol_version_str.push_str(match req.version {
            http::Version::HTTP_09 => "0.9",
            http::Version::HTTP_10 => "1.0",
            http::Version::HTTP_11 => "1.1",
            http::Version::HTTP_2 => "2",
            http::Version::HTTP_3 => "3",
            _ => "1.1",
        });

        // Build full URI: scheme://host[:port]/path[?query]
        {
            let scheme = if effective_tls { "https" } else { "http" };
            let port = data.port_val;
            let default_port: u16 = if effective_tls { 443 } else { 80 };
            let path = req.uri.path();
            let qs = &req.query_string;
            let port_part = if port != default_port {
                format!(":{port}")
            } else {
                String::new()
            };
            let qs_part = if qs.is_empty() {
                String::new()
            } else {
                format!("?{qs}")
            };
            data.full_uri_str = format!("{scheme}://{}{port_part}{path}{qs_part}", data.host_str);
        }

        // Store raw headers for object API iteration
        data.headers_raw.clear();
        for (name, value) in req.headers.iter() {
            if let Ok(v) = value.to_str() {
                data.headers_raw
                    .push((name.as_str().to_owned(), v.to_owned()));
            }
        }

        // Parse cookies from Cookie header
        data.cookies_parsed.clear();
        if let Some(cookie_hdr) = req.headers.get(header::COOKIE) {
            if let Ok(cookie_str) = cookie_hdr.to_str() {
                for pair in cookie_str.split(';') {
                    let pair = pair.trim();
                    if let Some(eq) = pair.find('=') {
                        data.cookies_parsed
                            .push((pair[..eq].to_owned(), pair[eq + 1..].to_owned()));
                    }
                }
            }
        }

        data.active = true;

        // Set request ID in bridge TLS so oxphp_request_id() returns it.
        let rid_cstr = CString::new(req.request_id.as_str()).unwrap_or_default();
        unsafe {
            bindings::oxphp_bridge_set_request_id(rid_cstr.as_ptr());
        }
        data.request_id_cstr = Some(rid_cstr);

        // Set SG(request_info) so PHP parses $_GET, $_POST, $_FILES, $_COOKIE.
        // This MUST happen before php_request_startup().
        // When superglobals disabled, still set method/content-type for php://input.
        let method_cstr_owned = CString::new(req.method.as_str()).ok();
        let method_cstr = if sg_enabled {
            data.server_vars
                .iter()
                .find(|(k, _)| k.as_bytes() == b"REQUEST_METHOD")
                .map(|(_, v)| v.as_ptr())
                .unwrap_or(std::ptr::null())
        } else {
            method_cstr_owned
                .as_ref()
                .map(|c| c.as_ptr())
                .unwrap_or(std::ptr::null())
        };

        let qs_ptr = if sg_enabled {
            data.query_string
                .as_ref()
                .map(|cs| cs.as_ptr())
                .unwrap_or(std::ptr::null())
        } else {
            std::ptr::null()
        };

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
        data.sg_enabled = true;

        // Clear object API fields (retain Vec capacity for reuse)
        data.method_str.clear();
        data.path_str.clear();
        data.full_uri_str.clear();
        data.scheme_str.clear();
        data.host_str.clear();
        data.port_val = 0;
        data.query_string_raw.clear();
        data.remote_addr_str.clear();
        data.protocol_version_str.clear();
        data.is_secure = false;
        data.headers_raw.clear();
        data.cookies_parsed.clear();
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
        // Cancellation state is reset by oxphp_bridge_reset_request_ctx()
        // (called earlier via clear_request_data).
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
        name: c"cli-server".as_ptr() as *mut c_char,
        pretty_name: c"OxPHP".as_ptr() as *mut c_char,

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
        #[cfg(php_v8_5)]
        pre_request_init: None,
    }
}

/// Build the CLI-personality SAPI module for the one-shot `oxphp run` role.
///
/// Mirrors PHP's own `sapi/cli/php_cli.c`: the SAPI name is `"cli"` (so
/// `PHP_SAPI === 'cli'` and `php_sapi_name()` returns `"cli"`), output goes
/// straight to stdout, errors to stderr, and header handling is a no-op
/// (there is no HTTP response). `register_server_variables` is shared with
/// the web module — the CLI variable set is staged via
/// [`set_cli_request_data`]. `$argv`/`$argc` come from
/// `SG(request_info).argc/argv` (see [`bindings::oxphp_bridge_set_cli_args`]).
///
/// `ini_entries` is a newline-separated `key=value` block (CLI defaults plus
/// any `-d` overrides); the pointer must remain valid for the process
/// lifetime. Unlike the web module this does NOT install the bridge
/// ub_write/flush trampolines — CLI output bypasses the per-request buffer.
pub fn build_cli_sapi_module(ini_entries: *const c_char) -> sapi_module_struct {
    sapi_module_struct {
        name: c"cli".as_ptr() as *mut c_char,
        pretty_name: c"OxPHP CLI".as_ptr() as *mut c_char,

        startup: Some(oxphp_startup),
        shutdown: Some(oxphp_shutdown),

        activate: Some(oxphp_activate),
        deactivate: Some(oxphp_deactivate),

        ub_write: Some(cli_ub_write),
        flush: Some(cli_flush),
        get_stat: None,
        getenv: None,

        sapi_error: zend_error as *mut c_void,

        // No HTTP response in CLI: headers are ignored.
        header_handler: None,
        send_headers: Some(cli_send_headers),
        send_header: None,

        // php://input from stdin is out of scope for the one-shot role.
        read_post: None,
        read_cookies: None,

        register_server_variables: Some(oxphp_register_server_variables),
        log_message: Some(cli_log_message),
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
        // php-cli sets this to 1 so phpinfo() emits plain text, not HTML
        // (sapi/cli/php_cli.c).
        phpinfo_as_text: 1,

        ini_entries,

        additional_functions: std::ptr::null(),
        input_filter_init: None,
        #[cfg(php_v8_5)]
        pre_request_init: None,
    }
}

// ─── CLI SAPI Callbacks (oxphp run) ──────────────────────────

/// Write `buf` to `fd`, returning the number of bytes actually written.
/// Retries on a signal interrupt (php-cli does the same — a SIGALRM or SIGCHLD
/// mid-write must not truncate output) and waits for writability on EAGAIN
/// (non-blocking fd); gives up on any other error or on a 0-length write.
unsafe fn write_all_fd(fd: c_int, buf: &[u8]) -> usize {
    let mut written = 0usize;
    while written < buf.len() {
        let n = libc::write(
            fd,
            buf.as_ptr().add(written) as *const std::ffi::c_void,
            buf.len() - written,
        );
        if n > 0 {
            written += n as usize;
            continue;
        }
        if n == 0 {
            break;
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EAGAIN) => {
                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                libc::poll(&mut pfd, 1, -1);
                continue;
            }
            _ => break,
        }
    }
    written
}

/// Write script output straight to stdout (fd 1). `write(2)` is unbuffered,
/// so `implicit_flush` semantics hold without an explicit flush.
unsafe extern "C" fn cli_ub_write(str: *const c_char, str_length: usize) -> usize {
    if str.is_null() || str_length == 0 {
        return 0;
    }
    let buf = std::slice::from_raw_parts(str as *const u8, str_length);
    write_all_fd(1, buf)
}

/// No-op: stdout writes via `write(2)` are not buffered at the libc level.
unsafe extern "C" fn cli_flush(_server_context: *mut c_void) {}

/// CLI has no HTTP headers; report them as sent so PHP proceeds.
unsafe extern "C" fn cli_send_headers(_sapi_headers: *mut sapi_headers_struct) -> c_int {
    1 // SAPI_HEADER_SENT_SUCCESSFULLY
}

/// Route PHP log/error messages to stderr (fd 2), matching php-cli.
unsafe extern "C" fn cli_log_message(message: *const c_char, _syslog_type: c_int) {
    if message.is_null() {
        return;
    }
    let msg = std::ffi::CStr::from_ptr(message);
    write_all_fd(2, msg.to_bytes());
    write_all_fd(2, b"\n");
}

// ─── SAPI Callbacks: Superglobals ────────────────────────────

/// Callback: register $_SERVER variables.
/// Called by PHP during request startup to populate $_SERVER.
///
/// Registers static env snapshot first (by reference — no per-request clone),
/// then per-request CGI/HTTP vars which override any env duplicates.
unsafe extern "C" fn oxphp_register_server_variables(track_vars_array: *mut c_void) {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if data.active {
            // Register static process environment vars first (zero clones).
            // Per-request CGI vars below will override any matching keys.
            if data.sg_enabled {
                for (key, value) in env_snapshot() {
                    php_register_variable_safe(
                        key.as_ptr(),
                        value.as_ptr(),
                        value.to_bytes().len(),
                        track_vars_array,
                    );
                }
            }

            // Register per-request CGI/HTTP variables (override env vars).
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
/// Sets the bridge cancellation flag so the C-level deadline check triggers bailout,
/// and marks `PG(connection_status) |= PHP_CONNECTION_ABORTED` so portable PHP
/// code using `connection_aborted()` can break out of streaming loops cleanly
/// before the next flush bailout. Only logs once per request.
///
/// Probes both channels:
/// - `EARLY_TX`: present until `send_streaming_headers()` consumes it (covers
///   pre-stream and non-streaming requests).
/// - `STREAM_TX`: present after streaming starts (covers SSE / chunked output).
unsafe fn check_client_disconnected() {
    if bindings::oxphp_bridge_get_cancel_reason() != 0 {
        return;
    }
    let disconnected = EARLY_TX
        .with(|slot| slot.borrow().as_ref().is_some_and(|(_, tx)| tx.is_closed()))
        || STREAM_TX.with(|slot| slot.borrow().as_ref().is_some_and(|tx| tx.is_closed()));
    if disconnected {
        tracing::warn!("Client disconnected, requesting PHP cancellation");
        let _ = bindings::oxphp_bridge_set_cancel_reason(1 /* CLIENT_ABORT */);
        bindings::oxphp_bridge_request_interrupt();
    }
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

// Return-value bit flags for sapi_module.header_handler (php-src/main/SAPI.h).
// SAPI_HEADER_ADD tells PHP to append the header into SG(sapi_headers).headers
// so builtins like headers_list() / apache_response_headers() can see it.
const SAPI_HEADER_ADD_TO_LIST: c_int = 1 << 0;

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
                0
            }
            sapi_header_op_enum::SAPI_HEADER_REPLACE | sapi_header_op_enum::SAPI_HEADER_ADD => {
                // PHP pre-removes prior occurrences for REPLACE in sapi_header_op() before
                // dispatching here, so both arms only decide append-or-not.
                let Some(colon_pos) = header_str.find(':') else {
                    // Malformed header (no colon): skip both our list and PHP's sapi_headers
                    // so headers_list() stays consistent with what goes on the wire.
                    return 0;
                };
                let name = header_str[..colon_pos].trim().to_string();
                let value = header_str[colon_pos + 1..].trim().to_string();

                // Auto-detect SSE: enable streaming when PHP sets Content-Type: text/event-stream
                if name.eq_ignore_ascii_case("content-type") && value.contains("text/event-stream")
                {
                    bindings::oxphp_bridge_set_stream_mode(true);
                }

                let mut resp = r.borrow_mut();
                if op == sapi_header_op_enum::SAPI_HEADER_REPLACE {
                    resp.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                }
                resp.headers.push((name, value));
                SAPI_HEADER_ADD_TO_LIST
            }
            #[cfg(php_v8_5)]
            sapi_header_op_enum::SAPI_HEADER_DELETE_PREFIX => {
                // Cold path: only invoked by an explicit two-arg
                // `header_remove($name, $prefix)` from PHP 8.5.6+.
                crate::php::header_match::delete_headers_with_prefix(
                    &mut r.borrow_mut().headers,
                    header_bytes,
                );
                0
            }
            _ => 0,
        }
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
    // Snapshot each thrown exception's class structurally so the traditional
    // path's "Uncaught …" fatal can report the true `exception.type` without
    // parsing it out of the (partly user-controlled) fatal text.
    crate::php::bindings::oxphp_bridge_install_throw_hook();
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
        // Zend's `zend_fcall_interrupt` checks `EG(timed_out)` BEFORE
        // dispatching to `zend_interrupt_function`, so SIGALRM-driven
        // `max_execution_time` reaches us as a plain `zend_error_noreturn`
        // with the canonical "Maximum execution time of N second(s) exceeded"
        // message — our oxphp_zend_interrupt_handler never sees it. Pattern-
        // match the message here so the cancel-reason mapping in
        // `set_fatal_error_status_if_default` can still surface 504 for the
        // PHP-native timeout path. The message format is stable across
        // PHP 8.4 / 8.5 (Zend/zend_execute_API.c::zend_timeout).
        if msg.starts_with("Maximum execution time of") {
            let _ = bindings::oxphp_bridge_set_cancel_reason(2 /* Timeout */);
        }
        set_fatal_error_status_if_default();

        // On async worker threads, capture the error message for exception propagation.
        // When an uncaught exception triggers zend_exception_error → zend_bailout,
        // EG(exception) is cleared before bailout. The task scheduler's
        // fatal-capture path (task_capture_fatal in oxphp_fiber.c) reads this
        // captured message to extract the original exception class and message.
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

    // For a traditional-path uncaught-exception fatal, read the class the throw
    // hook snapshotted structurally — the engine already nulled EG(exception)
    // before this callback ran, so the class can't be read live. This becomes the
    // authoritative `exception.type` downstream, overriding whatever the message
    // text would parse to (so a forged "\n\nNext …" segment cannot poison it).
    //
    // Gate on the exact type `E_ERROR`, not merely "error"-level + prefix: only
    // the engine's own `zend_exception_error(…, E_ERROR)` emits an uncaught-
    // exception fatal, and it is always preceded by this request's escaping throw.
    // `trigger_error()` can only raise E_USER_ERROR/WARNING/…, never E_ERROR — so
    // a forged `trigger_error('Uncaught FakeClass: …', E_USER_ERROR)` (which has no
    // corresponding throw) can no longer borrow the class of an earlier *caught*
    // throw. Consume the snapshot after copying so no later request can read it.
    let exception_class = if type_name == "E_ERROR" && msg.starts_with("Uncaught ") {
        let mut len = 0usize;
        let ptr = bindings::oxphp_bridge_peek_thrown_class(&mut len);
        let class = if !ptr.is_null() && len > 0 {
            let bytes = std::slice::from_raw_parts(ptr as *const u8, len);
            Some(String::from_utf8_lossy(bytes).into_owned())
        } else {
            None
        };
        bindings::oxphp_bridge_clear_thrown_class();
        class
    } else {
        None
    };

    // Capture error into REQUEST_ERRORS for inclusion in ScriptResponse.
    REQUEST_ERRORS.with(|errors| {
        errors.borrow_mut().push(crate::types::PhpScriptError {
            level,
            error_type: type_name,
            message: msg.to_string(),
            file: file.to_string(),
            line: error_lineno,
            stacktrace: None, // Stack trace capture will be added later
            exception_class,
        });
    });

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
    let cancel_reason = unsafe { bindings::oxphp_bridge_get_cancel_reason() };
    if let Some(status) = resolve_collected_status(code, cancel_reason) {
        RESPONSE.with(|r| r.borrow_mut().status_code = status);
    }
}

/// Decide whether PHP's `SG(sapi_headers).http_response_code` should overwrite
/// the status already on RESPONSE. `Some(status)` overwrites; `None` leaves it.
///
/// `sg_code` is PHP's authoritative status — whatever the script set via
/// `http_response_code()` or `header()`. An explicit non-200 is adopted even over
/// a fatal fallback (e.g. a 500 that `oxphp_error_cb` stamped onto RESPONSE
/// mid-request), mirroring PHP's own fatal handler, which substitutes 500 only
/// when the code is still 200 (`main/main.c`).
///
/// But a server-side cancellation wins over `sg_code`. When `cancel_reason != 0`,
/// `set_fatal_error_status_if_default` has already mapped the cause onto RESPONSE
/// (504 timeout, 503 shutdown, 499 client-abort). By that point `SG` cannot be
/// trusted: on the PHP-native `max_execution_time` path the fatal aborts with `SG`
/// still 200, so PHP's own handler substitutes a generic 500 into `SG`
/// (`main/main.c` — the `== 200` guard holds) — adopting it here would mask the
/// timeout as a crash; and a stale explicit non-200 the script set before the
/// cancel (e.g. a 206 that never completed) is equally a lie once the server cut
/// the request off. So a cancel reason makes RESPONSE authoritative and `SG` is
/// ignored. Only an un-cancelled request lets `SG` speak.
fn resolve_collected_status(sg_code: std::os::raw::c_int, cancel_reason: u8) -> Option<u16> {
    if cancel_reason != 0 {
        return None;
    }
    (sg_code > 0 && sg_code != 200).then_some(sg_code as u16)
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

/// Drop any output a background async task left in this worker thread's
/// RESPONSE buffer, releasing a large backing allocation. Returns the byte
/// count cleared. Safe only when no task is mid-write — the async driver calls
/// it when the worker is idle. A fire-and-forget task has no client, so any
/// bytes here are leaked output.
pub fn clear_response_output() -> usize {
    RESPONSE.with(|r| {
        let out = &mut r.borrow_mut().output;
        let n = out.len();
        out.clear();
        if out.capacity() > 65536 {
            out.shrink_to_fit();
        }
        n
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
    // Clear captured PHP errors from previous request. A streaming request's
    // errors accumulated here after the header-time snapshot are dropped, not
    // delivered — a documented streaming boundary (see `ScriptResponse::errors`).
    REQUEST_ERRORS.with(|errors| errors.borrow_mut().clear());
}

/// Set the response status if the current status is still the default 200.
///
/// Called from the fatal-error callback and from `execute_request` on bailout.
/// When a cancellation reason is recorded on the bridge, the status reflects
/// the cause: ClientAbort → 499 (nginx-style, log-only — connection is gone),
/// Timeout → 504, Shutdown → 503. Other reasons (None / Stuck / UserCancel)
/// fall through to the generic 500. The `status == 200` guard preserves any
/// status the userland set explicitly via `http_response_code()` before bailout.
pub fn set_fatal_error_status_if_default() {
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        if resp.status_code != 200 {
            return;
        }
        let reason = unsafe { bindings::oxphp_bridge_get_cancel_reason() };
        resp.status_code = match reason {
            1 => 499, // CancelReason::ClientAbort
            2 => 504, // CancelReason::Timeout
            3 => 503, // CancelReason::Shutdown
            // 0 None, 4 Stuck, 5 UserCancel — generic server error.
            _ => 500,
        };
    });
}

// ─── Native Plugin Function Bridge ──────────────────────────

use std::collections::HashMap;

/// Global registry of native plugin PHP function handlers, keyed by function name.
/// O(1) lookup on every dispatch instead of O(n) linear scan.
/// Set once from main.rs after plugin_manager.init_all().
static NATIVE_DISPATCH_MAP: OnceLock<HashMap<String, Box<dyn PluginNativeFunction>>> =
    OnceLock::new();

/// Builder-API function handlers (registered via ctx.function().handler()).
/// Separate from NATIVE_DISPATCH_MAP because builder functions are registered
/// after legacy functions, and OnceLock can only be set once.
static BUILDER_FN_DISPATCH_MAP: OnceLock<HashMap<String, Box<dyn PluginNativeFunction>>> =
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

    // Look up handler in legacy map first, then builder map
    let legacy_map = NATIVE_DISPATCH_MAP.get();
    let builder_map = BUILDER_FN_DISPATCH_MAP.get();

    let handler = legacy_map
        .and_then(|m| m.get(name_str))
        .or_else(|| builder_map.and_then(|m| m.get(name_str)));

    let handler = match handler {
        Some(h) => h,
        None => return -1,
    };

    // Catch panics — unwinding through extern "C" is an abort on Rust 2021.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut call = crate::bridge::call::NativeCall::new(args, argc, retval, None, None);
        handler.handle(&mut call)
    }));

    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            // Throw a PHP exception for PhpError::Exception, or a RuntimeException for others
            let (class, message, code) = match &e {
                crate::plugin::php::PhpError::Exception {
                    class,
                    message,
                    code,
                } => (class.as_str(), message.as_str(), *code),
                other => ("RuntimeException", &*other.to_string(), 0),
            };
            let cls_c = CString::new(class).unwrap_or_default();
            let msg_c = CString::new(message).unwrap_or_default();
            crate::bridge::ffi::oxphp_throw_exception(cls_c.as_ptr(), msg_c.as_ptr(), code);
            -1
        }
        Err(_) => {
            tracing::error!(func = name_str, "Plugin function panicked");
            let msg = CString::new("Internal error: plugin function panicked").unwrap();
            crate::bridge::ffi::oxphp_throw_exception(std::ptr::null(), msg.as_ptr(), 0);
            -1
        }
    }
}

// ─── PHP Definitions Registration ──────────────────────────

use crate::bridge::storage::{self, ClassMeta, CLASS_META};
use crate::plugin::builders::definitions::*;
use crate::plugin::types::{MagicMethod, PhpType, Visibility};

/// Method dispatch: class_index → method_name → handler.
static METHOD_DISPATCH_MAP: OnceLock<Vec<HashMap<String, Box<dyn PluginNativeFunction>>>> =
    OnceLock::new();

/// Magic dispatch: class_index → array of optional handlers.
type MagicFn = Box<
    dyn Fn(&mut crate::bridge::call::NativeCall) -> Result<(), crate::plugin::php::PhpError>
        + Send
        + Sync,
>;
static MAGIC_DISPATCH_MAP: OnceLock<Vec<[Option<MagicFn>; MagicMethod::COUNT]>> = OnceLock::new();

/// Dispatch callback for class method calls from C.
/// Routes to the correct Rust handler based on class_index + method_name.
///
/// # Safety
/// Called from C code.
unsafe extern "C" fn method_dispatch_callback(
    class_index: u32,
    method_name: *const c_char,
    args: *mut c_void,
    argc: u32,
    retval: *mut c_void,
    rust_data: *mut c_void,
    this_zval: *mut c_void,
) -> c_int {
    // Safety: method names are ASCII identifiers — UTF-8 validation is unnecessary overhead.
    let name_str = std::str::from_utf8_unchecked(CStr::from_ptr(method_name).to_bytes());

    let dispatch = match METHOD_DISPATCH_MAP.get() {
        Some(d) => d,
        None => return -1,
    };
    let class_methods = match dispatch.get(class_index as usize) {
        Some(m) => m,
        None => return -1,
    };
    let handler = match class_methods.get(name_str) {
        Some(h) => h,
        None => return -1,
    };

    let rust_data_opt = if rust_data.is_null() {
        None
    } else {
        Some(rust_data)
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut call = crate::bridge::call::NativeCall::new_with_this(
            args,
            argc,
            retval,
            Some(class_index as u64),
            rust_data_opt,
            this_zval,
        );
        handler.handle(&mut call)
    }));

    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            tracing::warn!(class_index, method = name_str, error = %e, "Plugin method error");
            // Mirror native-function dispatch: throw a PHP exception so the
            // caller's catch block sees it instead of the generic Error from
            // the C-side `if (rc != 0 && !EG(exception))` fallback.
            if unsafe { crate::bridge::ffi::oxphp_exception_pending() } == 0 {
                let (class, message, code) = match &e {
                    crate::plugin::php::PhpError::Exception {
                        class,
                        message,
                        code,
                    } => (class.as_str(), message.as_str(), *code),
                    other => ("RuntimeException", &*other.to_string(), 0),
                };
                let cls_c = CString::new(class).unwrap_or_default();
                let msg_c = CString::new(message).unwrap_or_default();
                unsafe {
                    crate::bridge::ffi::oxphp_throw_exception(cls_c.as_ptr(), msg_c.as_ptr(), code);
                }
            }
            -1
        }
        Err(_) => {
            tracing::error!(class_index, method = name_str, "Plugin method panicked");
            if unsafe { crate::bridge::ffi::oxphp_exception_pending() } == 0 {
                let msg = CString::new("Internal error: plugin method panicked").unwrap();
                unsafe {
                    crate::bridge::ffi::oxphp_throw_exception(std::ptr::null(), msg.as_ptr(), 0);
                }
            }
            -1
        }
    }
}

/// Register all PHP entity definitions from plugins on the C bridge.
/// Called from main.rs after plugin_manager.init_all(), before executor creation.
pub fn register_php_definitions(defs: PhpDefinitions) {
    let PhpDefinitions {
        classes,
        interfaces,
        enums,
        attributes,
        functions,
    } = defs;

    // 1. Register interfaces (classes may implement them)
    for iface in &interfaces {
        let fqn = CString::new(iface.fqn.as_str()).unwrap();
        let parent = iface.parent.as_deref().map(|s| CString::new(s).unwrap());
        let parent_ptr = parent
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());
        unsafe {
            let handle =
                crate::bridge::ffi::oxphp_bridge_register_interface(fqn.as_ptr(), parent_ptr);
            for method in &iface.methods {
                let mname = CString::new(method.name.as_str()).unwrap();
                let (rt, rn) = php_type_to_bridge(&method.return_type);
                let pa = build_param_arrays(&method.params);
                crate::bridge::ffi::oxphp_bridge_interface_add_method(
                    handle,
                    mname.as_ptr(),
                    method_modifiers_to_zend(method.modifiers),
                    method.required_params() as c_int,
                    method.total_params() as c_int,
                    method.is_variadic as c_int,
                    rt,
                    rn,
                    pa.name_ptrs.as_ptr(),
                    pa.types.as_ptr(),
                    pa.optional.as_ptr(),
                );
            }
            for constant in &iface.constants {
                let cname = CString::new(constant.name.as_str()).unwrap();
                let cval = CString::new(constant.value.to_string()).unwrap();
                crate::bridge::ffi::oxphp_bridge_interface_add_constant(
                    handle,
                    cname.as_ptr(),
                    visibility_to_zend(constant.visibility),
                    cval.as_ptr(),
                );
            }
        }
    }

    // 2. Register attributes
    for attr in &attributes {
        let fqn = CString::new(attr.fqn.as_str()).unwrap();
        unsafe {
            let handle = crate::bridge::ffi::oxphp_bridge_register_attribute(
                fqn.as_ptr(),
                attr.targets,
                attr.repeatable as c_int,
            );
            for param in &attr.params {
                let pname = CString::new(param.name.as_str()).unwrap();
                let default = param
                    .default
                    .as_ref()
                    .map(|d| CString::new(d.to_string()).unwrap());
                let default_ptr = default
                    .as_ref()
                    .map(|c| c.as_ptr())
                    .unwrap_or(std::ptr::null());
                crate::bridge::ffi::oxphp_bridge_attribute_add_param(
                    handle,
                    pname.as_ptr(),
                    -1,
                    param.required as c_int,
                    default_ptr,
                );
            }
        }
    }

    // 3. Register enums
    for enum_def in &enums {
        let fqn = CString::new(enum_def.fqn.as_str()).unwrap();
        let backing = match &enum_def.backing_type {
            None => 0,
            Some(crate::plugin::types::PhpType::Int) => 4, // IS_LONG
            Some(crate::plugin::types::PhpType::String) => 6, // IS_STRING
            _ => 0,
        };
        unsafe {
            let handle = crate::bridge::ffi::oxphp_bridge_register_enum(fqn.as_ptr(), backing);
            for iface_fqn in &enum_def.interfaces {
                let ifqn = CString::new(iface_fqn.as_str()).unwrap();
                crate::bridge::ffi::oxphp_bridge_enum_implements(handle, ifqn.as_ptr());
            }
            for case in &enum_def.cases {
                let cname = CString::new(case.name.as_str()).unwrap();
                let cval = case
                    .value
                    .as_ref()
                    .map(|v| CString::new(v.to_string()).unwrap());
                let cval_ptr = cval
                    .as_ref()
                    .map(|c| c.as_ptr())
                    .unwrap_or(std::ptr::null());
                crate::bridge::ffi::oxphp_bridge_enum_add_case(handle, cname.as_ptr(), cval_ptr);
            }
            for method in &enum_def.methods {
                let mname = CString::new(method.name.as_str()).unwrap();
                let (rt, rn) = php_type_to_bridge(&method.return_type);
                let pa = build_param_arrays(&method.params);
                crate::bridge::ffi::oxphp_bridge_enum_add_method(
                    handle,
                    mname.as_ptr(),
                    method_modifiers_to_zend(method.modifiers),
                    method.required_params() as c_int,
                    method.total_params() as c_int,
                    method.is_variadic as c_int,
                    rt,
                    rn,
                    pa.name_ptrs.as_ptr(),
                    pa.types.as_ptr(),
                    pa.optional.as_ptr(),
                );
            }
        }
    }

    // 4. Register classes (topologically sorted)
    let class_order = topological_sort_classes(&classes)
        .expect("Circular class inheritance in plugin definitions");

    let mut method_dispatch: Vec<HashMap<String, Box<dyn PluginNativeFunction>>> = Vec::new();
    let mut magic_dispatch: Vec<[Option<MagicFn>; MagicMethod::COUNT]> = Vec::new();
    let mut class_metas: Vec<ClassMeta> = Vec::new();

    // Consume classes in topological order using Option-wrapped Vec.
    let mut classes_vec: Vec<Option<PhpClassDef>> = classes.into_iter().map(Some).collect();

    for &idx in &class_order {
        let class = classes_vec[idx].take().unwrap();
        let fqn = CString::new(class.fqn.as_str()).unwrap();
        let parent = class.parent.as_deref().map(|s| CString::new(s).unwrap());
        let parent_ptr = parent
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());
        // Translate plugin-builder `Modifiers` bits into Zend `ce_flags`
        // bits. The C side ORs `flags` directly into `cls_ce->ce_flags`,
        // so the wire value must use Zend's encoding, not ours. Notably
        // Modifiers::FINAL is 0x02 which collides with ZEND_ACC_TRAIT —
        // forwarding the raw bits would mark every `.final_()` class as
        // a trait and break instantiation with "Cannot instantiate trait".
        let mods = class.modifiers;
        let mut flags: u32 = 0;
        // ZEND_ACC_FINAL = 1 << 5 (zend_compile.h).
        if mods.contains(crate::plugin::types::Modifiers::FINAL) {
            flags |= 1 << 5;
        }
        // ZEND_ACC_EXPLICIT_ABSTRACT_CLASS = 1 << 6.
        if mods.contains(crate::plugin::types::Modifiers::ABSTRACT) {
            flags |= 1 << 6;
        }
        // ZEND_ACC_READONLY_CLASS = 1 << 16 (PHP 8.2+).
        if mods.contains(crate::plugin::types::Modifiers::READONLY) {
            flags |= 1 << 16;
        }
        // STATIC is meaningless on a class; ignore.

        unsafe {
            let handle =
                crate::bridge::ffi::oxphp_bridge_register_class(fqn.as_ptr(), parent_ptr, flags);

            // Interfaces
            for iface_fqn in &class.interfaces {
                let ifqn = CString::new(iface_fqn.as_str()).unwrap();
                crate::bridge::ffi::oxphp_bridge_class_implements(handle, ifqn.as_ptr());
            }

            // Properties
            for prop in &class.properties {
                let pname = CString::new(prop.name.as_str()).unwrap();
                let default = prop
                    .default
                    .as_ref()
                    .map(|d| CString::new(d.to_string()).unwrap());
                let default_ptr = default
                    .as_ref()
                    .map(|c| c.as_ptr())
                    .unwrap_or(std::ptr::null());
                crate::bridge::ffi::oxphp_bridge_class_add_property(
                    handle,
                    pname.as_ptr(),
                    visibility_to_zend(prop.visibility),
                    prop.modifiers.bits() as u32,
                    -1, // type_info (complex types handled separately)
                    default_ptr,
                );
            }

            // Constants
            for constant in &class.constants {
                let cname = CString::new(constant.name.as_str()).unwrap();
                let cval = CString::new(constant.value.to_string()).unwrap();
                crate::bridge::ffi::oxphp_bridge_class_add_constant(
                    handle,
                    cname.as_ptr(),
                    visibility_to_zend(constant.visibility),
                    cval.as_ptr(),
                );
            }

            // Methods
            for method in &class.methods {
                let mname = CString::new(method.name.as_str()).unwrap();
                let (rt, rn) = php_type_to_bridge(&method.return_type);
                let pa = build_param_arrays(&method.params);
                crate::bridge::ffi::oxphp_bridge_class_add_method(
                    handle,
                    mname.as_ptr(),
                    visibility_to_zend(method.visibility),
                    method_modifiers_to_zend(method.modifiers),
                    method.required_params() as c_int,
                    method.total_params() as c_int,
                    method.is_variadic as c_int,
                    rt,
                    rn,
                    pa.name_ptrs.as_ptr(),
                    pa.types.as_ptr(),
                    pa.optional.as_ptr(),
                );
            }

            // Magic methods — register as real PHP methods on the class so
            // `zend_do_link_class` caches them into `ce->clone` / `ce->__get` /
            // etc. during class finalization. Without the add_method step,
            // `clone $obj` runs only the `clone_obj` handler and never calls
            // the user's `__clone` magic handler, which is why
            // `test_map_forbidden_clone` failed silently before this change.
            //
            // The legacy `set_magic` flag is kept for now in case any
            // C-side code still reads it, but the dispatch now goes
            // through the regular method path.
            for i in 0..MagicMethod::COUNT {
                if class.magic_handlers[i].is_some() {
                    crate::bridge::ffi::oxphp_bridge_class_set_magic(handle, i as c_int, 1);
                    let magic =
                        MagicMethod::from_index(i).expect("MagicMethod::from_index out of range");
                    let (req, total) = magic.arity();
                    // Arity-> zend_function_entry gap: the C side installs
                    // every method with `num_args = 0` and a variadic
                    // arginfo, because the dispatch hop into Rust doesn't
                    // care about typed slots. PHP's magic-method validator
                    // (`zend_check_magic_method_implementation`) disagrees —
                    // it trips a MINIT-time fatal like "Method ::__get()
                    // must take exactly 1 argument". Until the C side
                    // gains per-method arg_info, skip non-zero-arity
                    // magics and fall back to the legacy `set_magic` flag
                    // path (currently a no-op observer). `__clone` and
                    // other 0-arity magics go through the full add_method
                    // path so `zend_do_link_class` caches them into the
                    // class entry (required for `clone $obj` to invoke
                    // the user handler).
                    if (req, total) != (0, 0) {
                        continue;
                    }
                    let mname = CString::new(magic.php_name()).unwrap();
                    let (ret_tag, ret_nullable) = magic.return_tag();
                    crate::bridge::ffi::oxphp_bridge_class_add_method(
                        handle,
                        mname.as_ptr(),
                        visibility_to_zend(Visibility::Public),
                        0,
                        0,
                        0,
                        0,
                        ret_tag,
                        if ret_nullable { 1 } else { 0 },
                        std::ptr::null(),
                        std::ptr::null(),
                        std::ptr::null(),
                    );
                }
            }

            // Custom object storage
            if class.has_custom_storage {
                crate::bridge::ffi::oxphp_bridge_class_enable_custom_object(handle);
            }
        }

        // Build method dispatch map for this class.
        let mut methods_map: HashMap<String, Box<dyn PluginNativeFunction>> = HashMap::new();
        for method in class.methods {
            if let Some(handler) = method.handler {
                methods_map.insert(method.name, handler);
            }
        }
        // Magic handlers share the same dispatch table — keyed by their PHP
        // name (`__clone`, `__toString`, …) so a single
        // `oxphp_method_dispatch` callback covers both explicit methods and
        // magic methods registered via `.magic(...)`. `MagicHandler` is a
        // `Box<dyn Fn(...)>`, which does not unsize-coerce to
        // `Box<dyn PluginNativeFunction>`, so re-box through a closure that
        // forwards the call — the blanket `Fn -> PluginNativeFunction` impl
        // then kicks in.
        let mut magic_handlers = class.magic_handlers;
        for (i, slot) in magic_handlers.iter_mut().enumerate() {
            if let Some(handler) = slot.take() {
                let magic =
                    MagicMethod::from_index(i).expect("MagicMethod::from_index out of range");
                // Mirror the add_method skip above: only 0-arity magics
                // are registered on the class right now, so only their
                // handlers need to live in the dispatch map.
                if magic.arity() != (0, 0) {
                    continue;
                }
                let wrapped: Box<dyn PluginNativeFunction> =
                    Box::new(move |call: &mut crate::bridge::call::NativeCall| handler(call));
                methods_map.insert(magic.php_name().to_string(), wrapped);
            }
        }
        method_dispatch.push(methods_map);

        // Magic dispatch map is kept empty — magic handlers live in the
        // method dispatch map above. The array is still populated so the
        // per-class indexing assumed elsewhere stays in sync.
        magic_dispatch.push(magic_handlers);

        // Build class meta for storage.
        if class.has_custom_storage {
            class_metas.push(ClassMeta {
                fqn: class.fqn,
                factory: class
                    .storage_factory
                    .unwrap_or_else(|| Box::new(std::ptr::null_mut)),
                drop_fn: class.storage_drop.unwrap_or_else(|| Box::new(|_| {})),
                clone_fn: class.storage_clone,
            });
        } else {
            // Even classes without storage need an entry to keep indices aligned.
            class_metas.push(ClassMeta {
                fqn: class.fqn,
                factory: Box::new(std::ptr::null_mut),
                drop_fn: Box::new(|_| {}),
                clone_fn: None,
            });
        }
    }

    // 5. Register functions (bridge + dispatch map)
    let fn_count = functions.len();
    let mut fn_dispatch: HashMap<String, Box<dyn PluginNativeFunction>> = HashMap::new();
    for func in functions {
        let fqn = CString::new(func.fqn.as_str()).unwrap();
        let (rt, rn) = php_type_to_bridge(&func.return_type);
        let pa = build_param_arrays(&func.params);
        unsafe {
            crate::bridge::ffi::oxphp_bridge_register_plugin_function(
                fqn.as_ptr(),
                func.required_params() as c_int,
                func.total_params() as c_int,
                func.is_variadic as c_int,
                rt,
                rn,
                pa.name_ptrs.as_ptr(),
                pa.types.as_ptr(),
                pa.optional.as_ptr(),
            );
        }
        if let Some(handler) = func.handler {
            fn_dispatch.insert(func.fqn, handler);
        }
    }
    if !fn_dispatch.is_empty() {
        // Ensure native dispatch callback is set (may already be set by legacy path)
        unsafe {
            crate::bridge::ffi::oxphp_bridge_set_native_dispatch(Some(native_dispatch_callback));
        }
        BUILDER_FN_DISPATCH_MAP.set(fn_dispatch).ok();
    }

    // 6. Set dispatch callbacks
    unsafe {
        crate::bridge::ffi::oxphp_bridge_set_method_dispatch(Some(method_dispatch_callback));
        crate::bridge::ffi::oxphp_bridge_set_storage_callbacks(
            Some(storage::storage_create_callback),
            Some(storage::storage_drop_callback),
            Some(storage::storage_clone_callback),
        );
    }

    // 7. Populate static dispatch maps
    METHOD_DISPATCH_MAP.set(method_dispatch).ok();
    MAGIC_DISPATCH_MAP.set(magic_dispatch).ok();
    CLASS_META.set(class_metas).ok();

    let total = interfaces.len() + attributes.len() + enums.len() + class_order.len() + fn_count;
    tracing::info!(
        interfaces = interfaces.len(),
        attributes = attributes.len(),
        enums = enums.len(),
        classes = class_order.len(),
        functions = fn_count,
        total,
        "PHP definitions registered on bridge"
    );
}

fn visibility_to_zend(v: Visibility) -> u32 {
    match v {
        Visibility::Public => 0x01,    // ZEND_ACC_PUBLIC
        Visibility::Protected => 0x02, // ZEND_ACC_PROTECTED
        Visibility::Private => 0x04,   // ZEND_ACC_PRIVATE
    }
}

/// Translate plugin-builder `Modifiers` bits into Zend method `fn_flags`
/// bits. The C side ORs the result into `methods[m].flags`, which is then
/// poured into `zend_internal_function::fn_flags` by Zend — so the wire
/// value must use Zend's encoding, not ours. Symmetrically to the
/// class-level translation above: forwarding the raw `Modifiers::*` bits
/// would collide with the visibility low-nibble (ABSTRACT=0x01 = PUBLIC,
/// FINAL=0x02 = PROTECTED, STATIC=0x04 = PRIVATE), silently mis-marking
/// every `.static_()` method as private and breaking static dispatch.
fn method_modifiers_to_zend(mods: crate::plugin::types::Modifiers) -> u32 {
    use crate::plugin::types::Modifiers;
    let mut flags: u32 = 0;
    if mods.contains(Modifiers::STATIC) {
        flags |= 0x10; // ZEND_ACC_STATIC
    }
    if mods.contains(Modifiers::FINAL) {
        flags |= 0x20; // ZEND_ACC_FINAL
    }
    if mods.contains(Modifiers::ABSTRACT) {
        flags |= 0x40; // ZEND_ACC_ABSTRACT
    }
    flags
}

/// Map `Option<PhpType>` to bridge return type constants `(OXPHP_RT_*, is_nullable)`.
/// Returns `(0, 0)` for `None` (no return type declared).
fn php_type_to_bridge(t: &Option<PhpType>) -> (c_int, c_int) {
    match t {
        None => (0, 0),
        Some(inner) => {
            let (tag, nullable) = inner.to_bridge_tag();
            (tag as c_int, nullable as c_int)
        }
    }
}

/// Owning bundle of arrays passed to the bridge for parameter metadata.
///
/// Field order matters: `_names` (which owns the C strings) must be declared
/// before `name_ptrs` so that the strings outlive the pointer slice during
/// the FFI call.
struct ParamArrays {
    _names: Vec<CString>,
    name_ptrs: Vec<*const c_char>,
    types: Vec<c_int>,
    optional: Vec<c_int>,
}

fn build_param_arrays(params: &[PhpParamDef]) -> ParamArrays {
    let names: Vec<CString> = params
        .iter()
        .map(|p| CString::new(p.name.as_str()).expect("plugin param name contains NUL"))
        .collect();
    let name_ptrs: Vec<*const c_char> = names.iter().map(|c| c.as_ptr()).collect();
    let types: Vec<c_int> = params
        .iter()
        .map(|p| p.php_type.to_bridge_tag().0 as c_int)
        .collect();
    let optional: Vec<c_int> = params.iter().map(|p| c_int::from(!p.required)).collect();
    ParamArrays {
        _names: names,
        name_ptrs,
        types,
        optional,
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

/// Hand this worker thread the server-wide metrics, so a request refused at
/// pickup is counted in the same series as every other admission refusal.
pub fn set_server_metrics(m: Arc<crate::metrics::Metrics>) {
    SERVER_METRICS.with(|slot| {
        *slot.borrow_mut() = Some(m);
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
    loop {
        let incoming = WORKER_RX.with(|slot| {
            let rx = slot.borrow();
            match rx.as_ref() {
                Some(rx) => rx.recv().ok(),
                None => None,
            }
        });

        match incoming {
            Some(req) if req.queue_budget_expired() => continue_after_refusal(req),
            Some(req) => {
                // Direct call — no PENDING_REQUEST round-trip on the blocking path
                setup_request_tls(req);
                return 0; // success
            }
            None => return -1, // channel closed = shutdown
        }
    }
}

/// Answer a request whose queue budget ran out before this worker reached it.
///
/// Deliberately *before* `setup_request_tls`, unlike the 499 fast path inside
/// it: past that point the C serve loop creates a fiber and runs the PHP
/// handler either way, and a handler run for a request already answered here
/// has no response channel to write to — for a streaming handler, no client to
/// end it either. The 499 case survives that because a cancelled request's
/// fiber bails on the cancel cell; an expired one carries no such flag, so it
/// must never be handed over at all.
fn continue_after_refusal(req: WorkerIncomingRequest) {
    SERVER_METRICS.with(|slot| {
        if let Some(ref m) = *slot.borrow() {
            m.request_admission_refused(crate::executor::admission::ShedReason::WaitTimeout);
        }
    });
    let _ = req
        .response_tx
        .send(crate::executor::sapi::overloaded_response());
}

/// Worker send response callback — called from C bridge after each handler invocation.
/// Takes the accumulated response from SAPI output + headers and sends via oneshot.
/// Returns 0 on success.
///
/// # Safety
/// Called from C code via function pointer.
unsafe extern "C" fn worker_send_callback() -> std::os::raw::c_int {
    // Read the captured RINIT profiling flag once up front — mirrors the
    // `profiling_active` local in traditional.rs across the split
    // RINIT/RSHUTDOWN callbacks that worker mode uses. Only referenced
    // when at least one profiling plugin is compiled in.
    #[cfg(any(feature = "plugin-apm", feature = "plugin-profiler"))]
    let profiling_active = PROFILING_WAS_ACTIVE.with(|f| f.get());

    // On the early-response paths below we skip full finalize (the response
    // has already been sent, so profile data can't be attached), but the
    // bridge may still be in ProfileAll/ApmOnly. Drain and reset it so the
    // next request on this worker thread starts clean — otherwise the C
    // observer would keep firing for requests with `profiling_mode=Off`
    // whose `setup_request_tls` skipped `set_profiling_mode`.
    #[cfg(feature = "plugin-profiler")]
    let reset_profiler_bridge_if_dirty = || {
        if profiling_active
            || crate::profiling::get_profiling_mode()
                != crate::profiling::flush::PROFILING_MODE_OFF_RAW
        {
            crate::profiling::profiler_rshutdown_flush();
            crate::profiling::set_profiling_mode(crate::profiling::ProfilingMode::Off);
        }
    };

    // Async promise cleanup happens before this callback runs: the fiber
    // scheduler's finalize path drains the finishing fiber's own promises via
    // `cleanup_promises_for_fiber_callback`. A thread-wide drain here would
    // steal live promises from sibling fibers multiplexed on this thread.

    // Adopt the handler's explicit `http_response_code()` as the wire status.
    // That builtin writes `SG(sapi_headers).http_response_code` directly (not via
    // the header handler), so without this the non-streaming worker completion —
    // the last send path that never collected it — would ship
    // `http_response_code(201)` as 200. On the throw path it matters more: the
    // error callback stamps RESPONSE with a fatal 500 mid-request, so this must run
    // before the exception block below, and because `collect_response_code()` now
    // adopts an explicit non-200 SG status over that fallback, the script's 503
    // wins over the 500 (as under php-fpm). A 200 SG leaves the fallback in place.
    collect_response_code();

    // Pull the fiber's unhandled-exception capture into the error stream FIRST —
    // before the streaming / early-sent early-returns below. A handler-body throw
    // is swallowed by the fiber before zend_exception_error runs, so (unlike a
    // fatal or a shutdown-function throw, which the global oxphp_error_cb records)
    // it never reaches REQUEST_ERRORS on its own. Doing it here means the common
    // non-streaming send carries it: `take_request_errors()` below drains it onto
    // the ScriptResponse. (A streaming or already-early-sent response returns
    // before that drain — its accumulated errors are a documented streaming
    // boundary and are dropped at teardown.) Also force a 500 — the worker path
    // can't drive status off `ctx.handler_failed` (it is never set on the fiber
    // path). For an already-streaming response the status is committed on the
    // wire, so this only takes effect if the script had not set one yet.
    if let Some(exc) = pop_worker_unhandled_exception() {
        // This capture bypassed oxphp_error_cb, so emit the same error-level log
        // the traditional fatal path produces — otherwise a worker 500 is silent.
        tracing::error!(
            php_error_type = "E_ERROR",
            php_class = exc.exception_class.as_deref().unwrap_or(""),
            php_file = %exc.file,
            php_line = exc.line,
            "PHP: {}",
            exc.message
        );
        // Insert at the FRONT, not the back. The handler exception terminated
        // the request *before* the shutdown functions ran (they run at
        // `php_call_shutdown_functions()` in oxphp_fiber.c, after the fiber-catch
        // captured this exception); any error those shutdown functions raised was
        // already recorded into REQUEST_ERRORS by `oxphp_error_cb`. This capture
        // is only *mechanically* last — `extract_unhandled_exception` reports the
        // earliest error-level entry, so the true killer must lead, else a
        // shutdown-function error would shadow it on the root span.
        REQUEST_ERRORS.with(|e| e.borrow_mut().insert(0, exc));
        set_fatal_error_status_if_default();
    }

    // If streaming was active, close the stream
    if bindings::oxphp_bridge_is_streaming() {
        flush_stream_chunk();
        close_stream();
        // If early TX was already consumed by streaming headers, we're done
        if was_early_sent() {
            #[cfg(feature = "plugin-profiler")]
            reset_profiler_bridge_if_dirty();
            worker_request_teardown();
            return 0;
        }
    }

    // If response was already sent early (finish_request), we're done
    if was_early_sent() {
        #[cfg(feature = "plugin-profiler")]
        reset_profiler_bridge_if_dirty();
        worker_request_teardown();
        return 0;
    }

    // Take accumulated response and send via oneshot. Any handler-body throw was
    // already pulled into REQUEST_ERRORS (and forced the status) at the top.
    let (raw_output, raw_headers, status) = take_response();
    let body = Bytes::from(raw_output);
    let headers = parse_raw_headers(raw_headers);

    // Finalize APM / profiler spans on the PHP worker thread before sending
    // the response. Mirrors the traditional executor's RSHUTDOWN logic: drain
    // the C-side ring buffer, finalize the PROFILING_CONTEXT, and reset the
    // bridge mode so the next request on this worker starts clean. Gated on
    // the bridge mode (not the initial request mode) so mid-request SDK
    // promotion via `OxPHP\Profile\start()` is still captured.
    #[cfg(any(feature = "plugin-apm", feature = "plugin-profiler"))]
    let profile_tree = {
        #[cfg(feature = "plugin-profiler")]
        let do_finalize = profiling_active
            || crate::profiling::get_profiling_mode()
                != crate::profiling::flush::PROFILING_MODE_OFF_RAW;
        #[cfg(not(feature = "plugin-profiler"))]
        let do_finalize = profiling_active;

        if do_finalize {
            #[cfg(feature = "plugin-profiler")]
            crate::profiling::profiler_rshutdown_flush();

            let tree = crate::profiling::PROFILING_CONTEXT.with(|ctx| ctx.borrow_mut().finalize());

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

    let cancel_reason = bindings::oxphp_bridge_get_cancel_reason();
    EARLY_TX.with(|slot| {
        if let Some((start, tx)) = slot.borrow_mut().take() {
            let _ = tx.send(ScriptResponse {
                status,
                headers,
                body,
                execution_time_us: start.elapsed().as_micros() as u64,
                stream_rx: None,
                errors: take_request_errors(),
                profile_tree,
                cancel_reason,
                refused: false,
            });
        }
    });

    worker_request_teardown();

    0
}

/// Shared terminal cleanup for one worker-mode request: record metrics, clear
/// the response buffers, drop the worker's Arc to CancellationState, clear
/// the bridge cancel pointer (the next request installs fresh state in
/// `setup_request_tls`), unregister from the worker registry, and zero the
/// SAPI request time. Every exit path of `worker_send_callback` must run
/// exactly one of these — `end_request` inside balances the `begin_request`
/// from `setup_request_tls`.
///
/// # Safety
/// Must run on the worker thread that owns the current request's TLS.
unsafe fn worker_request_teardown() {
    record_worker_request_metrics();
    clear_buffers();
    // Defensive: worker_send_callback pops the unhandled-exception capture at its
    // top, before any early return, so this is normally a no-op. Kept as a belt-
    // and-braces free so a capture can never linger in the thread-active slot for
    // whichever request runs next on this thread.
    bindings::oxphp_bridge_clear_unhandled();
    bindings::oxphp_bridge_set_cancel_ptr(std::ptr::null());
    WORKER_CANCEL_STATE.with(|slot| slot.borrow_mut().take());
    crate::php::worker_registry::end_request(bindings::oxphp_bridge_get_worker_id() as usize);
    bindings::oxphp_bridge_set_request_time(0.0);
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
            // Loops so an expired request is answered and the next one
            // examined in the same poll — see `continue_after_refusal` for why
            // it must not be staged for the scheduler.
            Some(rx) => loop {
                match rx.try_recv() {
                    Ok(req) if req.queue_budget_expired() => continue_after_refusal(req),
                    Ok(req) => {
                        PENDING_REQUEST.with(|p| {
                            *p.borrow_mut() = Some(req);
                        });
                        return TryRecvResult::Ready;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => return TryRecvResult::Empty,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        return TryRecvResult::Disconnected
                    }
                }
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

    // Reset profiling context for the new request and prime the C-side
    // profiler observer mode — mirrors the traditional executor's RINIT
    // logic. Worker mode reuses a single `php_request_startup` for the
    // worker's lifetime, so this is the only place per-request profiling
    // TLS can be initialized. Skip entirely when the request's mode is
    // Off so the common case pays no FFI cost.
    //
    // Stash the initial `profiling_active` flag in TLS so the
    // `worker_send_callback` RSHUTDOWN path can match the
    // `profiling_active || bridge_mode != OFF` formula used by
    // `traditional.rs`. Always updated, even when the branch below is
    // skipped, so a prior request's value can't leak into this one.
    PROFILING_WAS_ACTIVE
        .with(|f| f.set(req.script.profiling_mode != crate::profiling::ProfilingMode::Off));
    #[cfg(any(feature = "plugin-apm", feature = "plugin-profiler"))]
    if req.script.profiling_mode != crate::profiling::ProfilingMode::Off {
        crate::profiling::PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                req.script.profiling_mode,
                req.script.trace_id.clone(),
                req.script.span_id.clone(),
            );
        });
        #[cfg(feature = "plugin-profiler")]
        crate::profiling::set_profiling_mode(req.script.profiling_mode);
    }

    let start = Instant::now();

    // Stash a strong Arc on the worker thread so the bridge's raw
    // pointer stays valid even if the tokio dispatch future is
    // dropped before the worker finishes. Cleared in
    // worker_send_callback's terminal cleanup.
    let cancel_ptr = req.script.cancel_state.as_ptr();
    WORKER_CANCEL_STATE.with(|slot| {
        *slot.borrow_mut() = Some(req.script.cancel_state.clone());
    });

    // Install the cancel cell into the bridge ctx BEFORE the fast-path check
    // below: the C serve loop creates a fiber for this request either way, and
    // oxphp_scheduler_create_fiber captures ctx.cancel_ptr at that moment. An
    // early return that skipped this install would hand the fiber a NULL
    // pointer (reset_request_ctx above zeroed it), making it invisible to the
    // drain sweep's cell write and the hard-drain self-cancel.
    unsafe {
        bindings::oxphp_bridge_set_cancel_ptr(cancel_ptr);
    }

    // Per-request: register a Weak back-ref so cancel_request() can find this
    // worker by Arc::ptr_eq, count the request for the drain broadcast kick,
    // and stamp request_start_us so the supervisor sees this worker as busy.
    crate::php::worker_registry::begin_request(
        unsafe { bindings::oxphp_bridge_get_worker_id() } as usize,
        &req.script.cancel_state,
    );

    // Fast-path: client disconnected while we were in the queue.
    // Ship 499 directly via the still-owned response_tx and bail
    // before any further setup (no PHP, no early_tx stash).
    if req.script.cancel_state.get() != crate::bridge::cancel::CancelReason::None {
        let _ = req.response_tx.send(ScriptResponse::client_closed());
        return;
    }

    set_early_tx(start, req.response_tx);

    // Store request start time for duration histogram
    WORKER_REQUEST_START.with(|slot| slot.set(Some(start)));

    // One-shot capture of &EG(vm_interrupt) on the worker thread +
    // publish into WORKERS[id].interrupt_flag_ptr so other threads
    // can raise the flag for cross-thread cancellation.
    VM_INTERRUPT_CAPTURED.with(|c| {
        if !c.get() {
            unsafe {
                bindings::oxphp_capture_vm_interrupt();
            }
            let id = unsafe { bindings::oxphp_bridge_get_worker_id() } as usize;
            if let Some(workers) = crate::php::worker_registry::WORKERS.get() {
                if let Some(slot) = workers.get(id) {
                    let addr = unsafe { bindings::oxphp_bridge_vm_interrupt_addr() };
                    slot.interrupt_flag_ptr
                        .store(addr, std::sync::atomic::Ordering::Release);
                    // Hand the per-worker tick counter to the C observer
                    // so each PHP function call bumps it.
                    let tick_ptr = &slot.heartbeat.ticks as *const std::sync::atomic::AtomicU64;
                    unsafe {
                        bindings::oxphp_bridge_set_tick_ptr(tick_ptr);
                    }
                }
            }
            c.set(true);
        }
    });

    // Increment soft_resets counter
    WORKER_METRICS_TLS.with(|slot| {
        if let Some(ref wm) = *slot.borrow() {
            wm.soft_resets_total.fetch_add(1, Ordering::Relaxed);
        }
    });
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

/// fiber_id of the request fiber currently executing on this thread, or 0
/// outside fiber context. Passed lazily to `or_insert_with` at the three
/// promise-registration choke points, so it is evaluated once per promise —
/// at the first choke point that sees the id (first-writer-wins: the owner is
/// the creating fiber). Later choke points for the same id (cleanup/strand
/// re-registration on that same fiber) find the entry already present and skip
/// the call. Every choke point still captures ownership should a future path
/// register a promise without a preceding `store_promise`.
fn current_promise_owner() -> u64 {
    unsafe { crate::bridge::ffi::oxphp_bridge_current_fiber_id() }
}

pub fn store_promise(
    id: u64,
    rx: tokio::sync::oneshot::Receiver<AsyncResult>,
    cancelled: std::sync::Arc<CancelShared>,
) {
    PROMISE_OWNER.with(|m| {
        m.borrow_mut()
            .entry(id)
            .or_insert_with(current_promise_owner);
    });
    PROMISE_MAP.with(|m| {
        m.borrow_mut().insert(id, (rx, cancelled));
    });
}

/// Register a synthetic-promise receiver with the current thread's
/// `PROMISE_MAP` so `oxphp_bridge_fiber_await(id, ...)` drains it the
/// same way as an async-pool task result.
///
/// Callers live in `src/plugins/ox_async/synthetic.rs`; the receiver's
/// payload is already an `AsyncResult` (synthetic sources construct
/// one from `PromisePayload` before sending), so no enum refactor to
/// `PROMISE_MAP` is required. `cancelled` is provided by the caller
/// for API symmetry with `store_promise`; synthetic promises today
/// ignore it (cancellation is signalled through the oneshot payload),
/// but future plumbing (e.g., fiber_await timeout signalling back to
/// the synthetic resolver) can use it.
pub(crate) fn register_synthetic_receiver(
    id: u64,
    rx: tokio::sync::oneshot::Receiver<AsyncResult>,
    cancelled: std::sync::Arc<CancelShared>,
) {
    store_promise(id, rx, cancelled);
}

pub fn take_promise(
    id: u64,
) -> Option<(
    tokio::sync::oneshot::Receiver<AsyncResult>,
    std::sync::Arc<CancelShared>,
)> {
    PROMISE_MAP.with(|m| m.borrow_mut().remove(&id))
}

pub fn store_promise_cleanup(id: u64, cleanup: PromiseCleanup) {
    PROMISE_OWNER.with(|m| {
        m.borrow_mut()
            .entry(id)
            .or_insert_with(current_promise_owner);
    });
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
    PROMISE_STRANDED.with(|m| {
        for &id in m.borrow().keys() {
            ids.insert(id);
        }
    });
    ids.into_iter().collect()
}

/// Stash an (rx, cancel) pair whose owning future was dropped by a
/// timeout (await_race / await_any). RSHUTDOWN block_on's these rxs
/// before unfreezing the matching PROMISE_CLEANUP entry, ensuring the
/// worker has actually finished touching the frozen captures.
fn stash_stranded(
    id: u64,
    rx: tokio::sync::oneshot::Receiver<AsyncResult>,
    cancelled: Arc<CancelShared>,
) {
    PROMISE_OWNER.with(|m| {
        m.borrow_mut()
            .entry(id)
            .or_insert_with(current_promise_owner);
    });
    PROMISE_STRANDED.with(|m| {
        m.borrow_mut().insert(id, (rx, cancelled));
    });
}

fn take_stranded(id: u64) -> Option<PromiseEntry> {
    PROMISE_STRANDED.with(|m| m.borrow_mut().remove(&id))
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
///      - `TryRecvError::Closed` → synthesize a rejection result, return true.
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
                // Sender dropped without ever sending a result — the task fiber
                // was abandoned (a worker panic). Synthesize a rejection and
                // report ready, so the awaiter throws instead of seeing the
                // promise as perpetually-not-ready. The latter livelocks the
                // fiber await_race / await_any poll loops — and spins a worker
                // forever when timeout <= 0 — because they only ever observe
                // `false` for this id and never settle it. Matches the blocking
                // await path's "promise channel closed unexpectedly" error.
                READY_RESULTS.with(|m| {
                    m.borrow_mut().insert(
                        promise_id,
                        AsyncResult {
                            success: false,
                            serialized_value: std::ptr::null_mut(),
                            serialized_value_len: 0,
                            exception_class: Some("OxPHP\\Async\\AsyncException".to_string()),
                            exception_message: Some(
                                "promise channel closed unexpectedly".to_string(),
                            ),
                            keepalive: None,
                        },
                    );
                });
                true
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

// ─── Async In-Flight Cap ─────────────────────────────────────

/// Process-global bound on `queued + running` async tasks. Installed once
/// alongside the pool; a dispatch reserves a permit and a completed task
/// releases one. When no counter is installed (pool disabled / unit tests),
/// acquisition is unbounded.
static GLOBAL_ASYNC_INFLIGHT: OnceLock<Arc<crate::executor::async_fiber::InFlightCounter>> =
    OnceLock::new();

/// Install the process-global in-flight counter. Called once from the pool
/// setup, with the same `Arc` the worker threads release through.
pub fn set_global_async_inflight(counter: Arc<crate::executor::async_fiber::InFlightCounter>) {
    GLOBAL_ASYNC_INFLIGHT
        .set(counter)
        .ok()
        .expect("GLOBAL_ASYNC_INFLIGHT already set");
}

/// Reserve one in-flight permit at dispatch. Returns `false` (reject) when the
/// process-global cap is reached. With no counter installed, always `true`.
pub fn async_inflight_try_acquire() -> bool {
    match GLOBAL_ASYNC_INFLIGHT.get() {
        Some(c) => c.try_acquire(),
        None => true,
    }
}

/// Release one in-flight permit reserved by a prior `async_inflight_try_acquire`.
/// No-op when no counter is installed.
pub fn async_inflight_release() {
    if let Some(c) = GLOBAL_ASYNC_INFLIGHT.get() {
        c.release();
    }
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

/// Borrow the process-global Tokio runtime handle, if it has been installed.
/// Safe to call from any thread (including PHP worker threads). Returns
/// `None` during early startup / unit tests where no runtime was registered.
pub fn async_tokio_handle() -> Option<&'static tokio::runtime::Handle> {
    ASYNC_TOKIO_HANDLE.get()
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

/// Releases a reserved in-flight permit on drop unless `committed`. Used by
/// `async_dispatch_callback` so every early-return path between reserving the
/// permit and successfully enqueuing the task gives it back; on success the
/// worker takes ownership and releases it at task completion instead.
struct InflightPermitGuard {
    committed: bool,
}

impl Drop for InflightPermitGuard {
    fn drop(&mut self) {
        if !self.committed {
            async_inflight_release();
        }
    }
}

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

    // 1b. Reserve an in-flight permit. The process-global cap bounds queued +
    // running tasks; past it we reject (non-blocking) so fan-out composition
    // can't deadlock. Done before any serialization so a reject stays cheap.
    if !async_inflight_try_acquire() {
        if let Some(m) = get_async_metrics() {
            m.async_task_rejected();
        }
        return -1;
    }
    // The permit is released by the worker once the task completes (drain). Until
    // the task is successfully enqueued, this guard releases it on every early
    // return below — serialization failures and a full/closed queue alike —
    // unless we hand ownership to the worker by committing it on success.
    let mut permit = InflightPermitGuard { committed: false };

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
    let cancelled = Arc::new(CancelShared::new());

    // 7. Copy op_array struct bytes into a system-malloc'd buffer.
    //    This eliminates cross-thread reads — the async worker uses this local copy
    //    instead of dereferencing the closure object on the PHP worker's emalloc heap.
    let op_array_size = ffi::oxphp_op_array_size();
    let op_array_buf = libc::malloc(op_array_size) as *mut u8;
    if op_array_buf.is_null() {
        return -1;
    }
    std::ptr::copy_nonoverlapping(op_array as *const u8, op_array_buf, op_array_size);

    // 8. Build and send task
    let task = AsyncTask {
        promise_id,
        op_array_buf,
        op_array_buf_len: op_array_size,
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
            // Task is enqueued; the worker now owns the permit and releases it
            // when the task completes. Don't let the guard give it back.
            permit.committed = true;
            store_promise(promise_id, result_rx, cancelled);
            store_promise_cleanup(promise_id, cleanup);
            if let Some(m) = get_async_metrics() {
                m.async_task_dispatched();
            }
            promise_id as i64
        }
        Err(crossbeam_channel::TrySendError::Full(task))
        | Err(crossbeam_channel::TrySendError::Disconnected(task)) => {
            // Free the op_array copy (system malloc'd)
            if !task.op_array_buf.is_null() {
                libc::free(task.op_array_buf as *mut c_void);
            }
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

/// Stores a generic error in bridge TLS so the PHP-side handler's
/// `read_bridge_exception` surfaces a meaningful class+message when an
/// internal failure (channel closed, unknown id, runtime not initialized)
/// occurs without a worker-thrown exception. Without this, the handler
/// reads stale or empty TLS and reports `[Unknown] unknown error`.
///
/// # Safety
/// Calls into the bridge C FFI; safe as long as the bridge module is loaded
/// (which it is for any await path that can reach this helper).
unsafe fn set_bridge_internal_error(message: &str) {
    use crate::bridge::ffi;
    let cls = CString::new("OxPHP\\Async\\AsyncException").unwrap_or_default();
    let msg = CString::new(message).unwrap_or_default();
    ffi::oxphp_bridge_set_async_exception(cls.as_ptr(), msg.as_ptr());
}

/// Break a running async task out of a CPU-bound loop after its awaiter has
/// given up. The cancel flag (`cancelled.cancelled`) must already be set: the
/// scheduler loop polls it to unwind a *suspended* fiber, but a busy-looping
/// fiber never returns to that loop. Kicking the worker's `EG(vm_interrupt)`
/// cross-thread breaks it out at the next opcode boundary; the interrupt
/// handler then throws into the matching fiber (per-fiber `cancel_cell`),
/// unwinding it. `worker_interrupt == 0` means the task hasn't been picked up
/// by a worker yet — nothing is running to interrupt, so this is a no-op.
///
/// # Safety
/// Calls into the C bridge; must run on a PHP thread.
unsafe fn kick_worker_interrupt(cancelled: &CancelShared) {
    // Acquire pairs with the worker's Release store of this address (async
    // pool, task pickup): once we observe a non-zero address, the publish is
    // visible — no stale 0 read past the synchronization on weak hardware.
    let addr = cancelled
        .worker_interrupt
        .load(std::sync::atomic::Ordering::Acquire);
    if addr != 0 {
        crate::bridge::ffi::oxphp_bridge_request_interrupt_at(addr as *mut c_void);
    }
}

/// Strand a still-pending promise whose awaiter has abandoned it — e.g. an
/// `await_all` that bailed out on timeout or a sibling's rejection, leaving the
/// remaining members running on the pool with no one to receive their result.
///
/// Flips the cancel flag (so a suspended fiber unwinds at the next scheduler
/// poll), kicks the running worker out of any CPU-bound loop (Path B), and
/// stashes the receiver so `cleanup_outstanding_promises` drains it at RSHUTDOWN
/// before releasing frozen captures — closing the same use-after-free window
/// `await_race`/`await_any` guard for their stragglers.
///
/// No-op if the id is unknown, already completed, or already stranded (the
/// promise is no longer in `PROMISE_MAP`), so it is safe to call over a range
/// that includes promises already consumed by earlier awaits.
///
/// # Safety
/// Calls into the C bridge (the interrupt kick); must run on a PHP worker thread.
pub unsafe fn strand_and_cancel_promise(id: u64) {
    if let Some((rx, cancelled)) = take_promise(id) {
        cancelled
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        kick_worker_interrupt(&cancelled);
        stash_stranded(id, rx, cancelled);
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
                ffi::oxphp_bridge_set_async_exception(cls_c.as_ptr(), msg_c.as_ptr());
            }
            return -1;
        }
    }

    // Take promise from map
    let (rx, cancelled) = match take_promise(id) {
        Some(p) => p,
        None => {
            set_bridge_internal_error(&format!(
                "unknown or already-awaited promise id {promise_id}"
            ));
            return -1;
        }
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
                        set_bridge_internal_error("promise channel closed unexpectedly");
                        return -1;
                    }
                    Err(_) => {
                        // Timeout — signal cancellation so the async worker stops.
                        // Note: rx was consumed by the timeout future, so we can't
                        // re-store it. The PROMISE_CLEANUP data remains and will be
                        // cleaned up by RSHUTDOWN (cleanup_outstanding_promises).
                        // The cancelled flag tells the worker to stop early (the
                        // worker's scheduler loop polls it to unwind a SUSPENDED
                        // fiber).
                        // Release: pairs with the worker's Acquire load of the
                        // cancel flag (scheduler poll and the C interrupt handler).
                        cancelled
                            .cancelled
                            .store(true, std::sync::atomic::Ordering::Release);
                        // The flag alone reaches only a suspended fiber; a
                        // CPU-bound one needs the cross-thread interrupt kick.
                        unsafe { kick_worker_interrupt(&cancelled) };
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
                        set_bridge_internal_error("promise channel closed unexpectedly");
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
                set_bridge_internal_error("promise channel closed unexpectedly");
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
        // create a proper exception with class/message from the worker.
        if let (Some(cls), Some(msg)) = (&result.exception_class, &result.exception_message) {
            let cls_c = CString::new(cls.as_str()).unwrap_or_default();
            let msg_c = CString::new(msg.as_str()).unwrap_or_default();
            ffi::oxphp_bridge_set_async_exception(cls_c.as_ptr(), msg_c.as_ptr());
        }
        -1
    }
}

/// Rust-side callback invoked from C when PHP calls `oxphp_async_await_race()`.
///
/// Races multiple promise receivers via a `poll_fn` over `&mut rxs[..]`,
/// returning the first to complete. Non-winning receivers are put back into
/// PROMISE_MAP so they can be awaited individually later via
/// `oxphp_async_await()`.
///
/// Returns: 0 = success, -1 = error, -2 = timeout.
/// On success: `*out_winner_id` is the winning promise ID, `retval` is populated.
///
/// **Timeout behavior**: On timeout, the cancel flag is set on every still-
/// pending promise and the (id, rx, cancelled) tuples are moved into
/// PROMISE_STRANDED. RSHUTDOWN's cleanup callback block_on's each stranded
/// rx (5 s per promise) before unfreezing captures, so workers finish
/// touching the frozen state before it's released. Stranded promises cannot
/// be awaited after timeout — their rxs live only in PROMISE_STRANDED.
///
/// # Safety
/// Called from C FFI. `promise_ids` must point to `count` valid i64 values.
/// `out_winner_id` and `retval` must be valid writable pointers.
pub unsafe extern "C" fn await_race_dispatch_callback(
    promise_ids: *const i64,
    count: u32,
    timeout: f64,
    out_winner_id: *mut i64,
    retval: *mut c_void,
) -> c_int {
    use crate::bridge::ffi;
    use std::future::{poll_fn, Future};
    use std::pin::Pin;
    use std::task::Poll;
    use std::time::Duration;

    if count == 0 || promise_ids.is_null() {
        return -1;
    }

    // Collect promise IDs from the C array
    let ids: Vec<u64> = (0..count as usize)
        .map(|i| *promise_ids.add(i) as u64)
        .collect();

    // Inside a task fiber, race cooperatively (poll + yield) so the worker
    // is not pinned while several promises are outstanding.
    if ffi::oxphp_bridge_in_fiber() == 1 {
        return await_race_in_fiber(&ids, timeout, out_winner_id, retval);
    }

    // Take all receivers and cancelled flags from PROMISE_MAP.
    let mut id_map: Vec<u64> = Vec::with_capacity(ids.len());
    let mut cancel_map: Vec<std::sync::Arc<CancelShared>> = Vec::with_capacity(ids.len());
    let mut rxs: Vec<tokio::sync::oneshot::Receiver<AsyncResult>> = Vec::with_capacity(ids.len());

    for &id in &ids {
        match take_promise(id) {
            Some((rx, cancelled)) => {
                id_map.push(id);
                cancel_map.push(cancelled);
                rxs.push(rx);
            }
            None => {
                // Unknown / already-awaited promise id. Restore any
                // receivers we already took and bail with -4. The handler
                // surfaces the offending id via *out_winner_id.
                for (rx, (taken_id, cancelled)) in
                    rxs.into_iter().zip(id_map.iter().zip(cancel_map.iter()))
                {
                    store_promise(*taken_id, rx, cancelled.clone());
                }
                *out_winner_id = id as i64;
                return -4;
            }
        }
    }

    let handle = match ASYNC_TOKIO_HANDLE.get() {
        Some(h) => h,
        None => {
            // No tokio handle — put receivers back and fail
            for (rx, (id, cancelled)) in rxs.into_iter().zip(id_map.iter().zip(cancel_map.iter())) {
                store_promise(*id, rx, cancelled.clone());
            }
            set_bridge_internal_error("async runtime not initialized");
            return -1;
        }
    };

    // Race rxs by polling them through a `&mut [Receiver]` borrow rather
    // than consuming the Vec via `select_all`. This is the soundness fix
    // for the timeout path: when `tokio::time::timeout` fires, it drops
    // the inner future, releasing the borrow — but `rxs` itself stays
    // owned by this function so we can stash the surviving receivers in
    // PROMISE_STRANDED. The previous `select_all(rxs)` design dropped the
    // receivers along with the timeout future, leaving RSHUTDOWN unable
    // to wait for the still-running workers before unfreezing captures.
    //
    // oneshot::Receiver<T> is Unpin, so `Pin::new(&mut rxs[i])` is sound.
    let race_result = {
        let race_fut = poll_fn(|cx| {
            for (i, rx) in rxs.iter_mut().enumerate() {
                if let Poll::Ready(res) = Pin::new(rx).poll(cx) {
                    return Poll::Ready((i, res));
                }
            }
            Poll::Pending
        });
        if timeout > 0.0 {
            let dur = Duration::from_secs_f64(timeout);
            // Construct `Sleep` inside `block_on` so the runtime context is
            // established before the timer driver registers.
            handle.block_on(async move { tokio::time::timeout(dur, race_fut).await })
        } else {
            Ok(handle.block_on(race_fut))
        }
    };

    match race_result {
        Ok((winner_idx, recv_result)) => {
            let winner_id = id_map[winner_idx];

            // Restore non-winning receivers to PROMISE_MAP. Use swap_remove on
            // all three parallel vecs so they stay in lockstep — the winner's
            // slot is filled with the last entry, then the last is popped.
            id_map.swap_remove(winner_idx);
            cancel_map.swap_remove(winner_idx);
            drop(rxs.swap_remove(winner_idx)); // already consumed via poll
            for ((id, cancelled), rx) in id_map.into_iter().zip(cancel_map).zip(rxs) {
                store_promise(id, rx, cancelled);
            }

            // Handle the winning result
            let mut result = match recv_result {
                Ok(r) => r,
                Err(_) => {
                    // Channel closed — task was dropped
                    cleanup_promise(winner_id);
                    set_bridge_internal_error("promise channel closed unexpectedly");
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
                    ffi::oxphp_bridge_set_async_exception(cls_c.as_ptr(), msg_c.as_ptr());
                }
                *out_winner_id = winner_id as i64;
                -1
            }
        }
        Err(_timeout) => {
            // Timeout — the poll_fn future was dropped, releasing its
            // `&mut rxs` borrow. `rxs` itself survived (it's owned here),
            // so we can stash the still-pending (id, rx, cancelled)
            // tuples in PROMISE_STRANDED. RSHUTDOWN's cleanup callback
            // block_on's each stranded rx with a 5 s budget BEFORE
            // unfreezing the matching PROMISE_CLEANUP entry — the
            // worker therefore finishes touching the frozen captures
            // before they're released, closing the UAF window.
            //
            // We also signal cancellation on every cancel flag so the
            // worker observes the request at the next vm_interrupt poll
            // and can return early instead of running to completion.
            //
            // Each stranded worker can extend RSHUTDOWN by up to 5 s
            // (the per-promise block_on budget). Tracked via
            // async_tasks_stranded so the stall risk shows up in metrics.
            let stranded_count = rxs.len() as u64;
            for ((id, rx), cancelled) in id_map.into_iter().zip(rxs).zip(cancel_map) {
                // Release: pairs with the worker's Acquire load of the cancel flag.
                cancelled
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Release);
                // Break any CPU-bound task out of its loop — the flag alone
                // reaches only suspended fibers.
                unsafe { kick_worker_interrupt(&cancelled) };
                stash_stranded(id, rx, cancelled);
            }
            if let Some(m) = get_async_metrics() {
                m.async_tasks_stranded(stranded_count);
            }
            -2
        }
    }
}

/// Per-promise rejection record collected by `await_any_dispatch_callback`.
struct AggregateRejection {
    promise_id: u64,
    exception_class: String,
    message: String,
}

/// Push accumulated rejections into the C-bridge aggregate buffer in input-
/// array position order (mirrors the blocking await_any ordering). Unlike the
/// blocking path this does NOT call `cleanup_promise` — the fiber poll loops
/// already clean up each promise as they record its rejection.
///
/// # Safety
/// Calls into the C bridge; must run on the PHP worker thread.
unsafe fn push_aggregate_rejections(collected: &mut Vec<AggregateRejection>, input_ids: &[u64]) {
    use crate::bridge::ffi;
    let position: std::collections::HashMap<u64, usize> = input_ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();
    let mut sorted = std::mem::take(collected);
    sorted.sort_by_key(|r| position.get(&r.promise_id).copied().unwrap_or(usize::MAX));
    ffi::oxphp_bridge_aggregate_clear();
    for r in &sorted {
        let cls = CString::new(r.exception_class.as_str()).unwrap_or_default();
        let msg = CString::new(r.message.as_str()).unwrap_or_default();
        ffi::oxphp_bridge_aggregate_push(cls.as_ptr(), msg.as_ptr(), r.promise_id as i64);
    }
}

/// What a fan-out loop should do after yielding its fiber for one scheduler
/// cycle.
enum Yielded {
    /// Resumed normally — keep polling.
    Poll,
    /// Cancelled while yielded. The cancellation exception is in bridge TLS;
    /// the caller aborts with -1.
    Cancelled,
    /// The fiber is being torn down and an exception is already pending. The
    /// caller must return without adding one of its own and without yielding
    /// again — the fiber is no longer the current one, so a second yield would
    /// report "outside a fiber" and turn the unwind into a synthetic error.
    Unwinding,
}

/// Yield the current task fiber for one scheduler cycle.
///
/// # Safety
/// Must run inside an oxphp task fiber (callers guarantee this).
unsafe fn yield_or_cancel() -> Yielded {
    use crate::bridge::ffi;
    match ffi::oxphp_bridge_fiber_yield() {
        ffi::OXPHP_FIBER_UNWIND => Yielded::Unwinding,
        -3 => {
            let cls = CString::new("OxPHP\\Async\\AsyncException").unwrap_or_default();
            let msg = CString::new("Async task cancelled").unwrap_or_default();
            ffi::oxphp_bridge_set_async_exception(cls.as_ptr(), msg.as_ptr());
            Yielded::Cancelled
        }
        0 => {
            // Not in a fiber (should be impossible — caller checked). Abort
            // rather than spin hot.
            set_bridge_internal_error("await fan-out: fiber yield outside a fiber");
            Yielded::Cancelled
        }
        _ => Yielded::Poll,
    }
}

/// Fiber-cooperative `oxphp_async_await_race`: poll the promises
/// non-destructively and suspend the task fiber between scans instead of
/// blocking the worker. The first promise to SETTLE (fulfil or reject) wins;
/// the losers are left untouched in PROMISE_MAP (still awaitable). Return
/// codes match `await_race_dispatch_callback`: 0 success, -1 error (exception
/// in bridge TLS), -2 timeout.
///
/// # Safety
/// `out_winner_id` / `retval` must be valid writable pointers, and the call
/// must run inside an oxphp task fiber.
unsafe fn await_race_in_fiber(
    ids: &[u64],
    timeout: f64,
    out_winner_id: *mut i64,
    retval: *mut c_void,
) -> c_int {
    use crate::bridge::ffi;
    let deadline =
        (timeout > 0.0).then(|| Instant::now() + std::time::Duration::from_secs_f64(timeout));

    loop {
        for &id in ids {
            if !await_is_ready(id) {
                continue;
            }
            // First to settle wins.
            *out_winner_id = id as i64;
            let mut result = match take_ready_result(id) {
                Some(r) => r,
                None => {
                    set_bridge_internal_error("await_race: winner result missing");
                    return -1;
                }
            };
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
                    result.serialized_value = std::ptr::null_mut();
                    if rc != 0 {
                        return -1;
                    }
                }
                return 0;
            }
            // Winner rejected — propagate its exception.
            if let (Some(cls), Some(msg)) = (&result.exception_class, &result.exception_message) {
                let cls_c = CString::new(cls.as_str()).unwrap_or_default();
                let msg_c = CString::new(msg.as_str()).unwrap_or_default();
                ffi::oxphp_bridge_set_async_exception(cls_c.as_ptr(), msg_c.as_ptr());
            }
            return -1;
        }

        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                return -2;
            }
        }

        match yield_or_cancel() {
            Yielded::Poll => {}
            Yielded::Cancelled => return -1,
            Yielded::Unwinding => return ffi::OXPHP_FIBER_UNWIND,
        }
    }
}

/// Fiber-cooperative `oxphp_async_await_any`: poll non-destructively and
/// suspend between scans. The first promise to FULFIL wins; rejections
/// accumulate. If every promise rejects, throws `AggregateAsyncException`
/// (returns -3). On timeout, throws `TimeoutException` carrying the partial
/// errors plus the still-pending ids (returns -2). Still-pending promises are
/// left in PROMISE_MAP — awaitable individually, and cleaned up safely at
/// RSHUTDOWN, which waits for their workers before unfreezing captures.
///
/// # Safety
/// As `await_race_in_fiber`.
unsafe fn await_any_in_fiber(
    ids: &[u64],
    timeout: f64,
    out_winner_id: *mut i64,
    retval: *mut c_void,
) -> c_int {
    use crate::bridge::ffi;
    let deadline =
        (timeout > 0.0).then(|| Instant::now() + std::time::Duration::from_secs_f64(timeout));
    let mut collected: Vec<AggregateRejection> = Vec::new();
    let mut remaining: Vec<u64> = ids.to_vec();

    loop {
        let mut i = 0;
        while i < remaining.len() {
            let id = remaining[i];
            if !await_is_ready(id) {
                i += 1;
                continue;
            }
            let mut result = match take_ready_result(id) {
                Some(r) => r,
                None => {
                    set_bridge_internal_error("await_any: result missing");
                    return -1;
                }
            };
            cleanup_promise(id);
            if result.success {
                *out_winner_id = id as i64;
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
                return 0;
            }
            // Rejection — record it and drop the promise from the poll set.
            let cls = result
                .exception_class
                .clone()
                .unwrap_or_else(|| "OxPHP\\Async\\AsyncException".to_string());
            let msg = result
                .exception_message
                .clone()
                .unwrap_or_else(|| "promise rejected without message".to_string());
            collected.push(AggregateRejection {
                promise_id: id,
                exception_class: cls,
                message: msg,
            });
            remaining.remove(i); // do not advance i — next element shifts down
        }

        if remaining.is_empty() {
            // Every promise rejected.
            push_aggregate_rejections(&mut collected, ids);
            ffi::oxphp_bridge_aggregate_throw();
            return -3;
        }

        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                push_aggregate_rejections(&mut collected, ids);
                let pending: Vec<i64> = remaining.iter().map(|&id| id as i64).collect();
                ffi::oxphp_bridge_aggregate_throw_timeout(pending.as_ptr(), pending.len() as u32);
                return -2;
            }
        }

        match yield_or_cancel() {
            Yielded::Poll => {}
            Yielded::Cancelled => return -1,
            Yielded::Unwinding => return ffi::OXPHP_FIBER_UNWIND,
        }
    }
}

/// Rust-side callback for `oxphp_async_await_any()`.
///
/// Promise.any semantics:
///   * First FULFILLED promise wins. Remaining still-pending promises are
///     restored to PROMISE_MAP (individually awaitable via `oxphp_async_await`).
///   * If every promise rejects before any fulfills, accumulates errors and
///     throws `OxPHP\Async\AggregateAsyncException` via the aggregate API.
///     Returns -3.
///   * If the timeout expires before a fulfilled winner, accumulates errors
///     that arrived before the deadline plus computes still-pending ids, then
///     throws `OxPHP\Async\TimeoutException` with both fields. Returns -2.
///   * Other internal failures return -1.
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
    use std::future::{poll_fn, Future};
    use std::pin::Pin;
    use std::task::Poll;
    use std::time::Duration;

    if count == 0 || promise_ids.is_null() {
        return -1;
    }

    // Snapshot input ids in input-array order — used later for sorting
    // accumulated rejections back into positional order.
    let input_ids: Vec<u64> = (0..count as usize)
        .map(|i| *promise_ids.add(i) as u64)
        .collect();

    // Inside a task fiber, resolve cooperatively (poll + yield) so the worker
    // is not pinned while waiting for one of several promises to fulfil.
    if ffi::oxphp_bridge_in_fiber() == 1 {
        return await_any_in_fiber(&input_ids, timeout, out_winner_id, retval);
    }

    // Pull receivers + cancel flags out of PROMISE_MAP.
    let mut id_vec: Vec<u64> = Vec::with_capacity(input_ids.len());
    let mut cancel_vec: Vec<Arc<CancelShared>> = Vec::with_capacity(input_ids.len());
    let mut rxs: Vec<tokio::sync::oneshot::Receiver<AsyncResult>> =
        Vec::with_capacity(input_ids.len());

    for &id in &input_ids {
        match take_promise(id) {
            Some((rx, cancelled)) => {
                id_vec.push(id);
                cancel_vec.push(cancelled);
                rxs.push(rx);
            }
            None => {
                // Unknown / already-awaited promise id. Restore any
                // receivers we already took and bail with -4. The handler
                // surfaces the offending id via *out_winner_id.
                for ((taken_id, rx), cancelled) in id_vec
                    .iter()
                    .copied()
                    .zip(rxs)
                    .zip(cancel_vec.iter().cloned())
                {
                    store_promise(taken_id, rx, cancelled);
                }
                *out_winner_id = id as i64;
                return -4;
            }
        }
    }

    if rxs.is_empty() {
        return -1;
    }

    let handle = match ASYNC_TOKIO_HANDLE.get() {
        Some(h) => h,
        None => {
            // Restore receivers to PROMISE_MAP before failing.
            for ((id, rx), cancelled) in id_vec
                .iter()
                .copied()
                .zip(rxs)
                .zip(cancel_vec.iter().cloned())
            {
                store_promise(id, rx, cancelled);
            }
            set_bridge_internal_error("async runtime not initialized");
            return -1;
        }
    };

    // Race loop. The future borrows `&mut rxs`, `&mut id_vec`, `&mut
    // cancel_vec`, and `&mut collected` rather than owning them — so
    // when `tokio::time::timeout` drops it on timeout, the residue
    // (still-pending promises and accumulated rejections) survives in
    // outer scope. swap_remove inside the loop keeps the three vecs
    // parallel with each rejection consumed.
    let mut collected: Vec<AggregateRejection> = Vec::new();
    let race_fut = async {
        loop {
            if rxs.is_empty() {
                return Err::<(u64, AsyncResult), ()>(());
            }
            let (idx, recv_result) = poll_fn(|cx| {
                for (i, rx) in rxs.iter_mut().enumerate() {
                    if let Poll::Ready(r) = Pin::new(rx).poll(cx) {
                        return Poll::Ready((i, r));
                    }
                }
                Poll::Pending
            })
            .await;
            let id = id_vec.swap_remove(idx);
            drop(rxs.swap_remove(idx));
            let _ = cancel_vec.swap_remove(idx);
            match recv_result {
                Ok(r) if r.success => return Ok((id, r)),
                Ok(r) => {
                    let cls = r
                        .exception_class
                        .clone()
                        .unwrap_or_else(|| "OxPHP\\Async\\AsyncException".to_string());
                    let msg = r
                        .exception_message
                        .clone()
                        .unwrap_or_else(|| "promise rejected without message".to_string());
                    collected.push(AggregateRejection {
                        promise_id: id,
                        exception_class: cls,
                        message: msg,
                    });
                }
                Err(_) => {
                    // Channel closed (worker dropped). Treat as rejection.
                    collected.push(AggregateRejection {
                        promise_id: id,
                        exception_class: "OxPHP\\Async\\AsyncException".to_string(),
                        message: "promise channel closed unexpectedly".to_string(),
                    });
                }
            }
        }
    };

    enum AnyOutcome {
        Winner(u64, AsyncResult),
        AllRejected,
        Timeout,
    }

    let outcome = if timeout > 0.0 {
        let dur = Duration::from_secs_f64(timeout);
        // Construct `Sleep` inside `block_on` so the runtime context is
        // established before the timer driver registers.
        match handle.block_on(async move { tokio::time::timeout(dur, race_fut).await }) {
            Ok(Ok((id, r))) => AnyOutcome::Winner(id, r),
            Ok(Err(())) => AnyOutcome::AllRejected,
            Err(_elapsed) => AnyOutcome::Timeout,
        }
    } else {
        match handle.block_on(race_fut) {
            Ok((id, r)) => AnyOutcome::Winner(id, r),
            Err(()) => AnyOutcome::AllRejected,
        }
    };

    // Push collected rejections in input-array position order into the
    // C-bridge aggregate buffer. Used by both the all-rejected (-3) and
    // timeout (-2) paths.
    let push_collected_in_position_order =
        |collected: &mut Vec<AggregateRejection>, input_ids: &[u64]| {
            let position: std::collections::HashMap<u64, usize> = input_ids
                .iter()
                .enumerate()
                .map(|(i, &id)| (id, i))
                .collect();
            let mut sorted: Vec<AggregateRejection> = std::mem::take(collected);
            sorted.sort_by_key(|r| position.get(&r.promise_id).copied().unwrap_or(usize::MAX));

            // Free frozen-zval state for each rejected promise. Safe here
            // because a promise only enters `collected` after its rx has
            // resolved — i.e., the worker thread is done touching the
            // captured zvals and the closure's op_array.
            //
            // Pending promises on the timeout (-2) path are intentionally
            // NOT cleaned up here. Their workers are still running on
            // dedicated OS threads and still reading the borrowed/frozen
            // zvals + holding the closure refcount; calling
            // cleanup_promise on them would unfreeze captures (letting
            // PHP free buffers the worker still reads) and release the
            // closure (potentially freeing the op_array mid-execution) —
            // a use-after-free. The timeout branch instead stashes the
            // (id, rx, cancelled) tuples in PROMISE_STRANDED;
            // cleanup_outstanding_promises_callback at RSHUTDOWN
            // block_on's each rx (5 s budget) before unfreezing the
            // matching PROMISE_CLEANUP entry, so cleanup only runs after
            // the worker actually finishes.
            for r in &sorted {
                cleanup_promise(r.promise_id);
            }

            ffi::oxphp_bridge_aggregate_clear();
            for r in &sorted {
                let cls = CString::new(r.exception_class.as_str()).unwrap_or_default();
                let msg = CString::new(r.message.as_str()).unwrap_or_default();
                ffi::oxphp_bridge_aggregate_push(cls.as_ptr(), msg.as_ptr(), r.promise_id as i64);
            }
        };

    match outcome {
        AnyOutcome::Winner(winner_id, mut result) => {
            // Restore non-winner pending receivers to PROMISE_MAP.
            // After race_fut returned with the winner, id_vec/rxs/cancel_vec
            // hold exactly the still-pending non-winner non-rejected entries
            // (race_fut swap_removed both rejected and the winning entry
            // before returning).
            for ((id, rx), cancelled) in id_vec.into_iter().zip(rxs).zip(cancel_vec) {
                store_promise(id, rx, cancelled);
            }
            cleanup_promise(winner_id);
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
        }
        AnyOutcome::AllRejected => {
            push_collected_in_position_order(&mut collected, &input_ids);
            ffi::oxphp_bridge_aggregate_throw();
            -3
        }
        AnyOutcome::Timeout => {
            // Stash (id, rx, cancelled) tuples for every still-pending
            // promise into PROMISE_STRANDED. After race_fut was dropped
            // by tokio::time::timeout, id_vec/rxs/cancel_vec hold exactly
            // these — race_fut had already swap_removed each promise it
            // managed to record as a rejection. RSHUTDOWN's cleanup
            // callback block_on's each stranded rx (5 s per promise)
            // before unfreezing the matching PROMISE_CLEANUP entry, so
            // workers finish touching the frozen captures before they're
            // released. Each stranded worker can therefore extend
            // RSHUTDOWN by up to 5 s — tracked via async_tasks_stranded.
            //
            // We also signal cancellation on every cancel flag so the
            // worker can return early at the next vm_interrupt poll
            // instead of running to completion.
            let pending_ids: Vec<i64> = id_vec.iter().map(|&id| id as i64).collect();
            let stranded_count = pending_ids.len() as u64;
            for ((id, rx), cancelled) in id_vec.into_iter().zip(rxs).zip(cancel_vec) {
                // Release: pairs with the worker's Acquire load of the cancel flag.
                cancelled
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Release);
                // Break any CPU-bound task out of its loop — the flag alone
                // reaches only suspended fibers.
                unsafe { kick_worker_interrupt(&cancelled) };
                stash_stranded(id, rx, cancelled);
            }

            if let Some(m) = get_async_metrics() {
                m.async_tasks_stranded(stranded_count);
            }

            push_collected_in_position_order(&mut collected, &input_ids);
            ffi::oxphp_bridge_aggregate_throw_timeout(
                pending_ids.as_ptr(),
                pending_ids.len() as u32,
            );
            -2
        }
    }
}

/// Release a promise's frozen and borrowed captures: unfreeze frozen zvals,
/// restore borrowed proxy zvals, and drop the closure object reference. Must run
/// on the owning worker thread. Shared by the normal-completion, per-fiber
/// deferred, and RSHUTDOWN drain paths.
unsafe fn run_promise_cleanup(cleanup: &PromiseCleanup) {
    use crate::bridge::ffi;

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

/// Restore frozen and borrowed zvals for a completed/timed-out promise.
unsafe fn cleanup_promise(id: u64) {
    PROMISE_OWNER.with(|m| {
        m.borrow_mut().remove(&id);
    });
    if let Some(cleanup) = take_promise_cleanup(id) {
        run_promise_cleanup(&cleanup);
    }
}

/// RSHUTDOWN callback: clean up any async promises that were dispatched but
/// never awaited by user code.  Thread-wide drain — correct only when no
/// other request shares this thread's promise maps: traditional mode (one
/// request per thread at a time) and final worker-thread teardown.  The
/// worker-mode per-request path uses `cleanup_promises_for_fiber_callback`
/// instead.  Safe to call when the maps are empty (returns immediately).
///
/// # Safety
/// Called from C FFI (RSHUTDOWN).
unsafe extern "C" fn cleanup_outstanding_promises_callback() {
    let ids = outstanding_promise_ids();
    // Forget all ownership records — including stale ones for fully-consumed
    // promises. Nothing outlives a thread-wide drain.
    PROMISE_OWNER.with(|m| m.borrow_mut().clear());
    if !ids.is_empty() {
        tracing::warn!(count = ids.len(), "Cleaning up non-awaited async promises");
        drain_promises(&ids);
    }
    // Flush promises deferred by the per-fiber path that were not yet polled to
    // completion before this thread began tearing down (blocking is fine here —
    // the thread is exiting and nothing else shares it).
    flush_deferred_drains();
}

/// Worker-mode per-request callback: clean up promises owned by the request
/// fiber that just completed, leaving sibling fibers' live promises alone.
/// Called from the fiber scheduler's finalize path via the bridge.
///
/// # Safety
/// Called from C FFI on the owning worker thread.
unsafe extern "C" fn cleanup_promises_for_fiber_callback(fiber_id: u64) {
    // Collect and forget this fiber's ownership records in one pass.
    let owned: Vec<u64> = PROMISE_OWNER.with(|m| {
        let mut ids = Vec::new();
        m.borrow_mut().retain(|&id, &mut owner| {
            if owner == fiber_id {
                ids.push(id);
                false
            } else {
                true
            }
        });
        ids
    });
    if owned.is_empty() {
        return;
    }
    // Keep only ids still present in a promise map — a record whose promise
    // was fully consumed carries nothing to drain.
    let live: Vec<u64> = owned
        .into_iter()
        .filter(|id| {
            PROMISE_MAP.with(|m| m.borrow().contains_key(id))
                || PROMISE_CLEANUP.with(|m| m.borrow().contains_key(id))
                || PROMISE_STRANDED.with(|m| m.borrow().contains_key(id))
        })
        .collect();
    if live.is_empty() {
        return;
    }
    tracing::warn!(
        count = live.len(),
        fiber_id,
        "Deferring cleanup of non-awaited async promises"
    );
    // Defer instead of block_on'ing on the worker thread: finalize returns
    // immediately and the scheduler tick reclaims each promise once its task
    // settles. Worker mode keeps the request heap alive across handlers, so the
    // unfreeze can safely happen after the response is sent.
    for id in live {
        defer_promise_drain(id);
    }
}

/// Cancel and drain a batch of promises: signal cancellation, wait for each
/// worker to acknowledge (5 s budget per promise), then release frozen and
/// borrowed captures. Blocking — reserved for the RSHUTDOWN / worker-teardown
/// drain-all path, where the thread is exiting and no scheduler shares it. The
/// worker-mode per-request path defers instead (see `defer_promise_drain`).
unsafe fn drain_promises(ids: &[u64]) {
    for &id in ids {
        // Receivers can live in either PROMISE_MAP (never-awaited) or
        // PROMISE_STRANDED (await_race / await_any timed out). Drain
        // both before unfreezing — the rx must complete (or hit the 5 s
        // budget) before the matching PROMISE_CLEANUP entry releases
        // frozen captures, otherwise the still-running worker observes
        // freed memory.
        let entry = take_promise(id).or_else(|| take_stranded(id));
        if let Some((rx, cancelled)) = entry {
            // Signal cancellation (Release pairs with the worker's Acquire load).
            cancelled
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
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
            run_promise_cleanup(&cleanup);
        }
    }
}

/// Move a finished fiber's orphaned promise into the deferred-drain list:
/// signal cancellation, then hand its receiver and cleanup to the scheduler
/// tick, which releases the captures once the task settles. Non-blocking —
/// unlike `drain_promises`, it never `block_on`s the worker thread.
///
/// If the receiver was already consumed (only a cleanup record remains) there
/// is nothing to wait on, so the cleanup runs immediately — no ordering hazard.
unsafe fn defer_promise_drain(id: u64) {
    match take_promise(id).or_else(|| take_stranded(id)) {
        Some((rx, cancelled)) => {
            // Signal cancellation (Release pairs with the worker's Acquire
            // load). The flag lives in the async side's clone, so dropping our
            // `cancelled` here is fine — we only needed to set it.
            cancelled
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            let cleanup = take_promise_cleanup(id);
            DEFERRED_DRAIN.with(|d| {
                d.borrow_mut().push(DeferredDrain {
                    rx,
                    cleanup,
                    deadline: Instant::now() + std::time::Duration::from_secs(5),
                });
            });
        }
        None => {
            if let Some(cleanup) = take_promise_cleanup(id) {
                run_promise_cleanup(&cleanup);
            }
        }
    }
}

/// Scheduler-tick callback (worker mode): release each deferred promise whose
/// task has settled or whose 5 s budget has elapsed. Non-blocking — ready
/// entries are removed under the borrow, then their captures are released after
/// the borrow is dropped.
///
/// # Safety
/// Called from C FFI on the owning HTTP worker thread.
unsafe extern "C" fn poll_deferred_drains_callback() {
    use tokio::sync::oneshot::error::TryRecvError;

    let now = Instant::now();
    let ready: Vec<DeferredDrain> = DEFERRED_DRAIN.with(|d| {
        let mut list = d.borrow_mut();
        let mut out = Vec::new();
        let mut i = 0;
        while i < list.len() {
            let settled = match list[i].rx.try_recv() {
                Ok(_) | Err(TryRecvError::Closed) => true,
                Err(TryRecvError::Empty) => now >= list[i].deadline,
            };
            if settled {
                out.push(list.swap_remove(i));
            } else {
                i += 1;
            }
        }
        out
    });
    for entry in &ready {
        if let Some(cleanup) = &entry.cleanup {
            run_promise_cleanup(cleanup);
        }
    }
}

/// Scheduler-loop predicate (worker mode): non-zero while any deferred promise
/// drain is still pending, so the worker keeps ticking (and reclaiming them)
/// instead of blocking in `worker_wait`.
///
/// # Safety
/// Called from C FFI on the owning HTTP worker thread.
unsafe extern "C" fn has_deferred_drains_callback() -> c_int {
    DEFERRED_DRAIN.with(|d| c_int::from(!d.borrow().is_empty()))
}

/// Blocking teardown flush of the deferred-drain list (RSHUTDOWN / worker
/// teardown): block on each remaining rx (bounded by its own 5 s budget) before
/// releasing its captures. Safe to block here — the thread is exiting and no
/// scheduler shares it.
unsafe fn flush_deferred_drains() {
    let pending: Vec<DeferredDrain> = DEFERRED_DRAIN.with(|d| d.borrow_mut().drain(..).collect());
    for entry in pending {
        if let Some(handle) = ASYNC_TOKIO_HANDLE.get() {
            let remaining = entry.deadline.saturating_duration_since(Instant::now());
            let _ = handle.block_on(async { tokio::time::timeout(remaining, entry.rx).await });
        } else {
            drop(entry.rx);
        }
        if let Some(cleanup) = &entry.cleanup {
            run_promise_cleanup(cleanup);
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

        // Non-blocking drain of finished fibers' orphaned promises (worker mode)
        crate::bridge::ffi::oxphp_bridge_set_deferred_drain_callbacks(
            Some(poll_deferred_drains_callback),
            Some(has_deferred_drains_callback),
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
        crate::bridge::ffi::oxphp_bridge_set_await_race_dispatch(Some(
            await_race_dispatch_callback,
        ));
        crate::bridge::ffi::oxphp_bridge_set_await_any_dispatch(Some(await_any_dispatch_callback));
        crate::bridge::ffi::oxphp_bridge_set_cleanup_promises(Some(
            cleanup_outstanding_promises_callback,
        ));
        crate::bridge::ffi::oxphp_bridge_set_cleanup_promises_for_fiber(Some(
            cleanup_promises_for_fiber_callback,
        ));
    }
}

// ─── HTTP Object API: Bridge callbacks for lazy request data access ───

/// Helper macro for bridge callbacks that return a string field from RequestData.
macro_rules! req_str_callback {
    ($name:ident, $field:ident) => {
        unsafe extern "C" fn $name(out_len: *mut usize) -> *const c_char {
            REQUEST_DATA.with(|rd| {
                let data = rd.borrow();
                if !data.active {
                    *out_len = 0;
                    return std::ptr::null();
                }
                *out_len = data.$field.len();
                data.$field.as_ptr() as *const c_char
            })
        }
    };
}

req_str_callback!(req_method_cb, method_str);
req_str_callback!(req_path_cb, path_str);
req_str_callback!(req_full_uri_cb, full_uri_str);
req_str_callback!(req_scheme_cb, scheme_str);
req_str_callback!(req_host_cb, host_str);
req_str_callback!(req_query_string_cb, query_string_raw);
req_str_callback!(req_ip_cb, remote_addr_str);
req_str_callback!(req_protocol_version_cb, protocol_version_str);

unsafe extern "C" fn req_port_cb() -> u16 {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active {
            return 0;
        }
        data.port_val
    })
}

unsafe extern "C" fn req_start_time_cb() -> f64 {
    // ctx.request_time is the single source of truth: 0.0 outside an
    // active request (enforced by worker_send_callback,
    // RequestDataGuard::drop and the worker boot reset), now() during
    // request handling. No REQUEST_DATA borrow needed.
    bindings::oxphp_bridge_get_request_time()
}

unsafe extern "C" fn req_is_secure_cb() -> c_int {
    REQUEST_DATA.with(|rd| if rd.borrow().is_secure { 1 } else { 0 })
}

unsafe extern "C" fn req_content_type_cb(out_len: *mut usize) -> *const c_char {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active {
            *out_len = 0;
            return std::ptr::null();
        }
        // Find content-type in headers_raw
        for (k, v) in &data.headers_raw {
            if k == "content-type" {
                *out_len = v.len();
                return v.as_ptr() as *const c_char;
            }
        }
        *out_len = 0;
        std::ptr::null()
    })
}

unsafe extern "C" fn req_header_cb(
    name: *const c_char,
    name_len: usize,
    out_len: *mut usize,
) -> *const c_char {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active {
            *out_len = 0;
            return std::ptr::null();
        }
        let key = std::slice::from_raw_parts(name as *const u8, name_len);
        let key_str = std::str::from_utf8_unchecked(key);
        for (k, v) in &data.headers_raw {
            if k.eq_ignore_ascii_case(key_str) {
                *out_len = v.len();
                return v.as_ptr() as *const c_char;
            }
        }
        *out_len = 0;
        std::ptr::null()
    })
}

unsafe extern "C" fn req_cookie_cb(
    name: *const c_char,
    name_len: usize,
    out_len: *mut usize,
) -> *const c_char {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active {
            *out_len = 0;
            return std::ptr::null();
        }
        let key = std::slice::from_raw_parts(name as *const u8, name_len);
        let key_str = std::str::from_utf8_unchecked(key);
        for (k, v) in &data.cookies_parsed {
            if k == key_str {
                *out_len = v.len();
                return v.as_ptr() as *const c_char;
            }
        }
        *out_len = 0;
        std::ptr::null()
    })
}

unsafe extern "C" fn req_query_param_cb(
    key: *const c_char,
    key_len: usize,
    out_len: *mut usize,
) -> *const c_char {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active || data.query_string_raw.is_empty() {
            *out_len = 0;
            return std::ptr::null();
        }
        let search = std::slice::from_raw_parts(key as *const u8, key_len);
        let search_str = std::str::from_utf8_unchecked(search);
        // Simple query string parsing for single-value lookup
        for pair in data.query_string_raw.split('&') {
            if let Some(eq) = pair.find('=') {
                if &pair[..eq] == search_str {
                    let val = &pair[eq + 1..];
                    *out_len = val.len();
                    return val.as_ptr() as *const c_char;
                }
            } else if pair == search_str {
                *out_len = 0;
                return c"".as_ptr(); // empty value, not null
            }
        }
        *out_len = 0;
        std::ptr::null()
    })
}

type PairsCb = unsafe extern "C" fn(*const c_char, usize, *const c_char, usize, *mut c_void);

unsafe extern "C" fn req_headers_all_cb(cb: PairsCb, user_data: *mut c_void) {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active {
            return;
        }
        for (k, v) in &data.headers_raw {
            cb(
                k.as_ptr() as *const c_char,
                k.len(),
                v.as_ptr() as *const c_char,
                v.len(),
                user_data,
            );
        }
    });
}

unsafe extern "C" fn req_cookies_all_cb(cb: PairsCb, user_data: *mut c_void) {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active {
            return;
        }
        for (k, v) in &data.cookies_parsed {
            cb(
                k.as_ptr() as *const c_char,
                k.len(),
                v.as_ptr() as *const c_char,
                v.len(),
                user_data,
            );
        }
    });
}

unsafe extern "C" fn req_query_params_all_cb(cb: PairsCb, user_data: *mut c_void) {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active || data.query_string_raw.is_empty() {
            return;
        }
        for pair in data.query_string_raw.split('&') {
            if let Some(eq) = pair.find('=') {
                let k = &pair[..eq];
                let v = &pair[eq + 1..];
                cb(
                    k.as_ptr() as *const c_char,
                    k.len(),
                    v.as_ptr() as *const c_char,
                    v.len(),
                    user_data,
                );
            } else {
                cb(
                    pair.as_ptr() as *const c_char,
                    pair.len(),
                    c"".as_ptr(),
                    0,
                    user_data,
                );
            }
        }
    });
}

unsafe extern "C" fn req_body_cb(out_len: *mut usize) -> *const u8 {
    REQUEST_DATA.with(|rd| {
        let data = rd.borrow();
        if !data.active || data.body.is_empty() {
            *out_len = 0;
            return std::ptr::null();
        }
        *out_len = data.body.len();
        data.body.as_ptr()
    })
}

unsafe extern "C" fn req_is_active_cb() -> c_int {
    REQUEST_DATA.with(|rd| if rd.borrow().active { 1 } else { 0 })
}

/// Register all request accessor callbacks with the bridge.
/// Must be called once at startup before any request processing.
pub fn register_request_accessors() {
    unsafe {
        bindings::oxphp_bridge_set_request_accessors(
            Some(req_method_cb),
            Some(req_path_cb),
            Some(req_full_uri_cb),
            Some(req_scheme_cb),
            Some(req_host_cb),
            Some(req_port_cb),
            Some(req_query_string_cb),
            Some(req_header_cb),
            Some(req_cookie_cb),
            Some(req_ip_cb),
            Some(req_protocol_version_cb),
            Some(req_start_time_cb),
            Some(req_is_secure_cb),
            Some(req_content_type_cb),
            Some(req_query_param_cb),
            Some(req_headers_all_cb),
            Some(req_cookies_all_cb),
            Some(req_query_params_all_cb),
            Some(req_body_cb),
            Some(req_is_active_cb),
        );
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

    // Serializes tests that mutate the global mock cancel-reason pointer so
    // they don't observe each other's setup. Tests that don't touch the
    // pointer (and rely on the default null/None reading) also acquire it
    // because a concurrent set-then-clear from another test would leak a
    // non-zero reason into their window.
    static FATAL_STATUS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn fatal_error_status_sets_500_from_default() {
        let _g = FATAL_STATUS_LOCK.lock().unwrap();
        unsafe { bindings::oxphp_bridge_set_cancel_ptr(std::ptr::null()) };
        RESPONSE.with(|r| r.borrow_mut().status_code = 200);
        set_fatal_error_status_if_default();
        RESPONSE.with(|r| assert_eq!(r.borrow().status_code, 500));
    }

    #[test]
    fn fatal_error_status_idempotent() {
        let _g = FATAL_STATUS_LOCK.lock().unwrap();
        unsafe { bindings::oxphp_bridge_set_cancel_ptr(std::ptr::null()) };
        RESPONSE.with(|r| r.borrow_mut().status_code = 200);
        set_fatal_error_status_if_default();
        set_fatal_error_status_if_default();
        RESPONSE.with(|r| assert_eq!(r.borrow().status_code, 500));
    }

    #[test]
    fn fatal_error_status_preserves_non_200() {
        let _g = FATAL_STATUS_LOCK.lock().unwrap();
        unsafe { bindings::oxphp_bridge_set_cancel_ptr(std::ptr::null()) };
        RESPONSE.with(|r| r.borrow_mut().status_code = 404);
        set_fatal_error_status_if_default();
        RESPONSE.with(|r| assert_eq!(r.borrow().status_code, 404));
    }

    #[test]
    fn fatal_error_status_maps_cancel_reason() {
        let _g = FATAL_STATUS_LOCK.lock().unwrap();
        let cell = std::sync::atomic::AtomicU8::new(0);
        unsafe { bindings::oxphp_bridge_set_cancel_ptr(&cell as *const _) };

        // (reason, expected_status). 0 None / 4 Stuck / 5 UserCancel → 500.
        for (reason, expected) in [
            (0u8, 500u16),
            (1, 499),
            (2, 504),
            (3, 503),
            (4, 500),
            (5, 500),
        ] {
            cell.store(reason, std::sync::atomic::Ordering::Relaxed);
            RESPONSE.with(|r| r.borrow_mut().status_code = 200);
            set_fatal_error_status_if_default();
            RESPONSE.with(|r| {
                assert_eq!(
                    r.borrow().status_code,
                    expected,
                    "cancel reason {reason} should map to {expected}"
                )
            });
        }

        unsafe { bindings::oxphp_bridge_set_cancel_ptr(std::ptr::null()) };
    }

    #[test]
    fn collect_status_cancel_reason_beats_sg() {
        // A server-side cancellation makes RESPONSE (already 504/503/499 via
        // set_fatal_error_status_if_default) authoritative — SG must be ignored,
        // whatever it holds. Two concrete regressions this guards:
        //   * PHP-native timeout: the fatal aborts with SG still 200, so PHP's own
        //     handler stamps a generic 500 into SG (main/main.c). Adopting it would
        //     mask a 504 timeout as a 500 crash.
        //   * A stale explicit non-200 the script set before the cancel (e.g. a 206
        //     that never completed) must not survive the cut-off.
        for reason in [
            1u8, /*abort*/
            2,   /*timeout*/
            3,   /*shutdown*/
            4,   /*stuck*/
            5,   /*user*/
        ] {
            assert_eq!(
                resolve_collected_status(500, reason),
                None,
                "reason {reason}, SG 500"
            );
            assert_eq!(
                resolve_collected_status(206, reason),
                None,
                "reason {reason}, SG 206"
            );
        }
    }

    #[test]
    fn collect_status_adopts_explicit_sg_when_not_cancelled() {
        // No cancellation: SG is the script's authoritative choice. An explicit
        // non-200 wins even over a fatal-500 already on RESPONSE:
        // http_response_code(503) before an uncaught throw ships 503, and a worker
        // http_response_code(201) ships 201.
        assert_eq!(resolve_collected_status(503, 0), Some(503));
        assert_eq!(resolve_collected_status(201, 0), Some(201));
        // A 200 or an unset 0 leaves RESPONSE untouched so the fatal / cancel
        // fallbacks keep whatever they stamped.
        assert_eq!(resolve_collected_status(200, 0), None);
        assert_eq!(resolve_collected_status(0, 0), None);
    }

    #[test]
    fn await_poll_returns_false_for_unknown_id() {
        // Unknown promise ID should return false
        assert!(!await_is_ready(99999));
    }

    #[test]
    fn await_poll_returns_true_when_result_sent() {
        let (tx, rx) = tokio::sync::oneshot::channel::<AsyncResult>();
        let cancelled = std::sync::Arc::new(CancelShared::new());
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
            keepalive: None,
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
        let cancelled = std::sync::Arc::new(CancelShared::new());
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

    #[test]
    fn parse_host_ipv6_bare() {
        assert_eq!(parse_host("[::1]", "80"), ("[::1]", "80"));
    }

    #[test]
    fn parse_host_ipv6_with_port() {
        assert_eq!(parse_host("[::1]:8080", "80"), ("[::1]", "8080"));
    }

    #[test]
    fn parse_host_ipv6_full_bare() {
        assert_eq!(parse_host("[2001:db8::1]", "443"), ("[2001:db8::1]", "443"));
    }

    #[test]
    fn parse_host_ipv6_full_with_port() {
        assert_eq!(
            parse_host("[2001:db8::1]:9000", "443"),
            ("[2001:db8::1]", "9000")
        );
    }

    #[test]
    fn parse_host_domain_with_port() {
        assert_eq!(
            parse_host("example.com:8080", "80"),
            ("example.com", "8080")
        );
    }

    #[test]
    fn parse_host_domain_without_port() {
        assert_eq!(parse_host("example.com", "80"), ("example.com", "80"));
    }

    #[test]
    fn parse_host_ipv4_with_port() {
        assert_eq!(parse_host("127.0.0.1:3000", "80"), ("127.0.0.1", "3000"));
    }

    #[test]
    fn parse_host_ipv4_without_port() {
        assert_eq!(parse_host("127.0.0.1", "80"), ("127.0.0.1", "80"));
    }

    #[test]
    fn denied_path_server_var_keeps_leading_slash() {
        // `OXPHP_DENIED_PATH` must carry a leading `/` — the same form as
        // `PATH_INFO`, both built from the one `original_path` buffer. A
        // SIEM/honeypot fallback script compares this value verbatim, so
        // silently dropping the `/` would blind path-based alert rules. The
        // matched glob in `OXPHP_DENIED_PATTERN` stays glob-normalized (no
        // leading `/`). End-to-end coverage also lives in
        // tests/fixtures/php_deny/public/_security/denied.php.
        unsafe { bindings::oxphp_bridge_set_superglobals_enabled(true) };

        let req = ScriptRequest {
            request_id: "test-denied".to_string(),
            script_path: std::path::PathBuf::from("/var/www/html/_security/denied.php"),
            method: http::Method::GET,
            uri: http::Uri::from_static("/uploads/shell.php"),
            query_string: String::new(),
            headers: http::HeaderMap::new(),
            body: Bytes::new(),
            remote_addr: "127.0.0.1:0".parse().unwrap(),
            document_root: Arc::new(std::path::PathBuf::from("/var/www/html")),
            cancel_state: Arc::new(crate::bridge::cancel::CancellationState::new()),
            trace_id: String::new(),
            span_id: String::new(),
            parent_span_id: String::new(),
            is_tls: false,
            version: http::Version::HTTP_11,
            path_info: None,
            forwarded_proto: None,
            forwarded_host: None,
            forwarded_port: None,
            denied_meta: Some(Arc::new(crate::config::DeniedMeta {
                path: "uploads/shell.php".to_string(),
                pattern: "uploads/**".to_string(),
                fallback_script_uri: "/_security/denied.php".to_string(),
            })),
            profiling_mode: crate::profiling::ProfilingMode::Off,
            profiling_run_id: None,
        };

        set_request_data(&req);

        let lookup = |key: &[u8]| -> Option<String> {
            REQUEST_DATA.with(|rd| {
                rd.borrow()
                    .server_vars
                    .iter()
                    .find(|(k, _)| k.as_bytes() == key)
                    .map(|(_, v)| String::from_utf8_lossy(v.as_bytes()).into_owned())
            })
        };

        assert_eq!(
            lookup(b"OXPHP_DENIED_PATH").as_deref(),
            Some("/uploads/shell.php"),
            "OXPHP_DENIED_PATH must keep the leading slash (PATH_INFO/CGI form)"
        );
        // Built from the same buffer as OXPHP_DENIED_PATH — they must agree.
        assert_eq!(
            lookup(b"PATH_INFO").as_deref(),
            Some("/uploads/shell.php"),
            "PATH_INFO and OXPHP_DENIED_PATH must agree on the leading slash"
        );
        // The matched glob is normalized without a leading slash.
        assert_eq!(
            lookup(b"OXPHP_DENIED_PATTERN").as_deref(),
            Some("uploads/**"),
            "OXPHP_DENIED_PATTERN is glob-normalized without a leading slash"
        );

        clear_request_data();
    }
}
