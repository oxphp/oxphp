//! FFI bindings to the native bridge C functions in liboxphp_bridge.so.
//! Only compiled when `feature = "php"`.

use std::os::raw::{c_char, c_int, c_void};

#[allow(dead_code)]
extern "C" {
    // ── Value reading ──
    pub fn oxphp_val_type(zv: *mut c_void) -> u8;
    pub fn oxphp_val_arg_type(args: *mut c_void, idx: u32) -> u8;

    pub fn oxphp_arg_long(args: *mut c_void, idx: u32) -> i64;
    pub fn oxphp_arg_double(args: *mut c_void, idx: u32) -> f64;
    pub fn oxphp_arg_bool(args: *mut c_void, idx: u32) -> c_int;
    pub fn oxphp_arg_str(args: *mut c_void, idx: u32, out_len: *mut usize) -> *const u8;

    pub fn oxphp_arg_array_count(args: *mut c_void, idx: u32) -> u32;
    pub fn oxphp_arg_array(args: *mut c_void, idx: u32) -> *mut c_void;

    pub fn oxphp_array_foreach(
        zv_array: *mut c_void,
        cb: unsafe extern "C" fn(*const u8, usize, i64, *mut c_void, *mut c_void),
        user_data: *mut c_void,
    );

    pub fn oxphp_val_long(zv: *mut c_void) -> i64;
    pub fn oxphp_val_double(zv: *mut c_void) -> f64;
    pub fn oxphp_val_bool(zv: *mut c_void) -> c_int;
    pub fn oxphp_val_str(zv: *mut c_void, out_len: *mut usize) -> *const u8;
    pub fn oxphp_val_array_count(zv: *mut c_void) -> u32;

    // ── Value writing ──
    pub fn oxphp_ret_null(retval: *mut c_void);
    pub fn oxphp_ret_bool(retval: *mut c_void, val: c_int);
    pub fn oxphp_ret_long(retval: *mut c_void, val: i64);
    pub fn oxphp_ret_double(retval: *mut c_void, val: f64);
    pub fn oxphp_ret_str(retval: *mut c_void, s: *const u8, len: usize);
    pub fn oxphp_ret_array_init(retval: *mut c_void, size_hint: u32);

    pub fn oxphp_arr_add_null(arr: *mut c_void, key: *const c_char, klen: usize);
    pub fn oxphp_arr_add_bool(arr: *mut c_void, key: *const c_char, klen: usize, val: c_int);
    pub fn oxphp_arr_add_long(arr: *mut c_void, key: *const c_char, klen: usize, val: i64);
    pub fn oxphp_arr_add_double(arr: *mut c_void, key: *const c_char, klen: usize, val: f64);
    pub fn oxphp_arr_add_str(
        arr: *mut c_void,
        key: *const c_char,
        klen: usize,
        s: *const u8,
        slen: usize,
    );
    pub fn oxphp_arr_add_array(
        arr: *mut c_void,
        key: *const c_char,
        klen: usize,
        size: u32,
    ) -> *mut c_void;

    pub fn oxphp_arr_push_null(arr: *mut c_void);
    pub fn oxphp_arr_push_bool(arr: *mut c_void, val: c_int);
    pub fn oxphp_arr_push_long(arr: *mut c_void, val: i64);
    pub fn oxphp_arr_push_double(arr: *mut c_void, val: f64);
    pub fn oxphp_arr_push_str(arr: *mut c_void, s: *const u8, len: usize);
    pub fn oxphp_arr_push_array(arr: *mut c_void, size: u32) -> *mut c_void;

    // ── Dispatch ──
    pub fn oxphp_bridge_set_native_dispatch(
        f: Option<unsafe extern "C" fn(*const c_char, *mut c_void, u32, *mut c_void) -> c_int>,
    );

    // ── Call PHP ──
    pub fn oxphp_call_php_native(
        name: *const c_char,
        args: *mut c_void,
        argc: u32,
        result: *mut c_void,
    ) -> c_int;

    // ── Zval lifecycle ──
    pub fn oxphp_zval_dtor(zv: *mut c_void);
    pub fn oxphp_zval_size() -> usize;
}
