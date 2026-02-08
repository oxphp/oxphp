use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::time::SystemTime;

use bytes::Bytes;
use http::header;

use crate::php::bindings::*;
use crate::types::ScriptRequest;

thread_local! {
    static OUTPUT_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(8192));
    static HEADERS_BUFFER: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
    static STATUS_CODE: RefCell<u16> = const { RefCell::new(200) };
    static REQUEST_DATA: RefCell<Option<RequestData>> = const { RefCell::new(None) };
}

/// Per-request data stored in thread-local for SAPI callbacks to access.
struct RequestData {
    /// Pre-built $_SERVER key-value pairs as CStrings (must outlive php_request_shutdown).
    server_vars: Vec<(CString, CString)>,
    /// Raw Cookie header string for read_cookies callback (must outlive request).
    cookie_string: Option<CString>,
    /// Request body for read_post callback.
    body: Bytes,
    /// How many bytes of body have been read so far.
    body_offset: usize,
}

/// Build request data from a ScriptRequest and store in thread-local.
/// Must be called BEFORE php_request_startup().
pub fn set_request_data(req: &ScriptRequest) {
    let mut server_vars = Vec::with_capacity(32);

    // Helper to add a server var (skips entries with embedded null bytes)
    let mut add = |key: &str, val: &str| {
        if let (Ok(k), Ok(v)) = (CString::new(key), CString::new(val)) {
            server_vars.push((k, v));
        }
    };

    // CGI/1.1 standard variables
    add("REQUEST_METHOD", req.method.as_str());
    add("REQUEST_URI", &req.uri.to_string());
    add("QUERY_STRING", &req.query_string);
    add("SERVER_PROTOCOL", "HTTP/1.1");

    // SCRIPT_NAME: URI path without query string
    let path = req.uri.path();
    add("SCRIPT_NAME", path);
    add("PHP_SELF", path);

    // SCRIPT_FILENAME: absolute filesystem path to the script
    add("SCRIPT_FILENAME", &req.script_path.to_string_lossy());

    // DOCUMENT_ROOT
    add("DOCUMENT_ROOT", &req.document_root.to_string_lossy());

    // Server identification
    add("SERVER_SOFTWARE", "OxPHP/0.1.0");
    add("GATEWAY_INTERFACE", "CGI/1.1");

    // Connection info
    add("REMOTE_ADDR", &req.remote_addr.ip().to_string());
    add("REMOTE_PORT", &req.remote_addr.port().to_string());

    // SERVER_NAME and SERVER_PORT from Host header
    if let Some(host) = req.headers.get(header::HOST) {
        if let Ok(host_str) = host.to_str() {
            if let Some(colon) = host_str.rfind(':') {
                add("SERVER_NAME", &host_str[..colon]);
                add("SERVER_PORT", &host_str[colon + 1..]);
            } else {
                add("SERVER_NAME", host_str);
                add("SERVER_PORT", "80");
            }
        }
    } else {
        add("SERVER_NAME", "localhost");
        add("SERVER_PORT", "80");
    }

    // CONTENT_TYPE and CONTENT_LENGTH (no HTTP_ prefix per CGI spec)
    if let Some(ct) = req.headers.get(header::CONTENT_TYPE) {
        if let Ok(ct_str) = ct.to_str() {
            add("CONTENT_TYPE", ct_str);
        }
    }
    if let Some(cl) = req.headers.get(header::CONTENT_LENGTH) {
        if let Ok(cl_str) = cl.to_str() {
            add("CONTENT_LENGTH", cl_str);
        }
    }

    // Extract cookie string for read_cookies callback
    let cookie_string = req
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
        add(&header_buf, val_str);
    }

    let data = RequestData {
        server_vars,
        cookie_string,
        body: req.body.clone(),
        body_offset: 0,
    };

    REQUEST_DATA.with(|rd| {
        *rd.borrow_mut() = Some(data);
    });
}

/// Clear request data from thread-local.
/// Must be called AFTER php_request_shutdown().
pub fn clear_request_data() {
    REQUEST_DATA.with(|rd| {
        *rd.borrow_mut() = None;
    });
}

/// Build the custom SAPI module struct.
///
/// All string pointers use `b"...\0"` byte literals which have `'static` lifetime.
pub fn build_sapi_module() -> sapi_module_struct {
    sapi_module_struct {
        name: b"oxphp\0".as_ptr() as *mut c_char,
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
        let borrow = rd.borrow();
        if let Some(data) = borrow.as_ref() {
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
        let borrow = rd.borrow();
        match borrow.as_ref().and_then(|d| d.cookie_string.as_ref()) {
            Some(cs) => cs.as_ptr() as *mut c_char,
            None => std::ptr::null_mut(),
        }
    })
}

/// Callback: read POST body data for PHP to parse into $_POST/$_FILES/php://input.
/// Called repeatedly until it returns 0.
unsafe extern "C" fn oxphp_read_post(buffer: *mut c_char, count_bytes: usize) -> usize {
    REQUEST_DATA.with(|rd| {
        let mut borrow = rd.borrow_mut();
        let data = match borrow.as_mut() {
            Some(d) => d,
            None => return 0,
        };

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
