#ifndef OXPHP_BRIDGE_H
#define OXPHP_BRIDGE_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdatomic.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ─── Return Type Constants ─────────────────────────────────────
 * Bridge-level type tags for method/function return types.
 * These are OxPHP constants, NOT PHP's IS_* — the C SAPI maps them
 * to the correct Zend type codes at registration time.
 * 0 means "no return type declared". */
#define OXPHP_RT_NONE     0
#define OXPHP_RT_NULL     1
#define OXPHP_RT_BOOL     2
#define OXPHP_RT_INT      3
#define OXPHP_RT_FLOAT    4
#define OXPHP_RT_STRING   5
#define OXPHP_RT_ARRAY    6
#define OXPHP_RT_OBJECT   7
#define OXPHP_RT_MIXED    8
#define OXPHP_RT_VOID     9
#define OXPHP_RT_CALLABLE 10
#define OXPHP_RT_ITERABLE 11
#define OXPHP_RT_NEVER    12
#define OXPHP_RT_FALSE    13
#define OXPHP_RT_TRUE     14
#define OXPHP_RT_SELF     15
#define OXPHP_RT_STATIC   16
#define OXPHP_RT_PARENT   17

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

    _Atomic(uint8_t)* cancel_ptr;    /* into Arc<CancellationState>; NULL outside request */
    void* vm_interrupt_addr;         /* &EG(vm_interrupt); NULL until first php_request_startup */

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

    /** Max memory in bytes before worker recycle (0 = unlimited).
     *  Pre-computed from MB to avoid per-request multiplication. */
    uint64_t max_memory_bytes;

    /** Exit reason for worker mode (0=shutdown, 1=scheduled, 2=max_memory, 3=error). */
    uint8_t exit_reason;

    /** Whether Worker::scheduleExit() has been called for this worker.
     *  Once true, the worker loop exits after the current request completes. */
    bool exit_scheduled;

    /** Whether the current handler invocation failed (bailout/fatal error). */
    bool handler_failed;

    /** Consecutive handler errors (bailout). Resets on success, worker exits at threshold. */
    uint32_t consecutive_errors;

    /** Current PHP heap usage in bytes (updated after each request). */
    uint64_t current_memory_bytes;

    /** Whether this thread is an async worker (not a request worker). */
    int is_async_worker;

    /** OS thread spawn time as unix seconds (float).
     *  Set ONCE per thread by Rust at thread boot via
     *  oxphp_bridge_set_worker_start_time(); preserved across all
     *  per-request resets (oxphp_bridge_reset_request_ctx). Zero before
     *  the setter is called (e.g. CLI without OxPHP host). */
    double worker_start_time;
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

/** Set worker thread spawn time. Called once per thread at boot. */
void oxphp_bridge_set_worker_start_time(double time);

/** Get worker thread spawn time. */
double oxphp_bridge_get_worker_start_time(void);

/** Set streaming mode. */
void oxphp_bridge_set_stream_mode(bool mode);

/** Check if streaming mode is active. */
bool oxphp_bridge_is_streaming(void);

/** Mark request as finished (oxphp_finish_request). */
void oxphp_bridge_set_finished(bool finished);

/** Check if request is finished. */
bool oxphp_bridge_is_finished(void);

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

/** Add a method to a class. return_type: OXPHP_RT_* constant (0 = no type info).
 *  param_names/types/optional are parallel arrays of length total_params. They may be
 *  NULL when total_params == 0. Names are strdup'd by the bridge. */
void oxphp_bridge_class_add_method(int class_handle, const char *name,
    uint32_t visibility, uint32_t flags, int required_params, int total_params, int is_variadic,
    int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional);

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

/** Get method return type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_class_method_return_type(int class_index, int method_index);

/** Get method return nullable flag. */
int oxphp_bridge_get_class_method_return_nullable(int class_index, int method_index);

/** Get class method parameter name (NULL if none). */
const char *oxphp_bridge_get_class_method_param_name(int class_index, int method_index, int param_index);

/** Get class method parameter type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_class_method_param_type(int class_index, int method_index, int param_index);

/** Get class method parameter optional flag (1 = optional). */
int oxphp_bridge_get_class_method_param_optional(int class_index, int method_index, int param_index);

/** Get magic method handler flag. magic_type is MagicMethod enum ordinal. */
int oxphp_bridge_get_class_magic(int class_index, int magic_type);

/* ─── Plugin Interface Registry (global, NOT __thread) ──────── */

/** Register an interface. Returns handle (index). parent_fqn may be NULL. */
int oxphp_bridge_register_interface(const char *fqn, const char *parent_fqn);

/** Add a method to an interface. return_type: OXPHP_RT_* constant (0 = no type info).
 *  param_names/types/optional are parallel arrays of length total_params. May be NULL
 *  when total_params == 0. Names are strdup'd by the bridge. */
void oxphp_bridge_interface_add_method(int iface_handle, const char *name,
    uint32_t flags, int required_params, int total_params, int is_variadic,
    int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional);

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

/** Get interface method return type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_interface_method_return_type(int iface_index, int method_index);

/** Get interface method return nullable flag. */
int oxphp_bridge_get_interface_method_return_nullable(int iface_index, int method_index);

/** Get interface method parameter name (NULL if none). */
const char *oxphp_bridge_get_interface_method_param_name(int iface_index, int method_index, int param_index);

/** Get interface method parameter type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_interface_method_param_type(int iface_index, int method_index, int param_index);

/** Get interface method parameter optional flag (1 = optional). */
int oxphp_bridge_get_interface_method_param_optional(int iface_index, int method_index, int param_index);

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

/** Add a method to an enum. return_type: OXPHP_RT_* constant (0 = no type info).
 *  param_names/types/optional are parallel arrays of length total_params. May be NULL
 *  when total_params == 0. Names are strdup'd by the bridge. */
void oxphp_bridge_enum_add_method(int enum_handle, const char *name,
    uint32_t flags, int required_params, int total_params, int is_variadic,
    int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional);

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

/** Get enum method return type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_enum_method_return_type(int enum_index, int method_index);

/** Get enum method return nullable flag. */
int oxphp_bridge_get_enum_method_return_nullable(int enum_index, int method_index);

/** Get enum method parameter name (NULL if none). */
const char *oxphp_bridge_get_enum_method_param_name(int enum_index, int method_index, int param_index);

/** Get enum method parameter type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_enum_method_param_type(int enum_index, int method_index, int param_index);

/** Get enum method parameter optional flag (1 = optional). */
int oxphp_bridge_get_enum_method_param_optional(int enum_index, int method_index, int param_index);

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

/** Register a plugin function via builder. Returns handle (index).
 *  param_names/types/optional are parallel arrays of length total_params. May be NULL
 *  when total_params == 0. Names are strdup'd by the bridge. */
int oxphp_bridge_register_plugin_function(const char *fqn, int required_params,
    int total_params, int is_variadic, int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional);

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

/** Get builder-based function return type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_plugin_function_return_type(int index);

/** Get builder-based function return nullable flag. */
int oxphp_bridge_get_plugin_function_return_nullable(int index);

/** Get builder-based function parameter name (NULL if none). */
const char *oxphp_bridge_get_plugin_function_param_name(int index, int param_index);

/** Get builder-based function parameter type (OXPHP_RT_* constant, 0 = no type). */
int oxphp_bridge_get_plugin_function_param_type(int index, int param_index);

/** Get builder-based function parameter optional flag (1 = optional). */
int oxphp_bridge_get_plugin_function_param_optional(int index, int param_index);

/* ─── Method Dispatch Callback ──────────────────────────────── */

/** Callback type for dispatching class method calls to Rust.
 * `this_zval` is the zval* of `$this` for instance methods, NULL for
 * static method calls and free functions. The pointer is valid only
 * for the duration of the dispatch call; do not store it. */
typedef int (*oxphp_method_dispatch_fn_t)(
    uint32_t class_index, const char *method_name,
    void *args, uint32_t argc, void *retval, void *rust_data,
    void *this_zval
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

/* ─── Object Property Access ───────────────────────────────── */

/* Read a private or protected property from a zend object zval, where the
 * property has NO __get magic, NO property hooks, and is NOT readonly.
 * Returns a pointer to the zval stored in the object's property table, or
 * NULL if the input zval is not an object, or &EG(uninitialized_zval) if
 * the property is unset.
 *
 * The returned pointer is valid for the duration of the current request.
 *
 * UNSAFE for properties with __get / hooks / asymmetric-readonly: those code
 * paths in zend_std_read_property write into a caller-provided rv buffer,
 * and this helper does not satisfy that contract. property_name is
 * NUL-terminated. */
void *oxphp_object_read_property(void *object_zval, const char *property_name);

/** Returns 1 if the zval pointer is NULL, IS_UNDEF, or IS_NULL; 0 otherwise. */
int oxphp_zval_is_null_or_unset(const void *zval_ptr);

/** Copy zval contents from `src` into `dst` (typically the retval slot).
 *  Increments refcounts as needed (uses ZVAL_COPY semantics). `dst` must
 *  point to an uninitialized or destroyed zval slot. */
void oxphp_zval_copy_to_retval(const void *src_zval, void *dst_zval);

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
/* Read a backed-int enum's value via zend_enum_fetch_case_value. Returns 0
 * if the arg is not a backed-int enum. */
int64_t oxphp_arg_enum_long(void *args, uint32_t idx);

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

/** Return sizeof(zend_op_array) for the running PHP build. */
size_t oxphp_op_array_size(void);

/** Trigger zend_bailout() — safely abort PHP execution from SAPI callbacks. */
void oxphp_bridge_bailout(void);

/* ── SAPI callback wrappers with cooperative cancellation check ── */

/**
 * Register the Rust-side ub_write and flush implementations.
 * The bridge provides wrapper functions that check the cancellation flag
 * BEFORE calling through to Rust, and call zend_bailout() from C if set.
 * This avoids longjmp crossing Rust FFI boundaries.
 */
typedef size_t (*oxphp_ub_write_fn_t)(const char *str, size_t str_length);
typedef void   (*oxphp_flush_fn_t)(void *server_context);

void oxphp_bridge_set_sapi_callbacks(oxphp_ub_write_fn_t ub_write, oxphp_flush_fn_t flush);

/** C wrapper for ub_write — checks cancellation, then calls Rust impl. */
size_t oxphp_bridge_ub_write(const char *str, size_t str_length);

/** C wrapper for flush — checks cancellation, then calls Rust impl. */
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
void oxphp_bridge_set_worker_mode(uint64_t max_memory_mib);

/** Check if this thread is in worker mode. */
bool oxphp_bridge_is_worker_mode(void);

/**
 * Reset per-request TLS fields between worker mode requests.
 * Clears: request_id, request_time, cancel_ptr,
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

/* ── Cancellation reason (sub-design A) ──
 *
 * Pointer-based replacement for the legacy bool cancelled flag.
 * Pointer references a Rust-owned Arc<CancellationState> whose
 * lifetime exceeds the request.
 */
typedef enum {
    OXPHP_CANCEL_NONE         = 0,
    OXPHP_CANCEL_CLIENT_ABORT = 1,
    OXPHP_CANCEL_TIMEOUT      = 2,
    OXPHP_CANCEL_SHUTDOWN     = 3,
    OXPHP_CANCEL_STUCK        = 4,
    OXPHP_CANCEL_USER         = 5,
} oxphp_cancel_reason_t;

void oxphp_bridge_set_cancel_ptr(_Atomic(uint8_t)* ptr);
oxphp_cancel_reason_t oxphp_bridge_get_cancel_reason(void);
bool oxphp_bridge_set_cancel_reason(oxphp_cancel_reason_t reason);

/* Returns &EG(vm_interrupt) for this worker; captured after the
 * first php_request_startup. */
void* oxphp_bridge_vm_interrupt_addr(void);

/* Set by the SAPI module right after capturing &EG(vm_interrupt). */
void oxphp_bridge_set_vm_interrupt_addr(void* addr);

/* Sub-design A: capture &EG(vm_interrupt) from the current thread's
 * Zend executor globals and store it in the bridge ctx. Must be called
 * from a thread where TSRM is initialised (i.e. after ts_resource_ex). */
void oxphp_capture_vm_interrupt(void);

/* In-thread helper: request the next opcode boundary to call our
 * registered zend_interrupt_function. Used from the worker thread
 * itself (e.g. streaming send-error). */
void oxphp_bridge_request_interrupt(void);

/* Cross-thread helper: same as oxphp_bridge_request_interrupt() but takes
 * the target thread's &EG(vm_interrupt) as a parameter. Routes the write
 * through zend_atomic_bool_store_ex so we don't C11-strict-aliasing the
 * underlying _Atomic(bool) via uint8_t*. Caller is responsible for keeping
 * the address valid (the target worker thread must still be alive). */
void oxphp_bridge_request_interrupt_at(void* addr);

/* ── Tick observer ──
 *
 * Per-worker counter incremented once per PHP function call by a
 * registered zend_observer_fcall_register callback. The supervisor
 * uses tick deltas (combined with thread CPU-time deltas) to
 * classify long-running workers: cpu_delta>0 + tick_delta==0 means
 * the worker is stuck inside a C extension, etc.
 *
 * set_tick_ptr() is called once per worker on the first request
 * (zero-once gate). oxphp_bridge_tick() is the inline fast path
 * invoked by the observer; it bumps the per-thread pointer using a
 * relaxed atomic add. Cost: ~3 ns per call. */
extern _Thread_local _Atomic(uint64_t)* g_tick_ptr;

void oxphp_bridge_set_tick_ptr(_Atomic(uint64_t)* ptr);

static inline void oxphp_bridge_tick(void) {
    _Atomic(uint64_t)* p = g_tick_ptr;
    if (p) {
        atomic_fetch_add_explicit(p, 1, memory_order_relaxed);
    }
}


/** Execute PHP script with zend_try protection. Returns 1 on success, 0 on bailout. */
int oxphp_execute_script_safe(void *file_handle);

/* ─── Worker Mode Metrics Getters ─────────────────────────── */

/** Set the exit flag with reason 'scheduled' (1). Idempotent.
 *  Called from PHP via Worker::scheduleExit(). */
void oxphp_bridge_schedule_exit(void);

/** True if Worker::scheduleExit() has been called for the current worker. */
bool oxphp_bridge_is_exit_scheduled(void);

/** Get the exit reason for the last worker mode exit
 *  (0=none, 1=scheduled, 2=max_memory, 3=error). */
uint8_t oxphp_bridge_get_exit_reason(void);

/** Get the number of requests completed by this worker. */
uint64_t oxphp_bridge_get_requests_done(void);

/** Increment requests_done by 1 and return the new value. Called from Rust
 *  at the start of each request handling — both traditional mode
 *  (per-request in execute_request) and worker mode (per-handler-dispatch
 *  in scheduler). Thread-local; survives per-request resets. */
uint64_t oxphp_bridge_increment_requests_done(void);

/** Get the current PHP memory usage (set after each request). */
uint64_t oxphp_bridge_get_memory_usage(void);

/** Process resident set size (RSS) in bytes.
 *  Linux: parses /proc/self/status VmRSS line (KiB → bytes).
 *  macOS / other: getrusage(RUSAGE_SELF, &ru).ru_maxrss (bytes on Darwin).
 *  Returns 0 on failure (rare; restrictive sandbox kernels). */
uint64_t oxphp_bridge_get_rss_bytes(void);

/** Configured per-worker memory cap in bytes (0 = unlimited). */
uint64_t oxphp_bridge_get_max_memory_bytes(void);

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
typedef int (*oxphp_await_race_dispatch_fn_t)(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
);
/* await_any dispatch (Promise.any-style: first FULFILLED wins).
 *
 * Same signature as await_race_dispatch_fn_t. The Rust implementation
 * accumulates rejections via the aggregate-exception API and only
 * succeeds on first fulfilled promise.
 *
 * Return codes:
 *   0  : success — *out_winner_id and retval populated.
 *   -2 : timeout — TimeoutException already thrown via aggregate API.
 *   -3 : all rejected — AggregateAsyncException already thrown via aggregate API.
 *   -4 : unknown / already-awaited promise id — *out_winner_id holds the bad id.
 *   -1 : other internal error.
 */
typedef int (*oxphp_await_any_dispatch_fn_t)(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
);

typedef int (*oxphp_fiber_await_fn_t)(int64_t promise_id, double timeout, void *retval);
typedef int (*oxphp_in_fiber_check_fn_t)(void);

/** Register Rust async dispatch callbacks (called once at init). */
void oxphp_bridge_set_async_dispatch(oxphp_async_dispatch_fn_t fn);
void oxphp_bridge_set_await_dispatch(oxphp_await_dispatch_fn_t fn);
void oxphp_bridge_set_await_race_dispatch(oxphp_await_race_dispatch_fn_t fn);
void oxphp_bridge_set_await_any_dispatch(oxphp_await_any_dispatch_fn_t fn);
void oxphp_bridge_set_fiber_await(oxphp_fiber_await_fn_t fn);
int oxphp_bridge_fiber_await(int64_t promise_id, double timeout, void *retval);

/** Register the SAPI predicate that decides whether the calling thread
 *  is inside an oxphp-managed scheduler fiber. The bridge has no way
 *  to tell on its own — `EG(current_fiber_context)` is non-null on the
 *  main thread of every request, and a user-level `Fiber::start()`
 *  installs a context that the oxphp scheduler does not own. The SAPI
 *  keys the predicate off its private `oxphp_current_fiber` __thread
 *  pointer, which is the only authoritative source. */
void oxphp_bridge_set_in_fiber_check(oxphp_in_fiber_check_fn_t fn);

/* Returns 1 if the calling thread is currently inside an oxphp
 * scheduler fiber (the only context where `oxphp_bridge_fiber_await`
 * can suspend), else 0. Cheap (single function pointer call). Used by
 * Shared\Channel (and other primitives) to choose between
 * synthetic-promise fiber-suspend and crossbeam thread-block when
 * timeout > 0. Returns 0 when no SAPI callback is registered (unit
 * tests, bare CLI without the extension). */
int oxphp_bridge_in_fiber(void);

/** Call Rust async dispatch. Returns promise_id (>= 0) or -1 on error. */
int64_t oxphp_bridge_async_dispatch(
    void *op_array, void *static_vars, void *this_ptr,
    uint32_t argc, void *args, void *closure_zval
);

/** Call Rust await dispatch. Returns 0 (success), -1 (error), -2 (timeout). */
int oxphp_bridge_await_dispatch(int64_t promise_id, double timeout, void *retval);

/** Call Rust await_race dispatch. Races multiple promises, returns the first to complete.
 *  On success: *out_winner_id is the winning promise ID, retval has the result.
 *  Returns 0 (success), -1 (error), -2 (timeout). */
int oxphp_bridge_await_race_dispatch(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
);

/** Call Rust await_any dispatch. First FULFILLED promise wins; rejections accumulate.
 *  On success: *out_winner_id is the winning promise ID, retval has the result.
 *  Returns 0 (success), -1 (error), -2 (timeout), -3 (all rejected). */
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
void oxphp_bridge_set_async_exception(const char *cls, const char *msg);
const char *oxphp_bridge_get_async_exc_class(void);
const char *oxphp_bridge_get_async_exc_message(void);
void oxphp_bridge_clear_async_exception(void);

/* ─── Async Aggregate Exception (multi-error) ────────────────────
 *
 * Used by oxphp_bridge_await_any_dispatch when accumulating rejections
 * from multiple promises. Buffer is __thread-local; one buffer per
 * worker thread. clear() must be called before push() sequence; throw()
 * synthesises a PHP exception from the accumulated entries and clears
 * the buffer.
 */
void oxphp_bridge_aggregate_clear(void);

void oxphp_bridge_aggregate_push(
    const char *exception_class,   /* PHP class name, NUL-terminated UTF-8; nullable → "OxPHP\\Async\\AsyncException" */
    const char *message,           /* nullable → empty */
    int64_t promise_id
);

/* Throws OxPHP\Async\AggregateAsyncException with accumulated entries.
 * Returns 0 on success, -1 if the AggregateAsyncException class can't be
 * looked up. Always clears the buffer (even on failure). */
int oxphp_bridge_aggregate_throw(void);

/* Throws OxPHP\Async\TimeoutException with partial errors (from the buffer)
 * + pending ids (from the parameters). pending_ids is an array of
 * pending_count int64 values. Always clears the buffer.
 * Returns 0 on success, -1 on class lookup failure. */
int oxphp_bridge_aggregate_throw_timeout(
    const int64_t *pending_ids,
    uint32_t pending_count
);

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

/* Iterate a PHP array (passed as zval*) and portbuf-serialize each value
 * independently. Useful for batch channel send: each array element becomes
 * one channel payload.
 *
 * Returns 0 on success, -1 on internal failure, -3 if the zval is not an
 * array. On success:
 *   *out_concat      — libc::malloc'd buffer with all per-element portbufs
 *                      concatenated (NULL when total length == 0).
 *   *out_concat_len  — total byte length.
 *   *out_offsets     — libc::malloc'd [size_t; n+1] array of payload
 *                      boundaries (NULL when n == 0).
 *   *out_n           — number of elements (== number of payloads).
 *
 * Caller frees both outputs via oxphp_portable_free (forwards to libc::free).
 * String keys are ignored — the array is treated as a values-only sequence,
 * mirroring PHP's `array_values()` semantics for batch operations. */
int oxphp_iter_array_to_portbufs(const zval *arr,
                                  unsigned char **out_concat,
                                  size_t *out_concat_len,
                                  size_t **out_offsets,
                                  size_t *out_n);

/* Deserialize a portbuf and append the resulting zval to `arr` via
 * zend_hash_next_index_insert (indexed push). Returns 0 on success,
 * -1 on deserialize failure. `arr` must already be a zval of type
 * IS_ARRAY (caller initialises via array_init_size / oxphp_ret_array_init). */
int oxphp_arr_push_portbuf(zval *arr, const unsigned char *buf, size_t len);

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

/* Check if a HashTable contains any IS_RESOURCE or non-Shareable IS_OBJECT values.
 * Shareable objects (implementing OxPHP\Shared\Shareable) are allowed through.
 * Recurses into nested arrays. Returns 1 if a non-shareable value is found, 0 if clean.
 * Dereferences IS_REFERENCE wrappers. */
int oxphp_ht_has_non_shareable_objects(HashTable *ht);

/* Copy a zval into an array at a string key (ZVAL_COPY semantics). */
void oxphp_arr_add_zval(zval *arr, const char *key, zval *val);

/* Copy a zval into an array at an integer index (ZVAL_COPY semantics). */
void oxphp_arr_add_index_zval(zval *arr, zend_ulong idx, zval *val);

/* Execute an async task on an async worker thread.
 * Returns 0 on success, -1 on exception.
 * On exception: exc_class, exc_message are malloc'd strings (caller frees). */
int oxphp_execute_async_task(
    zend_op_array *op_array,
    HashTable *static_vars,
    zval *this_ptr,
    uint32_t argc,
    zval *args,
    zval *retval,
    char **exc_class,
    char **exc_message
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

/* ─── Profiler observer ──────────────────────────
 * Per-thread observer state used by the ox_profiler plugin. The C
 * Observer callbacks live in this library; Rust drives them via the
 * four entry points below and consumes batched events through
 * oxphp_profiler_flush_span_events (defined in Rust, called from C). */

/* Mode byte. Mirrors src/profiling/mod.rs::ProfilingMode. */
#define OXPHP_PROFILING_MODE_OFF         0
#define OXPHP_PROFILING_MODE_APM_ONLY    1
#define OXPHP_PROFILING_MODE_PROFILE_ALL 2

/* Event kind tag inside the ring buffer. */
#define OXPHP_SPAN_EVENT_KIND_BEGIN 1
#define OXPHP_SPAN_EVENT_KIND_END   2

/* Ring-buffer event. Sized to fit one cache line (64 bytes). Field
 * order is load-bearing — Rust mirrors it via #[repr(C)] in
 * src/profiling/flush.rs. The two reserved fields keep the struct
 * a clean 64-byte multiple and leave room for future extensions
 * (span_id back-reference, allocator tag, etc.). */
typedef struct ox_span_event_s {
    uint8_t  kind;             /* OXPHP_SPAN_EVENT_KIND_*               */
    uint8_t  reserved0;        /* keep zero — alignment / future flags  */
    uint16_t name_len;         /* bytes in name_ptr (0 if anonymous)    */
    uint32_t reserved1;        /* keep zero — alignment / future flags  */
    uint64_t seq;              /* monotonic per-thread BEGIN counter    */
    uint64_t ts_ns;            /* CLOCK_MONOTONIC_RAW                   */
    uint64_t cpu_ns;           /* CLOCK_THREAD_CPUTIME_ID               */
    int64_t  mem;              /* zend_memory_usage(0)                  */
    int64_t  mem_peak;         /* zend_memory_peak_usage(0)             */
    const char *name_ptr;      /* points into g_prof.name_arena         */
    uint64_t reserved2;        /* keep zero — pads to 64 bytes          */
} ox_span_event_t;

/* Set the per-thread profiling mode. Called from Rust at RINIT,
 * before php_request_startup(). When mode != PROFILE_ALL, the
 * observer begin/end callbacks early-return; when mode ==
 * PROFILE_ALL, they record events into the TLS ring buffer.
 * Setting OFF also resets the ring buffer / open-stack state. */
void oxphp_bridge_set_profiling_mode(uint8_t mode);

/* Read the current per-thread mode. Used by tests and by the
 * observer init callback to decide whether to attach handlers. */
uint8_t oxphp_bridge_get_profiling_mode(void);

/* Snapshot the per-thread open-span stack (BEGIN events without a
 * matching END yet). The heap hook reads this to attribute
 * allocations to the current span path.
 *
 * Writes up to `max_depth` u32 seq tags into `dst`, root → current.
 * Returns the actual depth, OR 255 if the real depth overflows the
 * 32-entry mirror (caller should set its truncated flag). */
uint8_t oxphp_bridge_snapshot_open_stack(uint32_t *dst, uint8_t max_depth);

/* Drain any partial ring-buffer contents into Rust. Called from
 * Rust at RSHUTDOWN, before PROFILING_CONTEXT::finalize. Idempotent
 * (a second call when the buffer is empty is a no-op). */
void oxphp_bridge_profiler_rshutdown_flush(void);

/* Set the per-thread "paused" flag for the profiler observer.
 * When 1, the begin callback early-returns
 * without pushing a span; the end callback still pops the
 * open_stack so already-open spans close naturally. Default 0
 * (not paused). Cleared on set_profiling_mode(OFF). */
void oxphp_bridge_set_profiling_paused(uint8_t paused);

/* Read the per-thread paused flag. */
uint8_t oxphp_bridge_is_profiling_paused(void);

/* Read zend_memory_usage(0) — current allocated bytes for this
 * thread's request (used by MemoryThresholdDecorator).
 * Returns 0 if not inside a PHP request. */
int64_t oxphp_bridge_get_memory_usage_bytes(void);

/* ─── Profiler observer filters ─────────────────
 *
 * Per-zend_function attribute resolution for the four filter
 * attributes #[Profile], #[Exclude], #[Sample(rate)],
 * #[Tag(key, value)]. Rust registers a resolver callback at plugin
 * init; observer init calls it once per function the first time the
 * function is observed; the resulting spec_id + decision quad is
 * cached per-thread for hot-path lookup in begin/end.
 *
 * spec_id = 0 means "no filter, default behaviour" — the fast path
 * for functions without any profiler attribute. */

/* Resolver callback type. Implemented in src/profiling/filter.rs.
 * Receives the function id, its class-scope and own attribute name
 * lists, and an opaque ctx pointer used by the arg-reader helpers
 * below. Populates the four out_* params with the decision values
 * the C hot path needs without re-entering Rust. Returns spec_id
 * (0 if no filter applies after composition). */
typedef uint32_t (*oxphp_profiler_resolve_filter_fn_t)(
    uintptr_t fn_id,
    const char *const *class_attr_names,
    uint32_t class_attr_count,
    const char *const *fn_attr_names,
    uint32_t fn_attr_count,
    void *attr_resolver_ctx,
    uint8_t *out_excluded,
    uint8_t *out_force_profile,
    uint8_t *out_has_sample,
    float   *out_sample_rate);

/* Register the resolver. Called by Rust at plugin init, BEFORE
 * the first observer begin fires. Setting NULL clears the resolver
 * (default — observer init won't try to resolve filters). */
void oxphp_bridge_set_filter_resolver(oxphp_profiler_resolve_filter_fn_t resolver);

/* Helper exposed back to Rust during a resolver call. Reads the
 * `attr_idx`-th occurrence of attribute `attr_name` and returns its
 * `arg_idx`-th constructor arg as a UTF-8 string. NUL-terminates;
 * returns the bytes written (capped at out_cap-1). Returns 0 when
 * the attribute / arg / type doesn't match. `is_class_scope`
 * selects between function and class attributes on the resolver
 * context. */
size_t oxphp_bridge_read_attr_arg_str(
    void *attr_resolver_ctx,
    int is_class_scope,
    const char *attr_name,
    uint32_t attr_idx,
    uint32_t arg_idx,
    char *out, size_t out_cap);

/* Same shape, returns a double via `*out`. Returns 1 on success,
 * 0 on absence / type mismatch. Integer args are widened to double. */
int oxphp_bridge_read_attr_arg_double(
    void *attr_resolver_ctx,
    int is_class_scope,
    const char *attr_name,
    uint32_t attr_idx,
    uint32_t arg_idx,
    double *out);

/* Diagnostic: read the cached spec_id for a function. Returns 255
 * if not yet cached. Used by tests. */
uint32_t oxphp_bridge_get_filter_spec_id_cached(uintptr_t fn_id);

/* Diagnostic: clear the per-thread filter cache. Used between
 * integration tests when the same Zend function should be re-resolved. */
void oxphp_bridge_clear_filter_cache(void);

/* Set the per-request span cap. Process-wide, set once at plugin
 * init from ProfilerConfig.max_spans. 0 means "unlimited". Reached
 * caps sets open_stack_overflow so Rust can flag the resulting
 * SpanTree as truncated. */
void oxphp_bridge_set_profiler_max_spans(uint32_t cap);

/* ─── Shareable interface ───────────────────
 *
 * OxPHP\Shared\Shareable is an internal-only PHP interface implemented
 * by every Shared\* wrapper type. Registered at MINIT; retained as a
 * process-global class_entry pointer (internal classes survive ZTS
 * per-thread cloning — this pointer is valid in every worker thread).
 */
#ifdef PHP_H
extern zend_class_entry *oxphp_shareable_ce;
#endif

/* Register `OxPHP\\Shared\\Shareable` as an internal interface.
 * Called once from PHP_MINIT_FUNCTION. Returns SUCCESS or FAILURE. */
int oxphp_shareable_register_ce(void);

/* Clear the cached ce pointer. Called from PHP_MSHUTDOWN_FUNCTION. */
int oxphp_shareable_unregister_ce(void);

/* Returns 1 iff z is a zval with an object whose class implements
 * Shareable. Returns 0 for non-objects, non-implementers, or if
 * oxphp_shareable_ce is NULL (not yet registered). Safe to call from
 * any thread after MINIT. Takes an opaque pointer (zval*) so callers
 * that do not include php.h (Rust FFI) can still invoke it. */
int oxphp_is_shareable(void *z);

/* ─── Synthetic promise callbacks ───────────
 *
 * Cross-thread promise plumbing that lets Shared primitives (Channel,
 * Mutex-with-timeout) park a fiber on an arbitrary Rust waker while
 * reusing `oxphp_bridge_fiber_await`. Rust registers four shims at
 * AsyncPlugin::init (main thread); C-side forwarders call through the
 * stored pointers.
 *
 * Contract:
 *   - alloc()      -> int64_t  synthetic promise id (always negative;
 *                               async-pool ids are >= 0, so the two id
 *                               spaces cannot collide).
 *   - resolve(id, payload_bytes, payload_len) -> 1 delivered, 0 noop.
 *   - reject(id, cls_fqn, message)            -> 1 delivered, 0 noop.
 *   - cancel(id)                              -> 1 delivered, 0 noop.
 *
 * Payload bytes are the portable-serialised zval representation of the
 * resolved value (empty buffer = void). See ext/bridge/oxphp_bridge.c
 * `oxphp_portable_serialize/deserialize` for the format.
 */
typedef int64_t (*oxphp_async_synth_alloc_fn_t)(void);
typedef int (*oxphp_async_synth_resolve_fn_t)(int64_t id,
                                               const uint8_t *payload_bytes,
                                               size_t payload_len);
typedef int (*oxphp_async_synth_reject_fn_t)(int64_t id,
                                              const char *cls_fqn,
                                              const char *message);
typedef int (*oxphp_async_synth_cancel_fn_t)(int64_t id);

void oxphp_bridge_set_async_synth_alloc(oxphp_async_synth_alloc_fn_t fn);
void oxphp_bridge_set_async_synth_resolve(oxphp_async_synth_resolve_fn_t fn);
void oxphp_bridge_set_async_synth_reject(oxphp_async_synth_reject_fn_t fn);
void oxphp_bridge_set_async_synth_cancel(oxphp_async_synth_cancel_fn_t fn);

int64_t oxphp_async_synthetic_promise_alloc(void);
int     oxphp_async_synthetic_promise_resolve(int64_t id,
                                               const uint8_t *payload_bytes,
                                               size_t payload_len);
int     oxphp_async_synthetic_promise_reject(int64_t id,
                                              const char *cls_fqn,
                                              const char *message);
int     oxphp_async_synthetic_promise_cancel(int64_t id);

/* ─── Shared wrapper cross-thread helpers ────────────────────
 *
 * Bridge between a PHP-side Shared\* wrapper object and the Rust-side
 * SharedRegistry. Used by the portable serializer's tag-7 path to
 * transfer Shared\* instances across worker threads in oxphp_async.
 *
 * oxphp_plugin_get_shared_handle: reads the SharedHandle
 *   { *const Entry entry_ptr, u8 type_tag } stored in a Shared\*
 *   wrapper's intern storage. Returns 0 and fills the out-params on
 *   success, -1 for a non-object / non-shareable / uninitialised
 *   wrapper.
 *
 * oxphp_shared_wrapper_new: constructs a fresh Shared\* wrapper bound
 *   to an existing registry entry. On success it MOVES the caller's
 *   strong-ref `entry_ptr` into the new wrapper's handle storage; the
 *   caller must NOT drop it afterwards. On failure (-1) the caller
 *   still owns `entry_ptr` and must release it via
 *   oxphp_shared_handle_drop.
 *
 * CONTRACT — oxphp_shared_handle_drop must be called EXACTLY ONCE per
 * strong reference. Calling it twice on the same pointer, or on a
 * pointer already moved into a wrapper, is undefined behaviour.
 *
 * Both helpers take `zval *`, so their declarations live inside the
 * `#ifdef PHP_H` block; callers are C units that include php.h.
 */
#ifdef PHP_H
int oxphp_plugin_get_shared_handle(zval *obj,
                                   uint8_t *out_type_tag,
                                   const void **out_entry_ptr);
int oxphp_shared_wrapper_new(zval *out,
                             uint8_t type_tag,
                             const void *entry_ptr);

/* ─── Synchronous closure-invoke shims ─────────────────
 * See oxphp_bridge.c for full contract.
 */
#ifndef OXPHP_SHARED_INVOKE_OK
#define OXPHP_SHARED_INVOKE_OK           0
#define OXPHP_SHARED_INVOKE_PHP_THREW    1
#define OXPHP_SHARED_INVOKE_BAD_CALLABLE -1
#endif

int oxphp_shared_invoke_0_portbuf(zval *callable,
                                  uint8_t **out_ret_buf,
                                  size_t *out_ret_len);

int oxphp_shared_invoke_byref_1_portbuf(zval *callable,
                                         const uint8_t *state_buf,
                                         size_t state_len,
                                         uint8_t **new_state_buf,
                                         size_t *new_state_len,
                                         uint8_t **out_ret_buf,
                                         size_t *out_ret_len,
                                         int *did_mutate);
#endif

/* ─── Cross-thread fcc spike helpers ─────────────────────────────────────
 * Cross-thread fcc invocation probe. Lives here so both Rust
 * (plugin functions) and C (runtime) can reach it. Not a stable
 * API — superseded by the real Pool FFI below. */

/**
 * Capture a PHP callable's fcc for later cross-thread invocation.
 * Stores the callable zval (refcount bumped) + the capturing
 * pthread id in a process-global slot. Returns the captured
 * pthread id via `out_tid` so the PHP-side test can compare with
 * the invoker's tid.
 *
 * Returns 0 on success, -1 if `zend_fcall_info_init` fails
 * (argument isn't a callable).
 *
 * Calling it repeatedly overwrites the slot and drops the old
 * callable's refcount on the CAPTURING thread, matching normal
 * zval lifetime; the assumption holds as long as the producer
 * keeps issuing capture calls on the original thread or the
 * process exits before the cross-thread dtor would run.
 */
int oxphp_pool_spike_capture(void *callable_zval, uint64_t *out_tid);

/**
 * Invoke the captured callable with zero arguments. Writes the
 * return value as a portbuf buffer to `*out_ret_buf` (caller frees
 * with `oxphp_portable_free`). Also writes the pthread id of the
 * capture and the invocation thread so the test can assert a
 * genuine cross-thread invocation occurred.
 *
 * Status codes:
 *   0   — OK, buffer filled.
 *  -1   — no captured fcc (capture never called).
 *  -2   — callable threw; `EG(exception)` is set.
 *  -3   — internal serialisation failure.
 */
int oxphp_pool_spike_invoke(
    uint64_t *out_captured_tid,
    uint64_t *out_current_tid,
    uint8_t **out_ret_buf,
    size_t *out_ret_len);

/**
 * Reset the spike slot (drop the stored callable). Meant to run
 * on the capturing thread. Safe to call when the slot is empty.
 * Used by tests to avoid leaking the captured zval across
 * consecutive capture/invoke cycles.
 */
void oxphp_pool_spike_reset(void);

/* ─── Shared\Pool helpers ───────────────────
 * Real pool factory / body / slot lifecycle helpers. The spike
 * above is a standalone probe and lives alongside these until it
 * can be retired.
 *
 * Lifetime contract:
 *   - `oxphp_pool_fcc_new`  → paired with `oxphp_pool_fcc_free`.
 *   - `oxphp_pool_factory_invoke` out-param is owned by pool;
 *      pair with `oxphp_pool_slot_free` at pool-drop.
 *   - `oxphp_pool_slot_to_user` does not transfer ownership;
 *      the pool retains the slot-zval.
 *
 * All *_free helpers invoke `zval_ptr_dtor` and must be called
 * on a Zend-initialised worker thread. v1 leaks on pool-drop;
 * a follow-up will wire the shutdown-driven cleanup path. */

int oxphp_pool_fcc_new(void *callable_zval, void **out_fcc_heap);
void oxphp_pool_fcc_free(void *fcc_heap);
int oxphp_pool_factory_invoke(void *fcc_heap, void **out_slot_zv_heap);
int oxphp_pool_body_invoke(void *body_callable_zv,
                            void *slot_zv_heap,
                            void *user_out_zv);
void oxphp_pool_slot_to_user(void *slot_zv_heap, void *user_out_zv);
void oxphp_pool_slot_free(void *slot_zv_heap);

/* Best-effort $destroy($resource) + slot release. See
 * oxphp_bridge.c for the full contract. `destroy_fcc_heap`
 * may be NULL (skip invocation). Exceptions are captured via
 * oxphp_bridge_capture_fatal and cleared. Always returns 0. */
int oxphp_pool_destroy_invoke(void *destroy_fcc_heap, void *slot_zv_heap);

/* Shared\Pool\Handle rust_data wrapper helpers. See
 * oxphp_bridge.c §Shared\Pool\Handle rust_data wrapper helpers
 * for the storage layout and semantics. */
int oxphp_shared_pool_handle_alloc(void *out_zv,
                                    uint64_t pool_id,
                                    uint64_t owner_tid,
                                    void *slot_zv_heap);

/* ─── Generic PHP object construction helpers ─────────────────────────────
 *
 * Used by value-typed return classes such as
 * `OxPHP\Shared\Channel\RecvResult` / `SendResult` (and any future class
 * whose handler needs to construct a PHP object from a Rust FFI handler
 * and stamp a few declared properties on it).
 *
 *  oxphp_bridge_make_object:
 *      Look up `cls_fqn` by name and run `object_init_ex` into `out`.
 *      Returns 0 on success, -1 if the class is not registered or
 *      object_init fails. `out` must be a writable 16-byte zval slot.
 *
 *  oxphp_bridge_object_set_property_long / _zval:
 *      Set a declared property by name on the constructed object.
 *      `_zval` copies the value via zend_update_property semantics
 *      (refcount handled by Zend). Both return 0 on success, -1 if
 *      `obj` is not an object zval.
 *
 *  oxphp_bridge_get_enum_case:
 *      Resolve `cls_fqn::case_name` to its singleton enum-case object
 *      and write it into `out` as an `IS_OBJECT` zval. Returns 0 on
 *      success, -1 if the enum class or case is not registered.
 *
 * Pointers are `void *` so Rust FFI declarations don't need to
 * forward-declare `zval`. Callers must pass valid zval-shaped (16-byte)
 * storage. */
int oxphp_bridge_make_object(void *out, const char *cls_fqn, size_t cls_len);
int oxphp_bridge_object_set_property_long(void *obj,
                                          const char *name,
                                          size_t name_len,
                                          long val);
int oxphp_bridge_object_set_property_zval(void *obj,
                                          const char *name,
                                          size_t name_len,
                                          void *src);
int oxphp_bridge_get_enum_case(void *out,
                               const char *cls_fqn,
                               size_t cls_len,
                               const char *case_name,
                               size_t case_len);

#ifdef __cplusplus
}
#endif

#endif /* OXPHP_BRIDGE_H */
