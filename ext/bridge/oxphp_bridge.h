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

/** Per-request context stored in __thread TLS. */
typedef struct {
    /** Hex request ID (64 chars + null). */
    char request_id[65];

    /** Worker thread index. */
    int32_t worker_id;

    /** Request start time (Unix epoch, microseconds). */
    double request_time;

    /** Whether streaming mode is active. */
    bool stream_mode;

    /** Whether headers have been sent (streaming mode). */
    bool headers_sent;

    /** Whether oxphp_finish_request() was called. */
    bool finished;
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

/** Mark headers as sent (streaming mode). */
void oxphp_bridge_set_headers_sent(bool sent);

/** Check if headers have been sent. */
bool oxphp_bridge_get_headers_sent(void);

/* ─── Plugin Function Registry (global, NOT __thread) ─────────── */

/** Function pointer types for cross-boundary dispatch. */
typedef char* (*oxphp_dispatch_fn_t)(const char* func_name, const char* json_args);
typedef char* (*oxphp_call_php_fn_t)(const char* func_name, const char* json_args);

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

/** Set the Rust dispatch callback (called by Rust). */
void oxphp_bridge_set_dispatch_fn(oxphp_dispatch_fn_t fn);

/** Get the Rust dispatch callback (called by extension). */
oxphp_dispatch_fn_t oxphp_bridge_get_dispatch_fn(void);

/** Set the PHP call callback (called by extension MINIT). */
void oxphp_bridge_set_call_php_fn(oxphp_call_php_fn_t fn);

/** Get the PHP call callback (called by Rust). */
oxphp_call_php_fn_t oxphp_bridge_get_call_php_fn(void);

/** Dispatch to Rust handler (NULL-checks + invokes dispatch_fn). */
char* oxphp_bridge_dispatch(const char* name, const char* json_args);

/** Call a PHP function from Rust (NULL-checks + invokes call_php_fn). */
char* oxphp_bridge_call_php(const char* name, const char* json_args);

/** Duplicate a string using C malloc (for cross-boundary returns). */
char* oxphp_bridge_strdup(const char* s);

/** Free a string allocated by oxphp_bridge_strdup. */
void oxphp_bridge_free_string(char* ptr);

#ifdef __cplusplus
}
#endif

#endif /* OXPHP_BRIDGE_H */
