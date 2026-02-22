#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};

pub type zend_result = c_int;

#[repr(C)]
pub struct zend_module_entry {
    _opaque: [u8; 0],
}

#[repr(C)]
#[allow(non_camel_case_types, dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum sapi_header_op_enum {
    SAPI_HEADER_REPLACE = 0,
    SAPI_HEADER_ADD = 1,
    SAPI_HEADER_DELETE = 2,
    SAPI_HEADER_DELETE_ALL = 3,
    SAPI_HEADER_SET_STATUS = 4,
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

/// The SAPI module struct — central registration point for our SAPI.
/// Layout must match PHP 8.4 `main/SAPI.h` exactly.
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct sapi_module_struct {
    pub name: *mut c_char,
    pub pretty_name: *mut c_char,

    pub startup: Option<unsafe extern "C" fn(module: *mut sapi_module_struct) -> c_int>,
    pub shutdown: Option<unsafe extern "C" fn(module: *mut sapi_module_struct) -> c_int>,

    pub activate: Option<unsafe extern "C" fn() -> c_int>,
    pub deactivate: Option<unsafe extern "C" fn() -> c_int>,

    pub ub_write: Option<unsafe extern "C" fn(str: *const c_char, str_length: usize) -> usize>,
    pub flush: Option<unsafe extern "C" fn(server_context: *mut c_void)>,
    pub get_stat: Option<unsafe extern "C" fn() -> *mut c_void>,
    pub getenv: Option<unsafe extern "C" fn(name: *const c_char, name_len: usize) -> *mut c_char>,

    // Variadic function pointer — can't be represented as Option<fn> in Rust
    pub sapi_error: *mut c_void,

    pub header_handler: Option<
        unsafe extern "C" fn(
            header: *mut sapi_header_struct,
            op: sapi_header_op_enum,
            headers: *mut sapi_headers_struct,
        ) -> c_int,
    >,
    pub send_headers: Option<unsafe extern "C" fn(headers: *mut sapi_headers_struct) -> c_int>,
    pub send_header:
        Option<unsafe extern "C" fn(header: *mut sapi_header_struct, server_context: *mut c_void)>,

    pub read_post: Option<unsafe extern "C" fn(buffer: *mut c_char, count_bytes: usize) -> usize>,
    pub read_cookies: Option<unsafe extern "C" fn() -> *mut c_char>,

    pub register_server_variables: Option<unsafe extern "C" fn(track_vars_array: *mut c_void)>,
    pub log_message: Option<unsafe extern "C" fn(message: *const c_char, syslog_type_int: c_int)>,
    pub get_request_time: Option<unsafe extern "C" fn(request_time: *mut f64) -> zend_result>,
    pub terminate_process: Option<unsafe extern "C" fn()>,

    pub php_ini_path_override: *mut c_char,
    pub default_post_reader: Option<unsafe extern "C" fn()>,
    pub treat_data: Option<unsafe extern "C" fn(arg: c_int, str: *mut c_char, dest: *mut c_void)>,
    pub executable_location: *mut c_char,

    pub php_ini_ignore: c_int,
    pub php_ini_ignore_cwd: c_int,

    pub get_fd: Option<unsafe extern "C" fn(fd: *mut c_int) -> c_int>,
    pub force_http_10: Option<unsafe extern "C" fn() -> c_int>,
    pub get_target_uid: Option<unsafe extern "C" fn(uid: *mut c_int) -> c_int>,
    pub get_target_gid: Option<unsafe extern "C" fn(gid: *mut c_int) -> c_int>,
    pub input_filter: Option<
        unsafe extern "C" fn(
            arg: c_int,
            var: *const c_char,
            val: *mut *mut c_char,
            val_len: usize,
            new_val_len: *mut usize,
        ) -> c_uint,
    >,
    pub ini_defaults: Option<unsafe extern "C" fn(cfg: *mut c_void)>,
    pub phpinfo_as_text: c_int,

    pub ini_entries: *const c_char,
    pub additional_functions: *const c_void,
    pub input_filter_init: Option<unsafe extern "C" fn() -> c_uint>,
}

unsafe impl Send for sapi_module_struct {}
unsafe impl Sync for sapi_module_struct {}

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

    pub fn oxphp_execute_script_safe(file_handle: *mut c_void) -> c_int;

    pub fn oxphp_bridge_set_sapi_callbacks(
        ub_write: Option<unsafe extern "C" fn(*const c_char, usize) -> usize>,
        flush: Option<unsafe extern "C" fn(*mut c_void)>,
    );

    // ─── SAPI request_info ──────────────────────────────

    pub fn oxphp_bridge_set_request_info(
        method: *const c_char,
        query_string: *const c_char,
        content_type: *const c_char,
        content_length: c_long,
    );
}
