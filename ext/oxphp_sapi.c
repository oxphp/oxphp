#include "php_oxphp_sapi.h"
#include "SAPI.h"
#include "oxphp_bridge.h"
#include "Zend/zend_API.h"
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
