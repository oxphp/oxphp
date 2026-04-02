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

    // ── Decorator system ──
    pub fn oxphp_bridge_set_decorator_registry(ptr: *const c_void);

    pub fn oxphp_bridge_set_decorator_resolve(
        f: Option<
            unsafe extern "C" fn(
                fn_id: usize,
                attr_names: *const *const c_char,
                attr_count: u32,
            ) -> c_int,
        >,
    );

    pub fn oxphp_bridge_set_decorator_begin(
        f: Option<
            unsafe extern "C" fn(
                fn_id: usize,
                target: *const c_char,
                class_name: *const c_char,
                object_id: u64,
                timestamp_ns: u64,
            ) -> c_int,
        >,
    );

    pub fn oxphp_bridge_set_decorator_end(
        f: Option<
            unsafe extern "C" fn(
                fn_id: usize,
                elapsed_ns: u64,
                success: c_int,
                exception_class: *const c_char,
            ),
        >,
    );

    pub fn oxphp_bridge_get_decorator_reject_reason(out_len: *mut usize) -> *const c_char;

    pub fn oxphp_bridge_set_decorator_reject_reason(reason: *const c_char, len: usize);

    pub fn oxphp_bridge_clear_decorator_reject_reason();

    pub fn oxphp_bridge_register_php_decorator(class_name: *const c_char, targets: u32);

    pub fn oxphp_bridge_set_decorator_register_php(
        f: Option<unsafe extern "C" fn(class_name: *const c_char, targets: u32)>,
    );

    // ── PHP decorator query callbacks ──
    pub fn oxphp_bridge_set_php_decorator_count(
        f: Option<unsafe extern "C" fn(fn_id: usize) -> u32>,
    );
    pub fn oxphp_bridge_set_php_decorator_class(
        f: Option<unsafe extern "C" fn(fn_id: usize, index: u32) -> *const c_char>,
    );
    pub fn oxphp_bridge_set_php_decorator_cache_key(
        f: Option<unsafe extern "C" fn(fn_id: usize, index: u32) -> u64>,
    );
    pub fn oxphp_bridge_set_decorator_class_buf(s: *const c_char, len: usize);
    pub fn oxphp_bridge_get_decorator_class_buf() -> *const c_char;

    // ── Call PHP ──
    pub fn oxphp_call_php_native(
        name: *const c_char,
        args: *mut c_void,
        argc: u32,
        result: *mut c_void,
    ) -> c_int;

    // ── Zval lifecycle ──
    pub fn oxphp_zval_dtor(zv: *mut c_void);
    pub fn oxphp_zval_addref(zv: *mut c_void);
    pub fn oxphp_closure_addref(closure_zv: *mut c_void) -> *mut c_void;
    pub fn oxphp_closure_release(obj_ptr: *mut c_void);
    pub fn oxphp_zval_size() -> usize;
    pub fn oxphp_op_array_size() -> usize;

    // ── Async dispatch function pointer registration ──
    pub fn oxphp_bridge_set_async_dispatch(
        f: Option<
            unsafe extern "C" fn(
                *const c_void,
                *mut c_void,
                *mut c_void,
                u32,
                *mut c_void,
                *mut c_void,
            ) -> i64,
        >,
    );
    pub fn oxphp_bridge_set_await_dispatch(
        f: Option<unsafe extern "C" fn(i64, f64, *mut c_void) -> c_int>,
    );
    pub fn oxphp_bridge_set_await_any_dispatch(
        f: Option<unsafe extern "C" fn(*const i64, u32, f64, *mut i64, *mut c_void) -> c_int>,
    );

    // ── Non-blocking await poll ──
    pub fn oxphp_bridge_set_await_poll(f: Option<unsafe extern "C" fn(i64) -> c_int>);

    // ── Async promise cleanup ──
    pub fn oxphp_bridge_set_cleanup_promises(f: Option<unsafe extern "C" fn()>);
    pub fn oxphp_bridge_cleanup_outstanding_promises();

    // ── Async exception details ──
    pub fn oxphp_bridge_set_async_exception(
        cls: *const c_char,
        msg: *const c_char,
        trace: *const c_char,
    );
    pub fn oxphp_bridge_get_async_exc_class() -> *const c_char;
    pub fn oxphp_bridge_get_async_exc_message() -> *const c_char;
    pub fn oxphp_bridge_get_async_exc_trace() -> *const c_char;
    pub fn oxphp_bridge_clear_async_exception();

    // === Async promise bridge functions ===

    // Freeze/unfreeze
    pub fn oxphp_freeze_zval(
        zv: *mut c_void,
        out_orig_refcount: *mut u32,
        out_orig_gc_flags: *mut u32,
        out_orig_type_flags: *mut u32,
    ) -> c_int;
    pub fn oxphp_unfreeze_zval(
        zv: *mut c_void,
        orig_refcount: u32,
        orig_gc_flags: u32,
        orig_type_flags: u32,
    );

    // Deep copy (malloc-based, thread-independent)
    pub fn oxphp_deep_copy_zval(dst: *mut c_void, src: *const c_void);
    pub fn oxphp_deep_free_zval(zv: *mut c_void);

    // Portable cross-thread serialization (system malloc buffer)
    pub fn oxphp_portable_serialize(
        args: *const c_void,
        argc: u32,
        out_buf: *mut *mut u8,
        out_len: *mut usize,
    ) -> c_int;
    pub fn oxphp_portable_deserialize(
        buf: *const u8,
        len: usize,
        argc: u32,
        out: *mut c_void,
    ) -> c_int;
    pub fn oxphp_portable_serialize_ht(
        ht: *mut c_void,
        out_buf: *mut *mut u8,
        out_len: *mut usize,
    ) -> c_int;
    pub fn oxphp_portable_deserialize_ht(
        buf: *const u8,
        len: usize,
        out_ht: *mut *mut c_void,
    ) -> c_int;
    pub fn oxphp_portable_free(buf: *mut u8);
    pub fn oxphp_portable_free_ht(ht: *mut c_void);

    // Closure inspection
    pub fn oxphp_closure_get_op_array(closure: *mut c_void) -> *const c_void;
    pub fn oxphp_closure_get_static_vars(closure: *mut c_void, out_ht: *mut *mut c_void) -> c_int;
    pub fn oxphp_closure_has_this(closure: *mut c_void) -> c_int;
    pub fn oxphp_closure_get_this(closure: *mut c_void) -> *mut c_void;

    // Borrow proxy
    pub fn oxphp_create_borrow_proxy(dst: *mut c_void, promise_id: u64);

    // Async worker
    pub fn oxphp_async_reset();
    pub fn oxphp_bridge_set_async_worker(is_async: c_int);
    pub fn oxphp_bridge_is_async_worker() -> c_int;

    // ── Async plugin helpers ──
    pub fn oxphp_ht_has_objects_or_resources(ht: *mut c_void) -> c_int;
    pub fn oxphp_bridge_fiber_await(promise_id: i64, timeout: f64, retval: *mut c_void) -> c_int;
    pub fn oxphp_bridge_set_borrow_proxy_ce(ce: *mut c_void);
    pub fn oxphp_arr_add_zval(arr: *mut c_void, key: *const c_char, val: *mut c_void);
    pub fn oxphp_arr_add_index_zval(arr: *mut c_void, idx: u64, val: *mut c_void);

    // ── Async direct dispatch ──
    pub fn oxphp_bridge_async_dispatch(
        op_array: *const c_void,
        static_vars: *mut c_void,
        this_ptr: *mut c_void,
        argc: u32,
        args: *mut c_void,
        closure_zval: *mut c_void,
    ) -> i64;
    pub fn oxphp_bridge_await_dispatch(promise_id: i64, timeout: f64, retval: *mut c_void)
        -> c_int;
    pub fn oxphp_bridge_await_any_dispatch(
        promise_ids: *const i64,
        count: u32,
        timeout: f64,
        out_winner_id: *mut i64,
        retval: *mut c_void,
    ) -> c_int;

    // Async fatal error capture
    pub fn oxphp_bridge_capture_fatal(msg: *const c_char, len: usize);
    pub fn oxphp_bridge_pop_fatal() -> *mut c_char;

    // ── Fiber timer service ──
    pub fn oxphp_bridge_set_timer_callbacks(
        register_fn: Option<unsafe extern "C" fn(u64) -> u64>,
        poll_fn: Option<unsafe extern "C" fn(*mut u64, u32) -> u32>,
        remove_fn: Option<unsafe extern "C" fn(u64)>,
    );

    // ── Fiber TLS context callbacks ──
    pub fn oxphp_bridge_set_fiber_ctx_callbacks(
        save_fn: Option<unsafe extern "C" fn(u64)>,
        restore_fn: Option<unsafe extern "C" fn(u64)>,
        drop_fn: Option<unsafe extern "C" fn(u64)>,
    );

    // ── Fiber scheduler callbacks ──
    pub fn oxphp_bridge_set_fiber_callbacks(
        try_recv_fn: Option<unsafe extern "C" fn() -> std::os::raw::c_int>,
        prepare_fn: Option<unsafe extern "C" fn() -> std::os::raw::c_int>,
    );

    // Async task execution
    pub fn oxphp_execute_async_task(
        op_array: *const c_void,
        static_vars: *const c_void,
        this_ptr: *mut c_void,
        argc: u32,
        args: *mut c_void,
        retval: *mut c_void,
        exc_class: *mut *mut c_char,
        exc_message: *mut *mut c_char,
        exc_trace: *mut *mut c_char,
    ) -> c_int;

    // ─── Plugin Class Registry ──────────────────────────────────
    pub fn oxphp_bridge_register_class(
        fqn: *const c_char,
        parent_fqn: *const c_char,
        flags: u32,
    ) -> c_int;
    pub fn oxphp_bridge_class_implements(class_handle: c_int, interface_fqn: *const c_char);
    pub fn oxphp_bridge_class_add_property(
        class_handle: c_int,
        name: *const c_char,
        visibility: u32,
        modifiers: u32,
        type_info: c_int,
        default_value: *const c_char,
    );
    pub fn oxphp_bridge_class_add_constant(
        class_handle: c_int,
        name: *const c_char,
        visibility: u32,
        value: *const c_char,
    );
    pub fn oxphp_bridge_class_add_method(
        class_handle: c_int,
        name: *const c_char,
        visibility: u32,
        flags: u32,
        required_params: c_int,
        total_params: c_int,
        is_variadic: c_int,
    );
    pub fn oxphp_bridge_class_set_magic(class_handle: c_int, magic_type: c_int, has_handler: c_int);
    pub fn oxphp_bridge_class_enable_custom_object(class_handle: c_int);

    // ─── Plugin Interface Registry ──────────────────────────────
    pub fn oxphp_bridge_register_interface(fqn: *const c_char, parent_fqn: *const c_char) -> c_int;
    pub fn oxphp_bridge_interface_add_method(
        iface_handle: c_int,
        name: *const c_char,
        flags: u32,
        required_params: c_int,
        total_params: c_int,
        is_variadic: c_int,
    );
    pub fn oxphp_bridge_interface_add_constant(
        iface_handle: c_int,
        name: *const c_char,
        visibility: u32,
        value: *const c_char,
    );

    // ─── Plugin Enum Registry ───────────────────────────────────
    pub fn oxphp_bridge_register_enum(fqn: *const c_char, backing_type: c_int) -> c_int;
    pub fn oxphp_bridge_enum_implements(enum_handle: c_int, interface_fqn: *const c_char);
    pub fn oxphp_bridge_enum_add_case(
        enum_handle: c_int,
        name: *const c_char,
        value: *const c_char,
    );
    pub fn oxphp_bridge_enum_add_method(
        enum_handle: c_int,
        name: *const c_char,
        flags: u32,
        required_params: c_int,
        total_params: c_int,
        is_variadic: c_int,
    );

    // ─── Plugin Attribute Registry ──────────────────────────────
    pub fn oxphp_bridge_register_attribute(
        fqn: *const c_char,
        targets: u32,
        is_repeatable: c_int,
    ) -> c_int;
    pub fn oxphp_bridge_attribute_add_param(
        attr_handle: c_int,
        name: *const c_char,
        type_info: c_int,
        is_required: c_int,
        default_value: *const c_char,
    );
    pub fn oxphp_bridge_attribute_add_property(
        attr_handle: c_int,
        name: *const c_char,
        type_info: c_int,
        visibility: u32,
    );

    // ─── Plugin Function Registry (new builder-based) ───────────
    pub fn oxphp_bridge_register_plugin_function(
        fqn: *const c_char,
        required_params: c_int,
        total_params: c_int,
        is_variadic: c_int,
    ) -> c_int;

    // ─── Method Dispatch ────────────────────────────────────────
    pub fn oxphp_bridge_set_method_dispatch(
        dispatch: Option<
            unsafe extern "C" fn(
                class_index: u32,
                method_name: *const c_char,
                args: *mut c_void,
                argc: u32,
                retval: *mut c_void,
                rust_data: *mut c_void,
            ) -> c_int,
        >,
    );

    // ─── Storage Callbacks ──────────────────────────────────────
    pub fn oxphp_bridge_set_storage_callbacks(
        create_fn: Option<unsafe extern "C" fn(class_index: u32) -> *mut c_void>,
        drop_fn: Option<unsafe extern "C" fn(class_index: u32, rust_data: *mut c_void)>,
        clone_fn: Option<
            unsafe extern "C" fn(class_index: u32, rust_data: *mut c_void) -> *mut c_void,
        >,
    );

    // ─── Exception Bridge ───────────────────────────────────────
    pub fn oxphp_throw_exception(class_fqn: *const c_char, message: *const c_char, code: i64);
    pub fn oxphp_exception_pending() -> c_int;
    pub fn oxphp_exception_get(
        class_out: *mut *const c_char,
        message_out: *mut *const c_char,
        code_out: *mut i64,
    );
    pub fn oxphp_exception_clear();
}
