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
    /* Set a non-NULL server_context — PHP checks this in sapi_activate()
     * to decide whether to read POST data and cookies. Without it,
     * $_POST/$_FILES/$_COOKIE are never populated. */
    SG(server_context) = (void*)(method ? 1 : 0);
    SG(request_info).request_method = method;
    SG(request_info).query_string = (char*)query_string;
    SG(request_info).content_type = content_type;
    SG(request_info).content_length = content_length;
}

/* ── Zval lifecycle ── */

void oxphp_zval_dtor(void *zv) {
    zval_ptr_dtor((zval*)zv);
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
