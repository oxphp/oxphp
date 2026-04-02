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

// ── Decorator system ──

pub unsafe fn oxphp_bridge_set_decorator_registry(_ptr: *const c_void) {}

pub unsafe fn oxphp_bridge_set_decorator_resolve(
    _f: Option<
        unsafe extern "C" fn(
            fn_id: usize,
            attr_names: *const *const c_char,
            attr_count: u32,
        ) -> c_int,
    >,
) {
}

pub unsafe fn oxphp_bridge_set_decorator_begin(
    _f: Option<
        unsafe extern "C" fn(
            fn_id: usize,
            target: *const c_char,
            class_name: *const c_char,
            object_id: u64,
            timestamp_ns: u64,
        ) -> c_int,
    >,
) {
}

pub unsafe fn oxphp_bridge_set_decorator_end(
    _f: Option<
        unsafe extern "C" fn(
            fn_id: usize,
            elapsed_ns: u64,
            success: c_int,
            exception_class: *const c_char,
        ),
    >,
) {
}

pub unsafe fn oxphp_bridge_get_decorator_reject_reason(_out_len: *mut usize) -> *const c_char {
    std::ptr::null()
}

pub unsafe fn oxphp_bridge_set_decorator_reject_reason(_reason: *const c_char, _len: usize) {}

pub unsafe fn oxphp_bridge_clear_decorator_reject_reason() {}

pub unsafe fn oxphp_bridge_register_php_decorator(_class_name: *const c_char, _targets: u32) {}

pub unsafe fn oxphp_bridge_set_decorator_register_php(
    _f: Option<unsafe extern "C" fn(*const c_char, u32)>,
) {
}

// ── PHP decorator query callbacks ──
pub unsafe fn oxphp_bridge_set_php_decorator_count(_f: Option<unsafe extern "C" fn(usize) -> u32>) {
}
pub unsafe fn oxphp_bridge_set_php_decorator_class(
    _f: Option<unsafe extern "C" fn(usize, u32) -> *const c_char>,
) {
}
pub unsafe fn oxphp_bridge_set_php_decorator_cache_key(
    _f: Option<unsafe extern "C" fn(usize, u32) -> u64>,
) {
}
pub unsafe fn oxphp_bridge_set_decorator_class_buf(_s: *const c_char, _len: usize) {}
pub unsafe fn oxphp_bridge_get_decorator_class_buf() -> *const c_char {
    std::ptr::null()
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
pub unsafe fn oxphp_zval_addref(_zv: *mut c_void) {}
pub unsafe fn oxphp_closure_addref(_closure_zv: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}
pub unsafe fn oxphp_closure_release(_obj_ptr: *mut c_void) {}
pub unsafe fn oxphp_zval_size() -> usize {
    16
}

// ── Async dispatch function pointer registration ──

pub unsafe fn oxphp_bridge_set_async_dispatch(
    _f: Option<
        unsafe extern "C" fn(
            *const c_void,
            *mut c_void,
            *mut c_void,
            u32,
            *mut c_void,
            *mut c_void,
        ) -> i64,
    >,
) {
}

pub unsafe fn oxphp_bridge_set_await_dispatch(
    _f: Option<unsafe extern "C" fn(i64, f64, *mut c_void) -> c_int>,
) {
}

pub unsafe fn oxphp_bridge_set_await_any_dispatch(
    _f: Option<unsafe extern "C" fn(*const i64, u32, f64, *mut i64, *mut c_void) -> c_int>,
) {
}

// ── Non-blocking await poll ──

pub unsafe fn oxphp_bridge_set_await_poll(_f: Option<unsafe extern "C" fn(i64) -> c_int>) {}

// ── Async promise cleanup ──

pub unsafe fn oxphp_bridge_set_cleanup_promises(_f: Option<unsafe extern "C" fn()>) {}
pub unsafe fn oxphp_bridge_cleanup_outstanding_promises() {}

// ── Async exception details ──

pub unsafe fn oxphp_bridge_set_async_exception(
    _cls: *const c_char,
    _msg: *const c_char,
    _trace: *const c_char,
) {
}
pub unsafe fn oxphp_bridge_get_async_exc_class() -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_get_async_exc_message() -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_get_async_exc_trace() -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_clear_async_exception() {}

// === Async promise bridge functions ===

// Freeze/unfreeze
pub unsafe fn oxphp_freeze_zval(
    _zv: *mut c_void,
    _out_orig_refcount: *mut u32,
    _out_orig_gc_flags: *mut u32,
    _out_orig_type_flags: *mut u32,
) -> c_int {
    0
}

pub unsafe fn oxphp_unfreeze_zval(
    _zv: *mut c_void,
    _orig_refcount: u32,
    _orig_gc_flags: u32,
    _orig_type_flags: u32,
) {
}

// Deep copy
pub unsafe fn oxphp_deep_copy_zval(_dst: *mut c_void, _src: *const c_void) {}
pub unsafe fn oxphp_deep_free_zval(_zv: *mut c_void) {}

// Portable cross-thread serialization
pub unsafe fn oxphp_portable_serialize(
    _args: *const c_void,
    _argc: u32,
    _out_buf: *mut *mut u8,
    _out_len: *mut usize,
) -> c_int {
    0
}
pub unsafe fn oxphp_portable_deserialize(
    _buf: *const u8,
    _len: usize,
    _argc: u32,
    _out: *mut c_void,
) -> c_int {
    0
}
pub unsafe fn oxphp_portable_serialize_ht(
    _ht: *mut c_void,
    _out_buf: *mut *mut u8,
    _out_len: *mut usize,
) -> c_int {
    0
}
pub unsafe fn oxphp_portable_deserialize_ht(
    _buf: *const u8,
    _len: usize,
    _out_ht: *mut *mut c_void,
) -> c_int {
    0
}
pub unsafe fn oxphp_portable_free(_buf: *mut u8) {}
pub unsafe fn oxphp_portable_free_ht(_ht: *mut c_void) {}

// Closure inspection
pub unsafe fn oxphp_closure_get_op_array(_closure: *mut c_void) -> *const c_void {
    std::ptr::null()
}
pub unsafe fn oxphp_closure_get_static_vars(
    _closure: *mut c_void,
    _out_ht: *mut *mut c_void,
) -> c_int {
    0
}
pub unsafe fn oxphp_closure_has_this(_closure: *mut c_void) -> c_int {
    0
}
pub unsafe fn oxphp_closure_get_this(_closure: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}

// Borrow proxy
pub unsafe fn oxphp_create_borrow_proxy(_dst: *mut c_void, _promise_id: u64) {}

// Async worker
pub unsafe fn oxphp_async_reset() {}
pub unsafe fn oxphp_bridge_set_async_worker(_is_async: c_int) {}
pub unsafe fn oxphp_bridge_is_async_worker() -> c_int {
    0
}

// Async fatal error capture
pub unsafe fn oxphp_bridge_capture_fatal(_msg: *const c_char, _len: usize) {}
pub unsafe fn oxphp_bridge_pop_fatal() -> *mut c_char {
    std::ptr::null_mut()
}

// ── Fiber timer service ──
pub unsafe fn oxphp_bridge_set_timer_callbacks(
    _register_fn: Option<unsafe extern "C" fn(u64) -> u64>,
    _poll_fn: Option<unsafe extern "C" fn(*mut u64, u32) -> u32>,
    _remove_fn: Option<unsafe extern "C" fn(u64)>,
) {
}

// ── Fiber TLS context callbacks ──
pub unsafe fn oxphp_bridge_set_fiber_ctx_callbacks(
    _save_fn: Option<unsafe extern "C" fn(u64)>,
    _restore_fn: Option<unsafe extern "C" fn(u64)>,
    _drop_fn: Option<unsafe extern "C" fn(u64)>,
) {
}

// ── Fiber scheduler callbacks ──
pub unsafe fn oxphp_bridge_set_fiber_callbacks(
    _try_recv_fn: Option<unsafe extern "C" fn() -> std::os::raw::c_int>,
    _prepare_fn: Option<unsafe extern "C" fn() -> std::os::raw::c_int>,
) {
}

// Async task execution
#[allow(clippy::too_many_arguments)]
pub unsafe fn oxphp_execute_async_task(
    _op_array: *const c_void,
    _static_vars: *const c_void,
    _this_ptr: *mut c_void,
    _argc: u32,
    _args: *mut c_void,
    _retval: *mut c_void,
    _exc_class: *mut *mut c_char,
    _exc_message: *mut *mut c_char,
    _exc_trace: *mut *mut c_char,
) -> c_int {
    0
}

// ─── Plugin Class Registry ──────────────────────────────────

pub unsafe fn oxphp_bridge_register_class(_fqn: *const c_char, _parent_fqn: *const c_char, _flags: u32) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_class_implements(_class_handle: c_int, _interface_fqn: *const c_char) {}
pub unsafe fn oxphp_bridge_class_add_property(_class_handle: c_int, _name: *const c_char, _visibility: u32, _modifiers: u32, _type_info: c_int, _default_value: *const c_char) {}
pub unsafe fn oxphp_bridge_class_add_constant(_class_handle: c_int, _name: *const c_char, _visibility: u32, _value: *const c_char) {}
pub unsafe fn oxphp_bridge_class_add_method(_class_handle: c_int, _name: *const c_char, _visibility: u32, _flags: u32, _required_params: c_int, _total_params: c_int, _is_variadic: c_int) {}
pub unsafe fn oxphp_bridge_class_set_magic(_class_handle: c_int, _magic_type: c_int, _has_handler: c_int) {}
pub unsafe fn oxphp_bridge_class_enable_custom_object(_class_handle: c_int) {}

// ─── Plugin Interface Registry ──────────────────────────────

pub unsafe fn oxphp_bridge_register_interface(_fqn: *const c_char, _parent_fqn: *const c_char) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_interface_add_method(_iface_handle: c_int, _name: *const c_char, _flags: u32, _required_params: c_int, _total_params: c_int, _is_variadic: c_int) {}
pub unsafe fn oxphp_bridge_interface_add_constant(_iface_handle: c_int, _name: *const c_char, _visibility: u32, _value: *const c_char) {}

// ─── Plugin Enum Registry ───────────────────────────────────

pub unsafe fn oxphp_bridge_register_enum(_fqn: *const c_char, _backing_type: c_int) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_enum_implements(_enum_handle: c_int, _interface_fqn: *const c_char) {}
pub unsafe fn oxphp_bridge_enum_add_case(_enum_handle: c_int, _name: *const c_char, _value: *const c_char) {}
pub unsafe fn oxphp_bridge_enum_add_method(_enum_handle: c_int, _name: *const c_char, _flags: u32, _required_params: c_int, _total_params: c_int, _is_variadic: c_int) {}

// ─── Plugin Attribute Registry ──────────────────────────────

pub unsafe fn oxphp_bridge_register_attribute(_fqn: *const c_char, _targets: u32, _is_repeatable: c_int) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_attribute_add_param(_attr_handle: c_int, _name: *const c_char, _type_info: c_int, _is_required: c_int, _default_value: *const c_char) {}
pub unsafe fn oxphp_bridge_attribute_add_property(_attr_handle: c_int, _name: *const c_char, _type_info: c_int, _visibility: u32) {}

// ─── Plugin Function Registry (new builder-based) ───────────

pub unsafe fn oxphp_bridge_register_plugin_function(_fqn: *const c_char, _required_params: c_int, _total_params: c_int, _is_variadic: c_int) -> c_int {
    0
}

// ─── Method Dispatch ────────────────────────────────────────

pub unsafe fn oxphp_bridge_set_method_dispatch(
    _dispatch: Option<unsafe extern "C" fn(class_index: u32, method_name: *const c_char, args: *mut c_void, argc: u32, retval: *mut c_void, rust_data: *mut c_void) -> c_int>,
) {
}

// ─── Storage Callbacks ──────────────────────────────────────

pub unsafe fn oxphp_bridge_set_storage_callbacks(
    _create_fn: Option<unsafe extern "C" fn(class_index: u32) -> *mut c_void>,
    _drop_fn: Option<unsafe extern "C" fn(class_index: u32, rust_data: *mut c_void)>,
    _clone_fn: Option<unsafe extern "C" fn(class_index: u32, rust_data: *mut c_void) -> *mut c_void>,
) {
}

// ─── Exception Bridge ───────────────────────────────────────

pub unsafe fn oxphp_throw_exception(_class_fqn: *const c_char, _message: *const c_char, _code: i64) {}
pub unsafe fn oxphp_exception_pending() -> c_int {
    0
}
pub unsafe fn oxphp_exception_get(_class_out: *mut *const c_char, _message_out: *mut *const c_char, _code_out: *mut i64) {}
pub unsafe fn oxphp_exception_clear() {}
