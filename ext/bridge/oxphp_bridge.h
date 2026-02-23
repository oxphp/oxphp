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

    /** Current PHP heap usage in bytes (updated after each request). */
    uint64_t current_memory_bytes;
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

/* ── Zval lifecycle ── */

/** Destroy a zval (decrement refcount, free if needed). */
void oxphp_zval_dtor(void *zv);

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

/* ─── Worker Mode ─────────────────────────────────────────── */

/** Rust callback: blocks until next request arrives, returns 0 on success, -1 on shutdown. */
typedef int (*oxphp_worker_wait_fn_t)(void);

/** Rust callback: sends current response back to HTTP layer, returns 0 on success. */
typedef int (*oxphp_worker_send_fn_t)(void);

/** Register Rust worker callbacks (called once at init). */
void oxphp_bridge_set_worker_callbacks(oxphp_worker_wait_fn_t wait_fn, oxphp_worker_send_fn_t send_fn);

/** Set worker mode TLS flags for this thread. */
void oxphp_bridge_set_worker_mode(uint64_t max_requests, uint64_t max_memory_mb);

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

#ifdef __cplusplus
}
#endif

#endif /* OXPHP_BRIDGE_H */
