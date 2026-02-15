use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;
use std::time::SystemTime;

use bytes::Bytes;
use http::header;

use crate::php::bindings::{self, *};
use crate::plugin::php::{PhpCallContext, PhpValue, PluginPhpFunctionDef};
use crate::types::ScriptRequest;

thread_local! {
    static OUTPUT_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(8192));
    static HEADERS_BUFFER: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
    static STATUS_CODE: RefCell<u16> = const { RefCell::new(200) };
    static REQUEST_DATA: RefCell<RequestData> = RefCell::new(RequestData::new());
}

/// Per-request data stored in thread-local for SAPI callbacks to access.
/// Reused across requests — `server_vars` Vec capacity is retained.
struct RequestData {
    /// Pre-built $_SERVER key-value pairs as CStrings (must outlive php_request_shutdown).
    server_vars: Vec<(CString, CString)>,
    /// Raw Cookie header string for read_cookies callback (must outlive request).
    cookie_string: Option<CString>,
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

        data.body = req.body.clone();
        data.body_offset = 0;
        data.active = true;
    });
}

/// Clear request data from thread-local.
/// Retains Vec capacity for reuse by the next request.
/// Must be called AFTER php_request_shutdown().
pub fn clear_request_data() {
    REQUEST_DATA.with(|rd| {
        let mut data = rd.borrow_mut();
        data.server_vars.clear();
        data.cookie_string = None;
        data.body = Bytes::new();
        data.body_offset = 0;
        data.active = false;
    });
}

/// Build the custom SAPI module struct.
///
/// All string pointers use `b"...\0"` byte literals which have `'static` lifetime.
pub fn build_sapi_module() -> sapi_module_struct {
    sapi_module_struct {
        name: b"cli-server\0".as_ptr() as *mut c_char,
        pretty_name: b"OxPHP\0".as_ptr() as *mut c_char,

        startup: Some(oxphp_startup),
        shutdown: Some(oxphp_shutdown),

        activate: Some(oxphp_activate),
        deactivate: Some(oxphp_deactivate),

        ub_write: Some(oxphp_ub_write),
        flush: Some(oxphp_flush),
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
    HEADERS_BUFFER.with(|h| h.borrow_mut().clear());
    STATUS_CODE.with(|s| *s.borrow_mut() = 200);
    0 // SUCCESS
}

/// Called by PHP at the end of each request (during php_request_shutdown).
unsafe extern "C" fn oxphp_deactivate() -> c_int {
    0 // SUCCESS
}

// ─── Output Capture ─────────────────────────────────────────

unsafe extern "C" fn oxphp_ub_write(str: *const c_char, str_length: usize) -> usize {
    if str.is_null() || str_length == 0 {
        return 0;
    }

    let data = std::slice::from_raw_parts(str as *const u8, str_length);

    OUTPUT_BUFFER.with(|buf| {
        buf.borrow_mut().extend_from_slice(data);
    });

    str_length
}

unsafe extern "C" fn oxphp_flush(_server_context: *mut c_void) {
    // No-op: output is collected in ub_write buffer
}

// ─── Header Handling ────────────────────────────────────────

unsafe extern "C" fn oxphp_header_handler(
    sapi_header: *mut sapi_header_struct,
    op: sapi_header_op_enum,
    _sapi_headers: *mut sapi_headers_struct,
) -> c_int {
    match op {
        sapi_header_op_enum::SAPI_HEADER_DELETE_ALL => {
            HEADERS_BUFFER.with(|buf| buf.borrow_mut().clear());
            return 0;
        }
        sapi_header_op_enum::SAPI_HEADER_SET_STATUS => {
            let code = sapi_header as usize as u16;
            if (100..600).contains(&code) {
                STATUS_CODE.with(|sc| *sc.borrow_mut() = code);
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
            HEADERS_BUFFER.with(|buf| {
                buf.borrow_mut()
                    .retain(|(n, _)| !n.eq_ignore_ascii_case(name));
            });
        }
        sapi_header_op_enum::SAPI_HEADER_REPLACE | sapi_header_op_enum::SAPI_HEADER_ADD => {
            if let Some(colon_pos) = header_str.find(':') {
                let name = header_str[..colon_pos].trim().to_string();
                let value = header_str[colon_pos + 1..].trim().to_string();

                HEADERS_BUFFER.with(|buf| {
                    let mut headers = buf.borrow_mut();
                    if op == sapi_header_op_enum::SAPI_HEADER_REPLACE {
                        headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                    }
                    headers.push((name, value));
                });
            }
        }
        _ => {}
    }

    0
}

unsafe extern "C" fn oxphp_send_headers(sapi_headers: *mut sapi_headers_struct) -> c_int {
    if !sapi_headers.is_null() {
        let code = (*sapi_headers).http_response_code;
        if code > 0 {
            STATUS_CODE.with(|sc| {
                *sc.borrow_mut() = code as u16;
            });
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

/// Take output, headers, and status code in a single TLS lookup batch.
/// Avoids 3 separate thread-local accesses + RefCell borrows.
pub fn take_response() -> (Vec<u8>, Vec<(String, String)>, u16) {
    let output = OUTPUT_BUFFER.with(|buf| std::mem::take(&mut *buf.borrow_mut()));
    let headers = HEADERS_BUFFER.with(|buf| std::mem::take(&mut *buf.borrow_mut()));
    let status = STATUS_CODE.with(|sc| {
        let code = *sc.borrow();
        *sc.borrow_mut() = 200;
        code
    });
    (output, headers, status)
}

pub fn clear_buffers() {
    OUTPUT_BUFFER.with(|buf| buf.borrow_mut().clear());
    HEADERS_BUFFER.with(|buf| buf.borrow_mut().clear());
    STATUS_CODE.with(|sc| *sc.borrow_mut() = 200);
}

// ─── Plugin PHP Function Bridge ─────────────────────────────

/// Global registry of plugin PHP function definitions.
/// Set once from main.rs after plugin_manager.init_all().
static PLUGIN_FUNCTIONS: OnceLock<Vec<PluginPhpFunctionDef>> = OnceLock::new();

/// Register plugin functions on the bridge and store handlers for dispatch.
/// Called from main.rs after plugin_manager.init_all().
pub fn register_plugin_functions(fns: Vec<PluginPhpFunctionDef>) {
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
        bindings::oxphp_bridge_set_dispatch_fn(Some(dispatch_callback));
    }
    let count = fns.len();
    PLUGIN_FUNCTIONS.set(fns).ok();
    tracing::info!(count, "Plugin PHP functions registered on bridge");
}

/// Dispatch callback invoked from C extension via bridge.
///
/// # Safety
/// Called from C code. `name` and `json_args` must be valid C strings.
unsafe extern "C" fn dispatch_callback(
    name: *const c_char,
    json_args: *const c_char,
) -> *mut c_char {
    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return make_error_envelope("invalid UTF-8 in function name"),
    };

    let args_str = match CStr::from_ptr(json_args).to_str() {
        Ok(s) => s,
        Err(_) => return make_error_envelope("invalid UTF-8 in json_args"),
    };

    let fns = match PLUGIN_FUNCTIONS.get() {
        Some(f) => f,
        None => return make_error_envelope("plugin functions not initialized"),
    };

    let func_def = match fns.iter().find(|f| f.name == name_str) {
        Some(f) => f,
        None => {
            return make_error_envelope(&format!("function not found: {name_str}"));
        }
    };

    // Deserialize JSON args → Vec<PhpValue>
    let args: Vec<PhpValue> = match serde_json::from_str::<serde_json::Value>(args_str) {
        Ok(serde_json::Value::Array(arr)) => arr.iter().map(PhpValue::from_json_value).collect(),
        Ok(_) => return make_error_envelope("json_args must be an array"),
        Err(e) => return make_error_envelope(&format!("JSON parse error: {e}")),
    };

    // Catch panics — unwinding through extern "C" is an abort on Rust 2021.
    let ctx = PhpCallContext::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        func_def.handler.handle(&ctx, &args)
    }));

    match result {
        Ok(Ok(value)) => {
            let json_val = value.to_json_value();
            let envelope = serde_json::json!({ "ok": json_val });
            let envelope_str = envelope.to_string();
            match CString::new(envelope_str) {
                Ok(c_str) => bindings::oxphp_bridge_strdup(c_str.as_ptr()),
                Err(_) => make_error_envelope("result contains null byte"),
            }
        }
        Ok(Err(e)) => make_error_envelope(&e.to_string()),
        Err(_) => make_error_envelope("handler panicked"),
    }
}

/// Create an error envelope string, allocated via bridge strdup.
unsafe fn make_error_envelope(msg: &str) -> *mut c_char {
    let envelope = serde_json::json!({ "err": msg });
    let s = envelope.to_string();
    // NUL bytes in error messages are impossible (JSON-encoded string escapes them),
    // but handle defensively to avoid UB.
    let c_str = CString::new(s).unwrap_or_else(|_| {
        CString::new("{\"err\":\"error message contained null byte\"}").unwrap()
    });
    bindings::oxphp_bridge_strdup(c_str.as_ptr())
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
