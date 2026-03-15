#include "php_oxphp_sapi.h"
#include "SAPI.h"
#include "oxphp_bridge.h"
#include "Zend/zend_API.h"
#include "Zend/zend_closures.h"
#include "Zend/zend_exceptions.h"
#include "main/php_output.h"
#include "main/php_main.h"
#include "ext/standard/basic_functions.h"
#include <stdlib.h>
#include <time.h>

/* Async promise exception and proxy classes */
static zend_class_entry *oxphp_async_exception_ce = NULL;
static zend_class_entry *oxphp_async_timeout_ce = NULL;
static zend_class_entry *oxphp_async_borrow_ce = NULL;
zend_class_entry *oxphp_borrowed_proxy_ce = NULL;

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
    add_assoc_bool(return_value, "worker_mode", oxphp_bridge_is_worker_mode());
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

/* {{{ oxphp_is_worker(): bool
 * Check if the current request is being handled in worker mode. */
PHP_FUNCTION(oxphp_is_worker)
{
    ZEND_PARSE_PARAMETERS_NONE();

    RETURN_BOOL(oxphp_bridge_is_worker_mode());
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
    /* 0. Clear stale engine state from previous bailout or exit/die.
     * Without this, a leftover UnwindExit exception or unclean_shutdown flag
     * would corrupt subsequent requests. */
    CG(unclean_shutdown) = 0;
    if (EG(exception)) {
        OBJ_RELEASE(EG(exception));
        EG(exception) = NULL;
    }

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
    #define WORKER_MAX_CONSECUTIVE_ERRORS 3

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

        /* 3. Call handler with zend_try protection.
         * Save execute_data so we can restore it after a bailout — longjmp
         * leaves the pointer dangling at the frame that was executing. */
        int handler_failed = 0;
        zend_execute_data *saved_execute_data = EG(current_execute_data);
        ZVAL_UNDEF(&retval);

        zend_try {
            fci.retval = &retval;
            fci.param_count = 0;
            fci.params = NULL;
            if (zend_call_function(&fci, &fcc) == SUCCESS) {
                zval_ptr_dtor(&retval);
            }
            /* PHP 8.4: exit/die throws UnwindExit exception instead of bailout.
             * zend_call_function returns SUCCESS but EG(exception) is set.
             * Clear it so the worker can continue serving requests. */
            if (EG(exception)) {
                if (zend_is_unwind_exit(EG(exception)) || zend_is_graceful_exit(EG(exception))) {
                    /* exit/die is NOT a handler failure — just ends current request */
                } else {
                    /* Unexpected lingering exception — treat as error */
                    handler_failed = 1;
                }
                OBJ_RELEASE(EG(exception));
                EG(exception) = NULL;
            }
        } zend_catch {
            /* Actual zend_bailout: fatal error, timeout, cancellation.
             * Restore execution context and clean up stale engine state. */
            handler_failed = 1;
            EG(current_execute_data) = saved_execute_data;
            if (EG(exception)) {
                OBJ_RELEASE(EG(exception));
                EG(exception) = NULL;
            }
            CG(unclean_shutdown) = 0;
        } zend_end_try();

        /* 4. Run shutdown functions (register_shutdown_function support) */
        php_call_shutdown_functions();
        php_free_shutdown_functions();

        /* 5. Capture memory usage and handler failure state before send */
        ctx->current_memory_bytes = (uint64_t)zend_memory_usage(0);
        ctx->handler_failed = handler_failed ? true : false;

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
            ctx->consecutive_errors++;
            if (ctx->consecutive_errors >= WORKER_MAX_CONSECUTIVE_ERRORS) {
                ctx->exit_reason = 3; /* too many consecutive errors */
                break;
            }
            /* Isolated error — continue serving next request */
        } else {
            ctx->consecutive_errors = 0;
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

/* ─── Async Promise PHP Functions ─────────────────────────── */

/* {{{ oxphp_async(Closure $closure, mixed ...$args): int
 * Dispatch a closure for async execution on a dedicated worker thread.
 * Returns a promise ID (int) that can be passed to oxphp_async_await(). */
PHP_FUNCTION(oxphp_async)
{
    zval *closure_zv;
    zval *args = NULL;
    uint32_t argc = 0;

    ZEND_PARSE_PARAMETERS_START(1, -1)
        Z_PARAM_OBJECT_OF_CLASS(closure_zv, zend_ce_closure)
        Z_PARAM_OPTIONAL
        Z_PARAM_VARIADIC('+', args, argc)
    ZEND_PARSE_PARAMETERS_END();

    /* Prevent nested async calls from async worker threads */
    if (oxphp_bridge_is_async_worker()) {
        zend_throw_exception(oxphp_async_exception_ce,
            "Cannot call oxphp_async() from within an async worker", 0);
        RETURN_THROWS();
    }

    /* Get op_array — must be a user function (not internal) */
    void *op_array = oxphp_closure_get_op_array(closure_zv);
    if (!op_array) {
        zend_throw_exception(oxphp_async_exception_ce,
            "Closure must be a user-defined function (not internal/built-in)", 0);
        RETURN_THROWS();
    }

    /* Get this_ptr (may be NULL for unbound closures) */
    zval *this_ptr = oxphp_closure_get_this(closure_zv);

    /* Get static_vars HashTable (captured use-vars) */
    HashTable *static_vars = NULL;
    oxphp_closure_get_static_vars(closure_zv, &static_vars);

    /* Validate: reject resources and objects in use-vars.
     * Objects cannot be serialized across threads (PDO, sockets, etc.
     * would silently become null). Resources are inherently non-portable. */
    if (static_vars) {
        zval *val;
        ZEND_HASH_FOREACH_VAL(static_vars, val) {
            zval *check = val;
            if (Z_TYPE_P(check) == IS_REFERENCE) {
                check = Z_REFVAL_P(check);
            }
            if (Z_TYPE_P(check) == IS_RESOURCE) {
                zend_throw_exception(oxphp_async_exception_ce,
                    "Cannot pass resource values in use-vars to async closure", 0);
                RETURN_THROWS();
            }
            if (Z_TYPE_P(check) == IS_OBJECT) {
                zend_throw_exception(oxphp_async_exception_ce,
                    "Cannot pass object values in use-vars to async closure"
                    " (objects cannot be serialized across threads)", 0);
                RETURN_THROWS();
            }
        } ZEND_HASH_FOREACH_END();
    }

    /* Validate: reject resources and objects in args */
    for (uint32_t i = 0; i < argc; i++) {
        zval *arg = &args[i];
        if (Z_TYPE_P(arg) == IS_REFERENCE) {
            arg = Z_REFVAL_P(arg);
        }
        if (Z_TYPE_P(arg) == IS_RESOURCE) {
            zend_throw_exception(oxphp_async_exception_ce,
                "Cannot pass resource values as arguments to async closure", 0);
            RETURN_THROWS();
        }
        if (Z_TYPE_P(arg) == IS_OBJECT) {
            zend_throw_exception(oxphp_async_exception_ce,
                "Cannot pass object values as arguments to async closure (use use-vars for object binding)", 0);
            RETURN_THROWS();
        }
    }

    /* Dispatch to Rust via bridge function pointer */
    int64_t promise_id = oxphp_bridge_async_dispatch(
        op_array, static_vars, this_ptr, argc, args, closure_zv
    );

    if (promise_id < 0) {
        zend_throw_exception(oxphp_async_exception_ce,
            "Failed to dispatch async task (pool full or not configured)", 0);
        RETURN_THROWS();
    }

    RETURN_LONG((zend_long)promise_id);
}
/* }}} */

/* {{{ oxphp_async_await(int $promise_id, float $timeout = 0.0): mixed
 * Block until an async promise completes and return its result.
 * Timeout of 0.0 means wait indefinitely.
 * Throws AsyncTimeoutException on timeout, AsyncException on error. */
PHP_FUNCTION(oxphp_async_await)
{
    zend_long promise_id;
    double timeout = 0.0;

    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_LONG(promise_id)
        Z_PARAM_OPTIONAL
        Z_PARAM_DOUBLE(timeout)
    ZEND_PARSE_PARAMETERS_END();

    int result = oxphp_bridge_await_dispatch((int64_t)promise_id, timeout, return_value);

    if (result == -2) {
        zend_throw_exception_ex(oxphp_async_timeout_ce, 0,
            "Async promise %ld timed out after %.3f seconds",
            (long)promise_id, timeout);
        RETURN_THROWS();
    } else if (result == -1) {
        const char *exc_class = oxphp_bridge_get_async_exc_class();
        const char *exc_msg = oxphp_bridge_get_async_exc_message();

        zend_string *msg;
        if (exc_msg) {
            msg = zend_strpprintf(0, "Async task failed: [%s] %s",
                exc_class ? exc_class : "Unknown", exc_msg);
        } else {
            msg = zend_strpprintf(0, "Async promise %ld failed", (long)promise_id);
        }

        zend_throw_exception(oxphp_async_exception_ce, ZSTR_VAL(msg), 0);
        zend_string_release(msg);

        oxphp_bridge_clear_async_exception();
        RETURN_THROWS();
    }
    /* return_value already populated by Rust via retval pointer */
}
/* }}} */

/* {{{ oxphp_async_await_all(array $promise_ids, float $timeout = 0.0): array
 * Await all promises and return an associative array of results keyed by promise ID.
 * Throws on the first failure or timeout encountered. */
PHP_FUNCTION(oxphp_async_await_all)
{
    zval *promises_zv;
    double timeout = 0.0;

    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_ARRAY(promises_zv)
        Z_PARAM_OPTIONAL
        Z_PARAM_DOUBLE(timeout)
    ZEND_PARSE_PARAMETERS_END();

    HashTable *ht = Z_ARRVAL_P(promises_zv);
    uint32_t count = zend_hash_num_elements(ht);

    array_init_size(return_value, count);

    zval *entry;
    ZEND_HASH_FOREACH_VAL(ht, entry) {
        if (Z_TYPE_P(entry) != IS_LONG) {
            zend_throw_exception(oxphp_async_exception_ce,
                "oxphp_async_await_all() expects an array of integer promise IDs", 0);
            zval_ptr_dtor(return_value);
            RETURN_THROWS();
        }

        zend_long pid = Z_LVAL_P(entry);
        zval result;
        ZVAL_NULL(&result);

        int status = oxphp_bridge_await_dispatch((int64_t)pid, timeout, &result);

        if (status == -2) {
            zval_ptr_dtor(&result);
            zval_ptr_dtor(return_value);
            zend_throw_exception_ex(oxphp_async_timeout_ce, 0,
                "Async promise %ld timed out after %.3f seconds",
                (long)pid, timeout);
            RETURN_THROWS();
        } else if (status == -1) {
            zval_ptr_dtor(&result);
            zval_ptr_dtor(return_value);

            const char *exc_class = oxphp_bridge_get_async_exc_class();
            const char *exc_msg = oxphp_bridge_get_async_exc_message();

            zend_string *msg;
            if (exc_msg) {
                msg = zend_strpprintf(0, "Async task failed: [%s] %s",
                    exc_class ? exc_class : "Unknown", exc_msg);
            } else {
                msg = zend_strpprintf(0, "Async promise %ld failed", (long)pid);
            }

            zend_throw_exception(oxphp_async_exception_ce, ZSTR_VAL(msg), 0);
            zend_string_release(msg);
            oxphp_bridge_clear_async_exception();
            RETURN_THROWS();
        }

        zend_hash_index_add_new(Z_ARRVAL_P(return_value), (zend_ulong)pid, &result);
    } ZEND_HASH_FOREACH_END();
}
/* }}} */

/* {{{ oxphp_async_await_any(array $promise_ids, float $timeout = 0.0): array
 * Race multiple promises and return the first to complete.
 * Returns ['id' => int, 'value' => mixed].
 * Uses futures::select_all for true race semantics — the fastest promise wins
 * regardless of array order. Non-winning promises remain awaitable individually.
 * On timeout, all specified promises are cancelled. */
PHP_FUNCTION(oxphp_async_await_any)
{
    zval *promises_zv;
    double timeout = 0.0;

    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_ARRAY(promises_zv)
        Z_PARAM_OPTIONAL
        Z_PARAM_DOUBLE(timeout)
    ZEND_PARSE_PARAMETERS_END();

    HashTable *ht = Z_ARRVAL_P(promises_zv);
    uint32_t count = zend_hash_num_elements(ht);

    if (count == 0) {
        zend_throw_exception(oxphp_async_exception_ce,
            "oxphp_async_await_any() requires at least one promise ID", 0);
        RETURN_THROWS();
    }

    /* Collect promise IDs into a C array for the bridge call */
    int64_t *pids = emalloc(sizeof(int64_t) * count);
    uint32_t idx = 0;
    zval *entry;
    ZEND_HASH_FOREACH_VAL(ht, entry) {
        if (Z_TYPE_P(entry) != IS_LONG) {
            efree(pids);
            zend_throw_exception(oxphp_async_exception_ce,
                "oxphp_async_await_any() expects an array of integer promise IDs", 0);
            RETURN_THROWS();
        }
        pids[idx++] = (int64_t)Z_LVAL_P(entry);
    } ZEND_HASH_FOREACH_END();

    int64_t winner_id = -1;
    zval result;
    ZVAL_NULL(&result);

    int status = oxphp_bridge_await_any_dispatch(pids, count, timeout, &winner_id, &result);
    efree(pids);

    if (status == -2) {
        zval_ptr_dtor(&result);
        zend_throw_exception_ex(oxphp_async_timeout_ce, 0,
            "oxphp_async_await_any() timed out after %.3f seconds waiting for %u promises",
            timeout, count);
        RETURN_THROWS();
    } else if (status == -1) {
        zval_ptr_dtor(&result);

        const char *exc_class = oxphp_bridge_get_async_exc_class();
        const char *exc_msg = oxphp_bridge_get_async_exc_message();

        zend_string *msg;
        if (exc_msg) {
            msg = zend_strpprintf(0, "Async task failed: [%s] %s",
                exc_class ? exc_class : "Unknown", exc_msg);
        } else {
            msg = zend_strpprintf(0, "Async promise %ld failed", (long)winner_id);
        }

        zend_throw_exception(oxphp_async_exception_ce, ZSTR_VAL(msg), 0);
        zend_string_release(msg);
        oxphp_bridge_clear_async_exception();
        RETURN_THROWS();
    }

    /* Return winner result */
    array_init_size(return_value, 2);
    add_assoc_long(return_value, "id", (zend_long)winner_id);
    zend_hash_str_add_new(Z_ARRVAL_P(return_value), "value", sizeof("value") - 1, &result);
}
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

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_is_worker, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_is_streaming, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_stream_flush, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_CALLABLE_INFO(0, handler, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_async, 0, 1, IS_LONG, 0)
    ZEND_ARG_OBJ_INFO(0, closure, Closure, 0)
    ZEND_ARG_VARIADIC_TYPE_INFO(0, args, IS_MIXED, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_async_await, 0, 1, IS_MIXED, 0)
    ZEND_ARG_TYPE_INFO(0, promise_id, IS_LONG, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, timeout, IS_DOUBLE, 0, "0.0")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_async_await_all, 0, 1, IS_ARRAY, 0)
    ZEND_ARG_TYPE_INFO(0, promise_ids, IS_ARRAY, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, timeout, IS_DOUBLE, 0, "0.0")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_async_await_any, 0, 1, IS_ARRAY, 0)
    ZEND_ARG_TYPE_INFO(0, promise_ids, IS_ARRAY, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, timeout, IS_DOUBLE, 0, "0.0")
ZEND_END_ARG_INFO()
/* }}} */

/* {{{ function entries */
static const zend_function_entry oxphp_sapi_functions[] = {
    PHP_FE(oxphp_request_id,        arginfo_oxphp_request_id)
    PHP_FE(oxphp_worker_id,         arginfo_oxphp_worker_id)
    PHP_FE(oxphp_server_info,       arginfo_oxphp_server_info)
    PHP_FE(oxphp_request_heartbeat, arginfo_oxphp_request_heartbeat)
    PHP_FE(oxphp_finish_request,    arginfo_oxphp_finish_request)
    PHP_FE(oxphp_is_worker,          arginfo_oxphp_is_worker)
    PHP_FE(oxphp_is_streaming,      arginfo_oxphp_is_streaming)
    PHP_FE(oxphp_stream_flush,      arginfo_oxphp_stream_flush)
    PHP_FE(oxphp_worker,            arginfo_oxphp_worker)
    PHP_FE(oxphp_async,             arginfo_oxphp_async)
    PHP_FE(oxphp_async_await,             arginfo_oxphp_async_await)
    PHP_FE(oxphp_async_await_all,         arginfo_oxphp_async_await_all)
    PHP_FE(oxphp_async_await_any,         arginfo_oxphp_async_await_any)
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

/* === BorrowedProxy — all access throws AsyncBorrowException === */

static void oxphp_borrow_throw(const char *method) {
    zend_throw_exception_ex(oxphp_async_borrow_ce, 0,
        "Cannot access borrowed object via %s — awaiting async promise", method);
}

PHP_METHOD(BorrowedProxy, __get) {
    zend_string *name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(name)
    ZEND_PARSE_PARAMETERS_END();
    oxphp_borrow_throw("__get");
}

PHP_METHOD(BorrowedProxy, __set) {
    zend_string *name;
    zval *value;
    ZEND_PARSE_PARAMETERS_START(2, 2)
        Z_PARAM_STR(name)
        Z_PARAM_ZVAL(value)
    ZEND_PARSE_PARAMETERS_END();
    oxphp_borrow_throw("__set");
}

PHP_METHOD(BorrowedProxy, __call) {
    zend_string *name;
    zval *arguments;
    ZEND_PARSE_PARAMETERS_START(2, 2)
        Z_PARAM_STR(name)
        Z_PARAM_ARRAY(arguments)
    ZEND_PARSE_PARAMETERS_END();
    oxphp_borrow_throw("__call");
}

PHP_METHOD(BorrowedProxy, __isset) {
    zend_string *name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(name)
    ZEND_PARSE_PARAMETERS_END();
    oxphp_borrow_throw("__isset");
    RETURN_FALSE;
}

PHP_METHOD(BorrowedProxy, __unset) {
    zend_string *name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(name)
    ZEND_PARSE_PARAMETERS_END();
    oxphp_borrow_throw("__unset");
}

PHP_METHOD(BorrowedProxy, __toString) {
    ZEND_PARSE_PARAMETERS_NONE();
    oxphp_borrow_throw("__toString");
    RETURN_THROWS();
}

PHP_METHOD(BorrowedProxy, __debugInfo) {
    ZEND_PARSE_PARAMETERS_NONE();
    oxphp_borrow_throw("__debugInfo");
}

PHP_METHOD(BorrowedProxy, jsonSerialize) {
    ZEND_PARSE_PARAMETERS_NONE();
    oxphp_borrow_throw("jsonSerialize");
}

/* Arginfo for BorrowedProxy methods */
ZEND_BEGIN_ARG_INFO_EX(arginfo_borrowed_proxy_get, 0, 0, 1)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_borrowed_proxy_set, 0, 0, 2)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO(0, value, IS_MIXED, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_borrowed_proxy_call, 0, 0, 2)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO(0, arguments, IS_ARRAY, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_borrowed_proxy_isset, 0, 0, 1)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_borrowed_proxy_unset, 0, 0, 1)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_borrowed_proxy_tostring, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_borrowed_proxy_debuginfo, 0, 0, IS_ARRAY, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_borrowed_proxy_jsonserialize, 0, 0, IS_MIXED, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_borrowed_proxy_methods[] = {
    PHP_ME(BorrowedProxy, __get,          arginfo_borrowed_proxy_get,           ZEND_ACC_PUBLIC)
    PHP_ME(BorrowedProxy, __set,          arginfo_borrowed_proxy_set,           ZEND_ACC_PUBLIC)
    PHP_ME(BorrowedProxy, __call,         arginfo_borrowed_proxy_call,          ZEND_ACC_PUBLIC)
    PHP_ME(BorrowedProxy, __isset,        arginfo_borrowed_proxy_isset,         ZEND_ACC_PUBLIC)
    PHP_ME(BorrowedProxy, __unset,        arginfo_borrowed_proxy_unset,         ZEND_ACC_PUBLIC)
    PHP_ME(BorrowedProxy, __toString,     arginfo_borrowed_proxy_tostring,      ZEND_ACC_PUBLIC)
    PHP_ME(BorrowedProxy, __debugInfo,    arginfo_borrowed_proxy_debuginfo,     ZEND_ACC_PUBLIC)
    PHP_ME(BorrowedProxy, jsonSerialize,  arginfo_borrowed_proxy_jsonserialize, ZEND_ACC_PUBLIC)
    PHP_FE_END
};

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

    /* Async exception classes */
    zend_class_entry ce;

    INIT_NS_CLASS_ENTRY(ce, "OxPHP", "AsyncException", NULL);
    oxphp_async_exception_ce = zend_register_internal_class_ex(&ce, zend_ce_exception);

    INIT_NS_CLASS_ENTRY(ce, "OxPHP", "AsyncTimeoutException", NULL);
    oxphp_async_timeout_ce = zend_register_internal_class_ex(&ce, oxphp_async_exception_ce);

    INIT_NS_CLASS_ENTRY(ce, "OxPHP", "AsyncBorrowException", NULL);
    oxphp_async_borrow_ce = zend_register_internal_class_ex(&ce, zend_ce_exception);

    /* BorrowedProxy class */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP", "BorrowedProxy", oxphp_borrowed_proxy_methods);
    oxphp_borrowed_proxy_ce = zend_register_internal_class(&ce);
    /* Share CE with bridge library so oxphp_create_borrow_proxy() can use it */
    oxphp_bridge_set_borrow_proxy_ce(oxphp_borrowed_proxy_ce);

    return SUCCESS;
}
/* }}} */

/* {{{ RSHUTDOWN — cleanup outstanding async promises */
PHP_RSHUTDOWN_FUNCTION(oxphp_sapi)
{
    /* Cleanup any outstanding promises not awaited by user code. */
    oxphp_bridge_cleanup_outstanding_promises();
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
    PHP_RSHUTDOWN(oxphp_sapi),   /* RSHUTDOWN */
    PHP_MINFO(oxphp_sapi),
    PHP_OXPHP_SAPI_VERSION,
    STANDARD_MODULE_PROPERTIES
};
/* }}} */

#ifdef COMPILE_DL_OXPHP_SAPI
ZEND_GET_MODULE(oxphp_sapi)
#endif
