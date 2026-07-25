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
    pub fn oxphp_arg_enum_long(args: *mut c_void, idx: u32) -> i64;

    pub fn oxphp_arg_array_count(args: *mut c_void, idx: u32) -> u32;
    pub fn oxphp_arg_array(args: *mut c_void, idx: u32) -> *mut c_void;

    pub fn oxphp_array_foreach(
        zv_array: *mut c_void,
        cb: unsafe extern "C" fn(*const u8, usize, i64, *mut c_void, *mut c_void),
        user_data: *mut c_void,
    );

    pub fn oxphp_arg_exception_capture(
        args: *mut c_void,
        idx: u32,
        cb: unsafe extern "C" fn(
            *const c_char,
            usize,
            *const c_char,
            usize,
            *const c_char,
            usize,
            *mut c_void,
        ),
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
                fn_attr_names: *const *const c_char,
                fn_attr_count: u32,
                class_attr_names: *const *const c_char,
                class_attr_count: u32,
                attr_ctx: *mut c_void,
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
                exception_class_len: usize,
                exception_message: *const c_char,
                exception_message_len: usize,
                exception_stacktrace: *const c_char,
                exception_stacktrace_len: usize,
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

    // ── Object construction helpers ──
    //
    // Used by Rust handlers for value-typed return classes
    // (e.g. OxPHP\Shared\Channel\RecvResult / SendResult) to build a
    // PHP object directly into a retval slot and stamp private
    // properties on it. See ext/bridge/oxphp_bridge.h for the
    // contract.
    pub fn oxphp_bridge_make_object(
        out: *mut c_void,
        cls_fqn: *const c_char,
        cls_len: usize,
    ) -> c_int;
    pub fn oxphp_bridge_object_set_property_long(
        obj: *mut c_void,
        name: *const c_char,
        name_len: usize,
        val: i64,
    ) -> c_int;
    pub fn oxphp_bridge_object_set_property_zval(
        obj: *mut c_void,
        name: *const c_char,
        name_len: usize,
        src: *mut c_void,
    ) -> c_int;
    pub fn oxphp_bridge_get_enum_case(
        out: *mut c_void,
        cls_fqn: *const c_char,
        cls_len: usize,
        case_name: *const c_char,
        case_len: usize,
    ) -> c_int;
    pub fn oxphp_bridge_wrap_result_ok_inplace(
        retval: *mut c_void,
        cls_fqn: *const c_char,
        cls_len: usize,
        value_prop: *const c_char,
        value_prop_len: usize,
        status_prop: *const c_char,
        status_prop_len: usize,
        status_val: std::os::raw::c_long,
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
    pub fn oxphp_bridge_set_await_race_dispatch(
        f: Option<unsafe extern "C" fn(*const i64, u32, f64, *mut i64, *mut c_void) -> c_int>,
    );
    pub fn oxphp_bridge_set_await_any_dispatch(
        f: Option<unsafe extern "C" fn(*const i64, u32, f64, *mut i64, *mut c_void) -> c_int>,
    );

    // ── Non-blocking await poll ──
    pub fn oxphp_bridge_set_await_poll(f: Option<unsafe extern "C" fn(i64) -> c_int>);

    // ── Async promise cleanup ──
    pub fn oxphp_bridge_set_cleanup_promises(f: Option<unsafe extern "C" fn()>);
    pub fn oxphp_bridge_cleanup_outstanding_promises();
    pub fn oxphp_bridge_set_cleanup_promises_for_fiber(f: Option<unsafe extern "C" fn(u64)>);

    // ── Deferred promise drain (worker mode): poll + pending predicate ──
    pub fn oxphp_bridge_set_deferred_drain_callbacks(
        poll: Option<unsafe extern "C" fn()>,
        pending: Option<unsafe extern "C" fn() -> c_int>,
    );

    // ── Current request fiber identity (0 outside fiber context) ──
    pub fn oxphp_bridge_current_fiber_id() -> u64;

    // ── Async exception details ──
    pub fn oxphp_bridge_set_async_exception(cls: *const c_char, msg: *const c_char);
    pub fn oxphp_bridge_get_async_exc_class() -> *const c_char;
    pub fn oxphp_bridge_get_async_exc_message() -> *const c_char;
    pub fn oxphp_bridge_clear_async_exception();

    // ── Aggregate exception API (multi-error) ──

    pub fn oxphp_bridge_aggregate_clear();

    pub fn oxphp_bridge_aggregate_push(
        exception_class: *const c_char,
        message: *const c_char,
        promise_id: i64,
    );

    pub fn oxphp_bridge_aggregate_throw() -> c_int;

    pub fn oxphp_bridge_aggregate_throw_timeout(
        pending_ids: *const i64,
        pending_count: u32,
    ) -> c_int;

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

    // Cross-thread fcc spike (temporary probe; superseded by the real
    // Pool FFI below).
    pub fn oxphp_pool_spike_capture(callable_zv: *mut c_void, out_tid: *mut u64) -> c_int;
    pub fn oxphp_pool_spike_invoke(
        out_captured_tid: *mut u64,
        out_current_tid: *mut u64,
        out_ret_buf: *mut *mut u8,
        out_ret_len: *mut usize,
    ) -> c_int;
    pub fn oxphp_pool_spike_reset();

    /// Split a PHP array into N independent portbuf-serialized payloads.
    /// Returns 0 on success, -3 if `arr` is not an array, -1 other.
    /// On success `*out_concat` and `*out_offsets` are libc::malloc'd;
    /// caller frees via `oxphp_portable_free`.
    pub fn oxphp_iter_array_to_portbufs(
        arr: *const c_void,
        out_concat: *mut *mut u8,
        out_concat_len: *mut usize,
        out_offsets: *mut *mut usize,
        out_n: *mut usize,
    ) -> c_int;

    /// Deserialize a portbuf and push the resulting zval into `arr`
    /// (which must already be IS_ARRAY). Returns 0 on success, -1 on
    /// deserialize failure or bad arr.
    pub fn oxphp_arr_push_portbuf(arr: *mut c_void, buf: *const u8, len: usize) -> c_int;

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
    pub fn oxphp_ht_has_non_shareable_objects(ht: *mut c_void) -> c_int;
    pub fn oxphp_bridge_fiber_await(promise_id: i64, timeout: f64, retval: *mut c_void) -> c_int;
    pub fn oxphp_bridge_in_fiber() -> c_int;
    /// Cooperatively yield the current task fiber for one scheduler cycle.
    /// Returns 1 if it suspended (in a fiber), 0 if not in a fiber, -3 if
    /// the task was cancelled while yielded.
    pub fn oxphp_bridge_fiber_yield() -> c_int;

    /// Returns 1 iff the zval is an object implementing OxPHP\Shared\Shareable.
    /// Returns 0 for non-objects, non-implementers, or if the CE isn't
    /// registered yet (MINIT hasn't run).
    pub fn oxphp_is_shareable(z: *const c_void) -> c_int;
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
    pub fn oxphp_bridge_await_race_dispatch(
        promise_ids: *const i64,
        count: u32,
        timeout: f64,
        out_winner_id: *mut i64,
        retval: *mut c_void,
    ) -> c_int;
    pub fn oxphp_bridge_await_any_dispatch(
        promise_ids: *const i64,
        count: u32,
        timeout: f64,
        out_winner_id: *mut i64,
        retval: *mut c_void,
    ) -> c_int;

    // ── Synthetic promise bridge setters ──
    //
    // Rust registers these four shims from `AsyncPlugin::init` so future
    // Shared primitives can create a promise id, park it on PROMISE_MAP,
    // and resolve it from any thread. The C-side forwarders live in
    // ext/bridge/oxphp_bridge.c.
    pub fn oxphp_bridge_set_async_synth_alloc(f: extern "C" fn() -> i64);
    pub fn oxphp_bridge_set_async_synth_resolve(f: extern "C" fn(i64, *const u8, usize) -> c_int);
    pub fn oxphp_bridge_set_async_synth_reject(
        f: extern "C" fn(i64, *const c_char, *const c_char) -> c_int,
    );
    pub fn oxphp_bridge_set_async_synth_cancel(f: extern "C" fn(i64) -> c_int);

    // ── Async-task fiber scheduler (Rust-driven) ──
    //
    // The extension owns the scheduler (zend_fiber contexts) and registers
    // the implementations at MINIT; the Rust driver reaches them through
    // these bridge forwarders. See ext/bridge/oxphp_bridge.h for the
    // spawn/tick/poll/release/cancel contract. Pointer params are opaque:
    // op_array/static_vars/this_ptr/args are Rust-owned (borrowed by the
    // fiber); out_retval points into the fiber's owned storage.
    pub fn oxphp_bridge_async_spawn(
        op_array: *const c_void,
        static_vars: *mut c_void,
        this_ptr: *mut c_void,
        argc: u32,
        args: *mut c_void,
        cancel_cell: *mut c_void,
    ) -> i64;
    pub fn oxphp_bridge_async_tick() -> c_int;
    pub fn oxphp_bridge_async_poll_completed(
        out_retval: *mut *mut c_void,
        out_exc_class: *mut *const c_char,
        out_exc_message: *mut *const c_char,
    ) -> i64;
    pub fn oxphp_bridge_async_release(fiber_id: i64);
    pub fn oxphp_bridge_async_cancel(fiber_id: i64) -> c_int;
    pub fn oxphp_bridge_async_drain_output() -> u64;

    // ── Shared\* synchronous invoke shims ──────────────

    pub fn oxphp_shared_invoke_0_portbuf(
        callable: *mut c_void,
        out_ret_buf: *mut *mut u8,
        out_ret_len: *mut usize,
        out_retained_entry: *mut *const c_void,
    ) -> c_int;

    pub fn oxphp_shared_invoke_byref_1_portbuf(
        callable: *mut c_void,
        state_buf: *const u8,
        state_len: usize,
        new_state_buf: *mut *mut u8,
        new_state_len: *mut usize,
        out_ret_buf: *mut *mut u8,
        out_ret_len: *mut usize,
        did_mutate: *mut c_int,
        out_retained_state: *mut *mut c_void,
        out_retained_ret: *mut *mut c_void,
    ) -> c_int;

    /// Release a retained zval pointer produced by
    /// [`oxphp_shared_invoke_byref_1_portbuf`] — either
    /// `out_retained_state` (pins Shareables in the by-ref state) or
    /// `out_retained_ret` (pins Shareables in the closure's return).
    /// Once Rust has finished decoding the corresponding wire buffer
    /// into owned `SharedValue`s (which take their own `Arc` strong
    /// refs), the pin can be released. Null-safe.
    pub fn oxphp_shared_free_zval(p: *mut c_void);

    /// Invoke `$fn(key, value)` for `Shared\Map::forEach`. The key is the
    /// tagged tuple `(key_kind: 0=int/1=str, key_int, key_ptr, key_len)`;
    /// the value is a portbuf buffer deserialised into a zval. Returns
    /// `1` to STOP (callback returned `false`), `0` to continue, `<0` on
    /// call failure (EG(exception) set on the PHP side).
    pub fn oxphp_shared_invoke_2_ret_stop(
        callable: *mut c_void,
        key_kind: c_int,
        key_int: i64,
        key_ptr: *const u8,
        key_len: usize,
        val_buf: *const u8,
        val_len: usize,
    ) -> c_int;

    /// Construct a `OxPHP\Shared\Map\KeyCursor` object into `out_zv` and
    /// stamp `state_ptr` (a `Box::into_raw(Box<KeyCursorState>)`) into its
    /// rust_data storage slot (offset 0). Mirrors
    /// `oxphp_shared_pool_handle_alloc`. Returns `0` on success, `-1` if
    /// the class is not registered / object_init_ex fails (the caller
    /// then reclaims the box).
    pub fn oxphp_shared_map_cursor_alloc(out_zv: *mut c_void, state_ptr: *mut c_void) -> c_int;

    // ── Shared\Pool bridge ───────────────────
    // See ext/bridge/oxphp_bridge.h §Shared\Pool helpers for the
    // lifetime contract. Pointers returned via out-params are
    // emalloc'd in C and owned by the pool (Rust) until the pool
    // drops. All *_free calls must run on a Zend-initialised worker.
    pub fn oxphp_pool_fcc_new(callable_zval: *mut c_void, out_fcc_heap: *mut *mut c_void) -> c_int;
    pub fn oxphp_pool_fcc_free(fcc_heap: *mut c_void);
    pub fn oxphp_pool_factory_invoke(
        fcc_heap: *mut c_void,
        out_slot_zv_heap: *mut *mut c_void,
    ) -> c_int;
    pub fn oxphp_pool_body_invoke(
        body_callable_zv: *mut c_void,
        slot_zv_heap: *mut c_void,
        user_out_zv: *mut c_void,
    ) -> c_int;
    pub fn oxphp_pool_slot_to_user(slot_zv_heap: *mut c_void, user_out_zv: *mut c_void);
    pub fn oxphp_pool_slot_free(slot_zv_heap: *mut c_void);
    pub fn oxphp_pool_destroy_invoke(
        destroy_fcc_heap: *mut c_void,
        slot_zv_heap: *mut c_void,
    ) -> c_int;

    // Shared\Pool\Handle rust_data wrapper helpers. Handle storage
    // layout mirrors `PoolHandleStorage` (repr(C)): u64 pool_id,
    // u64 owner_tid, `*mut c_void` slot_zv_heap.
    pub fn oxphp_shared_pool_handle_alloc(
        out_zv: *mut c_void,
        pool_id: u64,
        owner_tid: u64,
        slot_zv_heap: *mut c_void,
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

    /// Idle backoff for the async task driver, implemented by the PHP
    /// extension: waits `ns` on the descriptors task fibers are parked on and
    /// returns 1, or returns 0 when nothing is parked and the caller should
    /// back off on its own. Waiting on the descriptors is what keeps a hooked
    /// socket round trip from paying the driver's whole idle interval.
    pub fn oxphp_bridge_async_io_backoff(ns: u64) -> c_int;

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
        return_type: c_int,
        return_nullable: c_int,
        param_names: *const *const c_char,
        param_types: *const c_int,
        param_optional: *const c_int,
    );
    pub fn oxphp_bridge_get_class_method_param_name(
        class_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> *const c_char;
    pub fn oxphp_bridge_get_class_method_param_type(
        class_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> c_int;
    pub fn oxphp_bridge_get_class_method_param_optional(
        class_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> c_int;
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
        return_type: c_int,
        return_nullable: c_int,
        param_names: *const *const c_char,
        param_types: *const c_int,
        param_optional: *const c_int,
    );
    pub fn oxphp_bridge_get_interface_method_param_name(
        iface_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> *const c_char;
    pub fn oxphp_bridge_get_interface_method_param_type(
        iface_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> c_int;
    pub fn oxphp_bridge_get_interface_method_param_optional(
        iface_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> c_int;
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
        return_type: c_int,
        return_nullable: c_int,
        param_names: *const *const c_char,
        param_types: *const c_int,
        param_optional: *const c_int,
    );
    pub fn oxphp_bridge_get_enum_method_param_name(
        enum_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> *const c_char;
    pub fn oxphp_bridge_get_enum_method_param_type(
        enum_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> c_int;
    pub fn oxphp_bridge_get_enum_method_param_optional(
        enum_index: c_int,
        method_index: c_int,
        param_index: c_int,
    ) -> c_int;

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
        return_type: c_int,
        return_nullable: c_int,
        param_names: *const *const c_char,
        param_types: *const c_int,
        param_optional: *const c_int,
    ) -> c_int;
    pub fn oxphp_bridge_get_plugin_function_param_name(
        index: c_int,
        param_index: c_int,
    ) -> *const c_char;
    pub fn oxphp_bridge_get_plugin_function_param_type(index: c_int, param_index: c_int) -> c_int;
    pub fn oxphp_bridge_get_plugin_function_param_optional(
        index: c_int,
        param_index: c_int,
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
                this_zval: *mut c_void,
            ) -> c_int,
        >,
    );

    // ─── Object property access ─────────────────────────────────
    pub fn oxphp_object_read_property(
        object_zval: *mut c_void,
        property_name: *const c_char,
    ) -> *mut c_void;

    pub fn oxphp_zval_is_null_or_unset(zval_ptr: *const c_void) -> c_int;

    pub fn oxphp_zval_copy_to_retval(src_zval: *const c_void, dst_zval: *mut c_void);

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

    // ── Sub-design A: cancellation reason API ──
    pub fn oxphp_bridge_set_cancel_ptr(ptr: *const std::sync::atomic::AtomicU8);
    pub fn oxphp_bridge_get_cancel_reason() -> u8;
    pub fn oxphp_bridge_set_cancel_reason(reason: u8) -> bool;
    // Process-global graceful-shutdown drain latches (NOT the per-request
    // cancel reason above). Both one-way. `set_draining` latches at SIGTERM
    // (read by the fiber scheduler and stream-flush path); `set_drain_hard`
    // latches when the drain deadline passes (read by the interrupt handler
    // to self-cancel the request running when the broadcast kick lands).
    pub fn oxphp_bridge_set_draining();
    pub fn oxphp_bridge_is_draining() -> bool;
    pub fn oxphp_bridge_set_drain_hard();
    pub fn oxphp_bridge_vm_interrupt_addr() -> *mut u8;
    pub fn oxphp_bridge_set_vm_interrupt_addr(addr: *mut std::os::raw::c_void);
    pub fn oxphp_bridge_request_interrupt();
    pub fn oxphp_bridge_request_interrupt_at(addr: *mut std::os::raw::c_void);
    pub fn oxphp_capture_vm_interrupt();
    pub fn oxphp_bridge_get_worker_id() -> std::os::raw::c_int;

    // Tick counter pointer; see common.rs binding for full docs.
    pub fn oxphp_bridge_set_tick_ptr(ptr: *const std::sync::atomic::AtomicU64);
}

pub const OXPHP_SHARED_INVOKE_OK: c_int = 0;
pub const OXPHP_SHARED_INVOKE_PHP_THREW: c_int = 1;
/// "Cannot invoke at all": null pointer, `zend_fcall_info_init` failed, etc.
/// The callable argument itself is unusable. Defined to mirror the C-side
/// ABI; Rust callers fold this into the generic-error arm of their match
/// rather than branching on it explicitly.
#[allow(dead_code)]
pub const OXPHP_SHARED_INVOKE_BAD_CALLABLE: c_int = -1;
/// "Invoked OK, but the returned value cannot be ferried back": the
/// callable ran to completion without throwing, but `oxphp_portable_serialize`
/// rejected the result (closure, resource, non-Shareable object …).
/// Distinct from `BAD_CALLABLE` so callers can surface a precise error
/// instead of conflating "invalid callable" with "invalid return".
pub const OXPHP_SHARED_INVOKE_BAD_RETURN: c_int = -2;
