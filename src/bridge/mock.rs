//! Mock FFI implementations for host testing without PHP.
//! Mirrors the signatures from `ffi.rs` so `call.rs` compiles on all platforms.

#![allow(
    unused_variables,
    dead_code,
    clippy::missing_safety_doc,
    clippy::too_many_arguments
)]

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
pub unsafe fn oxphp_arg_enum_long(_args: *mut c_void, _idx: u32) -> i64 {
    0
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
            attr_ctx: *mut c_void,
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

// ── Object construction helpers (mock) ──
//
// Host tests cannot construct PHP objects — these always fail so that
// any handler calling them on the host treats the call as unsupported
// and bails. Property setters are no-ops (return success) so handlers
// that ignore the rc compile-time-clean.

pub unsafe fn oxphp_bridge_make_object(
    _out: *mut c_void,
    _cls_fqn: *const c_char,
    _cls_len: usize,
) -> c_int {
    -1
}

pub unsafe fn oxphp_bridge_object_set_property_long(
    _obj: *mut c_void,
    _name: *const c_char,
    _name_len: usize,
    _val: i64,
) -> c_int {
    0
}

pub unsafe fn oxphp_bridge_object_set_property_zval(
    _obj: *mut c_void,
    _name: *const c_char,
    _name_len: usize,
    _src: *mut c_void,
) -> c_int {
    0
}

pub unsafe fn oxphp_bridge_get_enum_case(
    _out: *mut c_void,
    _cls_fqn: *const c_char,
    _cls_len: usize,
    _case_name: *const c_char,
    _case_len: usize,
) -> c_int {
    -1
}

#[allow(clippy::too_many_arguments)]
pub unsafe fn oxphp_bridge_wrap_result_ok_inplace(
    _retval: *mut c_void,
    _cls_fqn: *const c_char,
    _cls_len: usize,
    _value_prop: *const c_char,
    _value_prop_len: usize,
    _status_prop: *const c_char,
    _status_prop_len: usize,
    _status_val: std::os::raw::c_long,
) -> c_int {
    -1
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

pub unsafe fn oxphp_op_array_size() -> usize {
    256 // approximate; only used in non-php builds for compilation
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

pub unsafe fn oxphp_bridge_set_await_race_dispatch(
    _f: Option<unsafe extern "C" fn(*const i64, u32, f64, *mut i64, *mut c_void) -> c_int>,
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

pub unsafe fn oxphp_bridge_set_async_exception(_cls: *const c_char, _msg: *const c_char) {}
pub unsafe fn oxphp_bridge_get_async_exc_class() -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_get_async_exc_message() -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_clear_async_exception() {}

// ── Aggregate exception API (multi-error) ──

pub unsafe fn oxphp_bridge_aggregate_clear() {}

pub unsafe fn oxphp_bridge_aggregate_push(
    _exception_class: *const c_char,
    _message: *const c_char,
    _promise_id: i64,
) {
}

pub unsafe fn oxphp_bridge_aggregate_throw() -> c_int {
    0
}

pub unsafe fn oxphp_bridge_aggregate_throw_timeout(
    _pending_ids: *const i64,
    _pending_count: u32,
) -> c_int {
    0
}

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

// Cross-thread fcc spike mocks (no-op on host).
pub unsafe fn oxphp_pool_spike_capture(_callable: *mut c_void, _out_tid: *mut u64) -> c_int {
    -1
}
pub unsafe fn oxphp_pool_spike_invoke(
    _out_captured: *mut u64,
    _out_current: *mut u64,
    _out_buf: *mut *mut u8,
    _out_len: *mut usize,
) -> c_int {
    -1
}
pub unsafe fn oxphp_pool_spike_reset() {}

pub unsafe fn oxphp_iter_array_to_portbufs(
    _arr: *const c_void,
    out_concat: *mut *mut u8,
    out_concat_len: *mut usize,
    out_offsets: *mut *mut usize,
    out_n: *mut usize,
) -> c_int {
    unsafe {
        *out_concat = std::ptr::null_mut();
        *out_concat_len = 0;
        *out_offsets = std::ptr::null_mut();
        *out_n = 0;
    }
    -1
}

pub unsafe fn oxphp_arr_push_portbuf(_arr: *mut c_void, _buf: *const u8, _len: usize) -> c_int {
    -1
}

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

// ── Async plugin helpers ──

pub unsafe fn oxphp_ht_has_non_shareable_objects(_ht: *mut c_void) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_fiber_await(
    _promise_id: i64,
    _timeout: f64,
    _retval: *mut c_void,
) -> c_int {
    1 // not in fiber — blocking path
}
pub unsafe fn oxphp_bridge_in_fiber() -> c_int {
    0
}
pub unsafe fn oxphp_is_shareable(_z: *const c_void) -> c_int {
    0 // no CE registered in host tests
}
pub unsafe fn oxphp_bridge_set_borrow_proxy_ce(_ce: *mut c_void) {}
pub unsafe fn oxphp_arr_add_zval(_arr: *mut c_void, _key: *const c_char, _val: *mut c_void) {}
pub unsafe fn oxphp_arr_add_index_zval(_arr: *mut c_void, _idx: u64, _val: *mut c_void) {}

// ── Async direct dispatch ──

pub unsafe fn oxphp_bridge_async_dispatch(
    _op_array: *const c_void,
    _static_vars: *mut c_void,
    _this_ptr: *mut c_void,
    _argc: u32,
    _args: *mut c_void,
    _closure_zval: *mut c_void,
) -> i64 {
    -1
}
pub unsafe fn oxphp_bridge_await_dispatch(
    _promise_id: i64,
    _timeout: f64,
    _retval: *mut c_void,
) -> c_int {
    -1
}
pub unsafe fn oxphp_bridge_await_race_dispatch(
    _promise_ids: *const i64,
    _count: u32,
    _timeout: f64,
    _out_winner_id: *mut i64,
    _retval: *mut c_void,
) -> c_int {
    -1
}
pub unsafe fn oxphp_bridge_await_any_dispatch(
    _promise_ids: *const i64,
    _count: u32,
    _timeout: f64,
    _out_winner_id: *mut i64,
    _retval: *mut c_void,
) -> c_int {
    -1
}

// ── Synthetic promise bridge setters ──

pub unsafe fn oxphp_bridge_set_async_synth_alloc(_f: extern "C" fn() -> i64) {}
pub unsafe fn oxphp_bridge_set_async_synth_resolve(
    _f: extern "C" fn(i64, *const u8, usize) -> c_int,
) {
}
pub unsafe fn oxphp_bridge_set_async_synth_reject(
    _f: extern "C" fn(i64, *const c_char, *const c_char) -> c_int,
) {
}
pub unsafe fn oxphp_bridge_set_async_synth_cancel(_f: extern "C" fn(i64) -> c_int) {}

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
) -> c_int {
    0
}

// ─── Plugin Class Registry ──────────────────────────────────

pub unsafe fn oxphp_bridge_register_class(
    _fqn: *const c_char,
    _parent_fqn: *const c_char,
    _flags: u32,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_class_implements(_class_handle: c_int, _interface_fqn: *const c_char) {}
pub unsafe fn oxphp_bridge_class_add_property(
    _class_handle: c_int,
    _name: *const c_char,
    _visibility: u32,
    _modifiers: u32,
    _type_info: c_int,
    _default_value: *const c_char,
) {
}
pub unsafe fn oxphp_bridge_class_add_constant(
    _class_handle: c_int,
    _name: *const c_char,
    _visibility: u32,
    _value: *const c_char,
) {
}
pub unsafe fn oxphp_bridge_class_add_method(
    _class_handle: c_int,
    _name: *const c_char,
    _visibility: u32,
    _flags: u32,
    _required_params: c_int,
    _total_params: c_int,
    _is_variadic: c_int,
    _return_type: c_int,
    _return_nullable: c_int,
    _param_names: *const *const c_char,
    _param_types: *const c_int,
    _param_optional: *const c_int,
) {
}
pub unsafe fn oxphp_bridge_get_class_method_param_name(
    _class_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_get_class_method_param_type(
    _class_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_get_class_method_param_optional(
    _class_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_class_set_magic(
    _class_handle: c_int,
    _magic_type: c_int,
    _has_handler: c_int,
) {
}
pub unsafe fn oxphp_bridge_class_enable_custom_object(_class_handle: c_int) {}

// ─── Plugin Interface Registry ──────────────────────────────

pub unsafe fn oxphp_bridge_register_interface(
    _fqn: *const c_char,
    _parent_fqn: *const c_char,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_interface_add_method(
    _iface_handle: c_int,
    _name: *const c_char,
    _flags: u32,
    _required_params: c_int,
    _total_params: c_int,
    _is_variadic: c_int,
    _return_type: c_int,
    _return_nullable: c_int,
    _param_names: *const *const c_char,
    _param_types: *const c_int,
    _param_optional: *const c_int,
) {
}
pub unsafe fn oxphp_bridge_get_interface_method_param_name(
    _iface_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_get_interface_method_param_type(
    _iface_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_get_interface_method_param_optional(
    _iface_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_interface_add_constant(
    _iface_handle: c_int,
    _name: *const c_char,
    _visibility: u32,
    _value: *const c_char,
) {
}

// ─── Plugin Enum Registry ───────────────────────────────────

pub unsafe fn oxphp_bridge_register_enum(_fqn: *const c_char, _backing_type: c_int) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_enum_implements(_enum_handle: c_int, _interface_fqn: *const c_char) {}
pub unsafe fn oxphp_bridge_enum_add_case(
    _enum_handle: c_int,
    _name: *const c_char,
    _value: *const c_char,
) {
}
pub unsafe fn oxphp_bridge_enum_add_method(
    _enum_handle: c_int,
    _name: *const c_char,
    _flags: u32,
    _required_params: c_int,
    _total_params: c_int,
    _is_variadic: c_int,
    _return_type: c_int,
    _return_nullable: c_int,
    _param_names: *const *const c_char,
    _param_types: *const c_int,
    _param_optional: *const c_int,
) {
}
pub unsafe fn oxphp_bridge_get_enum_method_param_name(
    _enum_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_get_enum_method_param_type(
    _enum_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_get_enum_method_param_optional(
    _enum_index: c_int,
    _method_index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}

// ─── Plugin Attribute Registry ──────────────────────────────

pub unsafe fn oxphp_bridge_register_attribute(
    _fqn: *const c_char,
    _targets: u32,
    _is_repeatable: c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_attribute_add_param(
    _attr_handle: c_int,
    _name: *const c_char,
    _type_info: c_int,
    _is_required: c_int,
    _default_value: *const c_char,
) {
}
pub unsafe fn oxphp_bridge_attribute_add_property(
    _attr_handle: c_int,
    _name: *const c_char,
    _type_info: c_int,
    _visibility: u32,
) {
}

// ─── Plugin Function Registry (new builder-based) ───────────

pub unsafe fn oxphp_bridge_register_plugin_function(
    _fqn: *const c_char,
    _required_params: c_int,
    _total_params: c_int,
    _is_variadic: c_int,
    _return_type: c_int,
    _return_nullable: c_int,
    _param_names: *const *const c_char,
    _param_types: *const c_int,
    _param_optional: *const c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_get_plugin_function_param_name(
    _index: c_int,
    _param_index: c_int,
) -> *const c_char {
    std::ptr::null()
}
pub unsafe fn oxphp_bridge_get_plugin_function_param_type(
    _index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}
pub unsafe fn oxphp_bridge_get_plugin_function_param_optional(
    _index: c_int,
    _param_index: c_int,
) -> c_int {
    0
}

// ─── Method Dispatch ────────────────────────────────────────

pub unsafe fn oxphp_bridge_set_method_dispatch(
    _dispatch: Option<
        unsafe extern "C" fn(
            class_index: u32,
            method_name: *const c_char,
            args: *mut c_void,
            argc: u32,
            retval: *mut c_void,
            rust_data: *mut c_void,
            this_zval: *mut c_void,
        ) -> c_int,
    >,
) {
}

// ─── Object property access ─────────────────────────────────

pub unsafe fn oxphp_object_read_property(
    _object_zval: *mut c_void,
    _property_name: *const c_char,
) -> *mut c_void {
    std::ptr::null_mut()
}

pub unsafe fn oxphp_zval_is_null_or_unset(_zval_ptr: *const c_void) -> c_int {
    1
}

pub unsafe fn oxphp_zval_copy_to_retval(_src_zval: *const c_void, _dst_zval: *mut c_void) {}

// ─── Storage Callbacks ──────────────────────────────────────

pub unsafe fn oxphp_bridge_set_storage_callbacks(
    _create_fn: Option<unsafe extern "C" fn(class_index: u32) -> *mut c_void>,
    _drop_fn: Option<unsafe extern "C" fn(class_index: u32, rust_data: *mut c_void)>,
    _clone_fn: Option<
        unsafe extern "C" fn(class_index: u32, rust_data: *mut c_void) -> *mut c_void,
    >,
) {
}

// ─── Exception Bridge ───────────────────────────────────────

pub unsafe fn oxphp_throw_exception(
    _class_fqn: *const c_char,
    _message: *const c_char,
    _code: i64,
) {
}
pub unsafe fn oxphp_exception_pending() -> c_int {
    0
}
pub unsafe fn oxphp_exception_get(
    _class_out: *mut *const c_char,
    _message_out: *mut *const c_char,
    _code_out: *mut i64,
) {
}
pub unsafe fn oxphp_exception_clear() {}

// ── Shared\* synchronous invoke shims ──────────────

#[allow(clippy::missing_safety_doc)]
pub unsafe fn oxphp_shared_invoke_0_portbuf(
    _callable: *mut c_void,
    _out_ret_buf: *mut *mut u8,
    _out_ret_len: *mut usize,
    _out_retained_entry: *mut *const c_void,
) -> c_int {
    -1
}

#[allow(clippy::missing_safety_doc)]
pub unsafe fn oxphp_shared_invoke_byref_1_portbuf(
    _callable: *mut c_void,
    _state_buf: *const u8,
    _state_len: usize,
    _new_state_buf: *mut *mut u8,
    _new_state_len: *mut usize,
    _out_ret_buf: *mut *mut u8,
    _out_ret_len: *mut usize,
    _did_mutate: *mut c_int,
    _out_retained_state: *mut *mut c_void,
    _out_retained_ret: *mut *mut c_void,
) -> c_int {
    -1
}

#[allow(clippy::missing_safety_doc)]
pub unsafe fn oxphp_shared_free_zval(_p: *mut c_void) {
    // host mock: no zval ever materialised, so nothing to free.
}

#[allow(clippy::missing_safety_doc)]
pub unsafe fn oxphp_shared_invoke_2_ret_stop(
    _callable: *mut c_void,
    _key_kind: c_int,
    _key_int: i64,
    _key_ptr: *const u8,
    _key_len: usize,
    _val_buf: *const u8,
    _val_len: usize,
) -> c_int {
    0 // host stub: continue iteration
}

#[allow(clippy::missing_safety_doc)]
pub unsafe fn oxphp_shared_map_cursor_alloc(
    _out_zv: *mut c_void,
    _state_ptr: *mut c_void,
) -> c_int {
    -1 // host cannot construct PHP objects
}

pub const OXPHP_SHARED_INVOKE_OK: c_int = 0;
pub const OXPHP_SHARED_INVOKE_PHP_THREW: c_int = 1;
pub const OXPHP_SHARED_INVOKE_BAD_CALLABLE: c_int = -1;
pub const OXPHP_SHARED_INVOKE_BAD_RETURN: c_int = -2;

// ── Shared\Pool bridge (host mock) ───────────────────────
// Host tests cannot invoke PHP, so every factory/body path fails
// immediately. Tests that exercise pool logic must live under
// feature=php (docker) — the mock here is enough to compile the
// FFI declarations on host.
pub unsafe fn oxphp_pool_fcc_new(
    _callable_zval: *mut c_void,
    _out_fcc_heap: *mut *mut c_void,
) -> c_int {
    -1
}
pub unsafe fn oxphp_pool_fcc_free(_fcc_heap: *mut c_void) {}
pub unsafe fn oxphp_pool_factory_invoke(
    _fcc_heap: *mut c_void,
    _out_slot_zv_heap: *mut *mut c_void,
) -> c_int {
    -1
}
pub unsafe fn oxphp_pool_body_invoke(
    _body_callable_zv: *mut c_void,
    _slot_zv_heap: *mut c_void,
    _user_out_zv: *mut c_void,
) -> c_int {
    -1
}
pub unsafe fn oxphp_pool_slot_to_user(_slot_zv_heap: *mut c_void, _user_out_zv: *mut c_void) {}
pub unsafe fn oxphp_pool_slot_free(_slot_zv_heap: *mut c_void) {}
pub unsafe fn oxphp_pool_destroy_invoke(
    _destroy_fcc_heap: *mut c_void,
    _slot_zv_heap: *mut c_void,
) -> c_int {
    0
}

pub unsafe fn oxphp_shared_pool_handle_alloc(
    _out_zv: *mut c_void,
    _pool_id: u64,
    _owner_tid: u64,
    _slot_zv_heap: *mut c_void,
) -> c_int {
    -1
}

// ── Worker class accessors (mock state) ──
//
// Mirror the C accessors declared in `src/php/bindings/common.rs`. These are
// real `extern "C"` exports (gated to host builds only — see
// `src/bridge/mod.rs`) so tests can call them through the FFI boundary if
// desired and so the symbols satisfy any host code that links against them.

use std::cell::Cell;

thread_local! {
    static WORKER_START_TIME: Cell<f64> = const { Cell::new(0.0) };
    static REQUESTS_DONE: Cell<u64> = const { Cell::new(0) };
}

#[no_mangle]
pub unsafe extern "C" fn oxphp_bridge_set_worker_start_time(t: f64) {
    WORKER_START_TIME.with(|c| c.set(t));
}

#[no_mangle]
pub unsafe extern "C" fn oxphp_bridge_get_worker_start_time() -> f64 {
    WORKER_START_TIME.with(|c| c.get())
}

#[no_mangle]
pub unsafe extern "C" fn oxphp_bridge_increment_requests_done() -> u64 {
    REQUESTS_DONE.with(|c| {
        let v = c.get() + 1;
        c.set(v);
        v
    })
}

#[no_mangle]
pub unsafe extern "C" fn oxphp_bridge_get_requests_done() -> u64 {
    REQUESTS_DONE.with(|c| c.get())
}

#[no_mangle]
pub unsafe extern "C" fn oxphp_bridge_get_rss_bytes() -> u64 {
    /* Mock returns a plausible non-zero value for tests that just assert > 0.
     * Tests that check exact bytes must run under Docker against the real
     * accessor. */
    1024 * 1024
}

#[no_mangle]
pub unsafe extern "C" fn oxphp_bridge_get_max_memory_bytes() -> u64 {
    0
}

// Cancellation reason API (mock).

use std::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

static MOCK_CANCEL_PTR: AtomicPtr<AtomicU8> = AtomicPtr::new(std::ptr::null_mut());
static MOCK_VM_INTERRUPT: AtomicU8 = AtomicU8::new(0);

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_set_cancel_ptr(ptr: *const AtomicU8) {
    MOCK_CANCEL_PTR.store(ptr as *mut AtomicU8, Ordering::Relaxed);
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_get_cancel_reason() -> u8 {
    let p = MOCK_CANCEL_PTR.load(Ordering::Relaxed);
    if p.is_null() {
        0
    } else {
        (*p).load(Ordering::Relaxed)
    }
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_set_cancel_reason(reason: u8) -> bool {
    let p = MOCK_CANCEL_PTR.load(Ordering::Relaxed);
    if p.is_null() || reason == 0 {
        return false;
    }
    let expected = 0u8;
    (*p).compare_exchange(expected, reason, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_vm_interrupt_addr() -> *mut u8 {
    &MOCK_VM_INTERRUPT as *const _ as *mut u8
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_set_vm_interrupt_addr(_addr: *mut std::os::raw::c_void) {}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_request_interrupt() {
    MOCK_VM_INTERRUPT.store(1, Ordering::Relaxed);
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_request_interrupt_at(_addr: *mut std::os::raw::c_void) {
    MOCK_VM_INTERRUPT.store(1, Ordering::Relaxed);
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_capture_vm_interrupt() {}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_get_worker_id() -> std::os::raw::c_int {
    0
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn oxphp_bridge_set_tick_ptr(_ptr: *const std::sync::atomic::AtomicU64) {}

#[cfg(test)]
mod worker_class_mock_tests {
    use super::*;

    #[test]
    fn worker_start_time_round_trip() {
        unsafe {
            oxphp_bridge_set_worker_start_time(123.456);
        }
        let v = unsafe { oxphp_bridge_get_worker_start_time() };
        assert!((v - 123.456).abs() < 1e-9);
    }

    #[test]
    fn increment_requests_done_is_monotonic() {
        REQUESTS_DONE.with(|c| c.set(0));
        unsafe {
            oxphp_bridge_increment_requests_done();
            oxphp_bridge_increment_requests_done();
            oxphp_bridge_increment_requests_done();
        }
        assert_eq!(unsafe { oxphp_bridge_get_requests_done() }, 3);
    }

    #[test]
    fn get_rss_bytes_is_nonzero() {
        assert!(unsafe { oxphp_bridge_get_rss_bytes() } > 0);
    }
}
