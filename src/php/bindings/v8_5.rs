//! PHP 8.5 SAPI types.
//!
//! Active when `cfg(php_v8_5)` is set by `build.rs`. Layout must match
//! `main/SAPI.h` of PHP 8.5 exactly.
//!
//! Deltas vs PHP 8.4 (verified against PHP-8.5.6 headers, 2026-04-30):
//! - `sapi_module_struct` gained `pre_request_init` at the end. Called by
//!   `sapi_activate()` (`main/SAPI.c:458`); ignoring it caused SIGBUS on
//!   startup before this module was added.
//! - `sapi_header_op_enum` gained `SAPI_HEADER_DELETE_PREFIX` at position 3,
//!   shifting `DELETE_ALL` from 3→4 and `SET_STATUS` from 4→5.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

#[repr(C)]
#[allow(non_camel_case_types, dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum sapi_header_op_enum {
    SAPI_HEADER_REPLACE = 0,
    SAPI_HEADER_ADD = 1,
    SAPI_HEADER_DELETE = 2,
    SAPI_HEADER_DELETE_PREFIX = 3,
    SAPI_HEADER_DELETE_ALL = 4,
    SAPI_HEADER_SET_STATUS = 5,
}

use super::{sapi_header_struct, sapi_headers_struct, zend_result};

/// The SAPI module struct — central registration point for our SAPI.
/// Layout must match PHP 8.5 `main/SAPI.h` exactly.
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

    /// PHP 8.5+ only. Called by `sapi_activate()` after the SAPI module
    /// struct is initialised but before request init. We don't override
    /// it, so initialise to `None` in `build_sapi_module()`.
    pub pre_request_init: Option<unsafe extern "C" fn() -> c_int>,
}

unsafe impl Send for sapi_module_struct {}
unsafe impl Sync for sapi_module_struct {}
