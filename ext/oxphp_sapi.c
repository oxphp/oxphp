#include "php_oxphp_sapi.h"
#include "SAPI.h"
#include "oxphp_bridge.h"
#include "Zend/zend_API.h"
#include "main/php_output.h"
#include "main/php_main.h"
#include "ext/standard/basic_functions.h"
#include <stdlib.h>
#include <time.h>

/* {{{ oxphp_request_id(): string
 * Returns the hex request ID for the current request. */
PHP_FUNCTION(oxphp_request_id)
{
    ZEND_PARSE_PARAMETERS_NONE();

    const char *id = oxphp_bridge_get_request_id();
    if (id && id[0] != '\0') {
        RETURN_STRING(id);
    }
    RETURN_EMPTY_STRING();
}
/* }}} */

/* {{{ oxphp_worker_id(): int
 * Returns the worker thread index handling this request. */
PHP_FUNCTION(oxphp_worker_id)
{
    ZEND_PARSE_PARAMETERS_NONE();

    RETURN_LONG(oxphp_bridge_get_worker_id());
}
/* }}} */

/* {{{ oxphp_server_info(): array
 * Returns an array with SAPI information. */
PHP_FUNCTION(oxphp_server_info)
{
    ZEND_PARSE_PARAMETERS_NONE();

    array_init(return_value);
    add_assoc_string(return_value, "sapi", "oxphp");
    add_assoc_string(return_value, "version", PHP_OXPHP_SAPI_VERSION);
    add_assoc_long(return_value, "worker_id", oxphp_bridge_get_worker_id());
    add_assoc_double(return_value, "request_time", oxphp_bridge_get_request_time());
}
/* }}} */

/* {{{ oxphp_request_heartbeat(int $time = 10): bool
 * Extend the execution deadline by $time seconds from now.
 * Returns false if $time is non-positive or no deadline is set. */
PHP_FUNCTION(oxphp_request_heartbeat)
{
    zend_long time = 10;

    ZEND_PARSE_PARAMETERS_START(0, 1)
        Z_PARAM_OPTIONAL
        Z_PARAM_LONG(time)
    ZEND_PARSE_PARAMETERS_END();

    if (time <= 0) {
        RETURN_FALSE;
    }

    /* Extend deadline by $time seconds from now */
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    int64_t now_us = (int64_t)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
    oxphp_bridge_set_deadline(now_us + time * 1000000);

    RETURN_TRUE;
}
/* }}} */

/* {{{ oxphp_finish_request(): bool
 * Flush the HTTP response to the client immediately.
 * PHP continues executing for background tasks (analytics, cleanup, etc.).
 * Returns false if already called once in this request. */
PHP_FUNCTION(oxphp_finish_request)
{
    ZEND_PARSE_PARAMETERS_NONE();

    if (oxphp_bridge_is_finished()) {
        RETURN_FALSE;
    }

    /* 1. Flush all PHP output buffering layers — triggers ub_write for
     *    any buffered content, landing it in the Rust RESPONSE buffer. */
    php_output_end_all();

    /* 2. Mark the request as finished so subsequent output is discarded. */
    oxphp_bridge_set_finished(true);

    /* 3. Trigger oxphp_flush callback which calls try_early_send() in Rust,
     *    snapshotting the current response and sending via oneshot channel. */
    sapi_flush();

    RETURN_TRUE;
}
/* }}} */

/* {{{ oxphp_is_streaming(): bool
 * Check if the current request is in streaming mode. */
PHP_FUNCTION(oxphp_is_streaming)
{
    ZEND_PARSE_PARAMETERS_NONE();

    RETURN_BOOL(oxphp_bridge_is_streaming());
}
/* }}} */

/* {{{ oxphp_stream_flush(): bool
 * Activate streaming mode (if not already active), flush PHP output buffers,
 * and send buffered output as a chunk to the client.
 * The first call also sends HTTP headers to the client. */
PHP_FUNCTION(oxphp_stream_flush)
{
    ZEND_PARSE_PARAMETERS_NONE();

    /* If already finished, streaming is not possible */
    if (oxphp_bridge_is_finished()) {
        RETURN_FALSE;
    }

    /* Activate streaming mode on first call */
    if (!oxphp_bridge_is_streaming()) {
        oxphp_bridge_set_stream_mode(true);
    }

    /* Flush PHP output buffers → triggers ub_write for any buffered content */
    if (php_output_get_level() > 0) {
        php_output_flush_all();
    }

    /* Trigger SAPI flush → sends headers (first time) and chunk */
    sapi_flush();

    RETURN_TRUE;
}
/* }}} */

/* ─── Worker Mode: soft reset between requests ─────────────── */

/**
 * Reset per-request PHP state without destroying the PHP heap.
 * Called between worker mode requests to prevent response bleed.
 */
static void oxphp_soft_reset(void) {
    /* 1. Output: discard all buffers, re-activate clean.
     * Skip end_all if no output buffers exist (avoids iterating empty stack). */
    if (php_output_get_level() > 0) {
        php_output_end_all();
    }
    php_output_deactivate();
    php_output_activate();

    /* 2. SAPI headers: clear list, reset status to 200 */
    zend_llist_clean(&SG(sapi_headers).headers);
    SG(sapi_headers).http_response_code = 200;
    SG(sapi_headers).send_default_content_type = 1;
    SG(headers_sent) = 0;

    /* 3. SAPI request state: allow POST re-read and cookie refresh.
     * This replaces the heavyweight sapi_activate() — we only reset
     * the fields needed for superglobal repopulation. */
    SG(read_post_bytes) = 0;
    SG(post_read) = 0;
    SG(request_info).request_body = NULL;
    SG(request_info).post_entry = NULL;
    SG(request_info).current_user = NULL;
    SG(request_info).current_user_length = 0;
    SG(rfc1867_uploaded_files) = NULL;
    /* Cookie data for PARSE_COOKIE callback. server_context was set by
     * set_request_data() in worker_wait_callback. */
    if (SG(server_context)) {
        SG(request_info).cookie_data = sapi_module.read_cookies();
    }

    /* 4. Clear error state */
    if (PG(last_error_message)) {
        zend_string_release(PG(last_error_message));
        PG(last_error_message) = NULL;
    }
    if (PG(last_error_file)) {
        zend_string_release(PG(last_error_file));
        PG(last_error_file) = NULL;
    }
    PG(last_error_type) = 0;
    PG(last_error_lineno) = 0;
    PG(connection_status) = PHP_CONNECTION_NORMAL;

    /* 5. Reset execution timer (max_execution_time) to prevent timeout across requests */
    zend_set_timeout(EG(timeout_seconds), /* reset_signals */ 0);

    /* 6. Destroy http_globals and repopulate superglobals.
     * zval_ptr_dtor_nogc skips the cycle collector — intentional: superglobals
     * are simple string arrays that never contain cyclic refs, and _nogc avoids
     * the cycle buffer insertion overhead on every request.
     * zend_activate_auto_globals() fires non-JIT callbacks (_GET, _POST, _COOKIE, _FILES)
     * which create new PG(http_globals) entries and zend_hash_update into EG(symbol_table),
     * replacing the stale entries and releasing the old zvals properly.
     * JIT globals (_SERVER, _ENV, _REQUEST) are re-armed but only _SERVER is forced
     * here — $_ENV rarely changes between requests and $_REQUEST is a merge of
     * _GET+_POST+_COOKIE that PHP can resolve lazily on first access. */
    for (int i = 0; i < 6; i++) {
        zval_ptr_dtor_nogc(&PG(http_globals)[i]);
        ZVAL_UNDEF(&PG(http_globals)[i]);
    }
    zend_activate_auto_globals();
    zend_is_auto_global(ZSTR_KNOWN(ZEND_STR_AUTOGLOBAL_SERVER));

    /* 7. Inject REQUEST_TIME and REQUEST_TIME_FLOAT into $_SERVER.
     * In normal mode php_request_startup() does this internally, but in worker
     * mode we skip php_request_startup() per request — the soft reset rebuilds
     * $_SERVER from scratch via register_server_variables which doesn't include
     * these. Read the current request time from bridge TLS (set by
     * worker_wait_callback before this function runs). */
    {
        double rt = oxphp_bridge_get_request_time();
        zval *server = &PG(http_globals)[TRACK_VARS_SERVER];
        if (Z_TYPE_P(server) == IS_ARRAY && rt > 0.0) {
            zval zt;
            ZVAL_LONG(&zt, (zend_long)rt);
            zend_hash_str_update(Z_ARRVAL_P(server), "REQUEST_TIME", sizeof("REQUEST_TIME") - 1, &zt);

            zval zf;
            ZVAL_DOUBLE(&zf, rt);
            zend_hash_str_update(Z_ARRVAL_P(server), "REQUEST_TIME_FLOAT", sizeof("REQUEST_TIME_FLOAT") - 1, &zf);
        }
    }

    /* Note: bridge TLS reset (request_id, request_time, deadline, etc.) is handled
     * by worker_wait_callback BEFORE populating new request data, not here.
     * This ensures the soft reset only touches PHP-level state. */
}

/* {{{ oxphp_worker(callable $handler): bool
 * Enter worker mode loop. Calls $handler for each HTTP request.
 * Between requests, a soft reset cleans per-request state without
 * destroying the PHP heap (bootstrap state persists).
 * Returns true on graceful shutdown, false if not in worker mode. */
PHP_FUNCTION(oxphp_worker)
{
    zend_fcall_info fci;
    zend_fcall_info_cache fcc;

    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_FUNC(fci, fcc)
    ZEND_PARSE_PARAMETERS_END();

    oxphp_ctx_t *ctx = oxphp_bridge_get_ctx();
    if (!ctx->worker_mode) {
        php_error_docref(NULL, E_WARNING, "oxphp_worker() only available in worker mode");
        RETURN_FALSE;
    }

    /* Prevent handler closure from being GC'd during worker lifetime */
    zend_fcc_addref(&fcc);
    zval retval;

    /* GC cycle collection interval — trades p99 latency for memory.
     * Every N requests, run a full mark-and-sweep to reclaim cyclic refs. */
    #define WORKER_GC_INTERVAL 100

    while (1) {
        /* 1. Wait for next request (blocks in Rust via channel recv) */
        if (oxphp_bridge_worker_wait() != 0) {
            ctx->exit_reason = 0; /* shutdown */
            break;
        }

        /* 2. Soft reset: cleans per-request state and repopulates superglobals.
         * worker_wait_callback (Rust) already set SG(request_info) via
         * set_request_data() before returning, so soft_reset can read
         * cookies and POST data from the SAPI callbacks. */
        oxphp_soft_reset();

        /* 3. Call handler with zend_try protection */
        int handler_failed = 0;
        zend_try {
            fci.retval = &retval;
            fci.param_count = 0;
            fci.params = NULL;
            if (zend_call_function(&fci, &fcc) == SUCCESS) {
                zval_ptr_dtor(&retval);
            }
        } zend_catch {
            handler_failed = 1;
        } zend_end_try();

        /* 4. Run shutdown functions (register_shutdown_function support) */
        php_call_shutdown_functions();
        php_free_shutdown_functions();

        /* 5. Capture memory usage before send (for Rust-side metrics) */
        ctx->current_memory_bytes = (uint64_t)zend_memory_usage(0);

        /* 5b. Send response back to HTTP layer */
        oxphp_bridge_worker_send_response();

        /* 6. Track completed requests (after response sent, so limits check sees current count) */
        ctx->requests_done++;

        /* 7. GC cycle collection — periodic, not per-request.
         * Full cycle collection is expensive (~1ms+ with many objects).
         * Running every WORKER_GC_INTERVAL requests avoids p99 spikes
         * while still preventing cycle leaks in long-lived workers. */
        if (ctx->requests_done % WORKER_GC_INTERVAL == 0) {
            gc_collect_cycles();
        }

        /* 8. Check limits — set exit_reason before breaking */
        if (handler_failed) {
            ctx->exit_reason = 3; /* error */
            break;
        }
        if (ctx->max_requests > 0 && ctx->requests_done >= ctx->max_requests) {
            ctx->exit_reason = 1; /* max_requests */
            break;
        }
        if (ctx->max_memory_bytes > 0 && zend_memory_usage(0) > ctx->max_memory_bytes) {
            ctx->exit_reason = 2; /* max_memory */
            break;
        }
    }

    zend_fcc_dtor(&fcc);
    RETURN_TRUE;
}
/* }}} */

/* ─── Native plugin function dispatch ─────────────────────── */

/* {{{ oxphp_native_dispatch — zero-serialization handler for plugin functions.
 * Gets raw zval pointers and passes them directly to Rust via the native bridge.
 * No JSON encode/decode — Rust reads/writes zvals through C accessor functions. */
ZEND_FUNCTION(oxphp_native_dispatch)
{
    /* Get the function name from the Zend execute_data */
    const char *func_name = ZSTR_VAL(execute_data->func->common.function_name);

    /* Get raw args pointer — zvals start at ZEND_CALL_ARG position 1 */
    uint32_t argc = ZEND_NUM_ARGS();
    zval *args = (argc > 0) ? ZEND_CALL_ARG(execute_data, 1) : NULL;

    /* Dispatch to Rust via native bridge */
    oxphp_native_dispatch_fn_t dispatch = oxphp_bridge_get_native_dispatch();
    if (!dispatch) {
        php_error_docref(NULL, E_WARNING, "oxphp: native dispatch not set for %s", func_name);
        RETURN_NULL();
    }

    int rc = dispatch(func_name, args, argc, return_value);
    if (rc != 0) {
        php_error_docref(NULL, E_WARNING, "oxphp: dispatch failed for %s", func_name);
        /* return_value may have been partially written — reset to null on error */
        zval_ptr_dtor(return_value);
        ZVAL_NULL(return_value);
    }
}
/* }}} */

/* {{{ arginfo for native plugin dispatch (variadic mixed) */
ZEND_BEGIN_ARG_INFO_EX(arginfo_oxphp_native_dispatch, 0, 0, 0)
    ZEND_ARG_VARIADIC_INFO(0, args)
ZEND_END_ARG_INFO()
/* }}} */

/* {{{ arginfo */
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_request_id, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_id, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_server_info, 0, 0, IS_ARRAY, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_request_heartbeat, 0, 0, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, time, IS_LONG, 0, "10")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_finish_request, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_is_streaming, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_stream_flush, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_CALLABLE_INFO(0, handler, 0)
ZEND_END_ARG_INFO()
/* }}} */

/* {{{ function entries */
static const zend_function_entry oxphp_sapi_functions[] = {
    PHP_FE(oxphp_request_id,        arginfo_oxphp_request_id)
    PHP_FE(oxphp_worker_id,         arginfo_oxphp_worker_id)
    PHP_FE(oxphp_server_info,       arginfo_oxphp_server_info)
    PHP_FE(oxphp_request_heartbeat, arginfo_oxphp_request_heartbeat)
    PHP_FE(oxphp_finish_request,    arginfo_oxphp_finish_request)
    PHP_FE(oxphp_is_streaming,      arginfo_oxphp_is_streaming)
    PHP_FE(oxphp_stream_flush,      arginfo_oxphp_stream_flush)
    PHP_FE(oxphp_worker,            arginfo_oxphp_worker)
    PHP_FE_END
};
/* }}} */

/* {{{ module info */
PHP_MINFO_FUNCTION(oxphp_sapi)
{
    php_info_print_table_start();
    php_info_print_table_header(2, "OxPHP SAPI Extension", "enabled");
    php_info_print_table_row(2, "Version", PHP_OXPHP_SAPI_VERSION);
    php_info_print_table_end();
}
/* }}} */

/* {{{ MINIT — register plugin functions with native dispatch handler.
 * Plugin functions must be registered here (not RINIT) so OPcache's
 * compile-time optimization of function_exists('literal') can see them. */
PHP_MINIT_FUNCTION(oxphp_sapi)
{
    /* Register plugin functions (populated by Rust before php_module_startup) */
    int count = oxphp_bridge_get_plugin_fn_count();
    if (count > 0) {
        /* Use calloc (not ecalloc) — MINIT is module-level, not request-level. */
        zend_function_entry *entries = calloc(count + 1, sizeof(zend_function_entry));
        if (entries) {
            for (int i = 0; i < count; i++) {
                entries[i].fname = oxphp_bridge_get_plugin_fn_name(i);
                entries[i].handler = ZEND_FN(oxphp_native_dispatch);
                entries[i].arg_info = (const zend_internal_arg_info *)arginfo_oxphp_native_dispatch;
                entries[i].num_args = (uint32_t)oxphp_bridge_get_plugin_fn_total(i);
                entries[i].flags = 0;
            }
            /* Sentinel: last entry is all-zeroes (from calloc). */
            zend_register_functions(NULL, entries, NULL, MODULE_PERSISTENT);
            free(entries);
        }
    }

    return SUCCESS;
}
/* }}} */

/* {{{ module entry */
zend_module_entry oxphp_sapi_module_entry = {
    STANDARD_MODULE_HEADER,
    PHP_OXPHP_SAPI_EXTNAME,
    oxphp_sapi_functions,
    PHP_MINIT(oxphp_sapi),
    NULL,   /* MSHUTDOWN */
    NULL,   /* RINIT */
    NULL,   /* RSHUTDOWN */
    PHP_MINFO(oxphp_sapi),
    PHP_OXPHP_SAPI_VERSION,
    STANDARD_MODULE_PROPERTIES
};
/* }}} */

#ifdef COMPILE_DL_OXPHP_SAPI
ZEND_GET_MODULE(oxphp_sapi)
#endif
