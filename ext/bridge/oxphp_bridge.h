#ifndef OXPHP_BRIDGE_H
#define OXPHP_BRIDGE_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * OxPHP Bridge Library
 *
 * Shared C library with __thread TLS that both Rust and PHP link against.
 * This is the ONLY way to share per-request state between Rust and the
 * PHP extension — direct __thread vars in Rust are invisible to dlopen'd
 * PHP extensions.
 */

/** Per-request context stored in __thread TLS.
 *
 * Field ordering is cache-line optimized:
 * - Hot fields (accessed every ub_write, ~per opcode) come first so they
 *   share a single 64-byte cache line.
 * - Warm fields (accessed once per request) follow.
 * - Cold fields (worker-mode config, set once per thread) are last. */
typedef struct {
    /* ── Hot: accessed every ub_write (~per PHP opcode) ───── */

    /** ub_write call counter for periodic deadline checks. */
    uint32_t write_count;

    /** Whether cancellation has been requested (client disconnected). */
    bool cancelled;

    /** Deadline timestamp (Unix epoch, microseconds). 0 = no deadline. */
    int64_t deadline_us;

    /* ── Warm: accessed once per request ─────────────────── */

    /** Request start time (Unix epoch, microseconds). */
    double request_time;

    /** Whether streaming mode is active. */
    bool stream_mode;

    /** Whether headers have been sent (streaming mode). */
    bool headers_sent;

    /** Whether oxphp_finish_request() was called. */
    bool finished;

    /** Hex request ID (64 chars + null). */
    char request_id[65];

    /** Worker thread index. */
    int32_t worker_id;

    /* ── Cold: worker mode config (set once per thread) ──── */

    /** Whether this thread is in worker mode (persistent PHP process). */
    int worker_mode;

    /** Number of requests completed by this worker (worker mode). */
    uint64_t requests_done;

    /** Max requests before worker recycle (0 = unlimited). */
    uint64_t max_requests;

    /** Max memory in bytes before worker recycle (0 = unlimited).
     *  Pre-computed from MB to avoid per-request multiplication. */
    uint64_t max_memory_bytes;

    /** Exit reason for worker mode (0=shutdown, 1=max_requests, 2=max_memory, 3=error). */
    uint8_t exit_reason;

    /** Whether the current handler invocation failed (bailout/fatal error). */
    bool handler_failed;

    /** Consecutive handler errors (bailout). Resets on success, worker exits at threshold. */
    uint32_t consecutive_errors;

    /** Current PHP heap usage in bytes (updated after each request). */
    uint64_t current_memory_bytes;

    /** Whether this thread is an async worker (not a request worker). */
    int is_async_worker;
} oxphp_ctx_t;

/**
 * Initialize the thread-local context with default values.
 * Must be called at the start of each request, BEFORE php_request_startup().
 */
void oxphp_bridge_init_ctx(void);

/**
 * Clear the thread-local context.
 * Must be called after php_request_shutdown().
 */
void oxphp_bridge_clear_ctx(void);

/** Get pointer to the thread-local context (read/write). */
oxphp_ctx_t *oxphp_bridge_get_ctx(void);

/** Set the request ID (copies up to 64 chars). */
void oxphp_bridge_set_request_id(const char *id);

/** Get the request ID (returns pointer into TLS — valid until clear_ctx). */
const char *oxphp_bridge_get_request_id(void);

/** Set the worker ID. */
void oxphp_bridge_set_worker_id(int32_t id);

/** Get the worker ID. */
int32_t oxphp_bridge_get_worker_id(void);

/** Set request start time. */
void oxphp_bridge_set_request_time(double time);

/** Get request start time. */
double oxphp_bridge_get_request_time(void);

/** Set streaming mode. */
void oxphp_bridge_set_stream_mode(bool mode);

/** Check if streaming mode is active. */
bool oxphp_bridge_is_streaming(void);

/** Mark request as finished (oxphp_finish_request). */
void oxphp_bridge_set_finished(bool finished);

/** Check if request is finished. */
bool oxphp_bridge_is_finished(void);

/** Set the execution deadline (Unix epoch, microseconds). 0 = no deadline. */
void oxphp_bridge_set_deadline(int64_t deadline_us);

/** Get the execution deadline. */
int64_t oxphp_bridge_get_deadline(void);

/** Check if the execution deadline has expired. */
bool oxphp_bridge_is_deadline_expired(void);

/** Mark headers as sent (streaming mode). */
void oxphp_bridge_set_headers_sent(bool sent);

/** Check if headers have been sent. */
bool oxphp_bridge_get_headers_sent(void);

/* ─── Plugin Function Registry (global, NOT __thread) ─────────── */

/** Register a plugin function (called by Rust after plugin init). */
void oxphp_bridge_register_plugin_fn(const char* name, int required_params, int total_params);

/** Get number of registered plugin functions. */
int oxphp_bridge_get_plugin_fn_count(void);

/** Get plugin function name by index. */
const char* oxphp_bridge_get_plugin_fn_name(int index);

/** Get plugin function required param count by index. */
int oxphp_bridge_get_plugin_fn_required(int index);

/** Get plugin function total param count by index. */
int oxphp_bridge_get_plugin_fn_total(int index);

/* ─── Plugin Class Registry (global, NOT __thread) ──────────── */

/** Register a class definition. Returns handle (index) for subsequent calls. */
int oxphp_bridge_register_class(const char *fqn, const char *parent_fqn, uint32_t flags);

/** Add an interface implementation to a class. */
void oxphp_bridge_class_implements(int class_handle, const char *interface_fqn);

/** Add a property to a class. default_value may be NULL. */
void oxphp_bridge_class_add_property(int class_handle, const char *name,
    uint32_t visibility, uint32_t modifiers, int type_info, const char *default_value);

/** Add a constant to a class. */
void oxphp_bridge_class_add_constant(int class_handle, const char *name,
    uint32_t visibility, const char *value);

/** Add a method to a class. */
void oxphp_bridge_class_add_method(int class_handle, const char *name,
    uint32_t visibility, uint32_t flags, int required_params, int total_params, int is_variadic);

/** Set a magic method handler flag. magic_type is the MagicMethod enum ordinal (0-16). */
void oxphp_bridge_class_set_magic(int class_handle, int magic_type, int has_handler);

/** Enable custom object storage for a class. */
void oxphp_bridge_class_enable_custom_object(int class_handle);

/** Get number of registered plugin classes. */
int oxphp_bridge_get_plugin_class_count(void);

/** Get class FQN by index. */
const char *oxphp_bridge_get_class_fqn(int index);

/** Get class parent FQN (NULL if none). */
const char *oxphp_bridge_get_class_parent(int index);

/** Get class flags. */
uint32_t oxphp_bridge_get_class_flags(int index);

/** Get whether class has custom object storage. */
int oxphp_bridge_get_class_has_custom_object(int index);

/** Interface count for a class. */
int oxphp_bridge_get_class_interface_count(int index);

/** Get interface FQN by class + interface index. */
const char *oxphp_bridge_get_class_interface_fqn(int class_index, int iface_index);

/** Property count for a class. */
int oxphp_bridge_get_class_property_count(int index);

/** Get property name by class + property index. */
const char *oxphp_bridge_get_class_property_name(int class_index, int prop_index);

/** Get property visibility. */
uint32_t oxphp_bridge_get_class_property_visibility(int class_index, int prop_index);

/** Get property modifiers. */
uint32_t oxphp_bridge_get_class_property_modifiers(int class_index, int prop_index);

/** Get property default value (NULL if none). */
const char *oxphp_bridge_get_class_property_default(int class_index, int prop_index);

/** Constant count for a class. */
int oxphp_bridge_get_class_constant_count(int index);

/** Get constant name. */
const char *oxphp_bridge_get_class_constant_name(int class_index, int const_index);

/** Get constant visibility. */
uint32_t oxphp_bridge_get_class_constant_visibility(int class_index, int const_index);

/** Get constant value string. */
const char *oxphp_bridge_get_class_constant_value(int class_index, int const_index);

/** Method count for a class. */
int oxphp_bridge_get_class_method_count(int index);

/** Get method name. */
const char *oxphp_bridge_get_class_method_name(int class_index, int method_index);

/** Get method visibility. */
uint32_t oxphp_bridge_get_class_method_visibility(int class_index, int method_index);

/** Get method flags. */
uint32_t oxphp_bridge_get_class_method_flags(int class_index, int method_index);

/** Get method required param count. */
int oxphp_bridge_get_class_method_required(int class_index, int method_index);

/** Get method total param count. */
int oxphp_bridge_get_class_method_total(int class_index, int method_index);

/** Get whether method is variadic. */
int oxphp_bridge_get_class_method_is_variadic(int class_index, int method_index);

/** Get magic method handler flag. magic_type is MagicMethod enum ordinal. */
int oxphp_bridge_get_class_magic(int class_index, int magic_type);

/* ─── Plugin Interface Registry (global, NOT __thread) ──────── */

/** Register an interface. Returns handle (index). parent_fqn may be NULL. */
int oxphp_bridge_register_interface(const char *fqn, const char *parent_fqn);

/** Add a method to an interface. */
void oxphp_bridge_interface_add_method(int iface_handle, const char *name,
    uint32_t flags, int required_params, int total_params, int is_variadic);

/** Add a constant to an interface. */
void oxphp_bridge_interface_add_constant(int iface_handle, const char *name,
    uint32_t visibility, const char *value);

/** Get interface count. */
int oxphp_bridge_get_plugin_interface_count(void);

/** Get interface FQN. */
const char *oxphp_bridge_get_interface_fqn(int index);

/** Get interface parent FQN (NULL if none). */
const char *oxphp_bridge_get_interface_parent(int index);

/** Method count for an interface. */
int oxphp_bridge_get_interface_method_count(int index);

/** Get interface method name. */
const char *oxphp_bridge_get_interface_method_name(int iface_index, int method_index);

/** Get interface method flags. */
uint32_t oxphp_bridge_get_interface_method_flags(int iface_index, int method_index);

/** Get interface method required params. */
int oxphp_bridge_get_interface_method_required(int iface_index, int method_index);

/** Get interface method total params. */
int oxphp_bridge_get_interface_method_total(int iface_index, int method_index);

/** Get interface method is_variadic. */
int oxphp_bridge_get_interface_method_is_variadic(int iface_index, int method_index);

/** Constant count for an interface. */
int oxphp_bridge_get_interface_constant_count(int index);

/** Get interface constant name. */
const char *oxphp_bridge_get_interface_constant_name(int iface_index, int const_index);

/** Get interface constant visibility. */
uint32_t oxphp_bridge_get_interface_constant_visibility(int iface_index, int const_index);

/** Get interface constant value. */
const char *oxphp_bridge_get_interface_constant_value(int iface_index, int const_index);

/* ─── Plugin Enum Registry (global, NOT __thread) ───────────── */

/** Register an enum. backing_type: 0=unit, 4=IS_LONG, 6=IS_STRING. Returns handle. */
int oxphp_bridge_register_enum(const char *fqn, int backing_type);

/** Add an interface implementation to an enum. */
void oxphp_bridge_enum_implements(int enum_handle, const char *interface_fqn);

/** Add a case to an enum. value may be NULL for unit enums. */
void oxphp_bridge_enum_add_case(int enum_handle, const char *name, const char *value);

/** Add a method to an enum. */
void oxphp_bridge_enum_add_method(int enum_handle, const char *name,
    uint32_t flags, int required_params, int total_params, int is_variadic);

/** Get enum count. */
int oxphp_bridge_get_plugin_enum_count(void);

/** Get enum FQN. */
const char *oxphp_bridge_get_enum_fqn(int index);

/** Get enum backing type. */
int oxphp_bridge_get_enum_backing_type(int index);

/** Interface count for an enum. */
int oxphp_bridge_get_enum_interface_count(int index);

/** Get enum interface FQN. */
const char *oxphp_bridge_get_enum_interface_fqn(int enum_index, int iface_index);

/** Case count for an enum. */
int oxphp_bridge_get_enum_case_count(int index);

/** Get enum case name. */
const char *oxphp_bridge_get_enum_case_name(int enum_index, int case_index);

/** Get enum case value (NULL for unit enums). */
const char *oxphp_bridge_get_enum_case_value(int enum_index, int case_index);

/** Method count for an enum. */
int oxphp_bridge_get_enum_method_count(int index);

/** Get enum method name. */
const char *oxphp_bridge_get_enum_method_name(int enum_index, int method_index);

/** Get enum method flags. */
uint32_t oxphp_bridge_get_enum_method_flags(int enum_index, int method_index);

/** Get enum method required params. */
int oxphp_bridge_get_enum_method_required(int enum_index, int method_index);

/** Get enum method total params. */
int oxphp_bridge_get_enum_method_total(int enum_index, int method_index);

/** Get enum method is_variadic. */
int oxphp_bridge_get_enum_method_is_variadic(int enum_index, int method_index);

/* ─── Plugin Attribute Registry (global, NOT __thread) ──────── */

/** Register an attribute. targets is bitmask of Attribute::TARGET_*. Returns handle. */
int oxphp_bridge_register_attribute(const char *fqn, uint32_t targets, int is_repeatable);

/** Add a parameter to an attribute. */
void oxphp_bridge_attribute_add_param(int attr_handle, const char *name,
    int type_info, int is_required, const char *default_value);

/** Add a property to an attribute (for attributes that are also classes). */
void oxphp_bridge_attribute_add_property(int attr_handle, const char *name,
    int type_info, uint32_t visibility);

/** Get attribute count. */
int oxphp_bridge_get_plugin_attribute_count(void);

/** Get attribute FQN. */
const char *oxphp_bridge_get_attribute_fqn(int index);

/** Get attribute targets bitmask. */
uint32_t oxphp_bridge_get_attribute_targets(int index);

/** Get whether attribute is repeatable. */
int oxphp_bridge_get_attribute_is_repeatable(int index);

/** Param count for an attribute. */
int oxphp_bridge_get_attribute_param_count(int index);

/** Get attribute param name. */
const char *oxphp_bridge_get_attribute_param_name(int attr_index, int param_index);

/** Get attribute param is_required. */
int oxphp_bridge_get_attribute_param_is_required(int attr_index, int param_index);

/** Get attribute param default value (NULL if none). */
const char *oxphp_bridge_get_attribute_param_default(int attr_index, int param_index);

/** Property count for an attribute. */
int oxphp_bridge_get_attribute_property_count(int index);

/** Get attribute property name. */
const char *oxphp_bridge_get_attribute_property_name(int attr_index, int prop_index);

/** Get attribute property visibility. */
uint32_t oxphp_bridge_get_attribute_property_visibility(int attr_index, int prop_index);

/* ─── Plugin Function Registry (new builder-based) ──────────── */

/** Register a plugin function via builder. Returns handle (index). */
int oxphp_bridge_register_plugin_function(const char *fqn, int required_params,
    int total_params, int is_variadic);

/** Get number of registered builder-based functions. */
int oxphp_bridge_get_plugin_function_count(void);

/** Get builder-based function FQN. */
const char *oxphp_bridge_get_plugin_function_fqn(int index);

/** Get builder-based function required params. */
int oxphp_bridge_get_plugin_function_required(int index);

/** Get builder-based function total params. */
int oxphp_bridge_get_plugin_function_total(int index);

/** Get builder-based function is_variadic. */
int oxphp_bridge_get_plugin_function_is_variadic(int index);

/* ─── Method Dispatch Callback ──────────────────────────────── */

/** Callback type for dispatching class method calls to Rust. */
typedef int (*oxphp_method_dispatch_fn_t)(
    uint32_t class_index, const char *method_name,
    void *args, uint32_t argc, void *retval, void *rust_data
);

/** Set the method dispatch callback (called once at startup). */
void oxphp_bridge_set_method_dispatch(oxphp_method_dispatch_fn_t fn);

/** Get the method dispatch callback. */
oxphp_method_dispatch_fn_t oxphp_bridge_get_method_dispatch(void);

/* ─── Storage Callbacks ─────────────────────────────────────── */

/** Callback: create rust_data for a class instance. */
typedef void *(*oxphp_storage_create_fn_t)(uint32_t class_index);

/** Callback: drop rust_data. */
typedef void (*oxphp_storage_drop_fn_t)(uint32_t class_index, void *rust_data);

/** Callback: clone rust_data. */
typedef void *(*oxphp_storage_clone_fn_t)(uint32_t class_index, void *rust_data);

/** Set storage lifecycle callbacks (called once at startup). */
void oxphp_bridge_set_storage_callbacks(
    oxphp_storage_create_fn_t create_fn,
    oxphp_storage_drop_fn_t drop_fn,
    oxphp_storage_clone_fn_t clone_fn
);

/** Get storage create callback. */
oxphp_storage_create_fn_t oxphp_bridge_get_storage_create(void);

/** Get storage drop callback. */
oxphp_storage_drop_fn_t oxphp_bridge_get_storage_drop(void);

/** Get storage clone callback. */
oxphp_storage_clone_fn_t oxphp_bridge_get_storage_clone(void);

/* ─── Exception Bridge ──────────────────────────────────────── */

/** Throw a PHP exception from Rust. class_fqn may be NULL for RuntimeException. */
void oxphp_throw_exception(const char *class_fqn, const char *message, int64_t code);

/** Check if a PHP exception is pending. Returns 1 if pending, 0 otherwise. */
int oxphp_exception_pending(void);

/** Get the current pending exception details. Strings are temporary. */
void oxphp_exception_get(const char **class_out, const char **message_out, int64_t *code_out);

/** Clear the current pending exception. */
void oxphp_exception_clear(void);

/* ═══════════════════════════════════════════════════════════
 *  Native Bridge API — Zero-Serialization Value Access
 *  Rust reads/writes PHP zvals directly through these C helpers.
 *  All zval pointers passed as void* (Rust doesn't know zval layout).
 * ═══════════════════════════════════════════════════════════ */

/* ── Type constants (stable across PHP versions) ── */
#define OXPHP_TYPE_NULL     0
#define OXPHP_TYPE_FALSE    1
#define OXPHP_TYPE_TRUE     2
#define OXPHP_TYPE_LONG     3
#define OXPHP_TYPE_DOUBLE   4
#define OXPHP_TYPE_STRING   5
#define OXPHP_TYPE_ARRAY    6
#define OXPHP_TYPE_OBJECT   7
#define OXPHP_TYPE_RESOURCE 8

/* ── Type inspection ── */
uint8_t oxphp_val_type(void *zv);
uint8_t oxphp_val_arg_type(void *args, uint32_t idx);

/* ── Scalar reading from args[idx] ── */
int64_t oxphp_arg_long(void *args, uint32_t idx);
double oxphp_arg_double(void *args, uint32_t idx);
int oxphp_arg_bool(void *args, uint32_t idx);
const uint8_t *oxphp_arg_str(void *args, uint32_t idx, size_t *out_len);

/* ── Array reading from args[idx] ── */
uint32_t oxphp_arg_array_count(void *args, uint32_t idx);
void *oxphp_arg_array(void *args, uint32_t idx);

/* ── Array iteration ── */
typedef void (*oxphp_array_iter_fn)(
    const uint8_t *key, size_t key_len, int64_t idx,
    void *val, void *user_data
);
void oxphp_array_foreach(void *zv_array, oxphp_array_iter_fn cb, void *user_data);

/* ── Direct value reading (from zval*, not from args array) ── */
int64_t oxphp_val_long(void *zv);
double  oxphp_val_double(void *zv);
int     oxphp_val_bool(void *zv);
const uint8_t *oxphp_val_str(void *zv, size_t *out_len);
uint32_t oxphp_val_array_count(void *zv);

/* ── Scalar writing (into return_value zval) ── */
void oxphp_ret_null(void *retval);
void oxphp_ret_bool(void *retval, int val);
void oxphp_ret_long(void *retval, int64_t val);
void oxphp_ret_double(void *retval, double val);
void oxphp_ret_str(void *retval, const uint8_t *s, size_t len);

/* ── Array writing ── */
void oxphp_ret_array_init(void *retval, uint32_t size_hint);

/* Keyed (associative) */
void oxphp_arr_add_null(void *arr, const char *key, size_t klen);
void oxphp_arr_add_bool(void *arr, const char *key, size_t klen, int val);
void oxphp_arr_add_long(void *arr, const char *key, size_t klen, int64_t val);
void oxphp_arr_add_double(void *arr, const char *key, size_t klen, double val);
void oxphp_arr_add_str(void *arr, const char *key, size_t klen, const uint8_t *s, size_t slen);
void *oxphp_arr_add_array(void *arr, const char *key, size_t klen, uint32_t size_hint);

/* Indexed (push / append) */
void oxphp_arr_push_null(void *arr);
void oxphp_arr_push_bool(void *arr, int val);
void oxphp_arr_push_long(void *arr, int64_t val);
void oxphp_arr_push_double(void *arr, double val);
void oxphp_arr_push_str(void *arr, const uint8_t *s, size_t len);
void *oxphp_arr_push_array(void *arr, uint32_t size_hint);

/* ── Native dispatch callback (C extension → Rust) ── */
typedef int (*oxphp_native_dispatch_fn_t)(
    const char *name, void *args, uint32_t argc, void *retval
);
void oxphp_bridge_set_native_dispatch(oxphp_native_dispatch_fn_t fn);
oxphp_native_dispatch_fn_t oxphp_bridge_get_native_dispatch(void);

/* ── Decorator system ── */

typedef int (*oxphp_decorator_resolve_fn_t)(
    uintptr_t fn_id,
    const char **attr_names,
    uint32_t attr_count
);

typedef int (*oxphp_decorator_begin_fn_t)(
    uintptr_t fn_id,
    const char *target,
    const char *class_name,
    uint64_t object_id,
    uint64_t timestamp_ns
);

typedef void (*oxphp_decorator_end_fn_t)(
    uintptr_t fn_id,
    uint64_t elapsed_ns,
    int success,
    const char *exception_class
);

typedef void (*oxphp_decorator_register_php_fn_t)(
    const char *class_name,
    uint32_t targets
);

void oxphp_bridge_set_decorator_registry(void *ptr);
void *oxphp_bridge_get_decorator_registry(void);

void oxphp_bridge_set_decorator_resolve(oxphp_decorator_resolve_fn_t fn);
oxphp_decorator_resolve_fn_t oxphp_bridge_get_decorator_resolve(void);

void oxphp_bridge_set_decorator_begin(oxphp_decorator_begin_fn_t fn);
oxphp_decorator_begin_fn_t oxphp_bridge_get_decorator_begin(void);

void oxphp_bridge_set_decorator_end(oxphp_decorator_end_fn_t fn);
oxphp_decorator_end_fn_t oxphp_bridge_get_decorator_end(void);

void oxphp_bridge_set_decorator_register_php(oxphp_decorator_register_php_fn_t fn);
oxphp_decorator_register_php_fn_t oxphp_bridge_get_decorator_register_php(void);

void oxphp_bridge_set_decorator_reject_reason(const char *reason, size_t len);
const char *oxphp_bridge_get_decorator_reject_reason(size_t *out_len);
void oxphp_bridge_clear_decorator_reject_reason(void);

void oxphp_bridge_register_php_decorator(const char *class_name, uint32_t targets);

/* ── PHP decorator query callbacks ── */
typedef uint32_t (*oxphp_php_dec_count_fn_t)(uintptr_t fn_id);
typedef const char * (*oxphp_php_dec_class_fn_t)(uintptr_t fn_id, uint32_t index);
typedef uint64_t (*oxphp_php_dec_cache_key_fn_t)(uintptr_t fn_id, uint32_t index);

void oxphp_bridge_set_php_decorator_count(oxphp_php_dec_count_fn_t fn);
oxphp_php_dec_count_fn_t oxphp_bridge_get_php_decorator_count(void);

void oxphp_bridge_set_php_decorator_class(oxphp_php_dec_class_fn_t fn);
oxphp_php_dec_class_fn_t oxphp_bridge_get_php_decorator_class(void);

void oxphp_bridge_set_php_decorator_cache_key(oxphp_php_dec_cache_key_fn_t fn);
oxphp_php_dec_cache_key_fn_t oxphp_bridge_get_php_decorator_cache_key(void);

void oxphp_bridge_set_decorator_class_buf(const char *s, size_t len);
const char *oxphp_bridge_get_decorator_class_buf(void);

#define OXPHP_DECORATOR_CTX_STACK_MAX 32

typedef struct {
    uintptr_t fn_id;
    const char *target;
    const char *class_name;
    uint64_t object_id;
    uint64_t timestamp_ns;
    void *execute_data;
    int decorator_count;
} oxphp_decorator_ctx_t;

oxphp_decorator_ctx_t *oxphp_decorator_ctx_push(void);
oxphp_decorator_ctx_t *oxphp_decorator_ctx_peek(void);
void oxphp_decorator_ctx_pop(void);

/* ── Call PHP function from Rust (native, no serialization) ── */
int oxphp_call_php_native(
    const char *func_name, void *args, uint32_t argc, void *result
);

/* ── TSRM cache ── */

/**
 * Update the TSRM thread-local cache in this shared library.
 * Must be called on each worker thread after ts_resource_ex()
 * and before any SG()/CG()/EG() macro usage from this library.
 */
void oxphp_bridge_tsrm_update(void);

/* ── SAPI request_info ── */

/**
 * Set SG(request_info) fields BEFORE php_request_startup().
 * PHP uses these to parse $_GET, $_POST, $_FILES, $_COOKIE.
 *
 * method: "GET", "POST", etc.
 * query_string: raw query string (after '?'), or NULL.
 * content_type: "multipart/form-data; boundary=...", etc., or NULL.
 * content_length: value of Content-Length header, or 0 if absent.
 */
void oxphp_bridge_set_request_info(
    const char *method,
    const char *query_string,
    const char *content_type,
    long content_length
);

/* ── SAPI response code ── */

/** Read SG(sapi_headers).http_response_code from the C side (correct TSRM context). */
int oxphp_bridge_get_response_code(void);

/* ── Zval lifecycle ── */

/** Destroy a zval (decrement refcount, free if needed). */
void oxphp_zval_dtor(void *zv);

/** Increment zval refcount (prevent GC while async task holds op_array pointer). */
void oxphp_zval_addref(void *zv);

/** Addref the closure object and return the zend_object pointer (stable across stack frames). */
void *oxphp_closure_addref(void *closure_zv);

/** Release a closure object reference obtained via oxphp_closure_addref. */
void oxphp_closure_release(void *obj_ptr);

/** Return sizeof(zval) for the running PHP build. */
size_t oxphp_zval_size(void);

/** Trigger zend_bailout() — safely abort PHP execution from SAPI callbacks. */
void oxphp_bridge_bailout(void);

/* ── SAPI callback wrappers with cooperative deadline check ── */

/**
 * Register the Rust-side ub_write and flush implementations.
 * The bridge provides wrapper functions that check the deadline BEFORE
 * calling through to Rust, and call zend_bailout() from C if expired.
 * This avoids longjmp crossing Rust FFI boundaries.
 */
typedef size_t (*oxphp_ub_write_fn_t)(const char *str, size_t str_length);
typedef void   (*oxphp_flush_fn_t)(void *server_context);

void oxphp_bridge_set_sapi_callbacks(oxphp_ub_write_fn_t ub_write, oxphp_flush_fn_t flush);

/** C wrapper for ub_write — checks deadline, then calls Rust impl. */
size_t oxphp_bridge_ub_write(const char *str, size_t str_length);

/** C wrapper for flush — checks deadline, then calls Rust impl. */
void oxphp_bridge_flush(void *server_context);

/* ─── Superglobals Configuration ──────────────────────────── */

/** Set whether superglobals are enabled (called once at startup from Rust). */
void oxphp_bridge_set_superglobals_enabled(bool enabled);

/** Check if superglobals are enabled. */
bool oxphp_bridge_get_superglobals_enabled(void);

/* ─── HTTP Request Data Accessors (Rust callback pattern) ─── */

/** Rust callback types for lazy request data access.
 *  String accessors return pointer valid until next request or clear.
 *  out_len receives the byte length. NULL means absent/no value. */
typedef const char* (*oxphp_req_str_fn_t)(size_t *out_len);
typedef const char* (*oxphp_req_str_key_fn_t)(const char *key, size_t key_len, size_t *out_len);
typedef double (*oxphp_req_double_fn_t)(void);
typedef int (*oxphp_req_bool_fn_t)(void);
typedef uint16_t (*oxphp_req_u16_fn_t)(void);

/** Visitor callback for iterating key-value pairs. */
typedef void (*oxphp_req_pairs_cb_t)(const char *key, size_t klen,
                                      const char *val, size_t vlen,
                                      void *user_data);
typedef void (*oxphp_req_pairs_fn_t)(oxphp_req_pairs_cb_t cb, void *user_data);

/** Body accessor — returns pointer to raw body bytes. */
typedef const uint8_t* (*oxphp_req_body_fn_t)(size_t *out_len);

/** Register all request accessor callbacks (called once at startup from Rust). */
void oxphp_bridge_set_request_accessors(
    oxphp_req_str_fn_t      method_fn,
    oxphp_req_str_fn_t      path_fn,
    oxphp_req_str_fn_t      full_uri_fn,
    oxphp_req_str_fn_t      scheme_fn,
    oxphp_req_str_fn_t      host_fn,
    oxphp_req_u16_fn_t      port_fn,
    oxphp_req_str_fn_t      query_string_fn,
    oxphp_req_str_key_fn_t  header_fn,
    oxphp_req_str_key_fn_t  cookie_fn,
    oxphp_req_str_fn_t      ip_fn,
    oxphp_req_str_fn_t      protocol_version_fn,
    oxphp_req_double_fn_t   start_time_fn,
    oxphp_req_bool_fn_t     is_secure_fn,
    oxphp_req_str_fn_t      content_type_fn,
    oxphp_req_str_key_fn_t  query_param_fn,
    oxphp_req_pairs_fn_t    headers_all_fn,
    oxphp_req_pairs_fn_t    cookies_all_fn,
    oxphp_req_pairs_fn_t    query_params_all_fn,
    oxphp_req_body_fn_t     body_fn,
    oxphp_req_bool_fn_t     is_active_fn
);

/** Convenience getters — call through registered function pointers. */
const char* oxphp_req_method(size_t *out_len);
const char* oxphp_req_path(size_t *out_len);
const char* oxphp_req_full_uri(size_t *out_len);
const char* oxphp_req_scheme(size_t *out_len);
const char* oxphp_req_host(size_t *out_len);
uint16_t    oxphp_req_port(void);
const char* oxphp_req_query_string(size_t *out_len);
const char* oxphp_req_header(const char *name, size_t name_len, size_t *out_len);
const char* oxphp_req_cookie(const char *name, size_t name_len, size_t *out_len);
const char* oxphp_req_ip(size_t *out_len);
const char* oxphp_req_protocol_version(size_t *out_len);
double      oxphp_req_start_time(void);
int         oxphp_req_is_secure(void);
const char* oxphp_req_content_type(size_t *out_len);
const char* oxphp_req_query_param(const char *key, size_t key_len, size_t *out_len);
void        oxphp_req_headers_all(oxphp_req_pairs_cb_t cb, void *user_data);
void        oxphp_req_cookies_all(oxphp_req_pairs_cb_t cb, void *user_data);
void        oxphp_req_query_params_all(oxphp_req_pairs_cb_t cb, void *user_data);
const uint8_t* oxphp_req_body(size_t *out_len);
int         oxphp_req_is_active(void);

/* ─── Worker Mode ─────────────────────────────────────────── */

/** Rust callback: blocks until next request arrives, returns 0 on success, -1 on shutdown. */
typedef int (*oxphp_worker_wait_fn_t)(void);

/** Rust callback: sends current response back to HTTP layer, returns 0 on success. */
typedef int (*oxphp_worker_send_fn_t)(void);

/** Register Rust worker callbacks (called once at init). */
void oxphp_bridge_set_worker_callbacks(oxphp_worker_wait_fn_t wait_fn, oxphp_worker_send_fn_t send_fn);

/** Set worker mode TLS flags for this thread. */
void oxphp_bridge_set_worker_mode(uint64_t max_requests, uint64_t max_memory_mib);

/** Check if this thread is in worker mode. */
bool oxphp_bridge_is_worker_mode(void);

/**
 * Reset per-request TLS fields between worker mode requests.
 * Clears: request_id, request_time, deadline, cancelled, write_count,
 *         stream_mode, headers_sent, finished.
 * Increments: requests_done.
 */
void oxphp_bridge_reset_request_ctx(void);

/** Call Rust worker_wait callback. Returns 0 (request ready) or -1 (shutdown). */
int oxphp_bridge_worker_wait(void);

/** Call Rust worker_send callback. Returns 0 on success. */
int oxphp_bridge_worker_send_response(void);

/* ─── Fiber Scheduler Callbacks ────────────────────────── */

/** Rust callback: non-blocking receive. Returns 0=ready, 1=empty, -1=shutdown. */
typedef int (*oxphp_worker_try_recv_fn_t)(void);

/** Rust callback: set up TLS for a request received via try_recv. Returns 1=ok, 0=no pending. */
typedef int (*oxphp_prepare_request_fn_t)(void);

/** Register Rust fiber scheduler callbacks. */
void oxphp_bridge_set_fiber_callbacks(
    oxphp_worker_try_recv_fn_t try_recv_fn,
    oxphp_prepare_request_fn_t prepare_fn
);

/** Non-blocking receive: returns 0=ready, 1=empty, -1=shutdown. */
int oxphp_bridge_worker_try_recv(void);

/** Prepare TLS for pending request. Returns 1=ok, 0=nothing pending. */
int oxphp_bridge_prepare_request(void);

/** Set the cancellation flag (called from Rust when client disconnects). */
void oxphp_bridge_set_cancelled(bool cancelled);

/** Check if cancellation was requested. */
bool oxphp_bridge_is_cancelled(void);

/** Execute PHP script with zend_try protection. Returns 1 on success, 0 on bailout. */
int oxphp_execute_script_safe(void *file_handle);

/* ─── Worker Mode Metrics Getters ─────────────────────────── */

/** Get the exit reason for the last worker mode exit. */
uint8_t oxphp_bridge_get_exit_reason(void);

/** Get the number of requests completed by this worker. */
uint64_t oxphp_bridge_get_requests_done(void);

/** Get the current PHP memory usage (set after each request). */
uint64_t oxphp_bridge_get_memory_usage(void);

/** Check if the current handler invocation failed (fatal error/bailout). */
bool oxphp_bridge_get_handler_failed(void);

/* ─── Fiber TLS Context Callbacks ──────────────────────── */

/** Rust callback: save current fiber's TLS context. */
typedef void (*oxphp_fiber_save_ctx_fn_t)(uint64_t fiber_id);

/** Rust callback: restore a fiber's TLS context. */
typedef void (*oxphp_fiber_restore_ctx_fn_t)(uint64_t fiber_id);

/** Rust callback: drop a fiber's TLS slot (fiber completed/destroyed). */
typedef void (*oxphp_fiber_drop_ctx_fn_t)(uint64_t fiber_id);

/** Register Rust fiber TLS context callbacks (called once at init). */
void oxphp_bridge_set_fiber_ctx_callbacks(
    oxphp_fiber_save_ctx_fn_t save_fn,
    oxphp_fiber_restore_ctx_fn_t restore_fn,
    oxphp_fiber_drop_ctx_fn_t drop_fn
);

/** Save current fiber's Rust TLS context into per-fiber slot. */
void oxphp_bridge_fiber_save_ctx(uint64_t fiber_id);

/** Restore a fiber's Rust TLS context from per-fiber slot. */
void oxphp_bridge_fiber_restore_ctx(uint64_t fiber_id);

/** Drop a fiber's Rust TLS slot (cleanup on fiber destruction). */
void oxphp_bridge_fiber_drop_ctx(uint64_t fiber_id);

/* ─── Fiber Timer Service ──────────────────────────────── */
typedef uint64_t (*oxphp_timer_register_fn_t)(uint64_t duration_ms);
typedef uint32_t (*oxphp_timer_poll_fn_t)(uint64_t *out_ids, uint32_t max_count);
typedef void     (*oxphp_timer_remove_fn_t)(uint64_t timer_id);

void oxphp_bridge_set_timer_callbacks(oxphp_timer_register_fn_t, oxphp_timer_poll_fn_t, oxphp_timer_remove_fn_t);
uint64_t oxphp_bridge_timer_register(uint64_t duration_ms);
uint32_t oxphp_bridge_timer_poll(uint64_t *out_ids, uint32_t max_count);
void     oxphp_bridge_timer_remove(uint64_t timer_id);

/* === Async Promise Support === */

/* Async worker state (no PHP types — safe without php.h) */
void oxphp_async_reset(void);
void oxphp_bridge_set_async_worker(int is_async);
int oxphp_bridge_is_async_worker(void);

/* Capture last fatal error message (from zend_error_cb) for async exception propagation. */
void oxphp_bridge_capture_fatal(const char *msg, size_t len);
char *oxphp_bridge_pop_fatal(void);

/* ─── Async Dispatch Function Pointers ─────────────────────── */

/**
 * Function pointer types for async dispatch (C extension → Rust).
 * The extension calls these to dispatch closures and await results.
 */
typedef int64_t (*oxphp_async_dispatch_fn_t)(
    void *op_array, void *static_vars, void *this_ptr,
    uint32_t argc, void *args, void *closure_zval
);
typedef int (*oxphp_await_dispatch_fn_t)(
    int64_t promise_id, double timeout, void *retval
);
typedef int (*oxphp_await_any_dispatch_fn_t)(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
);

typedef int (*oxphp_fiber_await_fn_t)(int64_t promise_id, double timeout, void *retval);

/** Register Rust async dispatch callbacks (called once at init). */
void oxphp_bridge_set_async_dispatch(oxphp_async_dispatch_fn_t fn);
void oxphp_bridge_set_await_dispatch(oxphp_await_dispatch_fn_t fn);
void oxphp_bridge_set_await_any_dispatch(oxphp_await_any_dispatch_fn_t fn);
void oxphp_bridge_set_fiber_await(oxphp_fiber_await_fn_t fn);
int oxphp_bridge_fiber_await(int64_t promise_id, double timeout, void *retval);

/** Call Rust async dispatch. Returns promise_id (>= 0) or -1 on error. */
int64_t oxphp_bridge_async_dispatch(
    void *op_array, void *static_vars, void *this_ptr,
    uint32_t argc, void *args, void *closure_zval
);

/** Call Rust await dispatch. Returns 0 (success), -1 (error), -2 (timeout). */
int oxphp_bridge_await_dispatch(int64_t promise_id, double timeout, void *retval);

/** Call Rust await_any dispatch. Races multiple promises, returns the first to complete.
 *  On success: *out_winner_id is the winning promise ID, retval has the result.
 *  Returns 0 (success), -1 (error), -2 (timeout). */
int oxphp_bridge_await_any_dispatch(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
);

/* ─── Non-Blocking Await Poll ──────────────────────────────── */
typedef int (*oxphp_await_poll_fn_t)(int64_t promise_id);
void oxphp_bridge_set_await_poll(oxphp_await_poll_fn_t fn);
int  oxphp_bridge_await_poll(int64_t promise_id);

/* ─── Async Promise Cleanup ─────────────────────────────────── */
typedef void (*oxphp_cleanup_promises_fn_t)(void);
void oxphp_bridge_set_cleanup_promises(oxphp_cleanup_promises_fn_t fn);
void oxphp_bridge_cleanup_outstanding_promises(void);

/* ─── Async Exception Details ────────────────────────────── */
void oxphp_bridge_set_async_exception(const char *cls, const char *msg, const char *trace);
const char *oxphp_bridge_get_async_exc_class(void);
const char *oxphp_bridge_get_async_exc_message(void);
const char *oxphp_bridge_get_async_exc_trace(void);
void oxphp_bridge_clear_async_exception(void);

/* The remaining async functions use PHP types (zval, HashTable, zend_op_array)
 * and are only available when PHP headers have been included first.
 * Rust FFI uses *mut c_void for all these pointer types. */
#ifdef PHP_H

/* Freeze a zval in-place: arrays get IS_ARRAY_IMMUTABLE, strings get refcount flags cleared.
 * Saves original state into out params for later unfreeze.
 * Returns 0 on success, -1 if type cannot be frozen (IS_OBJECT, IS_RESOURCE). */
int oxphp_freeze_zval(zval *zv, uint32_t *out_orig_refcount, uint32_t *out_orig_gc_flags, uint32_t *out_orig_type_flags);

/* Unfreeze a zval, restoring original refcount and flags. */
void oxphp_unfreeze_zval(zval *zv, uint32_t orig_refcount, uint32_t orig_gc_flags, uint32_t orig_type_flags);

/* Deep-copy a zval using emalloc on the target thread. Result is thread-independent. */
void oxphp_deep_copy_zval(zval *dst, const zval *src);

/* Free a deep-copied zval. */
void oxphp_deep_free_zval(zval *zv);

/* === Portable (cross-thread) serialization ===
 * Serialize zvals into a flat system-malloc'd buffer that can safely cross
 * ZTS thread boundaries. The receiver calls deserialize on its own thread,
 * which allocates via emalloc on the correct per-thread heap. */

/* Serialize `argc` zvals into a portable buffer.
 * Returns 0 on success, -1 on failure.
 * On success, *out_buf and *out_len are set (caller owns, free with oxphp_portable_free). */
int oxphp_portable_serialize(const zval *args, uint32_t argc,
                             unsigned char **out_buf, size_t *out_len);

/* Serialize a HashTable (e.g. closure static_vars) into a portable buffer.
 * Returns 0 on success, -1 on failure. */
int oxphp_portable_serialize_ht(HashTable *ht,
                                unsigned char **out_buf, size_t *out_len);

/* Deserialize a portable buffer into `argc` zvals on the current thread's heap.
 * `out` must point to pre-allocated (zeroed) zval storage for argc zvals. */
int oxphp_portable_deserialize(const unsigned char *buf, size_t len,
                               uint32_t argc, zval *out);

/* Deserialize a portable buffer produced by oxphp_portable_serialize_ht
 * into a new HashTable on the current thread's heap.
 * Returns 0 on success, -1 on failure. Caller owns the returned HashTable. */
int oxphp_portable_deserialize_ht(const unsigned char *buf, size_t len,
                                  HashTable **out_ht);

/* Free a buffer returned by oxphp_portable_serialize / oxphp_portable_serialize_ht. */
void oxphp_portable_free(unsigned char *buf);

/* Free a HashTable returned by oxphp_portable_deserialize_ht. */
void oxphp_portable_free_ht(HashTable *ht);

/* Closure inspection */
void *oxphp_closure_get_op_array(zval *closure);
int oxphp_closure_get_static_vars(zval *closure, HashTable **out_ht);
int oxphp_closure_has_this(zval *closure);
zval *oxphp_closure_get_this(zval *closure);

/* Borrow proxy */
void oxphp_bridge_set_borrow_proxy_ce(zend_class_entry *ce);
void oxphp_create_borrow_proxy(zval *dst, uint64_t promise_id);

/* Check if a HashTable contains any IS_OBJECT or IS_RESOURCE values.
 * Returns 1 if found, 0 if clean. Dereferences IS_REFERENCE wrappers. */
int oxphp_ht_has_objects_or_resources(HashTable *ht);

/* Copy a zval into an array at a string key (ZVAL_COPY semantics). */
void oxphp_arr_add_zval(zval *arr, const char *key, zval *val);

/* Copy a zval into an array at an integer index (ZVAL_COPY semantics). */
void oxphp_arr_add_index_zval(zval *arr, zend_ulong idx, zval *val);

/* Execute an async task on an async worker thread.
 * Returns 0 on success, -1 on exception.
 * On exception: exc_class, exc_message, exc_trace are malloc'd strings (caller frees). */
int oxphp_execute_async_task(
    zend_op_array *op_array,
    HashTable *static_vars,
    zval *this_ptr,
    uint32_t argc,
    zval *args,
    zval *retval,
    char **exc_class,
    char **exc_message,
    char **exc_trace
);

/* ─── Custom Object for Plugin Classes ──────────────────────── */

/**
 * Custom object structure for plugin-defined classes with Rust storage.
 * The `std` field MUST be last — PHP uses container_of arithmetic to find it.
 *
 * Wrapped in OXPHP_CUSTOM_OBJECT_DEFINED so it can also be defined in
 * oxphp_bridge.c (which includes php.h after this header, meaning PHP_H is
 * not yet defined when this block is first seen).
 */
#ifndef OXPHP_CUSTOM_OBJECT_DEFINED
#define OXPHP_CUSTOM_OBJECT_DEFINED
typedef struct {
    void       *rust_data;      /**< Opaque pointer to Rust-allocated data. */
    uint32_t    class_index;    /**< Index into the class registry. */
    zend_object std;            /**< Standard zend_object — MUST be last. */
} oxphp_custom_object;

/**
 * Convert from zend_object* to oxphp_custom_object* using offsetof arithmetic.
 */
#define OXPHP_OBJ(zobj) \
    ((oxphp_custom_object *)((char *)(zobj) - XtOffsetOf(oxphp_custom_object, std)))
#endif /* OXPHP_CUSTOM_OBJECT_DEFINED */

/**
 * Allocate and initialize the custom object handler infrastructure.
 * Called from MINIT before registering plugin classes.
 * class_count: number of plugin classes (determines array sizes).
 */
void oxphp_plugin_init_custom_objects(int class_count);

/**
 * Store a class entry in the plugin class CE array.
 * Called during MINIT when registering each class.
 */
void oxphp_plugin_set_class_ce(int index, zend_class_entry *ce);

/**
 * Get the custom object handlers for a given class index.
 * Returns pointer to the zend_object_handlers for that class.
 */
zend_object_handlers *oxphp_plugin_get_handlers(int index);

/**
 * The create_object handler for plugin classes with custom storage.
 */
zend_object *oxphp_plugin_create_object(zend_class_entry *ce);

/**
 * The free_obj handler for plugin classes with custom storage.
 */
void oxphp_plugin_free_object(zend_object *obj);

/**
 * The clone_obj handler for plugin classes with custom storage.
 */
zend_object *oxphp_plugin_clone_object(zend_object *obj);

#endif /* PHP_H */

/* ═══════════════════════════════════════════════════════════
 *  APM Hook Infrastructure
 *  Replace internal PHP function handlers with wrappers that
 *  call Rust before/after callbacks for automatic tracing.
 * ═══════════════════════════════════════════════════════════ */

/** Maximum number of internal functions that can be hooked simultaneously. */
#define OXPHP_APM_MAX_HOOKS 128

/**
 * Callback types: Rust sets these, C calls them around the original handler.
 * class_name is "" for global functions.
 */
typedef void (*oxphp_apm_before_fn_t)(const char *class_name, const char *func_name,
                                       uint32_t argc, void *args);
typedef void (*oxphp_apm_after_fn_t)(const char *class_name, const char *func_name,
                                      uint32_t argc, void *args, void *return_value);

/** Set the before-hook callback (called by Rust during init). */
void oxphp_apm_set_before(oxphp_apm_before_fn_t fn);

/** Set the after-hook callback (called by Rust during init). */
void oxphp_apm_set_after(oxphp_apm_after_fn_t fn);

/**
 * Register a function for hooking. Called by Rust before PHP startup.
 * class_name may be NULL or "" for global functions.
 */
void oxphp_apm_register_hook(const char *class_name, const char *func_name);

/**
 * Approve registered hooks against loaded extensions. Called from MINIT.
 * Validates each pending hook target exists in CG tables.
 * Returns the number of approved hooks (available for installation).
 */
int oxphp_apm_approve_registered_hooks(void);

/**
 * Returns the number of approved hooks (global, available after MINIT).
 */
int oxphp_apm_hook_count_approved(void);

/**
 * Install approved hooks into this thread's function tables.
 * Called from RINIT. Idempotent — no-op after first call per thread.
 */
void oxphp_apm_install_on_thread(void);

/**
 * Restore all hooked functions to their original handlers on this thread.
 * Safe to call even if no hooks were installed.
 */
void oxphp_apm_unhook_all(void);

/** Get number of hooks currently installed on this thread (diagnostics). */
int oxphp_apm_hook_count_installed(void);

#ifdef __cplusplus
}
#endif

#endif /* OXPHP_BRIDGE_H */
