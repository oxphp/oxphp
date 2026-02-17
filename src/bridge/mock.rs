//! Mock FFI implementations for host testing without PHP.
//! Mirrors the signatures from `ffi.rs` so `call.rs` compiles on all platforms.

#![allow(unused_variables, dead_code, clippy::missing_safety_doc)]

use std::os::raw::{c_char, c_int, c_void};

// ── Value reading ──

pub unsafe fn oxphp_val_type(_zv: *mut c_void) -> u8 {
    0 // OXPHP_TYPE_NULL
}
pub unsafe fn oxphp_val_arg_type(_args: *mut c_void, _idx: u32) -> u8 {
    0
}

pub unsafe fn oxphp_arg_long(_args: *mut c_void, _idx: u32) -> i64 {
    0
}
pub unsafe fn oxphp_arg_double(_args: *mut c_void, _idx: u32) -> f64 {
    0.0
}
pub unsafe fn oxphp_arg_bool(_args: *mut c_void, _idx: u32) -> c_int {
    0
}
pub unsafe fn oxphp_arg_str(_args: *mut c_void, _idx: u32, out_len: *mut usize) -> *const u8 {
    unsafe { *out_len = 0 };
    std::ptr::null()
}

pub unsafe fn oxphp_arg_array_count(_args: *mut c_void, _idx: u32) -> u32 {
    0
}
pub unsafe fn oxphp_arg_array(_args: *mut c_void, _idx: u32) -> *mut c_void {
    std::ptr::null_mut()
}

pub unsafe fn oxphp_array_foreach(
    _zv_array: *mut c_void,
    _cb: unsafe extern "C" fn(*const u8, usize, i64, *mut c_void, *mut c_void),
    _user_data: *mut c_void,
) {
}

pub unsafe fn oxphp_val_long(_zv: *mut c_void) -> i64 {
    0
}
pub unsafe fn oxphp_val_double(_zv: *mut c_void) -> f64 {
    0.0
}
pub unsafe fn oxphp_val_bool(_zv: *mut c_void) -> c_int {
    0
}
pub unsafe fn oxphp_val_str(_zv: *mut c_void, out_len: *mut usize) -> *const u8 {
    unsafe { *out_len = 0 };
    std::ptr::null()
}
pub unsafe fn oxphp_val_array_count(_zv: *mut c_void) -> u32 {
    0
}

// ── Value writing ──

pub unsafe fn oxphp_ret_null(_retval: *mut c_void) {}
pub unsafe fn oxphp_ret_bool(_retval: *mut c_void, _val: c_int) {}
pub unsafe fn oxphp_ret_long(_retval: *mut c_void, _val: i64) {}
pub unsafe fn oxphp_ret_double(_retval: *mut c_void, _val: f64) {}
pub unsafe fn oxphp_ret_str(_retval: *mut c_void, _s: *const u8, _len: usize) {}
pub unsafe fn oxphp_ret_array_init(_retval: *mut c_void, _size_hint: u32) {}

pub unsafe fn oxphp_arr_add_null(_arr: *mut c_void, _key: *const c_char, _klen: usize) {}
pub unsafe fn oxphp_arr_add_bool(
    _arr: *mut c_void,
    _key: *const c_char,
    _klen: usize,
    _val: c_int,
) {
}
pub unsafe fn oxphp_arr_add_long(_arr: *mut c_void, _key: *const c_char, _klen: usize, _val: i64) {}
pub unsafe fn oxphp_arr_add_double(
    _arr: *mut c_void,
    _key: *const c_char,
    _klen: usize,
    _val: f64,
) {
}
pub unsafe fn oxphp_arr_add_str(
    _arr: *mut c_void,
    _key: *const c_char,
    _klen: usize,
    _s: *const u8,
    _slen: usize,
) {
}
pub unsafe fn oxphp_arr_add_array(
    _arr: *mut c_void,
    _key: *const c_char,
    _klen: usize,
    _size: u32,
) -> *mut c_void {
    std::ptr::null_mut()
}

pub unsafe fn oxphp_arr_push_null(_arr: *mut c_void) {}
pub unsafe fn oxphp_arr_push_bool(_arr: *mut c_void, _val: c_int) {}
pub unsafe fn oxphp_arr_push_long(_arr: *mut c_void, _val: i64) {}
pub unsafe fn oxphp_arr_push_double(_arr: *mut c_void, _val: f64) {}
pub unsafe fn oxphp_arr_push_str(_arr: *mut c_void, _s: *const u8, _len: usize) {}
pub unsafe fn oxphp_arr_push_array(_arr: *mut c_void, _size: u32) -> *mut c_void {
    std::ptr::null_mut()
}

// ── Dispatch ──

pub unsafe fn oxphp_bridge_set_native_dispatch(
    _f: Option<unsafe extern "C" fn(*const c_char, *mut c_void, u32, *mut c_void) -> c_int>,
) {
}

// ── Call PHP ──

pub unsafe fn oxphp_call_php_native(
    _name: *const c_char,
    _args: *mut c_void,
    _argc: u32,
    _result: *mut c_void,
) -> c_int {
    -1 // always fails in mock
}

// ── Zval lifecycle ──

pub unsafe fn oxphp_zval_dtor(_zv: *mut c_void) {}
pub unsafe fn oxphp_zval_size() -> usize {
    16
}
