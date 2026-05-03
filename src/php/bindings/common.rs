#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};

// Diverging types live in per-version modules and are re-exported by
// `super` (mod.rs). Importing here keeps the extern blocks below readable
// and gives a single point of failure if the wiring breaks.
#[cfg(any(php_v8_4, php_v8_5))]
use super::sapi_module_struct;

pub type zend_result = c_int;

#[repr(C)]
pub struct zend_module_entry {
    _opaque: [u8; 0],
}

#[repr(C)]
#[allow(non_camel_case_types)]
pub struct sapi_header_struct {
    pub header: *mut c_char,
    pub header_len: usize,
}

#[repr(C)]
#[allow(non_camel_case_types)]
pub struct sapi_headers_struct {
    // zend_llist headers: 7 pointers/size_t + 1 byte + padding = 56 bytes
    _headers_llist: [u8; 56],
    pub http_response_code: c_int,
}

// ─── zend_file_handle (must match PHP 8.4 layout exactly) ──────────────

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(non_camel_case_types)]
pub struct zend_stream {
    pub handle: *mut c_void,
    pub isatty: c_int,
    pub reader: *mut c_void,
    pub fsizer: *mut c_void,
    pub closer: *mut c_void,
}

#[repr(C)]
#[allow(non_camel_case_types)]
pub union zend_file_handle_union {
    pub fp: *mut c_void, // FILE*
    pub stream: zend_stream,
}

#[repr(C)]
#[allow(non_camel_case_types)]
pub struct zend_file_handle {
    pub handle: zend_file_handle_union,
    pub filename: *mut zend_string,
    pub opened_path: *mut zend_string,
    pub type_: c_int, // zend_stream_type is an enum = int (4 bytes)
    pub primary_script: bool,
    pub in_list: bool,
    // 2 bytes padding auto-inserted by repr(C) for buf alignment
    pub buf: *mut c_char,
    pub len: usize,
}

/// zend_string layout (PHP 8.4, 64-bit):
///   offset 0: zend_refcounted_h gc (8 bytes)
///   offset 8: zend_ulong h (8 bytes)
///   offset 16: size_t len (8 bytes)
///   offset 24: char val[1] (flexible array member)
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct zend_string {
    _gc: [u8; 8],
    _h: usize,
    pub len: usize,
    // val[1] follows — access via as_bytes()
}

impl zend_string {
    /// Read the string's content as a byte slice.
    ///
    /// # Safety
    /// The caller must ensure `self` points to a valid PHP zend_string.
    pub unsafe fn as_bytes(&self) -> &[u8] {
        let val_ptr = (self as *const Self as *const u8).add(std::mem::size_of::<Self>());
        std::slice::from_raw_parts(val_ptr, self.len)
    }

    /// Read the string's content as a UTF-8 string (lossy).
    ///
    /// # Safety
    /// The caller must ensure `self` points to a valid PHP zend_string.
    pub unsafe fn to_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.as_bytes())
    }
}

// Global sapi_module variable — set by sapi_startup(), used by PHP internally
extern "C" {
    pub static mut sapi_module: sapi_module_struct;
}

/// PHP 8.4 zend_error_cb signature:
///   void (*)(int type, zend_string *error_filename, uint32_t error_lineno, zend_string *message)
pub type ZendErrorCbT = unsafe extern "C" fn(
    type_: c_int,
    error_filename: *const zend_string,
    error_lineno: c_uint,
    message: *const zend_string,
);

extern "C" {
    pub static mut zend_error_cb: ZendErrorCbT;
}

extern "C" {
    // zend_error — PHP's error handler (php_error is a macro: #define php_error zend_error)
    // Used as sapi_error callback — MUST be set to avoid NULL dereference.
    pub fn zend_error(type_: c_int, fmt: *const c_char, ...);

    // TSRM — PHP wrapper (must be called before sapi_startup() for ZTS builds)
    pub fn php_tsrm_startup() -> bool;
    pub fn tsrm_shutdown();

    // Thread-local storage init (each worker thread must call this)
    pub fn ts_resource_ex(id: c_int, th_id: *mut c_void) -> *mut c_void;

    // SAPI lifecycle
    pub fn sapi_startup(module: *mut sapi_module_struct);
    pub fn sapi_shutdown();

    // Module lifecycle (PHP 8.4: 2 arguments, not 3)
    pub fn php_module_startup(
        module: *mut sapi_module_struct,
        additional_module: *mut c_void,
    ) -> zend_result;
    pub fn php_module_shutdown();

    // Request lifecycle
    pub fn php_request_startup() -> zend_result;
    pub fn php_request_shutdown(dummy: *mut c_void);

    // Script execution
    pub fn php_execute_script(primary_file: *mut zend_file_handle) -> zend_result;

    // File handle
    pub fn zend_stream_init_filename(handle: *mut zend_file_handle, filename: *const c_char);
    pub fn zend_destroy_file_handle(handle: *mut zend_file_handle);

    // Server variable registration — populates $_SERVER entries
    pub fn php_register_variable_safe(
        var: *const c_char,
        strval: *const c_char,
        str_len: usize,
        track_vars_array: *mut c_void,
    );

    // ─── Plugin function bridge ─────────────────────────────

    pub fn oxphp_bridge_register_plugin_fn(name: *const c_char, required: c_int, total: c_int);

    // ─── Native bridge dispatch ─────────────────────────────

    pub fn oxphp_bridge_set_native_dispatch(
        f: Option<unsafe extern "C" fn(*const c_char, *mut c_void, u32, *mut c_void) -> c_int>,
    );

    pub fn oxphp_call_php_native(
        name: *const c_char,
        args: *mut c_void,
        argc: u32,
        result: *mut c_void,
    ) -> c_int;

    // ─── Bridge context ─────────────────────────────────

    pub fn oxphp_bridge_tsrm_update();
    pub fn oxphp_bridge_init_ctx();
    pub fn oxphp_bridge_set_request_id(id: *const c_char);
    pub fn oxphp_bridge_set_worker_id(id: i32);
    pub fn oxphp_bridge_set_request_time(time: f64);
    pub fn oxphp_bridge_get_request_time() -> f64;
    pub fn oxphp_bridge_is_finished() -> bool;
    pub fn oxphp_bridge_set_finished(finished: bool);
    pub fn oxphp_bridge_is_streaming() -> bool;
    pub fn oxphp_bridge_set_stream_mode(mode: bool);
    pub fn oxphp_bridge_set_headers_sent(sent: bool);
    pub fn oxphp_bridge_get_headers_sent() -> bool;

    pub fn oxphp_bridge_set_deadline(deadline_us: i64);
    pub fn oxphp_bridge_is_deadline_expired() -> bool;
    pub fn oxphp_bridge_set_cancelled(cancelled: bool);
    pub fn oxphp_bridge_is_cancelled() -> bool;

    // ─── Profiler observer ──────────────
    // Defined in ext/bridge/oxphp_bridge.c. Safe Rust wrappers
    // live in src/profiling/flush.rs.
    pub fn oxphp_bridge_set_profiling_mode(mode: u8);
    pub fn oxphp_bridge_get_profiling_mode() -> u8;
    pub fn oxphp_bridge_snapshot_open_stack(dst: *mut u32, max_depth: u8) -> u8;
    pub fn oxphp_bridge_profiler_rshutdown_flush();
    pub fn oxphp_bridge_set_profiling_paused(paused: u8);
    pub fn oxphp_bridge_is_profiling_paused() -> u8;
    pub fn oxphp_bridge_get_memory_usage_bytes() -> i64;

    // ─── Filter cache ──────────────────
    // Defined in ext/bridge/oxphp_bridge.c. Rust safe wrappers and
    // the resolver impl live in src/profiling/filter.rs.
    pub fn oxphp_bridge_set_filter_resolver(
        resolver: Option<
            unsafe extern "C" fn(
                fn_id: usize,
                class_attr_names: *const *const c_char,
                class_attr_count: u32,
                fn_attr_names: *const *const c_char,
                fn_attr_count: u32,
                attr_resolver_ctx: *mut c_void,
                out_excluded: *mut u8,
                out_force_profile: *mut u8,
                out_has_sample: *mut u8,
                out_sample_rate: *mut f32,
            ) -> u32,
        >,
    );
    pub fn oxphp_bridge_read_attr_arg_str(
        ctx: *mut c_void,
        is_class_scope: i32,
        attr_name: *const c_char,
        attr_idx: u32,
        arg_idx: u32,
        out: *mut c_char,
        out_cap: usize,
    ) -> usize;
    pub fn oxphp_bridge_read_attr_arg_double(
        ctx: *mut c_void,
        is_class_scope: i32,
        attr_name: *const c_char,
        attr_idx: u32,
        arg_idx: u32,
        out: *mut f64,
    ) -> i32;
    pub fn oxphp_bridge_get_filter_spec_id_cached(fn_id: usize) -> u32;
    pub fn oxphp_bridge_clear_filter_cache();

    /// Process-wide span cap for the profiler observer. 0 = unlimited.
    /// Set once at plugin init from `ProfilerConfig.max_spans`.
    pub fn oxphp_bridge_set_profiler_max_spans(cap: u32);

    pub fn oxphp_execute_script_safe(file_handle: *mut c_void) -> c_int;

    pub fn oxphp_bridge_set_sapi_callbacks(
        ub_write: Option<unsafe extern "C" fn(*const c_char, usize) -> usize>,
        flush: Option<unsafe extern "C" fn(*mut c_void)>,
    );

    // ─── Superglobals configuration ─────────────────────
    pub fn oxphp_bridge_set_superglobals_enabled(enabled: bool);
    pub fn oxphp_bridge_get_superglobals_enabled() -> bool;

    // ─── HTTP Request data accessors ─────────────────────
    #[allow(clippy::too_many_arguments)]
    pub fn oxphp_bridge_set_request_accessors(
        method_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        path_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        full_uri_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        scheme_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        host_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        port_fn: Option<unsafe extern "C" fn() -> u16>,
        query_string_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        header_fn: Option<unsafe extern "C" fn(*const c_char, usize, *mut usize) -> *const c_char>,
        cookie_fn: Option<unsafe extern "C" fn(*const c_char, usize, *mut usize) -> *const c_char>,
        ip_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        protocol_version_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        start_time_fn: Option<unsafe extern "C" fn() -> f64>,
        is_secure_fn: Option<unsafe extern "C" fn() -> c_int>,
        content_type_fn: Option<unsafe extern "C" fn(*mut usize) -> *const c_char>,
        query_param_fn: Option<
            unsafe extern "C" fn(*const c_char, usize, *mut usize) -> *const c_char,
        >,
        headers_all_fn: Option<
            unsafe extern "C" fn(
                unsafe extern "C" fn(*const c_char, usize, *const c_char, usize, *mut c_void),
                *mut c_void,
            ),
        >,
        cookies_all_fn: Option<
            unsafe extern "C" fn(
                unsafe extern "C" fn(*const c_char, usize, *const c_char, usize, *mut c_void),
                *mut c_void,
            ),
        >,
        query_params_all_fn: Option<
            unsafe extern "C" fn(
                unsafe extern "C" fn(*const c_char, usize, *const c_char, usize, *mut c_void),
                *mut c_void,
            ),
        >,
        body_fn: Option<unsafe extern "C" fn(*mut usize) -> *const u8>,
        is_active_fn: Option<unsafe extern "C" fn() -> c_int>,
    );

    // ─── Worker mode ────────────────────────────────────

    pub fn oxphp_bridge_set_worker_callbacks(
        wait_fn: Option<unsafe extern "C" fn() -> c_int>,
        send_fn: Option<unsafe extern "C" fn() -> c_int>,
    );
    pub fn oxphp_bridge_set_worker_mode(max_requests: u64, max_memory_mib: u64);
    pub fn oxphp_bridge_set_worker_start_time(time: f64);
    pub fn oxphp_bridge_get_worker_start_time() -> f64;
    pub fn oxphp_bridge_is_worker_mode() -> bool;
    pub fn oxphp_bridge_reset_request_ctx();
    pub fn oxphp_bridge_worker_wait() -> c_int;
    pub fn oxphp_bridge_worker_send_response() -> c_int;

    // ─── Worker mode metrics ─────────────────────────────
    pub fn oxphp_bridge_get_exit_reason() -> u8;
    pub fn oxphp_bridge_get_requests_done() -> u64;
    pub fn oxphp_bridge_increment_requests_done();
    pub fn oxphp_bridge_get_rss_bytes() -> u64;
    pub fn oxphp_bridge_get_memory_usage() -> u64;
    pub fn oxphp_bridge_get_max_memory_bytes() -> u64;
    pub fn oxphp_bridge_get_handler_failed() -> bool;

    // ─── SAPI response code ─────────────────────────────
    pub fn oxphp_bridge_get_response_code() -> c_int;

    // ─── SAPI request_info ──────────────────────────────

    pub fn oxphp_bridge_set_request_info(
        method: *const c_char,
        query_string: *const c_char,
        content_type: *const c_char,
        content_length: c_long,
    );
}
