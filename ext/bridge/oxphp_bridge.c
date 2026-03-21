#include "oxphp_bridge.h"
#include <string.h>
#include <stdlib.h>
#include <time.h>

/**
 * Thread-local context — one per OS thread.
 * Both the Rust binary and the PHP extension link against this shared library,
 * so they share the same __thread variable.
 */
static __thread oxphp_ctx_t ctx;

void oxphp_bridge_init_ctx(void) {
    memset(&ctx, 0, sizeof(ctx));
}

void oxphp_bridge_clear_ctx(void) {
    memset(&ctx, 0, sizeof(ctx));
}

oxphp_ctx_t *oxphp_bridge_get_ctx(void) {
    return &ctx;
}

void oxphp_bridge_set_request_id(const char *id) {
    if (id) {
        strncpy(ctx.request_id, id, sizeof(ctx.request_id) - 1);
        ctx.request_id[sizeof(ctx.request_id) - 1] = '\0';
    } else {
        ctx.request_id[0] = '\0';
    }
}

const char *oxphp_bridge_get_request_id(void) {
    return ctx.request_id;
}

void oxphp_bridge_set_worker_id(int32_t id) {
    ctx.worker_id = id;
}

int32_t oxphp_bridge_get_worker_id(void) {
    return ctx.worker_id;
}

void oxphp_bridge_set_request_time(double time) {
    ctx.request_time = time;
}

double oxphp_bridge_get_request_time(void) {
    return ctx.request_time;
}

void oxphp_bridge_set_stream_mode(bool mode) {
    ctx.stream_mode = mode;
}

bool oxphp_bridge_is_streaming(void) {
    return ctx.stream_mode;
}

void oxphp_bridge_set_finished(bool finished) {
    ctx.finished = finished;
}

bool oxphp_bridge_is_finished(void) {
    return ctx.finished;
}

void oxphp_bridge_set_deadline(int64_t deadline_us) {
    ctx.deadline_us = deadline_us;
}

int64_t oxphp_bridge_get_deadline(void) {
    return ctx.deadline_us;
}

bool oxphp_bridge_is_deadline_expired(void) {
    if (ctx.deadline_us == 0) return false;
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    int64_t now_us = (int64_t)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
    return now_us >= ctx.deadline_us;
}

void oxphp_bridge_set_headers_sent(bool sent) {
    ctx.headers_sent = sent;
}

bool oxphp_bridge_get_headers_sent(void) {
    return ctx.headers_sent;
}

/* ─── Plugin Function Registry (global, NOT __thread) ─────────── */
/*
 * Thread safety: written once from the main thread during startup, then read
 * during MINIT on the same thread (before any worker thread is spawned).
 * No concurrent access — single-writer, single-reader, strictly sequential.
 *
 * The registry is never freed — it lives for the entire process lifetime.
 */

typedef struct {
    char *name;
    int required_params;
    int total_params;
} oxphp_plugin_fn_entry_t;

static oxphp_plugin_fn_entry_t *plugin_functions = NULL;
static int plugin_function_count = 0;
static int plugin_function_capacity = 0;

void oxphp_bridge_register_plugin_fn(const char* name, int required_params, int total_params) {
    if (!name) return;
    if (plugin_function_count >= plugin_function_capacity) {
        int new_cap = plugin_function_capacity == 0 ? 16 : plugin_function_capacity * 2;
        oxphp_plugin_fn_entry_t *new_arr = realloc(plugin_functions, new_cap * sizeof(oxphp_plugin_fn_entry_t));
        if (!new_arr) return;
        plugin_functions = new_arr;
        plugin_function_capacity = new_cap;
    }
    char *dup = strdup(name);
    if (!dup) return;
    plugin_functions[plugin_function_count].name = dup;
    plugin_functions[plugin_function_count].required_params = required_params;
    plugin_functions[plugin_function_count].total_params = total_params;
    plugin_function_count++;
}

int oxphp_bridge_get_plugin_fn_count(void) {
    return plugin_function_count;
}

const char* oxphp_bridge_get_plugin_fn_name(int index) {
    if (index < 0 || index >= plugin_function_count) return NULL;
    return plugin_functions[index].name;
}

int oxphp_bridge_get_plugin_fn_required(int index) {
    if (index < 0 || index >= plugin_function_count) return 0;
    return plugin_functions[index].required_params;
}

int oxphp_bridge_get_plugin_fn_total(int index) {
    if (index < 0 || index >= plugin_function_count) return 0;
    return plugin_functions[index].total_params;
}

/* ═══════════════════════════════════════════════════════════
 *  Native Bridge API — Zero-Serialization Value Access
 * ═══════════════════════════════════════════════════════════ */

#include "php.h"
#include "SAPI.h"
#include "Zend/zend_API.h"
#include "Zend/zend_hash.h"
#include "Zend/zend_closures.h"
#include "Zend/zend_exceptions.h"

/* Define thread-local TSRM cache for this compilation unit.
 * SG()/CG()/EG() macros expand to use TSRMLS_CACHE (_tsrm_ls_cache).
 * Each .so gets its own copy of this TLS variable.
 * Must call oxphp_bridge_tsrm_update() on each worker thread after ts_resource_ex(). */
#ifdef ZTS
TSRMLS_CACHE_DEFINE()
#endif

/* ── Type mapping (branchless lookup table) ── */

/* PHP IS_* constants → OXPHP_TYPE_* (max IS_* value is ~12, pad to 16). */
static const uint8_t ztype_to_oxphp[16] = {
    [IS_NULL]     = OXPHP_TYPE_NULL,
    [IS_FALSE]    = OXPHP_TYPE_FALSE,
    [IS_TRUE]     = OXPHP_TYPE_TRUE,
    [IS_LONG]     = OXPHP_TYPE_LONG,
    [IS_DOUBLE]   = OXPHP_TYPE_DOUBLE,
    [IS_STRING]   = OXPHP_TYPE_STRING,
    [IS_ARRAY]    = OXPHP_TYPE_ARRAY,
    [IS_OBJECT]   = OXPHP_TYPE_OBJECT,
    [IS_RESOURCE] = OXPHP_TYPE_RESOURCE,
    /* remaining slots default to 0 = OXPHP_TYPE_NULL */
};

static inline uint8_t type_to_oxphp(uint8_t ztype) {
    return (ztype < 16) ? ztype_to_oxphp[ztype] : OXPHP_TYPE_NULL;
}

uint8_t oxphp_val_type(void *zv) {
    return type_to_oxphp(Z_TYPE_P((zval*)zv));
}

uint8_t oxphp_val_arg_type(void *args, uint32_t idx) {
    return type_to_oxphp(Z_TYPE_P(((zval*)args) + idx));
}

/* ── Scalar reading from args[idx] ── */

int64_t oxphp_arg_long(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    return Z_TYPE_P(z) == IS_LONG ? Z_LVAL_P(z) : 0;
}

double oxphp_arg_double(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) == IS_DOUBLE) return Z_DVAL_P(z);
    if (Z_TYPE_P(z) == IS_LONG) return (double)Z_LVAL_P(z);
    return 0.0;
}

int oxphp_arg_bool(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) == IS_TRUE) return 1;
    if (Z_TYPE_P(z) == IS_FALSE) return 0;
    return zend_is_true(z);
}

const uint8_t *oxphp_arg_str(void *args, uint32_t idx, size_t *out_len) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_STRING) {
        *out_len = 0;
        return NULL;
    }
    *out_len = Z_STRLEN_P(z);
    return (const uint8_t*)Z_STRVAL_P(z);
}

/* ── Array reading ── */

uint32_t oxphp_arg_array_count(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_ARRAY) return 0;
    return zend_hash_num_elements(Z_ARRVAL_P(z));
}

void *oxphp_arg_array(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_ARRAY) return NULL;
    return z;
}

/* ── Array iteration ── */

void oxphp_array_foreach(void *zv_array, oxphp_array_iter_fn cb, void *user_data) {
    zval *arr = (zval*)zv_array;
    if (Z_TYPE_P(arr) != IS_ARRAY) return;

    HashTable *ht = Z_ARRVAL_P(arr);
    zend_ulong num_idx;
    zend_string *str_key;
    zval *val;

    ZEND_HASH_FOREACH_KEY_VAL(ht, num_idx, str_key, val) {
        if (str_key) {
            cb((const uint8_t*)ZSTR_VAL(str_key), ZSTR_LEN(str_key),
               (int64_t)num_idx, val, user_data);
        } else {
            cb(NULL, 0, (int64_t)num_idx, val, user_data);
        }
    } ZEND_HASH_FOREACH_END();
}

/* ── Direct value reading ── */

int64_t oxphp_val_long(void *zv)  { return Z_TYPE_P((zval*)zv) == IS_LONG ? Z_LVAL_P((zval*)zv) : 0; }
double  oxphp_val_double(void *zv){ return Z_TYPE_P((zval*)zv) == IS_DOUBLE ? Z_DVAL_P((zval*)zv) : 0.0; }
int     oxphp_val_bool(void *zv)  { return zend_is_true((zval*)zv); }

const uint8_t *oxphp_val_str(void *zv, size_t *out_len) {
    if (Z_TYPE_P((zval*)zv) != IS_STRING) { *out_len = 0; return NULL; }
    *out_len = Z_STRLEN_P((zval*)zv);
    return (const uint8_t*)Z_STRVAL_P((zval*)zv);
}

uint32_t oxphp_val_array_count(void *zv) {
    if (Z_TYPE_P((zval*)zv) != IS_ARRAY) return 0;
    return zend_hash_num_elements(Z_ARRVAL_P((zval*)zv));
}

/* ── Value writing ── */

void oxphp_ret_null(void *rv)                   { ZVAL_NULL((zval*)rv); }
void oxphp_ret_bool(void *rv, int val)          { ZVAL_BOOL((zval*)rv, val); }
void oxphp_ret_long(void *rv, int64_t val)      { ZVAL_LONG((zval*)rv, (zend_long)val); }
void oxphp_ret_double(void *rv, double val)     { ZVAL_DOUBLE((zval*)rv, val); }

void oxphp_ret_str(void *rv, const uint8_t *s, size_t len) {
    ZVAL_STRINGL((zval*)rv, (const char*)s, len);
}

void oxphp_ret_array_init(void *rv, uint32_t size_hint) {
    array_init_size((zval*)rv, size_hint);
}

/* ── Array builders (keyed) ── */

void oxphp_arr_add_null(void *arr, const char *key, size_t klen) {
    add_assoc_null_ex((zval*)arr, key, klen);
}
void oxphp_arr_add_bool(void *arr, const char *key, size_t klen, int val) {
    add_assoc_bool_ex((zval*)arr, key, klen, val);
}
void oxphp_arr_add_long(void *arr, const char *key, size_t klen, int64_t val) {
    add_assoc_long_ex((zval*)arr, key, klen, (zend_long)val);
}
void oxphp_arr_add_double(void *arr, const char *key, size_t klen, double val) {
    add_assoc_double_ex((zval*)arr, key, klen, val);
}
void oxphp_arr_add_str(void *arr, const char *key, size_t klen, const uint8_t *s, size_t slen) {
    add_assoc_stringl_ex((zval*)arr, key, klen, (const char*)s, slen);
}

void *oxphp_arr_add_array(void *arr, const char *key, size_t klen, uint32_t size_hint) {
    zval sub;
    array_init_size(&sub, size_hint);
    /* Single hash op: insert and return the stored zval pointer. */
    return zend_hash_str_update(Z_ARRVAL_P((zval*)arr), key, klen, &sub);
}

/* ── Array builders (indexed / push) ── */

void oxphp_arr_push_null(void *arr)                { add_next_index_null((zval*)arr); }
void oxphp_arr_push_bool(void *arr, int val)       { add_next_index_bool((zval*)arr, val); }
void oxphp_arr_push_long(void *arr, int64_t val)   { add_next_index_long((zval*)arr, (zend_long)val); }
void oxphp_arr_push_double(void *arr, double val)  { add_next_index_double((zval*)arr, val); }
void oxphp_arr_push_str(void *arr, const uint8_t *s, size_t len) {
    add_next_index_stringl((zval*)arr, (const char*)s, len);
}

void *oxphp_arr_push_array(void *arr, uint32_t size_hint) {
    zval sub;
    array_init_size(&sub, size_hint);
    /* Single hash op: append and return the stored zval pointer. */
    return zend_hash_next_index_insert(Z_ARRVAL_P((zval*)arr), &sub);
}

/* ── Native dispatch callback ── */

static oxphp_native_dispatch_fn_t native_dispatch_fn = NULL;

void oxphp_bridge_set_native_dispatch(oxphp_native_dispatch_fn_t fn) { native_dispatch_fn = fn; }
oxphp_native_dispatch_fn_t oxphp_bridge_get_native_dispatch(void)    { return native_dispatch_fn; }

/* ── PHP → Rust native call ── */

int oxphp_call_php_native(const char *func_name, void *args, uint32_t argc, void *result) {
    /* Resolve function entry directly — avoids ZVAL_STRING allocation +
     * zval_ptr_dtor for the function name zval on every call. */
    size_t name_len = strlen(func_name);
    zend_function *fbc = zend_hash_str_find_ptr(CG(function_table), func_name, name_len);
    if (!fbc) {
        ZVAL_NULL((zval*)result);
        return -1;
    }

    ZVAL_NULL((zval*)result);
    zend_call_known_function(fbc, NULL, NULL, (zval*)result, argc, (zval*)args, NULL);
    return 0;
}

/* ── TSRM cache update ── */

void oxphp_bridge_tsrm_update(void) {
#ifdef ZTS
    TSRMLS_CACHE_UPDATE();
#endif
}

/* ── SAPI request_info ── */

void oxphp_bridge_set_request_info(
    const char *method,
    const char *query_string,
    const char *content_type,
    long content_length
) {
    /* Reset response code from the bridge's TSRM context.
     * sapi_activate() (called by php_request_startup) resets this in libphp's
     * TSRM context, but the bridge library has its own _tsrm_ls_cache that
     * may resolve to stale memory. Explicitly resetting here ensures
     * collect_response_code() reads 200 (not a leaked value from the
     * previous request) when called after script execution. */
    SG(sapi_headers).http_response_code = 200;

    /* Set a non-NULL server_context — PHP checks this in sapi_activate()
     * to decide whether to read POST data and cookies. Without it,
     * $_POST/$_FILES/$_COOKIE are never populated. */
    SG(server_context) = (void*)(method ? 1 : 0);
    SG(request_info).request_method = method;
    SG(request_info).query_string = (char*)query_string;
    SG(request_info).content_type = content_type;
    SG(request_info).content_length = content_length;
}

/* ── SAPI response code ── */

int oxphp_bridge_get_response_code(void) {
#ifdef ZTS
    TSRMLS_CACHE_UPDATE();
#endif
    return SG(sapi_headers).http_response_code;
}

/* ── Zval lifecycle ── */

void oxphp_zval_dtor(void *zv) {
    zval_ptr_dtor((zval*)zv);
}

void oxphp_zval_addref(void *zv) {
    Z_TRY_ADDREF_P((zval*)zv);
}

void *oxphp_closure_addref(void *closure_zv) {
    zval *zv = (zval*)closure_zv;
    if (Z_TYPE_P(zv) == IS_OBJECT) {
        zend_object *obj = Z_OBJ_P(zv);
        GC_ADDREF(obj);
        return obj;
    }
    return NULL;
}

void oxphp_closure_release(void *obj_ptr) {
    if (obj_ptr) {
        OBJ_RELEASE((zend_object*)obj_ptr);
    }
}

size_t oxphp_zval_size(void) {
    return sizeof(zval);
}

/* ─── Worker Mode ─────────────────────────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. All workers share the same callback pointers.
 */
static oxphp_worker_wait_fn_t rust_worker_wait = NULL;
static oxphp_worker_send_fn_t rust_worker_send = NULL;

void oxphp_bridge_set_worker_callbacks(oxphp_worker_wait_fn_t wait_fn, oxphp_worker_send_fn_t send_fn) {
    rust_worker_wait = wait_fn;
    rust_worker_send = send_fn;
}

void oxphp_bridge_set_worker_mode(uint64_t max_requests, uint64_t max_memory_mib) {
    ctx.worker_mode = 1;
    ctx.max_requests = max_requests;
    ctx.max_memory_bytes = max_memory_mib * 1024 * 1024;  /* pre-compute to avoid per-request mul */
    ctx.requests_done = 0;
    ctx.exit_reason = 0;
    ctx.current_memory_bytes = 0;
}

bool oxphp_bridge_is_worker_mode(void) {
    return ctx.worker_mode != 0;
}

void oxphp_bridge_reset_request_ctx(void) {
    ctx.request_id[0] = '\0';
    ctx.request_time = 0.0;
    ctx.deadline_us = 0;
    ctx.cancelled = false;
    ctx.write_count = 0;
    ctx.stream_mode = false;
    ctx.headers_sent = false;
    ctx.finished = false;
    /* Note: requests_done is NOT incremented here — the caller (oxphp_worker loop)
     * increments it explicitly after send_response, keeping the side effect visible. */
}

int oxphp_bridge_worker_wait(void) {
    /* Callbacks are guaranteed non-NULL in worker mode (set once at startup).
     * Use __builtin_expect to hint the branch predictor. */
    if (__builtin_expect(rust_worker_wait != NULL, 1)) {
        return rust_worker_wait();
    }
    return -1;
}

int oxphp_bridge_worker_send_response(void) {
    if (__builtin_expect(rust_worker_send != NULL, 1)) {
        return rust_worker_send();
    }
    return -1;
}

/* ─── Fiber Scheduler Callbacks ────────────────────────── */

static oxphp_worker_try_recv_fn_t rust_worker_try_recv = NULL;
static oxphp_prepare_request_fn_t rust_prepare_request = NULL;

void oxphp_bridge_set_fiber_callbacks(
    oxphp_worker_try_recv_fn_t try_recv_fn,
    oxphp_prepare_request_fn_t prepare_fn
) {
    rust_worker_try_recv = try_recv_fn;
    rust_prepare_request = prepare_fn;
}

int oxphp_bridge_worker_try_recv(void) {
    if (__builtin_expect(rust_worker_try_recv != NULL, 1)) {
        return rust_worker_try_recv();
    }
    return 1; /* empty, not shutdown — safe fallback if callbacks not registered */
}

int oxphp_bridge_prepare_request(void) {
    if (__builtin_expect(rust_prepare_request != NULL, 1)) {
        return rust_prepare_request();
    }
    return 0;
}

void oxphp_bridge_set_cancelled(bool cancelled) {
    ctx.cancelled = cancelled;
}

bool oxphp_bridge_is_cancelled(void) {
    return ctx.cancelled;
}

uint8_t oxphp_bridge_get_exit_reason(void) {
    return ctx.exit_reason;
}

uint64_t oxphp_bridge_get_requests_done(void) {
    return ctx.requests_done;
}

uint64_t oxphp_bridge_get_memory_usage(void) {
    return ctx.current_memory_bytes;
}

bool oxphp_bridge_get_handler_failed(void) {
    return ctx.handler_failed;
}

/* ── Bailout wrapper ── */

void oxphp_bridge_bailout(void) {
    zend_bailout();
}

/* ── SAPI callback wrappers with cooperative deadline check ── */

/*
 * Global (not __thread) — set once at startup by build_sapi_module() BEFORE
 * any worker threads are spawned, so no data race.  All workers share the
 * same Rust function pointers; per-request state lives in __thread ctx.
 */
static oxphp_ub_write_fn_t rust_ub_write = NULL;
static oxphp_flush_fn_t    rust_flush    = NULL;

void oxphp_bridge_set_sapi_callbacks(oxphp_ub_write_fn_t ub_write, oxphp_flush_fn_t flush) {
    rust_ub_write = ub_write;
    rust_flush    = flush;
}

/**
 * Check deadline and cancellation, bailout if needed.
 * Called from C — longjmp stays within C frames, never crosses Rust FFI.
 */
static inline void check_deadline_c(void) {
    if (ctx.cancelled) {
        zend_bailout();
    }
    if (ctx.deadline_us != 0) {
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        int64_t now_us = (int64_t)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
        if (now_us >= ctx.deadline_us) {
            zend_bailout();
        }
    }
}

/**
 * Fast check — only tests the cancelled flag (no syscall).
 * Used on ub_write hot path between periodic full checks.
 */
static inline void check_cancelled_c(void) {
    if (ctx.cancelled) {
        zend_bailout();
    }
}

/* Check interval for ub_write: every 128 calls do a full deadline check,
 * otherwise only check the cancelled flag (a single bool read, no syscall). */
#define DEADLINE_CHECK_INTERVAL 128

size_t oxphp_bridge_ub_write(const char *str, size_t str_length) {
    if (++ctx.write_count >= DEADLINE_CHECK_INTERVAL) {
        ctx.write_count = 0;
        check_deadline_c();
    } else {
        check_cancelled_c();
    }
    if (rust_ub_write) {
        return rust_ub_write(str, str_length);
    }
    return 0;
}

void oxphp_bridge_flush(void *server_context) {
    /* flush is infrequent — always do a full check. */
    check_deadline_c();
    if (rust_flush) {
        rust_flush(server_context);
    }
}

/* ── Safe script execution with zend_try ── */

#include "main/php_main.h"  /* php_execute_script() */

/* Takes void* to avoid exposing zend_file_handle in the Rust FFI bindings.
 * Caller is responsible for passing a valid zend_file_handle pointer. */
int oxphp_execute_script_safe(void *file_handle) {
    int result = 0;
    zend_try {
        php_execute_script((zend_file_handle *)file_handle);
        result = 1;  /* success */
    } zend_catch {
        result = 0;  /* bailout occurred */
    } zend_end_try();
    return result;
}

/* ─── Async Dispatch Function Pointers ─────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. Same pattern as worker callbacks.
 */
static oxphp_async_dispatch_fn_t rust_async_dispatch = NULL;
static oxphp_await_dispatch_fn_t rust_await_dispatch = NULL;
static oxphp_await_any_dispatch_fn_t rust_await_any_dispatch = NULL;

void oxphp_bridge_set_async_dispatch(oxphp_async_dispatch_fn_t fn) {
    rust_async_dispatch = fn;
}

void oxphp_bridge_set_await_dispatch(oxphp_await_dispatch_fn_t fn) {
    rust_await_dispatch = fn;
}

void oxphp_bridge_set_await_any_dispatch(oxphp_await_any_dispatch_fn_t fn) {
    rust_await_any_dispatch = fn;
}

int64_t oxphp_bridge_async_dispatch(
    void *op_array, void *static_vars, void *this_ptr,
    uint32_t argc, void *args, void *closure_zval
) {
    if (__builtin_expect(rust_async_dispatch != NULL, 1)) {
        return rust_async_dispatch(op_array, static_vars, this_ptr, argc, args, closure_zval);
    }
    return -1;
}

int oxphp_bridge_await_dispatch(int64_t promise_id, double timeout, void *retval) {
    if (__builtin_expect(rust_await_dispatch != NULL, 1)) {
        return rust_await_dispatch(promise_id, timeout, retval);
    }
    return -1;
}

int oxphp_bridge_await_any_dispatch(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
) {
    if (__builtin_expect(rust_await_any_dispatch != NULL, 1)) {
        return rust_await_any_dispatch(promise_ids, count, timeout, out_winner_id, retval);
    }
    return -1;
}

/* ─── Non-Blocking Await Poll ──────────────────────────────── */
static oxphp_await_poll_fn_t rust_await_poll = NULL;

void oxphp_bridge_set_await_poll(oxphp_await_poll_fn_t fn) {
    rust_await_poll = fn;
}

int oxphp_bridge_await_poll(int64_t promise_id) {
    if (__builtin_expect(rust_await_poll != NULL, 1)) {
        return rust_await_poll(promise_id);
    }
    return 0;
}

/* ─── Async Promise Cleanup ─────────────────────────────────── */
static oxphp_cleanup_promises_fn_t rust_cleanup_promises = NULL;

void oxphp_bridge_set_cleanup_promises(oxphp_cleanup_promises_fn_t fn) {
    rust_cleanup_promises = fn;
}

void oxphp_bridge_cleanup_outstanding_promises(void) {
    if (__builtin_expect(rust_cleanup_promises != NULL, 1)) {
        rust_cleanup_promises();
    }
}

/* === Async Promise: Freeze/Unfreeze === */

static void oxphp_freeze_zval_recursive(zval *zv);

int oxphp_freeze_zval(zval *zv, uint32_t *out_orig_refcount, uint32_t *out_orig_gc_flags, uint32_t *out_orig_type_flags) {
    /* Unwrap references */
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }

    switch (Z_TYPE_P(zv)) {
        case IS_ARRAY: {
            /* Separate COW-shared arrays before freezing */
            SEPARATE_ARRAY(zv);
            HashTable *ht = Z_ARRVAL_P(zv);
            *out_orig_refcount = GC_REFCOUNT(ht);
            *out_orig_gc_flags = GC_FLAGS(ht);
            *out_orig_type_flags = Z_TYPE_FLAGS_P(zv);

            GC_ADD_FLAGS(ht, IS_ARRAY_IMMUTABLE);
            GC_SET_REFCOUNT(ht, 2); /* GC_IMMUTABLE_REFCOUNT */

            zval *val;
            ZEND_HASH_FOREACH_VAL(ht, val) {
                oxphp_freeze_zval_recursive(val);
            } ZEND_HASH_FOREACH_END();
            return 0;
        }
        case IS_STRING: {
            *out_orig_refcount = 0;
            *out_orig_gc_flags = 0;
            *out_orig_type_flags = Z_TYPE_FLAGS_P(zv);
            /* Clear refcounted flag — engine skips refcount ops */
            Z_TYPE_FLAGS_P(zv) &= ~(IS_TYPE_REFCOUNTED | IS_TYPE_COLLECTABLE);
            return 0;
        }
        case IS_LONG:
        case IS_DOUBLE:
        case IS_TRUE:
        case IS_FALSE:
        case IS_NULL:
            /* Value types — no freeze needed */
            *out_orig_refcount = 0;
            *out_orig_gc_flags = 0;
            *out_orig_type_flags = 0;
            return 0;
        default:
            /* Objects, resources — cannot freeze */
            return -1;
    }
}

/* Recursive freeze for array elements */
static void oxphp_freeze_zval_recursive(zval *zv) {
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }
    if (Z_TYPE_P(zv) == IS_ARRAY) {
        HashTable *ht = Z_ARRVAL_P(zv);
        GC_ADD_FLAGS(ht, IS_ARRAY_IMMUTABLE);
        GC_SET_REFCOUNT(ht, 2);
        zval *val;
        ZEND_HASH_FOREACH_VAL(ht, val) {
            oxphp_freeze_zval_recursive(val);
        } ZEND_HASH_FOREACH_END();
    } else if (Z_TYPE_P(zv) == IS_STRING) {
        Z_TYPE_FLAGS_P(zv) &= ~(IS_TYPE_REFCOUNTED | IS_TYPE_COLLECTABLE);
    }
}

/* === Async Promise: Unfreeze === */

static void oxphp_unfreeze_zval_recursive(zval *zv);

void oxphp_unfreeze_zval(zval *zv, uint32_t orig_refcount, uint32_t orig_gc_flags, uint32_t orig_type_flags) {
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }
    switch (Z_TYPE_P(zv)) {
        case IS_ARRAY: {
            HashTable *ht = Z_ARRVAL_P(zv);
            GC_SET_REFCOUNT(ht, orig_refcount);
            /* Clear all flags then restore originals (GC_FLAGS is not an lvalue in PHP 8.4) */
            GC_DEL_FLAGS(ht, GC_FLAGS(ht));
            GC_ADD_FLAGS(ht, orig_gc_flags);
            Z_TYPE_FLAGS_P(zv) = (uint8_t)orig_type_flags;

            zval *val;
            ZEND_HASH_FOREACH_VAL(ht, val) {
                oxphp_unfreeze_zval_recursive(val);
            } ZEND_HASH_FOREACH_END();
            break;
        }
        case IS_STRING:
            Z_TYPE_FLAGS_P(zv) = (uint8_t)orig_type_flags;
            break;
        default:
            break;
    }
}

static void oxphp_unfreeze_zval_recursive(zval *zv) {
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }
    if (Z_TYPE_P(zv) == IS_ARRAY) {
        HashTable *ht = Z_ARRVAL_P(zv);
        GC_DEL_FLAGS(ht, IS_ARRAY_IMMUTABLE);
        GC_SET_REFCOUNT(ht, 1);
        zval *val;
        ZEND_HASH_FOREACH_VAL(ht, val) {
            oxphp_unfreeze_zval_recursive(val);
        } ZEND_HASH_FOREACH_END();
    } else if (Z_TYPE_P(zv) == IS_STRING) {
        Z_TYPE_FLAGS_P(zv) |= IS_TYPE_REFCOUNTED;
    }
}

/* === Async Promise: Deep Copy === */

void oxphp_deep_copy_zval(zval *dst, const zval *src) {
    switch (Z_TYPE_P(src)) {
        case IS_NULL:
        case IS_TRUE:
        case IS_FALSE:
        case IS_LONG:
        case IS_DOUBLE:
            ZVAL_COPY_VALUE(dst, src);
            break;
        case IS_STRING: {
            size_t len = Z_STRLEN_P(src);
            ZVAL_STRINGL(dst, Z_STRVAL_P(src), len);
            break;
        }
        case IS_ARRAY: {
            uint32_t count = zend_hash_num_elements(Z_ARRVAL_P(src));
            array_init_size(dst, count);
            zend_ulong idx;
            zend_string *key;
            zval *val;
            ZEND_HASH_FOREACH_KEY_VAL(Z_ARRVAL_P(src), idx, key, val) {
                zval copied;
                oxphp_deep_copy_zval(&copied, val);
                if (key) {
                    zend_string *key_copy = zend_string_init(
                        ZSTR_VAL(key), ZSTR_LEN(key), 0
                    );
                    zend_hash_add_new(Z_ARRVAL_P(dst), key_copy, &copied);
                    zend_string_release(key_copy);
                } else {
                    zend_hash_index_add_new(Z_ARRVAL_P(dst), idx, &copied);
                }
            } ZEND_HASH_FOREACH_END();
            break;
        }
        case IS_REFERENCE:
            oxphp_deep_copy_zval(dst, Z_REFVAL_P(src));
            break;
        default:
            /* Objects, resources — cannot deep copy across threads */
            ZVAL_NULL(dst);
            break;
    }
}

void oxphp_deep_free_zval(zval *zv) {
    zval_ptr_dtor(zv);
}

/* === Portable (cross-thread) serialization ===
 *
 * Serializes zvals into a flat byte buffer allocated via system malloc().
 * The buffer can cross ZTS thread boundaries safely.  The receiver calls
 * oxphp_portable_deserialize() which allocates strings/arrays via emalloc
 * on ITS OWN thread's zend_mm_heap — avoiding the cross-heap corruption
 * that oxphp_deep_copy_zval/oxphp_deep_free_zval cause.
 *
 * Wire format per zval:
 *   [1 byte type tag] [payload …]
 *
 * Type tags:
 *   0 = null, 1 = true, 2 = false, 3 = long (8 bytes),
 *   4 = double (8 bytes), 5 = string (4 bytes length + N bytes data),
 *   6 = array (4 bytes count + N×(1 byte key_type + key + value) entries),
 *       key_type: 0 = index (8 bytes ulong), 1 = string key (4 bytes len + data)
 */

/* Growable buffer using system malloc */
typedef struct {
    unsigned char *data;
    size_t len;
    size_t cap;
} portbuf_t;

static void portbuf_init(portbuf_t *b) {
    b->cap = 256;
    b->data = (unsigned char *)malloc(b->cap);
    b->len = 0;
}

static int portbuf_ensure(portbuf_t *b, size_t extra) {
    if (b->len + extra <= b->cap) return 0;
    size_t need = b->len + extra;
    size_t ncap = b->cap * 2;
    while (ncap < need) ncap *= 2;
    unsigned char *p = (unsigned char *)realloc(b->data, ncap);
    if (!p) return -1;
    b->data = p;
    b->cap = ncap;
    return 0;
}

static void portbuf_put(portbuf_t *b, const void *src, size_t n) {
    memcpy(b->data + b->len, src, n);
    b->len += n;
}

static void portbuf_u8(portbuf_t *b, uint8_t v) {
    b->data[b->len++] = v;
}

static void portbuf_u32(portbuf_t *b, uint32_t v) {
    memcpy(b->data + b->len, &v, 4);
    b->len += 4;
}

static void portbuf_u64(portbuf_t *b, uint64_t v) {
    memcpy(b->data + b->len, &v, 8);
    b->len += 8;
}

/* Forward declaration for recursive serialization */
static int portbuf_ser_zval(portbuf_t *b, const zval *zv);

static int portbuf_ser_zval(portbuf_t *b, const zval *zv) {
    if (portbuf_ensure(b, 16) != 0) return -1;

    switch (Z_TYPE_P(zv)) {
        case IS_NULL:
        case IS_UNDEF:
            portbuf_u8(b, 0);
            break;
        case IS_TRUE:
            portbuf_u8(b, 1);
            break;
        case IS_FALSE:
            portbuf_u8(b, 2);
            break;
        case IS_LONG: {
            portbuf_u8(b, 3);
            int64_t v = (int64_t)Z_LVAL_P(zv);
            if (portbuf_ensure(b, 8) != 0) return -1;
            memcpy(b->data + b->len, &v, 8);
            b->len += 8;
            break;
        }
        case IS_DOUBLE: {
            portbuf_u8(b, 4);
            double v = Z_DVAL_P(zv);
            if (portbuf_ensure(b, 8) != 0) return -1;
            memcpy(b->data + b->len, &v, 8);
            b->len += 8;
            break;
        }
        case IS_STRING: {
            size_t slen = Z_STRLEN_P(zv);
            uint32_t slen32 = (uint32_t)slen;
            if (portbuf_ensure(b, 1 + 4 + slen) != 0) return -1;
            portbuf_u8(b, 5);
            portbuf_u32(b, slen32);
            portbuf_put(b, Z_STRVAL_P(zv), slen);
            break;
        }
        case IS_ARRAY: {
            HashTable *ht = Z_ARRVAL_P(zv);
            uint32_t count = zend_hash_num_elements(ht);
            if (portbuf_ensure(b, 1 + 4) != 0) return -1;
            portbuf_u8(b, 6);
            portbuf_u32(b, count);

            zend_ulong idx;
            zend_string *key;
            zval *val;
            ZEND_HASH_FOREACH_KEY_VAL(ht, idx, key, val) {
                if (key) {
                    size_t klen = ZSTR_LEN(key);
                    if (portbuf_ensure(b, 1 + 4 + klen) != 0) return -1;
                    portbuf_u8(b, 1); /* string key */
                    portbuf_u32(b, (uint32_t)klen);
                    portbuf_put(b, ZSTR_VAL(key), klen);
                } else {
                    if (portbuf_ensure(b, 1 + 8) != 0) return -1;
                    portbuf_u8(b, 0); /* index key */
                    portbuf_u64(b, (uint64_t)idx);
                }
                if (portbuf_ser_zval(b, val) != 0) return -1;
            } ZEND_HASH_FOREACH_END();
            break;
        }
        case IS_REFERENCE:
            return portbuf_ser_zval(b, Z_REFVAL_P(zv));
        default:
            /* Objects, resources — serialize as null */
            portbuf_u8(b, 0);
            break;
    }
    return 0;
}

int oxphp_portable_serialize(const zval *args, uint32_t argc,
                             unsigned char **out_buf, size_t *out_len) {
    portbuf_t b;
    portbuf_init(&b);
    if (!b.data) return -1;

    for (uint32_t i = 0; i < argc; i++) {
        if (portbuf_ser_zval(&b, &args[i]) != 0) {
            free(b.data);
            return -1;
        }
    }
    *out_buf = b.data;
    *out_len = b.len;
    return 0;
}

/* Reader state for deserialization */
typedef struct {
    const unsigned char *data;
    size_t len;
    size_t pos;
} portrd_t;

static int portrd_u8(portrd_t *r, uint8_t *out) {
    if (r->pos >= r->len) return -1;
    *out = r->data[r->pos++];
    return 0;
}

static int portrd_u32(portrd_t *r, uint32_t *out) {
    if (r->pos + 4 > r->len) return -1;
    memcpy(out, r->data + r->pos, 4);
    r->pos += 4;
    return 0;
}

static int portrd_u64(portrd_t *r, uint64_t *out) {
    if (r->pos + 8 > r->len) return -1;
    memcpy(out, r->data + r->pos, 8);
    r->pos += 8;
    return 0;
}

static int portrd_bytes(portrd_t *r, size_t n, const unsigned char **out) {
    if (r->pos + n > r->len) return -1;
    *out = r->data + r->pos;
    r->pos += n;
    return 0;
}

/* Forward declaration for recursive deserialization */
static int portrd_deser_zval(portrd_t *r, zval *out);

static int portrd_deser_zval(portrd_t *r, zval *out) {
    uint8_t tag;
    if (portrd_u8(r, &tag) != 0) return -1;

    switch (tag) {
        case 0: /* null */
            ZVAL_NULL(out);
            break;
        case 1: /* true */
            ZVAL_TRUE(out);
            break;
        case 2: /* false */
            ZVAL_FALSE(out);
            break;
        case 3: { /* long */
            int64_t v;
            if (r->pos + 8 > r->len) return -1;
            memcpy(&v, r->data + r->pos, 8);
            r->pos += 8;
            ZVAL_LONG(out, (zend_long)v);
            break;
        }
        case 4: { /* double */
            double v;
            if (r->pos + 8 > r->len) return -1;
            memcpy(&v, r->data + r->pos, 8);
            r->pos += 8;
            ZVAL_DOUBLE(out, v);
            break;
        }
        case 5: { /* string */
            uint32_t slen;
            if (portrd_u32(r, &slen) != 0) return -1;
            const unsigned char *sdata;
            if (portrd_bytes(r, slen, &sdata) != 0) return -1;
            /* ZVAL_STRINGL uses emalloc on the CURRENT thread's heap — correct! */
            ZVAL_STRINGL(out, (const char *)sdata, slen);
            break;
        }
        case 6: { /* array */
            uint32_t count;
            if (portrd_u32(r, &count) != 0) return -1;
            /* array_init_size uses emalloc on the CURRENT thread's heap — correct! */
            array_init_size(out, count);
            for (uint32_t i = 0; i < count; i++) {
                uint8_t key_type;
                if (portrd_u8(r, &key_type) != 0) return -1;

                zval elem;
                ZVAL_UNDEF(&elem);

                if (key_type == 1) {
                    /* string key */
                    uint32_t klen;
                    if (portrd_u32(r, &klen) != 0) return -1;
                    const unsigned char *kdata;
                    if (portrd_bytes(r, klen, &kdata) != 0) return -1;
                    if (portrd_deser_zval(r, &elem) != 0) {
                        zval_ptr_dtor(&elem);
                        return -1;
                    }
                    zend_string *zkey = zend_string_init(
                        (const char *)kdata, klen, 0
                    );
                    zend_hash_add_new(Z_ARRVAL_P(out), zkey, &elem);
                    zend_string_release(zkey);
                } else {
                    /* index key */
                    uint64_t idx;
                    if (portrd_u64(r, &idx) != 0) return -1;
                    if (portrd_deser_zval(r, &elem) != 0) {
                        zval_ptr_dtor(&elem);
                        return -1;
                    }
                    zend_hash_index_add_new(Z_ARRVAL_P(out), (zend_ulong)idx, &elem);
                }
            }
            break;
        }
        default:
            ZVAL_NULL(out);
            break;
    }
    return 0;
}

int oxphp_portable_deserialize(const unsigned char *buf, size_t len,
                               uint32_t argc, zval *out) {
    portrd_t r = { buf, len, 0 };
    for (uint32_t i = 0; i < argc; i++) {
        if (portrd_deser_zval(&r, &out[i]) != 0) {
            /* Cleanup already-deserialized zvals on error */
            for (uint32_t j = 0; j < i; j++) {
                zval_ptr_dtor(&out[j]);
            }
            return -1;
        }
    }
    return 0;
}

int oxphp_portable_serialize_ht(HashTable *ht,
                                unsigned char **out_buf, size_t *out_len) {
    /* Wrap the HashTable in a temporary IS_ARRAY zval and serialize as 1 zval */
    zval tmp;
    ZVAL_ARR(&tmp, ht);
    return oxphp_portable_serialize(&tmp, 1, out_buf, out_len);
}

int oxphp_portable_deserialize_ht(const unsigned char *buf, size_t len,
                                  HashTable **out_ht) {
    /* Deserialize as 1 zval, then extract the HashTable */
    zval tmp;
    ZVAL_UNDEF(&tmp);
    if (oxphp_portable_deserialize(buf, len, 1, &tmp) != 0) {
        return -1;
    }
    if (Z_TYPE(tmp) != IS_ARRAY) {
        zval_ptr_dtor(&tmp);
        return -1;
    }
    /* Separate the HashTable from the zval — caller owns it.
     * Increment refcount so the zval_ptr_dtor below doesn't free it. */
    *out_ht = Z_ARRVAL(tmp);
    GC_ADDREF(*out_ht);
    zval_ptr_dtor(&tmp);
    return 0;
}

void oxphp_portable_free(unsigned char *buf) {
    free(buf);
}

void oxphp_portable_free_ht(HashTable *ht) {
    if (ht) {
        zend_array_destroy(ht);
    }
}

/* === Async Promise: Closure Inspection === */

/* PHP 8.4: zend_closure struct is opaque — use public API only */

void *oxphp_closure_get_op_array(zval *closure) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        return NULL;
    }
    const zend_function *func = zend_get_closure_method_def(Z_OBJ_P(closure));
    if (!func || func->type != ZEND_USER_FUNCTION) {
        return NULL; /* Internal function — cannot transfer */
    }
    return (void *)&func->op_array;
}

int oxphp_closure_get_static_vars(zval *closure, HashTable **out_ht) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        *out_ht = NULL;
        return -1;
    }
    const zend_function *func = zend_get_closure_method_def(Z_OBJ_P(closure));
    if (!func || func->type != ZEND_USER_FUNCTION) {
        *out_ht = NULL;
        return -1;
    }
    *out_ht = func->op_array.static_variables;
    return 0;
}

int oxphp_closure_has_this(zval *closure) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        return 0;
    }
    zval *this_ptr = zend_get_closure_this_ptr(closure);
    return (this_ptr && Z_TYPE_P(this_ptr) != IS_UNDEF) ? 1 : 0;
}

zval *oxphp_closure_get_this(zval *closure) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        return NULL;
    }
    zval *this_ptr = zend_get_closure_this_ptr(closure);
    return (this_ptr && Z_TYPE_P(this_ptr) != IS_UNDEF) ? this_ptr : NULL;
}

/* ─── Async Exception Details ────────────────────────────── */
static __thread char *async_exc_class = NULL;
static __thread char *async_exc_msg = NULL;
static __thread char *async_exc_trace = NULL;

void oxphp_bridge_set_async_exception(const char *cls, const char *msg, const char *trace) {
    free(async_exc_class);
    free(async_exc_msg);
    free(async_exc_trace);
    async_exc_class = cls ? strdup(cls) : NULL;
    async_exc_msg = msg ? strdup(msg) : NULL;
    async_exc_trace = trace ? strdup(trace) : NULL;
}

const char *oxphp_bridge_get_async_exc_class(void) { return async_exc_class; }
const char *oxphp_bridge_get_async_exc_message(void) { return async_exc_msg; }
const char *oxphp_bridge_get_async_exc_trace(void) { return async_exc_trace; }

void oxphp_bridge_clear_async_exception(void) {
    free(async_exc_class);
    free(async_exc_msg);
    free(async_exc_trace);
    async_exc_class = NULL;
    async_exc_msg = NULL;
    async_exc_trace = NULL;
}

/* === Async Promise: Async Worker State === */

void oxphp_bridge_set_async_worker(int is_async) {
    ctx.is_async_worker = is_async;
}

int oxphp_bridge_is_async_worker(void) {
    return ctx.is_async_worker;
}

/* ─── Async Fatal Error Capture ────────────────────────────── */
/* Thread-local buffer to capture the error message from zend_error_cb
 * before zend_bailout() is called. The Rust error callback writes here
 * for fatal errors on async worker threads; the zend_catch block in
 * oxphp_execute_async_task reads and parses it. */
static __thread char *captured_fatal_msg = NULL;

void oxphp_bridge_capture_fatal(const char *msg, size_t len) {
    free(captured_fatal_msg);
    if (msg && len > 0) {
        captured_fatal_msg = strndup(msg, len);
    } else {
        captured_fatal_msg = NULL;
    }
}

char *oxphp_bridge_pop_fatal(void) {
    char *msg = captured_fatal_msg;
    captured_fatal_msg = NULL;
    return msg; /* caller owns — free with free() */
}

/* === Async Promise: Async Reset === */

#include "main/php_output.h"

void oxphp_async_reset(void) {
    /* Clear error state */
    CG(unclean_shutdown) = 0;
    if (EG(exception)) {
        zend_clear_exception();
    }

    /* Reset output buffers */
    php_output_end_all();
    php_output_deactivate();
    php_output_activate();

    /* Clear PHP error state */
    if (PG(last_error_message)) {
        zend_string_release(PG(last_error_message));
        PG(last_error_message) = NULL;
    }
    PG(last_error_type) = 0;
    PG(last_error_lineno) = 0;
    if (PG(last_error_file)) {
        zend_string_release(PG(last_error_file));
        PG(last_error_file) = NULL;
    }

    /* Reset execution timer */
    zend_set_timeout(0, 0);
}

/* === Async Promise: Execute Async Task === */

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
) {
    zval closure;
    zend_function func;

    *exc_class = NULL;
    *exc_message = NULL;
    *exc_trace = NULL;
    ZVAL_NULL(retval);

    /* Reconstruct closure from op_array + static_vars */
    memcpy(&func, op_array, sizeof(zend_op_array));
    func.op_array.static_variables = static_vars;

    zend_create_closure(&closure, &func,
        NULL, /* scope */
        NULL, /* called_scope */
        this_ptr /* this_ptr, may be NULL */
    );

    /* Set up call info */
    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
    if (zend_fcall_info_init(&closure, 0, &fci, &fcc, NULL, NULL) != SUCCESS) {
        zval_ptr_dtor(&closure);
        *exc_class = strdup("RuntimeException");
        *exc_message = strdup("Failed to initialize async closure call");
        return -1;
    }

    fci.retval = retval;
    fci.param_count = argc;
    fci.params = args;

    int result = 0;

    zend_try {
        if (zend_call_function(&fci, &fcc) != SUCCESS) {
            *exc_class = strdup("RuntimeException");
            *exc_message = strdup("Failed to call async closure");
            result = -1;
        } else if (EG(exception)) {
            /* Capture exception details */
            zend_object *ex = EG(exception);
            zend_class_entry *ce = ex->ce;
            *exc_class = strdup(ZSTR_VAL(ce->name));

            /* Get message via property read */
            zval rv;
            zval *msg_zv = zend_read_property(ce, ex, "message", sizeof("message") - 1, 1, &rv);
            if (msg_zv && Z_TYPE_P(msg_zv) == IS_STRING) {
                *exc_message = strdup(Z_STRVAL_P(msg_zv));
            } else {
                *exc_message = strdup("(unknown)");
            }

            /* Get trace string via getTraceAsString() */
            zval trace_zv;
            zend_function *trace_fn = zend_hash_str_find_ptr(
                &ce->function_table, "gettraceasstring", sizeof("gettraceasstring") - 1
            );
            if (trace_fn) {
                zend_call_known_instance_method_with_0_params(trace_fn, ex, &trace_zv);
                if (Z_TYPE(trace_zv) == IS_STRING) {
                    *exc_trace = strdup(Z_STRVAL(trace_zv));
                }
                zval_ptr_dtor(&trace_zv);
            }

            zend_clear_exception();
            result = -1;
        }
    } zend_catch {
        /* Fatal error / zend_bailout — EG(exception) is cleared by zend_exception_error
         * before bailout, but our error callback captured the formatted message. */
        char *fatal_msg = oxphp_bridge_pop_fatal();
        if (fatal_msg && strncmp(fatal_msg, "Uncaught ", 9) == 0) {
            /* Parse "Uncaught ClassName: message in /path/to/file.php:NN" */
            const char *class_start = fatal_msg + 9;
            const char *colon = strchr(class_start, ':');
            if (colon && colon > class_start) {
                *exc_class = strndup(class_start, (size_t)(colon - class_start));
                /* Skip ": " after class name */
                const char *msg_start = colon + 2;
                /* Find " in " to strip the file location */
                const char *in_pos = strstr(msg_start, " in ");
                if (in_pos) {
                    *exc_message = strndup(msg_start, (size_t)(in_pos - msg_start));
                } else {
                    *exc_message = strdup(msg_start);
                }
            } else {
                /* Uncaught but no colon — use full message */
                *exc_class = strdup("Error");
                *exc_message = strdup(fatal_msg);
            }
            free(fatal_msg);
        } else if (fatal_msg) {
            /* Non-uncaught fatal: die()/exit() or other fatal */
            *exc_class = strdup("Error");
            *exc_message = fatal_msg; /* transfer ownership */
        } else {
            *exc_class = strdup("Error");
            *exc_message = strdup("Fatal error in async closure");
        }
        CG(unclean_shutdown) = 0;
        result = -1;
    } zend_end_try();

    zval_ptr_dtor(&closure);
    return result;
}

/* === Async Promise: Borrow Proxy === */

/* CE pointer set by oxphp_sapi.c during MINIT via oxphp_bridge_set_borrow_proxy_ce() */
static zend_class_entry *borrow_proxy_ce = NULL;

void oxphp_bridge_set_borrow_proxy_ce(zend_class_entry *ce) {
    borrow_proxy_ce = ce;
}

void oxphp_create_borrow_proxy(zval *dst, uint64_t promise_id) {
    if (!borrow_proxy_ce) {
        ZVAL_NULL(dst);
        return;
    }
    object_init_ex(dst, borrow_proxy_ce);
    zend_update_property_long(borrow_proxy_ce, Z_OBJ_P(dst),
        "promiseId", sizeof("promiseId") - 1, (zend_long)promise_id);
}

/* ─── Fiber TLS Context Callbacks ──────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. Same pattern as worker/async callbacks.
 */
static oxphp_fiber_save_ctx_fn_t    rust_fiber_save_ctx    = NULL;
static oxphp_fiber_restore_ctx_fn_t rust_fiber_restore_ctx = NULL;
static oxphp_fiber_drop_ctx_fn_t    rust_fiber_drop_ctx    = NULL;

void oxphp_bridge_set_fiber_ctx_callbacks(
    oxphp_fiber_save_ctx_fn_t save_fn,
    oxphp_fiber_restore_ctx_fn_t restore_fn,
    oxphp_fiber_drop_ctx_fn_t drop_fn
) {
    rust_fiber_save_ctx    = save_fn;
    rust_fiber_restore_ctx = restore_fn;
    rust_fiber_drop_ctx    = drop_fn;
}

void oxphp_bridge_fiber_save_ctx(uint64_t fiber_id) {
    if (__builtin_expect(rust_fiber_save_ctx != NULL, 1)) {
        rust_fiber_save_ctx(fiber_id);
    }
}

void oxphp_bridge_fiber_restore_ctx(uint64_t fiber_id) {
    if (__builtin_expect(rust_fiber_restore_ctx != NULL, 1)) {
        rust_fiber_restore_ctx(fiber_id);
    }
}

void oxphp_bridge_fiber_drop_ctx(uint64_t fiber_id) {
    if (__builtin_expect(rust_fiber_drop_ctx != NULL, 1)) {
        rust_fiber_drop_ctx(fiber_id);
    }
}

/* ─── Fiber Timer Service ──────────────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. Same pattern as worker/async callbacks.
 */
static oxphp_timer_register_fn_t rust_timer_register = NULL;
static oxphp_timer_poll_fn_t     rust_timer_poll     = NULL;
static oxphp_timer_remove_fn_t   rust_timer_remove   = NULL;

void oxphp_bridge_set_timer_callbacks(
    oxphp_timer_register_fn_t reg,
    oxphp_timer_poll_fn_t poll,
    oxphp_timer_remove_fn_t rem
) {
    rust_timer_register = reg;
    rust_timer_poll     = poll;
    rust_timer_remove   = rem;
}

uint64_t oxphp_bridge_timer_register(uint64_t duration_ms) {
    if (__builtin_expect(rust_timer_register != NULL, 1)) {
        return rust_timer_register(duration_ms);
    }
    return 0;
}

uint32_t oxphp_bridge_timer_poll(uint64_t *out_ids, uint32_t max_count) {
    if (__builtin_expect(rust_timer_poll != NULL, 1)) {
        return rust_timer_poll(out_ids, max_count);
    }
    return 0;
}

void oxphp_bridge_timer_remove(uint64_t timer_id) {
    if (__builtin_expect(rust_timer_remove != NULL, 1)) {
        rust_timer_remove(timer_id);
    }
}
