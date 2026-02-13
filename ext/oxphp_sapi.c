#include "php_oxphp_sapi.h"
#include "oxphp_bridge.h"
#include "ext/json/php_json.h"
#include "Zend/zend_API.h"
#include "zend_smart_str.h"
#include <stdlib.h>

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
 * Placeholder for timeout extension. Currently a no-op that returns true. */
PHP_FUNCTION(oxphp_request_heartbeat)
{
    zend_long time = 10;

    ZEND_PARSE_PARAMETERS_START(0, 1)
        Z_PARAM_OPTIONAL
        Z_PARAM_LONG(time)
    ZEND_PARSE_PARAMETERS_END();

    /* TODO: integrate with watchdog timer in future phase */
    RETURN_TRUE;
}
/* }}} */

/* {{{ oxphp_finish_request(): bool
 * Mark the request as finished — allows background processing. */
PHP_FUNCTION(oxphp_finish_request)
{
    ZEND_PARSE_PARAMETERS_NONE();

    if (oxphp_bridge_is_finished()) {
        RETURN_FALSE;
    }

    oxphp_bridge_set_finished(true);
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

/* ─── Plugin function dispatch ─────────────────────────────── */

/* {{{ oxphp_plugin_dispatch — generic handler for all plugin-registered functions */
ZEND_FUNCTION(oxphp_plugin_dispatch)
{
    /* Get the function name from the Zend execute_data */
    const char *func_name = ZSTR_VAL(execute_data->func->common.function_name);

    /* Collect all arguments into a PHP array, then json_encode */
    uint32_t argc = ZEND_NUM_ARGS();
    zval *args = NULL;
    zval args_array;

    if (argc > 0) {
        args = (zval *)safe_emalloc(argc, sizeof(zval), 0);
        if (zend_get_parameters_array_ex(argc, args) == FAILURE) {
            efree(args);
            RETURN_NULL();
        }
    }

    array_init_size(&args_array, argc);
    for (uint32_t i = 0; i < argc; i++) {
        zval copy;
        ZVAL_COPY(&copy, &args[i]);
        zend_hash_next_index_insert(Z_ARRVAL(args_array), &copy);
    }
    if (args) {
        efree(args);
    }

    /* json_encode the args array */
    smart_str json_args_buf = {0};
    if (php_json_encode(&json_args_buf, &args_array, 0) == FAILURE) {
        smart_str_free(&json_args_buf);
        zval_ptr_dtor(&args_array);
        php_error_docref(NULL, E_WARNING, "oxphp: failed to encode arguments for %s", func_name);
        RETURN_NULL();
    }
    smart_str_0(&json_args_buf);
    zval_ptr_dtor(&args_array);

    if (!json_args_buf.s) {
        php_error_docref(NULL, E_WARNING, "oxphp: empty args buffer for %s", func_name);
        RETURN_NULL();
    }

    /* Dispatch to Rust via bridge */
    char *result_json = oxphp_bridge_dispatch(func_name, ZSTR_VAL(json_args_buf.s));
    smart_str_free(&json_args_buf);

    if (!result_json) {
        php_error_docref(NULL, E_WARNING, "oxphp: dispatch returned NULL for %s", func_name);
        RETURN_NULL();
    }

    /* Parse the JSON envelope: {"ok": value} or {"err": "message"} */
    zval envelope;
    php_json_decode_ex(&envelope, result_json, strlen(result_json), PHP_JSON_OBJECT_AS_ARRAY, 512);
    oxphp_bridge_free_string(result_json);

    if (Z_TYPE(envelope) != IS_ARRAY) {
        php_error_docref(NULL, E_WARNING, "oxphp: invalid dispatch result for %s", func_name);
        zval_ptr_dtor(&envelope);
        RETURN_NULL();
    }

    /* Check for error */
    zval *err_val = zend_hash_str_find(Z_ARRVAL(envelope), "err", 3);
    if (err_val) {
        if (Z_TYPE_P(err_val) == IS_STRING) {
            php_error_docref(NULL, E_WARNING, "oxphp %s: %s", func_name, Z_STRVAL_P(err_val));
        }
        zval_ptr_dtor(&envelope);
        RETURN_NULL();
    }

    /* Extract "ok" value */
    zval *ok_val = zend_hash_str_find(Z_ARRVAL(envelope), "ok", 2);
    if (ok_val) {
        ZVAL_COPY(return_value, ok_val);
    }
    zval_ptr_dtor(&envelope);
}
/* }}} */

/* {{{ oxphp_sapi_call_php — called from Rust via bridge to invoke a PHP function */
static char* oxphp_sapi_call_php(const char* func_name, const char* json_args)
{
    if (!func_name || !json_args) {
        return oxphp_bridge_strdup("{\"err\":\"NULL argument to call_php\"}");
    }

    zval fname, retval;
    ZVAL_STRING(&fname, func_name);

    /* Decode json_args → PHP array of arguments */
    zval decoded_args;
    php_json_decode_ex(&decoded_args, json_args, strlen(json_args), PHP_JSON_OBJECT_AS_ARRAY, 512);

    uint32_t argc = 0;
    zval *argv = NULL;

    if (Z_TYPE(decoded_args) == IS_ARRAY) {
        argc = zend_hash_num_elements(Z_ARRVAL(decoded_args));
        if (argc > 0) {
            argv = (zval *)safe_emalloc(argc, sizeof(zval), 0);
            uint32_t i = 0;
            zval *val;
            ZEND_HASH_FOREACH_VAL(Z_ARRVAL(decoded_args), val) {
                ZVAL_COPY(&argv[i], val);
                i++;
            } ZEND_HASH_FOREACH_END();
        }
    }

    /* Call the PHP function */
    ZVAL_UNDEF(&retval);
    int call_result = call_user_function(CG(function_table), NULL, &fname, &retval, argc, argv);

    /* Clean up args */
    for (uint32_t i = 0; i < argc; i++) {
        zval_ptr_dtor(&argv[i]);
    }
    if (argv) {
        efree(argv);
    }
    zval_ptr_dtor(&decoded_args);
    zval_ptr_dtor(&fname);

    /* Build JSON envelope */
    smart_str result_buf = {0};

    if (call_result == FAILURE) {
        /* Use json_encode for the error message to avoid JSON injection from func_name */
        zval err_msg;
        char err_buf[256];
        snprintf(err_buf, sizeof(err_buf), "call_user_function failed for %.200s", func_name);
        ZVAL_STRING(&err_msg, err_buf);
        smart_str_appends(&result_buf, "{\"err\":");
        php_json_encode(&result_buf, &err_msg, 0);
        smart_str_appendc(&result_buf, '}');
        zval_ptr_dtor(&err_msg);
    } else {
        /* Wrap result: {"ok": json_encode(retval)} */
        smart_str_appends(&result_buf, "{\"ok\":");
        smart_str val_buf = {0};
        php_json_encode(&val_buf, &retval, 0);
        smart_str_0(&val_buf);
        if (val_buf.s) {
            smart_str_append(&result_buf, val_buf.s);
        } else {
            smart_str_appends(&result_buf, "null");
        }
        smart_str_free(&val_buf);
        smart_str_appendc(&result_buf, '}');
    }

    zval_ptr_dtor(&retval);
    smart_str_0(&result_buf);

    if (!result_buf.s) {
        return oxphp_bridge_strdup("{\"err\":\"out of memory\"}");
    }
    char *out = oxphp_bridge_strdup(ZSTR_VAL(result_buf.s));
    smart_str_free(&result_buf);
    return out;
}
/* }}} */

/* {{{ arginfo for generic plugin dispatch (variadic mixed) */
ZEND_BEGIN_ARG_INFO_EX(arginfo_oxphp_plugin_dispatch, 0, 0, 0)
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
/* }}} */

/* {{{ function entries */
static const zend_function_entry oxphp_sapi_functions[] = {
    PHP_FE(oxphp_request_id,        arginfo_oxphp_request_id)
    PHP_FE(oxphp_worker_id,         arginfo_oxphp_worker_id)
    PHP_FE(oxphp_server_info,       arginfo_oxphp_server_info)
    PHP_FE(oxphp_request_heartbeat, arginfo_oxphp_request_heartbeat)
    PHP_FE(oxphp_finish_request,    arginfo_oxphp_finish_request)
    PHP_FE(oxphp_is_streaming,      arginfo_oxphp_is_streaming)
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

/* {{{ MINIT — set call_php callback + register plugin functions.
 * Plugin functions must be registered here (not RINIT) so OPcache's
 * compile-time optimization of function_exists('literal') can see them. */
PHP_MINIT_FUNCTION(oxphp_sapi)
{
    oxphp_bridge_set_call_php_fn(oxphp_sapi_call_php);

    /* Register plugin functions (populated by Rust before php_module_startup) */
    int count = oxphp_bridge_get_plugin_fn_count();
    if (count > 0) {
        /* Use calloc (not ecalloc) — MINIT is module-level, not request-level. */
        zend_function_entry *entries = calloc(count + 1, sizeof(zend_function_entry));
        if (entries) {
            for (int i = 0; i < count; i++) {
                entries[i].fname = oxphp_bridge_get_plugin_fn_name(i);
                entries[i].handler = ZEND_FN(oxphp_plugin_dispatch);
                entries[i].arg_info = (const zend_internal_arg_info *)arginfo_oxphp_plugin_dispatch;
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
    NULL,   /* RINIT — plugin functions registered in MINIT */
    NULL,   /* RSHUTDOWN */
    PHP_MINFO(oxphp_sapi),
    PHP_OXPHP_SAPI_VERSION,
    STANDARD_MODULE_PROPERTIES
};
/* }}} */

#ifdef COMPILE_DL_OXPHP_SAPI
ZEND_GET_MODULE(oxphp_sapi)
#endif
