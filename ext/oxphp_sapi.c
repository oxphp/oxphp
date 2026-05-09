#include "php_oxphp_sapi.h"
#include "SAPI.h"
#include "oxphp_bridge.h"
#include "oxphp_fiber.h"
#include "Zend/zend_API.h"
#include "Zend/zend_closures.h"
#include "Zend/zend_exceptions.h"
#include "Zend/zend_observer.h"
#include "Zend/zend_attributes.h"
#include "Zend/zend_interfaces.h"
#include "Zend/zend_enum.h"
#include "main/php_output.h"
#include "main/php_main.h"
#include "ext/standard/basic_functions.h"
#include "ext/json/php_json.h"
#include "ext/spl/spl_exceptions.h"
#include "ext/session/php_session.h"
#include <limits.h>
#include <stdlib.h>
#include <time.h>

/* HTTP Request class */
static zend_class_entry *oxphp_http_request_ce = NULL;

/* HTTP Object API exception classes */
static zend_class_entry *oxphp_no_active_request_ce = NULL;
static zend_class_entry *oxphp_async_context_exc_ce = NULL;
static zend_class_entry *oxphp_worker_idle_exc_ce = NULL;
static zend_class_entry *oxphp_invalid_serve_ctx_exc_ce = NULL;

/* OxPHP\Server\Worker class */
static zend_class_entry *oxphp_worker_ce = NULL;
static zend_object_handlers oxphp_worker_object_handlers;

/* Per-thread cached singleton zval. Lazily allocated on first
 * Worker::current() call; never explicitly freed (process lifetime,
 * bounded leak ~32 bytes × N worker threads). */
static __thread zval *oxphp_worker_singleton = NULL;

/* Per-thread re-entry sentinel for Worker::serve(). */
static __thread bool oxphp_serve_in_progress = false;

/* HTTP Object API supporting classes */
static zend_class_entry *oxphp_http_session_ce = NULL;
static zend_class_entry *oxphp_http_attributes_ce = NULL;
static zend_class_entry *oxphp_http_uploaded_file_ce = NULL;

/* Decorator system class entries */
static zend_class_entry *oxphp_decorator_interface_ce = NULL;
static zend_class_entry *oxphp_decorator_context_ce = NULL;
static zend_class_entry *oxphp_decorator_rejected_ce = NULL;

/* HTTP Interface class entries */
static zend_class_entry *oxphp_http_request_iface_ce = NULL;
static zend_class_entry *oxphp_http_session_iface_ce = NULL;
static zend_class_entry *oxphp_http_uploaded_file_iface_ce = NULL;
static zend_class_entry *oxphp_http_attributes_iface_ce = NULL;

/* Custom object handlers to block cloning */
static zend_object_handlers oxphp_http_request_handlers;
static zend_object_handlers oxphp_http_session_handlers;
static zend_object_handlers oxphp_http_uploaded_file_handlers;
static zend_object_handlers oxphp_http_attributes_handlers;
static zend_object_handlers oxphp_decorator_context_handlers;

/* Decorator instance cache (TLS).
 * HashTable keyed by a composite (fn_id, attr_index) key so that decorator
 * instances never collide across functions. Cleared in RSHUTDOWN. */
static __thread HashTable decorator_instance_cache_ht;
static __thread int decorator_instance_cache_initialized = 0;

static inline zend_ulong oxphp_dec_cache_key(const void *fn_id, uint32_t attr_index) {
    /* Pack fn pointer with attr index into a single hash key.
     * Shift pointer by 8 (functions are 8-byte aligned, discard low zeros)
     * and store attr_index in the low bits (cap 255 decorators per function). */
    return ((zend_ulong)(uintptr_t)fn_id << 8) | (attr_index & 0xFFu);
}

static inline void oxphp_dec_cache_ensure_init(void) {
    if (!decorator_instance_cache_initialized) {
        zend_hash_init(&decorator_instance_cache_ht, 16, NULL, ZVAL_PTR_DTOR, 0);
        decorator_instance_cache_initialized = 1;
    }
}

/* Force the VM to dispatch the exception handler on the NEXT opcode of the
 * currently-active frame instead of executing its body. zend_throw_exception()
 * only patches the frame that was active when it was called — when we throw
 * from inside an observer's before() dispatch, that frame is the before()
 * call, NOT the frame we're about to enter. Replicate the opline swap here
 * for the correct frame so RejectedException actually aborts dispatch. */
static inline void oxphp_force_exception_on_current_frame(void) {
    if (EG(current_execute_data) &&
        EG(current_execute_data)->opline &&
        EG(current_execute_data)->opline != EG(exception_op)) {
        EG(opline_before_exception) = EG(current_execute_data)->opline;
        EG(current_execute_data)->opline = EG(exception_op);
    }
}

/* Forward declarations for observer functions */
static zend_observer_fcall_handlers oxphp_decorator_observer_init(zend_execute_data *execute_data);
static void oxphp_decorator_begin(zend_execute_data *execute_data);
static void oxphp_decorator_end(zend_execute_data *execute_data, zval *retval);

/* Tick observer: increments the per-worker heartbeat tick counter on
 * every PHP function call. Used by the supervisor to distinguish
 * "stuck inside C extension" (cpu>0, ticks==0) from "PHP loop making
 * progress" (cpu>0, ticks>0). The fast path is `oxphp_bridge_tick`,
 * an inline atomic_fetch_add on a per-thread pointer. */
static void oxphp_tick_observer_begin(zend_execute_data *execute_data)
{
    (void)execute_data;
    oxphp_bridge_tick();
}

static zend_observer_fcall_handlers
oxphp_tick_observer_init(zend_execute_data *execute_data)
{
    (void)execute_data;
    return (zend_observer_fcall_handlers){
        .begin = oxphp_tick_observer_begin,
        .end   = NULL,
    };
}

/* Profiler observer init — defined in ext/bridge/oxphp_bridge.c.
 * Registered globally at MINIT alongside the decorator observer;
 * multiple registrations are merged by the Zend Observer API. */
extern zend_observer_fcall_handlers
oxphp_profiler_observer_init(zend_execute_data *execute_data);

/* ═══════════════════════════════════════════════════════════════
 *  OxPHP\Http\Request — ZEND_METHOD implementations
 * ═══════════════════════════════════════════════════════════════ */

/* ─── URI methods ─────────────────────────────────────────── */

/* {{{ OxPHP\Http\Request::method(): string */
ZEND_METHOD(OxPHP_Http_Request, method) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_method(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_EMPTY_STRING();
}
/* }}} */

/* {{{ OxPHP\Http\Request::path(): string */
ZEND_METHOD(OxPHP_Http_Request, path) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_path(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_STRING("/");
}
/* }}} */

/* {{{ OxPHP\Http\Request::fullUri(): string */
ZEND_METHOD(OxPHP_Http_Request, fullUri) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_full_uri(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_EMPTY_STRING();
}
/* }}} */

/* {{{ OxPHP\Http\Request::scheme(): string */
ZEND_METHOD(OxPHP_Http_Request, scheme) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_scheme(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_STRING("http");
}
/* }}} */

/* {{{ OxPHP\Http\Request::host(): string */
ZEND_METHOD(OxPHP_Http_Request, host) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_host(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_EMPTY_STRING();
}
/* }}} */

/* {{{ OxPHP\Http\Request::port(): int */
ZEND_METHOD(OxPHP_Http_Request, port) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_LONG((zend_long)oxphp_req_port());
}
/* }}} */

/* {{{ OxPHP\Http\Request::queryString(): ?string */
ZEND_METHOD(OxPHP_Http_Request, queryString) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_query_string(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_NULL();
}
/* }}} */

/* {{{ OxPHP\Http\Request::isSecure(): bool */
ZEND_METHOD(OxPHP_Http_Request, isSecure) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_BOOL(oxphp_req_is_secure());
}
/* }}} */

/* {{{ OxPHP\Http\Request::isMethod(string $method): bool */
ZEND_METHOD(OxPHP_Http_Request, isMethod) {
    zend_string *method;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(method)
    ZEND_PARSE_PARAMETERS_END();

    size_t len = 0;
    const char *actual = oxphp_req_method(&len);
    if (actual && len == ZSTR_LEN(method)) {
        RETURN_BOOL(strncasecmp(actual, ZSTR_VAL(method), len) == 0);
    }
    RETURN_FALSE;
}
/* }}} */

/* ─── Protocol methods ────────────────────────────────────── */

/* {{{ OxPHP\Http\Request::httpProtocol(): string */
ZEND_METHOD(OxPHP_Http_Request, httpProtocol) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *ver = oxphp_req_protocol_version(&len);
    /* Build "HTTP/X.Y" */
    char buf[16];
    int n = snprintf(buf, sizeof(buf), "HTTP/%.*s", (int)len, ver ? ver : "1.1");
    RETURN_STRINGL(buf, n);
}
/* }}} */

/* {{{ OxPHP\Http\Request::httpProtocolVersion(): string */
ZEND_METHOD(OxPHP_Http_Request, httpProtocolVersion) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_protocol_version(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_STRING("1.1");
}
/* }}} */

/* ─── Header methods ──────────────────────────────────────── */

/* {{{ OxPHP\Http\Request::header(string $name, ?string $default = null): ?string */
ZEND_METHOD(OxPHP_Http_Request, header) {
    zend_string *name;
    zval *def = NULL;
    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_STR(name)
        Z_PARAM_OPTIONAL
        Z_PARAM_ZVAL_OR_NULL(def)
    ZEND_PARSE_PARAMETERS_END();

    size_t len = 0;
    const char *val = oxphp_req_header(ZSTR_VAL(name), ZSTR_LEN(name), &len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    if (def) {
        RETURN_COPY(def);
    }
    RETURN_NULL();
}
/* }}} */

/* Visitor callback for building PHP array from key-value pairs */
static void headers_visitor(const char *key, size_t klen,
                            const char *val, size_t vlen, void *user_data) {
    zval *arr = (zval *)user_data;
    add_assoc_stringl_ex(arr, key, klen, val, vlen);
}

/* {{{ OxPHP\Http\Request::headers(): array */
ZEND_METHOD(OxPHP_Http_Request, headers) {
    ZEND_PARSE_PARAMETERS_NONE();
    array_init(return_value);
    oxphp_req_headers_all(headers_visitor, return_value);
}
/* }}} */

/* {{{ OxPHP\Http\Request::hasHeader(string $name): bool */
ZEND_METHOD(OxPHP_Http_Request, hasHeader) {
    zend_string *name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(name)
    ZEND_PARSE_PARAMETERS_END();

    size_t len = 0;
    const char *val = oxphp_req_header(ZSTR_VAL(name), ZSTR_LEN(name), &len);
    RETURN_BOOL(val != NULL);
}
/* }}} */

/* ─── Cookie methods ──────────────────────────────────────── */

/* {{{ OxPHP\Http\Request::cookie(string $name, ?string $default = null): ?string */
ZEND_METHOD(OxPHP_Http_Request, cookie) {
    zend_string *name;
    zval *def = NULL;
    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_STR(name)
        Z_PARAM_OPTIONAL
        Z_PARAM_ZVAL_OR_NULL(def)
    ZEND_PARSE_PARAMETERS_END();

    size_t len = 0;
    const char *val = oxphp_req_cookie(ZSTR_VAL(name), ZSTR_LEN(name), &len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    if (def) {
        RETURN_COPY(def);
    }
    RETURN_NULL();
}
/* }}} */

/* {{{ OxPHP\Http\Request::cookies(): array */
ZEND_METHOD(OxPHP_Http_Request, cookies) {
    ZEND_PARSE_PARAMETERS_NONE();
    array_init(return_value);
    oxphp_req_cookies_all(headers_visitor, return_value);  /* reuse visitor */
}
/* }}} */

/* ─── Body methods ────────────────────────────────────────── */

/* {{{ OxPHP\Http\Request::body(): string */
ZEND_METHOD(OxPHP_Http_Request, body) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const uint8_t *data = oxphp_req_body(&len);
    if (data && len > 0) {
        RETURN_STRINGL((const char *)data, len);
    }
    RETURN_EMPTY_STRING();
}
/* }}} */

/* {{{ OxPHP\Http\Request::contentType(): ?string */
ZEND_METHOD(OxPHP_Http_Request, contentType) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_content_type(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_NULL();
}
/* }}} */

/* ─── Client / timing methods ─────────────────────────────── */

/* {{{ OxPHP\Http\Request::ip(): string */
ZEND_METHOD(OxPHP_Http_Request, ip) {
    ZEND_PARSE_PARAMETERS_NONE();
    size_t len = 0;
    const char *val = oxphp_req_ip(&len);
    if (val && len > 0) {
        RETURN_STRINGL(val, len);
    }
    RETURN_STRING("0.0.0.0");
}
/* }}} */

/* {{{ OxPHP\Http\Request::startTime(bool $asFloat = false): int|float */
ZEND_METHOD(OxPHP_Http_Request, startTime) {
    bool as_float = false;
    ZEND_PARSE_PARAMETERS_START(0, 1)
        Z_PARAM_OPTIONAL
        Z_PARAM_BOOL(as_float)
    ZEND_PARSE_PARAMETERS_END();

    double t = oxphp_req_start_time();
    if (as_float) {
        RETURN_DOUBLE(t);
    }
    RETURN_LONG((zend_long)t);
}
/* }}} */

/* ─── Query methods ───────────────────────────────────────── */

/* {{{ OxPHP\Http\Request::query(?string $key = null, mixed $default = null): mixed
 * When called with a key, does a single bridge lookup (fast path).
 * When called without arguments, builds the full array via visitor.
 * Bracket-notation is not yet supported in the initial version. */
ZEND_METHOD(OxPHP_Http_Request, query) {
    zend_string *key = NULL;
    zval *def = NULL;
    ZEND_PARSE_PARAMETERS_START(0, 2)
        Z_PARAM_OPTIONAL
        Z_PARAM_STR_OR_NULL(key)
        Z_PARAM_ZVAL_OR_NULL(def)
    ZEND_PARSE_PARAMETERS_END();

    if (key) {
        /* Single key lookup — direct bridge call, no array build */
        size_t len = 0;
        const char *val = oxphp_req_query_param(ZSTR_VAL(key), ZSTR_LEN(key), &len);
        if (val) {
            RETURN_STRINGL(val, len);
        }
        if (def) {
            RETURN_COPY(def);
        }
        RETURN_NULL();
    }

    /* No key — return full parsed query array.
     * Check if we have a cached version in the object property. */
    zval *cached = zend_read_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
        "_query_cache", sizeof("_query_cache")-1, 1, NULL);
    if (cached && Z_TYPE_P(cached) == IS_ARRAY) {
        RETURN_COPY(cached);
    }

    /* Build array from flat pairs via bridge visitor. */
    array_init(return_value);
    oxphp_req_query_params_all(headers_visitor, return_value);

    /* Cache on the object for subsequent calls */
    zend_update_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
        "_query_cache", sizeof("_query_cache")-1, return_value);
}
/* }}} */

/* ─── Payload method ──────────────────────────────────────── */

/* {{{ OxPHP\Http\Request::payload(?string $key = null, mixed $default = null): mixed
 *
 * Cache sentinel: IS_FALSE means "already parsed, body was empty or unsupported
 * content-type" — avoids re-parsing on every call.  IS_NULL means "not yet
 * parsed".  IS_ARRAY is the successful decode result.
 */
ZEND_METHOD(OxPHP_Http_Request, payload) {
    zend_string *key = NULL;
    zval *def = NULL;
    ZEND_PARSE_PARAMETERS_START(0, 2)
        Z_PARAM_OPTIONAL
        Z_PARAM_STR_OR_NULL(key)
        Z_PARAM_ZVAL_OR_NULL(def)
    ZEND_PARSE_PARAMETERS_END();

    /* Check cached payload (IS_NULL = not yet parsed) */
    zval *cached = zend_read_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
        "_payload_cache", sizeof("_payload_cache")-1, 1, NULL);

    if (!cached || Z_TYPE_P(cached) == IS_UNDEF || Z_TYPE_P(cached) == IS_NULL) {
        /* Parse body based on Content-Type */
        size_t ct_len = 0;
        const char *ct = oxphp_req_content_type(&ct_len);
        size_t body_len = 0;
        const uint8_t *body_data = oxphp_req_body(&body_len);

        zval parsed;
        ZVAL_FALSE(&parsed); /* sentinel: "parsed, nothing to return" */

        if (ct && body_data && body_len > 0) {
            if (ct_len >= 16 && strncasecmp(ct, "application/json", 16) == 0) {
                /* JSON decode — call php_json_decode_ex directly to avoid
                 * any issues with the static-inline php_json_decode wrapper
                 * across PHP ZTS builds. */
                zend_string *body_str = zend_string_init((const char *)body_data, body_len, 0);
                zval json_result;
                ZVAL_NULL(&json_result);
                php_json_decode_ex(&json_result, ZSTR_VAL(body_str), ZSTR_LEN(body_str),
                    PHP_JSON_OBJECT_AS_ARRAY, PHP_JSON_PARSER_DEFAULT_DEPTH);

                if (Z_TYPE(json_result) == IS_ARRAY) {
                    ZVAL_COPY_VALUE(&parsed, &json_result);
                } else {
                    /* Decode failed or returned scalar — try via call_user_function
                     * as a fallback (matches the json_decode() PHP code path). */
                    zval func_name, args[2];
                    ZVAL_STRING(&func_name, "json_decode");
                    ZVAL_STR_COPY(&args[0], body_str);
                    ZVAL_TRUE(&args[1]);

                    zval fallback;
                    ZVAL_NULL(&fallback);
                    if (call_user_function(EG(function_table), NULL, &func_name,
                            &fallback, 2, args) == SUCCESS
                        && Z_TYPE(fallback) == IS_ARRAY) {
                        ZVAL_COPY_VALUE(&parsed, &fallback);
                    } else {
                        zval_ptr_dtor(&fallback);
                    }
                    zval_ptr_dtor(&args[0]);
                    zval_ptr_dtor(&func_name);
                    zval_ptr_dtor(&json_result);
                }
                zend_string_release(body_str);
            } else if (
                (ct_len >= 33 && strncasecmp(ct, "application/x-www-form-urlencoded", 33) == 0) ||
                (ct_len >= 19 && strncasecmp(ct, "multipart/form-data", 19) == 0)
            ) {
                /* Form data — use $_POST if superglobals are enabled */
                if (oxphp_bridge_get_superglobals_enabled()) {
                    zval *post = &PG(http_globals)[TRACK_VARS_POST];
                    if (Z_TYPE_P(post) == IS_ARRAY) {
                        ZVAL_COPY(&parsed, post);
                    }
                }
            }
        }

        zend_update_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
            "_payload_cache", sizeof("_payload_cache")-1, &parsed);
        zval_ptr_dtor(&parsed);

        cached = zend_read_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
            "_payload_cache", sizeof("_payload_cache")-1, 1, NULL);
    }

    /* IS_FALSE sentinel = parsed but empty/unsupported */
    if (Z_TYPE_P(cached) == IS_FALSE) {
        if (def) {
            RETURN_COPY(def);
        }
        RETURN_NULL();
    }

    if (key) {
        if (Z_TYPE_P(cached) == IS_ARRAY) {
            zval *found = zend_hash_find(Z_ARRVAL_P(cached), key);
            if (found) {
                RETURN_COPY(found);
            }
        }
        if (def) {
            RETURN_COPY(def);
        }
        RETURN_NULL();
    }

    if (Z_TYPE_P(cached) == IS_ARRAY) {
        RETURN_COPY(cached);
    }
    RETURN_NULL();
}
/* }}} */

/* ─── Placeholder methods (depend on Task 5 supporting classes) */

/* {{{ OxPHP\Http\Request::attributes(): AttributesInterface */
ZEND_METHOD(OxPHP_Http_Request, attributes) {
    ZEND_PARSE_PARAMETERS_NONE();
    /* Return cached Attributes object, or create one on first call */
    zval *cached = zend_read_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
        "_attributes", sizeof("_attributes")-1, 1, NULL);
    if (cached && Z_TYPE_P(cached) == IS_OBJECT) {
        RETURN_COPY(cached);
    }
    /* Create new Attributes object and cache it on this Request */
    zval attr_obj;
    object_init_ex(&attr_obj, oxphp_http_attributes_ce);
    zend_update_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
        "_attributes", sizeof("_attributes")-1, &attr_obj);
    RETURN_ZVAL(&attr_obj, 1, 1);
}
/* }}} */

/* {{{ OxPHP\Http\Request::session(): ?SessionInterface */
ZEND_METHOD(OxPHP_Http_Request, session) {
    ZEND_PARSE_PARAMETERS_NONE();
    /* Return null if session_start() hasn't been called */
    if (PS(session_status) != php_session_active) {
        RETURN_NULL();
    }
    object_init_ex(return_value, oxphp_http_session_ce);
}
/* }}} */

/* {{{ OxPHP\Http\Request::file(string $name): null — placeholder */
ZEND_METHOD(OxPHP_Http_Request, file) {
    zend_string *name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(name)
    ZEND_PARSE_PARAMETERS_END();
    /* File support implemented in Task 5 */
    RETURN_NULL();
}
/* }}} */

/* {{{ OxPHP\Http\Request::files(?string $name = null): array — placeholder */
ZEND_METHOD(OxPHP_Http_Request, files) {
    zend_string *name = NULL;
    ZEND_PARSE_PARAMETERS_START(0, 1)
        Z_PARAM_OPTIONAL
        Z_PARAM_STR_OR_NULL(name)
    ZEND_PARSE_PARAMETERS_END();
    /* File support implemented in Task 5 */
    array_init(return_value);
}
/* }}} */

/* ═══════════════════════════════════════════════════════════════
 *  End of OxPHP\Http\Request methods
 * ═══════════════════════════════════════════════════════════════ */

/* ═══════════════════════════════════════════════════════════════
 *  OxPHP\Http\Attributes — ZEND_METHOD implementations
 * ═══════════════════════════════════════════════════════════════ */

/* {{{ OxPHP\Http\Attributes::get(string $key, mixed $default = null): mixed */
ZEND_METHOD(OxPHP_Http_Attributes, get) {
    zend_string *key;
    zval *def = NULL;
    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_STR(key)
        Z_PARAM_OPTIONAL
        Z_PARAM_ZVAL_OR_NULL(def)
    ZEND_PARSE_PARAMETERS_END();

    zval *store = zend_read_property(oxphp_http_attributes_ce, Z_OBJ_P(ZEND_THIS),
        "_store", sizeof("_store")-1, 1, NULL);
    if (store && Z_TYPE_P(store) == IS_ARRAY) {
        zval *found = zend_hash_find(Z_ARRVAL_P(store), key);
        if (found) {
            RETURN_COPY(found);
        }
    }
    if (def) {
        RETURN_COPY(def);
    }
    RETURN_NULL();
}
/* }}} */

/* {{{ OxPHP\Http\Attributes::set(string $key, mixed $value): void */
ZEND_METHOD(OxPHP_Http_Attributes, set) {
    zend_string *key;
    zval *value;
    ZEND_PARSE_PARAMETERS_START(2, 2)
        Z_PARAM_STR(key)
        Z_PARAM_ZVAL(value)
    ZEND_PARSE_PARAMETERS_END();

    zval rv;
    zval *store = zend_read_property(oxphp_http_attributes_ce, Z_OBJ_P(ZEND_THIS),
        "_store", sizeof("_store")-1, 1, &rv);

    zval new_arr;
    if (store && Z_TYPE_P(store) == IS_ARRAY) {
        /* Copy existing array */
        ZVAL_DUP(&new_arr, store);
    } else {
        array_init(&new_arr);
    }

    Z_TRY_ADDREF_P(value);
    zend_hash_update(Z_ARRVAL(new_arr), key, value);
    zend_update_property(oxphp_http_attributes_ce, Z_OBJ_P(ZEND_THIS),
        "_store", sizeof("_store")-1, &new_arr);
    zval_ptr_dtor(&new_arr);
}
/* }}} */

/* {{{ OxPHP\Http\Attributes::has(string $key): bool */
ZEND_METHOD(OxPHP_Http_Attributes, has) {
    zend_string *key;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(key)
    ZEND_PARSE_PARAMETERS_END();

    zval *store = zend_read_property(oxphp_http_attributes_ce, Z_OBJ_P(ZEND_THIS),
        "_store", sizeof("_store")-1, 1, NULL);
    RETURN_BOOL(store && Z_TYPE_P(store) == IS_ARRAY &&
        zend_hash_exists(Z_ARRVAL_P(store), key));
}
/* }}} */

/* {{{ OxPHP\Http\Attributes::remove(string $key): void */
ZEND_METHOD(OxPHP_Http_Attributes, remove) {
    zend_string *key;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(key)
    ZEND_PARSE_PARAMETERS_END();

    zval rv;
    zval *store = zend_read_property(oxphp_http_attributes_ce, Z_OBJ_P(ZEND_THIS),
        "_store", sizeof("_store")-1, 1, &rv);
    if (store && Z_TYPE_P(store) == IS_ARRAY) {
        zval new_arr;
        ZVAL_DUP(&new_arr, store);
        zend_hash_del(Z_ARRVAL(new_arr), key);
        zend_update_property(oxphp_http_attributes_ce, Z_OBJ_P(ZEND_THIS),
            "_store", sizeof("_store")-1, &new_arr);
        zval_ptr_dtor(&new_arr);
    }
}
/* }}} */

/* {{{ OxPHP\Http\Attributes::all(): array */
ZEND_METHOD(OxPHP_Http_Attributes, all) {
    ZEND_PARSE_PARAMETERS_NONE();

    zval *store = zend_read_property(oxphp_http_attributes_ce, Z_OBJ_P(ZEND_THIS),
        "_store", sizeof("_store")-1, 1, NULL);
    if (store && Z_TYPE_P(store) == IS_ARRAY) {
        RETURN_COPY(store);
    }
    RETURN_EMPTY_ARRAY();
}
/* }}} */

/* ═══════════════════════════════════════════════════════════════
 *  OxPHP\Http\Session — ZEND_METHOD implementations
 * ═══════════════════════════════════════════════════════════════ */

/* {{{ OxPHP\Http\Session::id(): string */
ZEND_METHOD(OxPHP_Http_Session, id) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval func_name, retval;
    ZVAL_STRING(&func_name, "session_id");
    if (call_user_function(NULL, NULL, &func_name, &retval, 0, NULL) == SUCCESS) {
        zval_ptr_dtor(&func_name);
        RETURN_ZVAL(&retval, 0, 0);
    }
    zval_ptr_dtor(&func_name);
    RETURN_EMPTY_STRING();
}
/* }}} */

/* {{{ OxPHP\Http\Session::name(): string */
ZEND_METHOD(OxPHP_Http_Session, name) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval func_name, retval;
    ZVAL_STRING(&func_name, "session_name");
    if (call_user_function(NULL, NULL, &func_name, &retval, 0, NULL) == SUCCESS) {
        zval_ptr_dtor(&func_name);
        RETURN_ZVAL(&retval, 0, 0);
    }
    zval_ptr_dtor(&func_name);
    RETURN_EMPTY_STRING();
}
/* }}} */

/* {{{ OxPHP\Http\Session::get(string $key, mixed $default = null): mixed */
ZEND_METHOD(OxPHP_Http_Session, get) {
    zend_string *key;
    zval *def = NULL;
    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_STR(key)
        Z_PARAM_OPTIONAL
        Z_PARAM_ZVAL_OR_NULL(def)
    ZEND_PARSE_PARAMETERS_END();

    zval *session = zend_hash_str_find(&EG(symbol_table), "_SESSION", sizeof("_SESSION")-1);
    if (session) { ZVAL_DEREF(session); }
    if (session && Z_TYPE_P(session) == IS_ARRAY) {
        zval *found = zend_hash_find(Z_ARRVAL_P(session), key);
        if (found) {
            RETURN_COPY(found);
        }
    }
    if (def) {
        RETURN_COPY(def);
    }
    RETURN_NULL();
}
/* }}} */

/* {{{ OxPHP\Http\Session::has(string $key): bool */
ZEND_METHOD(OxPHP_Http_Session, has) {
    zend_string *key;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(key)
    ZEND_PARSE_PARAMETERS_END();

    zval *session = zend_hash_str_find(&EG(symbol_table), "_SESSION", sizeof("_SESSION")-1);
    if (session) { ZVAL_DEREF(session); }
    RETURN_BOOL(session && Z_TYPE_P(session) == IS_ARRAY &&
        zend_hash_exists(Z_ARRVAL_P(session), key));
}
/* }}} */

/* {{{ OxPHP\Http\Session::all(): array */
ZEND_METHOD(OxPHP_Http_Session, all) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval *session = zend_hash_str_find(&EG(symbol_table), "_SESSION", sizeof("_SESSION")-1);
    if (session) { ZVAL_DEREF(session); }
    if (session && Z_TYPE_P(session) == IS_ARRAY) {
        RETURN_COPY(session);
    }
    RETURN_EMPTY_ARRAY();
}
/* }}} */

/* ═══════════════════════════════════════════════════════════════
 *  OxPHP\Http\UploadedFile — ZEND_METHOD implementations
 * ═══════════════════════════════════════════════════════════════ */

/* {{{ OxPHP\Http\UploadedFile::name(): string */
ZEND_METHOD(OxPHP_Http_UploadedFile, name) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval *val = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "name", sizeof("name")-1, 1, NULL);
    RETURN_COPY(val);
}
/* }}} */

/* {{{ OxPHP\Http\UploadedFile::clientType(): string */
ZEND_METHOD(OxPHP_Http_UploadedFile, clientType) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval *val = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "clientType", sizeof("clientType")-1, 1, NULL);
    RETURN_COPY(val);
}
/* }}} */

/* {{{ OxPHP\Http\UploadedFile::type(): string
 * Detects real MIME type via mime_content_type(), caches in _type property. */
ZEND_METHOD(OxPHP_Http_UploadedFile, type) {
    ZEND_PARSE_PARAMETERS_NONE();
    /* Check cached _type first */
    zval *cached = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "_type", sizeof("_type")-1, 1, NULL);
    if (cached && Z_TYPE_P(cached) == IS_STRING) {
        RETURN_COPY(cached);
    }
    /* Use mime_content_type() for magic-bytes detection */
    zval *tmp_path = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "tmpPath", sizeof("tmpPath")-1, 1, NULL);
    if (tmp_path && Z_TYPE_P(tmp_path) == IS_STRING) {
        zval func_name, retval;
        ZVAL_STRING(&func_name, "mime_content_type");
        zval args[1];
        ZVAL_COPY(&args[0], tmp_path);
        if (call_user_function(NULL, NULL, &func_name, &retval, 1, args) == SUCCESS
            && Z_TYPE(retval) == IS_STRING) {
            zend_update_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
                "_type", sizeof("_type")-1, &retval);
            zval_ptr_dtor(&func_name);
            zval_ptr_dtor(&args[0]);
            RETURN_ZVAL(&retval, 0, 0);
        }
        zval_ptr_dtor(&func_name);
        zval_ptr_dtor(&args[0]);
    }
    zend_update_property_string(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "_type", sizeof("_type")-1, "application/octet-stream");
    RETURN_STRING("application/octet-stream");
}
/* }}} */

/* {{{ OxPHP\Http\UploadedFile::size(): int */
ZEND_METHOD(OxPHP_Http_UploadedFile, size) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval *val = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "size", sizeof("size")-1, 1, NULL);
    RETURN_COPY(val);
}
/* }}} */

/* {{{ OxPHP\Http\UploadedFile::tmpPath(): string */
ZEND_METHOD(OxPHP_Http_UploadedFile, tmpPath) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval *val = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "tmpPath", sizeof("tmpPath")-1, 1, NULL);
    RETURN_COPY(val);
}
/* }}} */

/* {{{ OxPHP\Http\UploadedFile::error(): int */
ZEND_METHOD(OxPHP_Http_UploadedFile, error) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval *val = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "error", sizeof("error")-1, 1, NULL);
    RETURN_COPY(val);
}
/* }}} */

/* {{{ OxPHP\Http\UploadedFile::isValid(): bool */
ZEND_METHOD(OxPHP_Http_UploadedFile, isValid) {
    ZEND_PARSE_PARAMETERS_NONE();
    zval *err = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "error", sizeof("error")-1, 1, NULL);
    RETURN_BOOL(err && Z_TYPE_P(err) == IS_LONG && Z_LVAL_P(err) == 0 /* UPLOAD_ERR_OK */);
}
/* }}} */

/* {{{ OxPHP\Http\UploadedFile::moveTo(string $destination): bool */
ZEND_METHOD(OxPHP_Http_UploadedFile, moveTo) {
    zend_string *destination;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(destination)
    ZEND_PARSE_PARAMETERS_END();

    /* Call type() to cache MIME before moving */
    zval tmp_retval;
    zend_call_method_with_0_params(Z_OBJ_P(ZEND_THIS), oxphp_http_uploaded_file_ce,
        NULL, "type", &tmp_retval);
    zval_ptr_dtor(&tmp_retval);

    /* Check isValid */
    zval *err = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "error", sizeof("error")-1, 1, NULL);
    if (!err || Z_TYPE_P(err) != IS_LONG || Z_LVAL_P(err) != 0) {
        RETURN_FALSE;
    }

    /* Call move_uploaded_file() */
    zval *tmp_path = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "tmpPath", sizeof("tmpPath")-1, 1, NULL);
    zval func_name, retval;
    ZVAL_STRING(&func_name, "move_uploaded_file");
    zval args[2];
    ZVAL_COPY(&args[0], tmp_path);
    ZVAL_STR_COPY(&args[1], destination);
    int rc = call_user_function(NULL, NULL, &func_name, &retval, 2, args);
    zval_ptr_dtor(&func_name);
    zval_ptr_dtor(&args[0]);
    zval_ptr_dtor(&args[1]);
    if (rc == SUCCESS && Z_TYPE(retval) == IS_TRUE) {
        RETURN_TRUE;
    }
    RETURN_FALSE;
}
/* }}} */

/* ═══════════════════════════════════════════════════════════════
 *  End of supporting classes
 * ═══════════════════════════════════════════════════════════════ */

/* {{{ oxphp_http_request(): OxPHP\Http\Request
 * Returns the HTTP Request object for the current request context.
 * Throws OxPHP\Http\Exception\* if no active request. */
PHP_FUNCTION(oxphp_http_request)
{
    ZEND_PARSE_PARAMETERS_NONE();

    if (!oxphp_req_is_active()) {
        if (oxphp_bridge_is_async_worker()) {
            zend_throw_exception(oxphp_async_context_exc_ce,
                "Cannot access HTTP request from async worker context", 0);
            RETURN_THROWS();
        }
        if (oxphp_bridge_is_worker_mode()) {
            zend_throw_exception(oxphp_worker_idle_exc_ce,
                "Cannot access HTTP request: worker is idle (waiting for next request)", 0);
            RETURN_THROWS();
        }
        zend_throw_exception(oxphp_no_active_request_ce,
            "Cannot access HTTP request: no active request context", 0);
        RETURN_THROWS();
    }

    object_init_ex(return_value, oxphp_http_request_ce);
}
/* }}} */

/* {{{ oxphp_superglobals_enabled(): bool */
PHP_FUNCTION(oxphp_superglobals_enabled)
{
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_BOOL(oxphp_bridge_get_superglobals_enabled());
}
/* }}} */

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
 * Returns an array with server and request metadata. */
PHP_FUNCTION(oxphp_server_info)
{
    ZEND_PARSE_PARAMETERS_NONE();

    array_init(return_value);
    add_assoc_string(return_value, "version", PHP_OXPHP_SAPI_VERSION);
    add_assoc_long(return_value, "worker_id", oxphp_bridge_get_worker_id());
    add_assoc_double(return_value, "request_time", oxphp_bridge_get_request_time());
    add_assoc_bool(return_value, "worker_mode", oxphp_bridge_is_worker_mode());
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

/* ─── Cooperative Sleep ───────────────────────────────────── */

/* Internal: register timer and suspend current fiber.
 * duration_us is the sleep duration in microseconds.
 * Returns 1 if fiber-suspended, 0 if no fiber (use blocking fallback). */
static int oxphp_fiber_sleep_us(uint64_t duration_us)
{
    if (oxphp_current_fiber == NULL) return 0;

    uint64_t duration_ms = (duration_us + 999) / 1000; /* round up */
    if (duration_ms == 0) duration_ms = 1;

    uint64_t timer_id = oxphp_bridge_timer_register(duration_ms);

    oxphp_current_fiber->suspend_reason = OXPHP_SUSPEND_SLEEP;
    oxphp_current_fiber->suspend_data.timer_id = timer_id;

    zend_fiber_transfer transfer = {
        .context = oxphp_current_fiber->scheduler,
        .flags = 0
    };
    ZVAL_NULL(&transfer.value);

    oxphp_current_fiber = NULL;
    zend_fiber_switch_context(&transfer);
    return 1;
}

/* {{{ oxphp_sleep(float $seconds): void
 * Cooperative sleep: suspends the current fiber (if inside one) to let other
 * requests proceed, falling back to blocking usleep when not in a fiber. */
PHP_FUNCTION(oxphp_sleep)
{
    double seconds;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_DOUBLE(seconds)
    ZEND_PARSE_PARAMETERS_END();

    if (seconds <= 0.0) return;

    uint64_t duration_us = (uint64_t)(seconds * 1000000.0);
    if (oxphp_fiber_sleep_us(duration_us)) return;

    usleep((useconds_t)duration_us);
}
/* }}} */

/* {{{ oxphp_usleep(int $microseconds): void
 * Cooperative microsecond sleep: suspends the current fiber (if inside one)
 * to let other requests proceed, falling back to blocking usleep. */
PHP_FUNCTION(oxphp_usleep)
{
    zend_long microseconds;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_LONG(microseconds)
    ZEND_PARSE_PARAMETERS_END();

    if (microseconds <= 0) return;

    if (oxphp_fiber_sleep_us((uint64_t)microseconds)) return;

    usleep((useconds_t)microseconds);
}
/* }}} */

/* ─── Worker Mode: soft reset between requests ─────────────── */

/**
 * Reset per-request PHP state without destroying the PHP heap.
 * Called between worker mode requests to prevent response bleed.
 */
static void oxphp_soft_reset(void) {
    /* 0. Session cleanup — MUST come before CG(unclean_shutdown) reset.
     * Matches PHP-FPM behavior: always write session data, even on crash.
     * This ensures the file lock is released and data is persisted.
     * SYNC: php-src/ext/session/session.c php_rshutdown_session_globals() */
    if (PS(session_status) == php_session_active) {
        zend_try {
            php_session_flush(1);
        } zend_end_try();
    }
    if (!Z_ISUNDEF(PS(http_session_vars))) {
        zval_ptr_dtor(&PS(http_session_vars));
        ZVAL_UNDEF(&PS(http_session_vars));
    }
    if (PS(id)) { zend_string_release(PS(id)); PS(id) = NULL; }
    if (PS(session_vars)) { zend_string_release(PS(session_vars)); PS(session_vars) = NULL; }
    if (PS(mod_user_class_name)) {
        zend_string_release(PS(mod_user_class_name));
        PS(mod_user_class_name) = NULL;
    }

    /* 1. Clear stale engine state from previous bailout or exit/die.
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

/* Shared loop body for Worker::serve() and oxphp_worker(). Caller has
 * already parsed (fci, fcc) and verified worker mode. */
static void oxphp_serve_loop(zend_fcall_info *fci, zend_fcall_info_cache *fcc);

/* {{{ oxphp_worker(callable $handler): bool
 * Enter worker mode loop with fiber-based request multiplexing.
 *
 * When only one request is in flight (no fibers suspended), the handler runs
 * directly via zend_call_function — zero fiber overhead (fast path).
 *
 * When a handler calls oxphp_async_await() or oxphp_sleep(), it suspends its
 * fiber, and the event loop picks up new requests or resumes ready fibers.
 *
 * Returns true on graceful shutdown, false if not in worker mode. */
PHP_FUNCTION(oxphp_worker)
{
    zend_fcall_info fci;
    zend_fcall_info_cache fcc;

    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_FUNC(fci, fcc)
    ZEND_PARSE_PARAMETERS_END();

    if (!oxphp_bridge_is_worker_mode()) {
        php_error_docref(NULL, E_WARNING, "oxphp_worker() only available in worker mode");
        RETURN_FALSE;
    }

    /* Mirror Worker::serve()'s re-entry guard so nested calls from either
     * entry point share the same per-thread flag. */
    if (oxphp_serve_in_progress) {
        zend_throw_exception(
            oxphp_invalid_serve_ctx_exc_ce,
            "oxphp_worker() is already running on this thread "
            "(nested calls are not supported)",
            0
        );
        RETURN_THROWS();
    }

    oxphp_serve_in_progress = true;
    zend_try {
        oxphp_serve_loop(&fci, &fcc);
    } zend_catch {
        oxphp_serve_in_progress = false;
        zend_bailout();
    } zend_end_try();
    oxphp_serve_in_progress = false;
    RETURN_TRUE;
}
/* }}} */

static void oxphp_serve_loop(zend_fcall_info *fci, zend_fcall_info_cache *fcc)
{
    oxphp_ctx_t *ctx = oxphp_bridge_get_ctx();

    /* Prevent handler closure from being GC'd during worker lifetime */
    zend_fcc_addref(fcc);

    /* Initialize the fiber scheduler */
    oxphp_fiber_scheduler sched;
    oxphp_scheduler_init(&sched);
    sched.shared_fci = fci;
    sched.shared_fcc = fcc;

    #define WORKER_GC_INTERVAL 100
    #define WORKER_MAX_CONSECUTIVE_ERRORS 3

    int consecutive_errors = 0;

    while (1) {
        if (sched.fiber_count == 0) {
            /* ── No active fibers: block-wait for next request ──────── */

            if (oxphp_bridge_worker_wait() != 0) {
                ctx->exit_reason = 0;
                break;
            }

            oxphp_soft_reset();

            /* Increment counter at request START so requestCount() inside
             * the handler observes the current request index (1-based). Also
             * syncs ctx->requests_done on the fast path (was only synced on
             * the event-loop path before — latent bug fix). */
            sched.total_requests_done = oxphp_bridge_increment_requests_done();
            ctx->requests_done = sched.total_requests_done;

            /* Create or reuse a fiber for the request */
            oxphp_request_fiber *fiber = oxphp_scheduler_create_fiber(&sched, fci, fcc);
            if (!fiber) break;

            if (fiber->started) {
                /* Reused fiber — coroutine is looping, just resume it */
                oxphp_scheduler_resume_fiber(&sched, fiber, NULL);
            } else {
                /* New fiber — start the coroutine for the first time */
                fiber->started = true;
                oxphp_scheduler_start_fiber(&sched, fiber);
            }

            if (fiber->completed) {
                oxphp_scheduler_finalize_fiber(&sched, fiber);
            }

        } else {
            /* ── Event loop: active fibers exist ──────────────────────
             * Run one tick: accept new requests, check await results,
             * check timers, resume ready fibers. */

            int rc = oxphp_scheduler_tick(&sched);
            if (rc == -1) {
                ctx->exit_reason = 0; /* shutdown */
                break;
            }

            /* Sync scheduler-level counters */
            consecutive_errors = sched.consecutive_errors;
            /* sched.total_requests_done is now mirrored from bridge state
             * at request entry inside oxphp_scheduler_tick; sync ctx for
             * the exit-condition check below. */
            ctx->requests_done = sched.total_requests_done;

            if (rc == 0) {
                /* No work done — brief sleep to avoid busy-wait.
                 * 100μs is short enough for responsive SSE,
                 * long enough to avoid CPU spin. */
                usleep(100);
            }
        }

        /* ── Check exit conditions ───────────────────────────────── */
        if (consecutive_errors >= WORKER_MAX_CONSECUTIVE_ERRORS) {
            ctx->exit_reason = 3;
            break;
        }
        if (ctx->exit_scheduled) {
            /* exit_reason was already set to 1 by oxphp_bridge_schedule_exit. */
            break;
        }
        if (ctx->max_memory_bytes > 0 && zend_memory_usage(0) > ctx->max_memory_bytes) {
            ctx->exit_reason = 2;
            break;
        }

        /* GC every N requests */
        if (ctx->requests_done > 0 && (ctx->requests_done % WORKER_GC_INTERVAL) == 0) {
            gc_collect_cycles();
        }
    }

    /* Cleanup: finalize any remaining fibers */
    oxphp_scheduler_destroy(&sched);
    zend_fcc_dtor(fcc);
}

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
        int has_exc = (EG(exception) != NULL) ? 1 : 0;
        /* If Rust already threw a PHP exception via oxphp_throw_exception(),
         * EG(exception) is set — just return and let Zend propagate it.
         * Otherwise fall back to a generic warning. */
        if (EG(exception)) {
            /* return_value may have been partially written — reset to null */
            zval_ptr_dtor(return_value);
            ZVAL_NULL(return_value);
            return;
        }
        php_error_docref(NULL, E_WARNING, "oxphp: dispatch failed for %s", func_name);
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

/* ─── Plugin class method dispatch ────────────────────────── */

/* {{{ arginfo for method dispatch (variadic mixed, same as native dispatch) */
ZEND_BEGIN_ARG_INFO_EX(arginfo_oxphp_method_dispatch, 0, 0, 0)
    ZEND_ARG_VARIADIC_INFO(0, args)
ZEND_END_ARG_INFO()
/* }}} */

/* {{{ oxphp_method_dispatch — single handler for all plugin class methods.
 * Routes calls through the Rust method dispatch callback using class_index
 * and method name to identify the target. */
ZEND_FUNCTION(oxphp_method_dispatch)
{
    const char *method_name = ZSTR_VAL(execute_data->func->common.function_name);
    uint32_t argc = ZEND_NUM_ARGS();
    zval *args = (argc > 0) ? ZEND_CALL_ARG(execute_data, 1) : NULL;

    void *rust_data = NULL;
    uint32_t class_index = 0;

    /* For instance methods, extract rust_data from the custom object */
    if (Z_TYPE(execute_data->This) == IS_OBJECT) {
        oxphp_custom_object *intern = OXPHP_OBJ(Z_OBJ(execute_data->This));
        rust_data = intern->rust_data;
        class_index = intern->class_index;
    } else if (execute_data->func->common.scope) {
        /* Static method — find class_index from the scope CE.
         * Walk the plugin class CE array to find the match. */
        zend_class_entry *scope = execute_data->func->common.scope;
        int cls_count = oxphp_bridge_get_plugin_class_count();
        for (int i = 0; i < cls_count; i++) {
            const char *fqn = oxphp_bridge_get_class_fqn(i);
            if (fqn && strcmp(ZSTR_VAL(scope->name), fqn) == 0) {
                class_index = (uint32_t)i;
                break;
            }
        }
    }

    oxphp_method_dispatch_fn_t dispatch = oxphp_bridge_get_method_dispatch();
    if (!dispatch) {
        zend_throw_error(NULL, "OxPHP method dispatch not initialized");
        return;
    }

    int rc = dispatch(class_index, method_name, args, argc, return_value, rust_data);
    if (rc != 0 && !EG(exception)) {
        zend_throw_error(NULL, "Plugin method %s::%s failed",
            execute_data->func->common.scope
                ? ZSTR_VAL(execute_data->func->common.scope->name) : "?",
            method_name);
    }
}
/* }}} */

/* ─── Decorator System ────────────────────────────────────── */

/* {{{ AttributeInterface — method arginfo and entries */
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_decorator_before, 0, 1, IS_VOID, 0)
    ZEND_ARG_OBJ_INFO(0, ctx, OxPHP\\Decorator\\Context, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_decorator_after, 0, 1, IS_VOID, 0)
    ZEND_ARG_OBJ_INFO(0, ctx, OxPHP\\Decorator\\Context, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_decorator_interface_methods[] = {
    ZEND_ABSTRACT_ME(AttributeInterface, before, arginfo_decorator_before)
    ZEND_ABSTRACT_ME(AttributeInterface, after, arginfo_decorator_after)
    PHP_FE_END
};
/* }}} */

/* {{{ Context class methods */
ZEND_METHOD(OxPHP_Decorator_Context, getParams) {
    ZEND_PARSE_PARAMETERS_NONE();

    /* Get execute_data from decorator context stack */
    oxphp_decorator_ctx_t *dctx = oxphp_decorator_ctx_peek();
    if (!dctx || !dctx->execute_data) {
        RETURN_EMPTY_ARRAY();
    }

    zend_execute_data *ex = (zend_execute_data *)dctx->execute_data;
    uint32_t argc = ZEND_CALL_NUM_ARGS(ex);

    array_init_size(return_value, argc);
    for (uint32_t i = 0; i < argc; i++) {
        zval *arg = ZEND_CALL_ARG(ex, i + 1);
        Z_TRY_ADDREF_P(arg);
        zend_hash_next_index_insert(Z_ARRVAL_P(return_value), arg);
    }
}

ZEND_METHOD(OxPHP_Decorator_Context, getResult) {
    ZEND_PARSE_PARAMETERS_NONE();

    zval *has_result = zend_read_property(oxphp_decorator_context_ce, Z_OBJ_P(ZEND_THIS),
        "_has_result", sizeof("_has_result")-1, 1, NULL);
    if (!has_result || Z_TYPE_P(has_result) != IS_TRUE) {
        RETURN_NULL();
    }

    zval *result = zend_read_property(oxphp_decorator_context_ce, Z_OBJ_P(ZEND_THIS),
        "_result", sizeof("_result")-1, 1, NULL);
    if (result && Z_TYPE_P(result) != IS_UNDEF) {
        RETURN_COPY(result);
    }
    RETURN_NULL();
}

ZEND_METHOD(OxPHP_Decorator_Context, hasResult) {
    ZEND_PARSE_PARAMETERS_NONE();

    zval *has_result = zend_read_property(oxphp_decorator_context_ce, Z_OBJ_P(ZEND_THIS),
        "_has_result", sizeof("_has_result")-1, 1, NULL);
    if (has_result && Z_TYPE_P(has_result) == IS_TRUE) {
        RETURN_TRUE;
    }
    RETURN_FALSE;
}

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_ctx_getParams, 0, 0, IS_ARRAY, 0)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_ctx_getResult, 0, 0, IS_MIXED, 0)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_ctx_hasResult, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_decorator_context_methods[] = {
    ZEND_ME(OxPHP_Decorator_Context, getParams, arginfo_ctx_getParams, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Decorator_Context, getResult, arginfo_ctx_getResult, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Decorator_Context, hasResult, arginfo_ctx_hasResult, ZEND_ACC_PUBLIC)
    PHP_FE_END
};
/* }}} */

/* {{{ oxphp_register_decorator(string $class): bool */
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_register_decorator, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO(0, class, IS_STRING, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(oxphp_register_decorator) {
    zend_string *class_name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(class_name)
    ZEND_PARSE_PARAMETERS_END();

    zend_class_entry *ce = zend_lookup_class(class_name);
    if (!ce) {
        php_error_docref(NULL, E_WARNING, "Class '%s' not found", ZSTR_VAL(class_name));
        RETURN_FALSE;
    }

    if (!instanceof_function(ce, oxphp_decorator_interface_ce)) {
        php_error_docref(NULL, E_WARNING,
            "Class '%s' does not implement OxPHP\\Decorator\\AttributeInterface",
            ZSTR_VAL(class_name));
        RETURN_FALSE;
    }

    /* Register with Rust registry via bridge */
    uint32_t targets = 0x07; /* ALL by default */

    /* Try to read Attribute targets from the class */
    if (ce->attributes) {
        zend_string *attr_name = zend_string_init("Attribute", sizeof("Attribute")-1, 0);
        zend_attribute *attr = zend_get_attribute_str(ce->attributes,
            ZSTR_VAL(attr_name), ZSTR_LEN(attr_name));
        zend_string_release(attr_name);

        if (attr && attr->argc > 0) {
            zval tmp;
            if (SUCCESS == zend_get_attribute_value(&tmp, attr, 0, ce)) {
                targets = (uint32_t)zval_get_long(&tmp);
                zval_ptr_dtor(&tmp);
                /* Convert PHP target flags to our flags:
                 * PHP: TARGET_CLASS=1, TARGET_FUNCTION=2, TARGET_METHOD=4, etc.
                 * Ours: FUNCTION=0x01, METHOD=0x02, CLASS=0x04
                 * We need to remap. */
                uint32_t our_targets = 0;
                if (targets & 2)  our_targets |= 0x01; /* TARGET_FUNCTION -> FUNCTION */
                if (targets & 4)  our_targets |= 0x02; /* TARGET_METHOD -> METHOD */
                if (targets & 16) our_targets |= 0x04; /* TARGET_CLASS -> CLASS */
                targets = our_targets ? our_targets : 0x07; /* fallback to ALL */
            }
        }
    }

    oxphp_bridge_register_php_decorator(ZSTR_VAL(class_name), targets);

    RETURN_TRUE;
}
/* }}} */

/* {{{ Observer implementation */

/* Helper: create and populate a Context object for PHP decorators */
static void oxphp_create_decorator_context(zval *ctx_zval, oxphp_decorator_ctx_t *dctx, int for_after, zval *retval) {
    object_init_ex(ctx_zval, oxphp_decorator_context_ce);

    /* Build full target name */
    char target_buf[512];
    if (dctx->class_name && dctx->target) {
        snprintf(target_buf, sizeof(target_buf), "%s::%s", dctx->class_name, dctx->target);
    } else if (dctx->target) {
        snprintf(target_buf, sizeof(target_buf), "%s", dctx->target);
    } else {
        target_buf[0] = '\0';
    }

    zend_update_property_string(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
        "target", sizeof("target")-1, target_buf);
    zend_update_property_string(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
        "class", sizeof("class")-1, dctx->class_name ? dctx->class_name : "");
    zend_update_property_string(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
        "method", sizeof("method")-1,
        dctx->class_name ? (dctx->target ? dctx->target : "") : "");
    zend_update_property_string(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
        "function", sizeof("function")-1,
        dctx->class_name ? "" : (dctx->target ? dctx->target : ""));
    zend_update_property_long(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
        "objectId", sizeof("objectId")-1, (zend_long)dctx->object_id);

    /* Request ID from bridge TLS */
    const char *req_id = oxphp_bridge_get_request_id();
    zend_update_property_string(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
        "requestId", sizeof("requestId")-1, req_id ? req_id : "");

    /* Trace ID — empty for now (trace context is in $_SERVER) */
    zend_update_property_string(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
        "traceId", sizeof("traceId")-1, "");

    /* Internal properties for result tracking */
    if (for_after && retval) {
        zend_update_property(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
            "_result", sizeof("_result")-1, retval);
        zend_update_property_bool(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
            "_has_result", sizeof("_has_result")-1, 1);
    } else {
        zend_update_property_bool(oxphp_decorator_context_ce, Z_OBJ_P(ctx_zval),
            "_has_result", sizeof("_has_result")-1, 0);
    }
}

/* Get or create a cached PHP decorator instance.
 * The cache is keyed by (decorated_func, attr_index) — globally unique
 * within a request, so decorators never collide across functions.
 * Returns a pointer to the cached zval (valid for the request lifetime). */
static zval *oxphp_get_cached_decorator(const char *class_name,
                                         zend_function *decorated_func, uint32_t attr_index)
{
    oxphp_dec_cache_ensure_init();
    zend_ulong key = oxphp_dec_cache_key(decorated_func, attr_index);

    /* Check if already cached — must match the expected class to defend
     * against any hypothetical key collision. */
    zval *existing = zend_hash_index_find(&decorator_instance_cache_ht, key);
    if (existing && Z_TYPE_P(existing) == IS_OBJECT) {
        zend_class_entry *existing_ce = Z_OBJCE_P(existing);
        if (existing_ce && ZSTR_LEN(existing_ce->name) == strlen(class_name) &&
            memcmp(ZSTR_VAL(existing_ce->name), class_name, strlen(class_name)) == 0) {
            return existing;
        }
        /* Class mismatch — discard and recreate. */
        zend_hash_index_del(&decorator_instance_cache_ht, key);
    }

    /* Look up the decorator class */
    zend_string *cls = zend_string_init(class_name, strlen(class_name), 0);
    zend_class_entry *ce = zend_lookup_class(cls);
    zend_string_release(cls);
    if (!ce) return NULL;

    /* Create instance in a scratch zval, then insert into the HT. */
    zval scratch;
    ZVAL_UNDEF(&scratch);
    zval *cached = &scratch;
    object_init_ex(cached, ce);

    /* Find the matching attribute to get constructor args */
    zend_attribute *attr = NULL;
    if (decorated_func->common.attributes) {
        uint32_t i = 0;
        zend_attribute *a;
        ZEND_HASH_FOREACH_PTR(decorated_func->common.attributes, a) {
            if (strcmp(ZSTR_VAL(a->name), class_name) == 0) {
                if (i == attr_index) {
                    attr = a;
                    break;
                }
                i++;
            }
        } ZEND_HASH_FOREACH_END();

        /* Also check class-level attributes */
        if (!attr && decorated_func->common.scope && decorated_func->common.scope->attributes) {
            ZEND_HASH_FOREACH_PTR(decorated_func->common.scope->attributes, a) {
                if (strcmp(ZSTR_VAL(a->name), class_name) == 0) {
                    if (i == attr_index) {
                        attr = a;
                        break;
                    }
                    i++;
                }
            } ZEND_HASH_FOREACH_END();
        }
    }

    /* Call constructor with attribute args if any */
    if (attr && attr->argc > 0) {
        zend_function *ctor = ce->constructor;
        if (ctor) {
            /* Evaluate attribute arguments and pass to constructor */
            zval *args = safe_emalloc(attr->argc, sizeof(zval), 0);
            uint32_t valid_argc = 0;
            for (uint32_t i = 0; i < attr->argc; i++) {
                if (SUCCESS == zend_get_attribute_value(&args[i], attr, i, ce)) {
                    valid_argc++;
                } else {
                    ZVAL_NULL(&args[i]);
                    valid_argc++;
                }
            }

            /* Call __construct */
            zval retval;
            ZVAL_UNDEF(&retval);
            zend_call_known_function(ctor, Z_OBJ_P(cached), ce, &retval,
                                     valid_argc, args, NULL);
            zval_ptr_dtor(&retval);

            for (uint32_t i = 0; i < valid_argc; i++) {
                zval_ptr_dtor(&args[i]);
            }
            efree(args);

            if (EG(exception)) {
                zval_ptr_dtor(cached);
                ZVAL_UNDEF(cached);
                return NULL;
            }
        }
    } else {
        /* No-arg constructor */
        zend_function *ctor = ce->constructor;
        if (ctor) {
            zval retval;
            ZVAL_UNDEF(&retval);
            zend_call_known_function(ctor, Z_OBJ_P(cached), ce, &retval, 0, NULL, NULL);
            zval_ptr_dtor(&retval);

            if (EG(exception)) {
                zval_ptr_dtor(cached);
                ZVAL_UNDEF(cached);
                return NULL;
            }
        }
    }

    /* Insert into HT — HT takes ownership of the refcount bumped by
     * object_init_ex(). Return a stable pointer via the HT lookup. */
    return zend_hash_index_update(&decorator_instance_cache_ht, key, cached);
}

static zend_observer_fcall_handlers oxphp_decorator_observer_init(
    zend_execute_data *execute_data
) {
    zend_function *func = execute_data->func;
    if (!func || func->type != ZEND_USER_FUNCTION) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    oxphp_decorator_resolve_fn_t resolve = oxphp_bridge_get_decorator_resolve();
    if (!resolve) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    const char *attr_names[64];
    uint32_t attr_count = 0;

    /* Function/method attributes */
    if (func->common.attributes) {
        zend_attribute *attr;
        ZEND_HASH_FOREACH_PTR(func->common.attributes, attr) {
            if (attr_count < 64) {
                attr_names[attr_count++] = ZSTR_VAL(attr->name);
            }
        } ZEND_HASH_FOREACH_END();
    }

    /* Class attributes (for TARGET_CLASS) */
    if (func->common.scope && func->common.scope->attributes) {
        zend_attribute *attr;
        ZEND_HASH_FOREACH_PTR(func->common.scope->attributes, attr) {
            if (attr_count < 64) {
                attr_names[attr_count++] = ZSTR_VAL(attr->name);
            }
        } ZEND_HASH_FOREACH_END();
    }

    if (attr_count == 0) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    uintptr_t fn_id = (uintptr_t)func;
    int found = resolve(fn_id, attr_names, attr_count);
    if (!found) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    return (zend_observer_fcall_handlers){
        oxphp_decorator_begin,
        oxphp_decorator_end
    };
}

static void oxphp_decorator_begin(zend_execute_data *execute_data) {
    oxphp_decorator_ctx_t *dctx = oxphp_decorator_ctx_push();
    zend_function *func = execute_data->func;

    dctx->fn_id = (uintptr_t)func;
    dctx->target = func->common.function_name ? ZSTR_VAL(func->common.function_name) : "";
    dctx->class_name = func->common.scope ? ZSTR_VAL(func->common.scope->name) : NULL;
    dctx->object_id = (Z_TYPE(execute_data->This) == IS_OBJECT)
        ? Z_OBJ(execute_data->This)->handle : 0;
    dctx->execute_data = execute_data;
    dctx->decorator_count = 0;

    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    dctx->timestamp_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;

    /* Dispatch to Rust decorators */
    oxphp_decorator_begin_fn_t begin_fn = oxphp_bridge_get_decorator_begin();
    if (begin_fn) {
        int action = begin_fn(dctx->fn_id, dctx->target, dctx->class_name,
                              dctx->object_id, dctx->timestamp_ns);
        if (action != 0) {
            size_t reason_len;
            const char *reason = oxphp_bridge_get_decorator_reject_reason(&reason_len);
            zend_throw_exception(oxphp_decorator_rejected_ce,
                reason_len > 0 ? reason : "Decorator rejected", 0);
            oxphp_bridge_clear_decorator_reject_reason();
            oxphp_force_exception_on_current_frame();
            return;
        }
    }

    /* Dispatch to PHP decorators: call before() on cached instances */
    {
        oxphp_php_dec_count_fn_t count_fn = oxphp_bridge_get_php_decorator_count();
        oxphp_php_dec_class_fn_t class_fn = oxphp_bridge_get_php_decorator_class();

        if (count_fn && class_fn) {
            uint32_t php_count = count_fn(dctx->fn_id);

            for (uint32_t i = 0; i < php_count; i++) {
                const char *cls = class_fn(dctx->fn_id, i);
                if (!cls) continue;

                zval *dec_instance = oxphp_get_cached_decorator(cls, func, i);
                if (!dec_instance) continue;

                /* Create context for before() */
                zval ctx_zval;
                oxphp_create_decorator_context(&ctx_zval, dctx, 0, NULL);

                /* Call $dec->before($ctx) */
                zval retval;
                ZVAL_UNDEF(&retval);
                zend_call_method_with_1_params(
                    Z_OBJ_P(dec_instance), Z_OBJCE_P(dec_instance),
                    NULL, "before", &retval, &ctx_zval);
                zval_ptr_dtor(&retval);
                zval_ptr_dtor(&ctx_zval);

                if (EG(exception)) {
                    /* Cleanup: call after() on previously-succeeded decorators in reverse */
                    zval cleanup_ctx;
                    oxphp_create_decorator_context(&cleanup_ctx, dctx, 0, NULL);
                    for (int j = (int)dctx->decorator_count - 1; j >= 0; j--) {
                        const char *prev_cls = class_fn(dctx->fn_id, (uint32_t)j);
                        if (!prev_cls) continue;
                        zend_ulong prev_key = oxphp_dec_cache_key(func, (uint32_t)j);
                        zval *prev_cached = zend_hash_index_find(
                            &decorator_instance_cache_ht, prev_key);
                        if (!prev_cached || Z_TYPE_P(prev_cached) != IS_OBJECT) continue;

                        zval cleanup_ret;
                        ZVAL_UNDEF(&cleanup_ret);
                        /* Save and clear exception to allow after() to run */
                        zend_object *saved_exception = EG(exception);
                        EG(exception) = NULL;
                        zend_call_method_with_1_params(
                            Z_OBJ_P(prev_cached),
                            Z_OBJCE_P(prev_cached),
                            NULL, "after", &cleanup_ret, &cleanup_ctx);
                        zval_ptr_dtor(&cleanup_ret);
                        /* Restore original exception */
                        if (EG(exception) && saved_exception) {
                            /* Discard cleanup exception, keep original */
                            zend_clear_exception();
                        }
                        if (!EG(exception)) {
                            EG(exception) = saved_exception;
                        }
                    }
                    zval_ptr_dtor(&cleanup_ctx);
                    oxphp_force_exception_on_current_frame();
                    return;
                }

                dctx->decorator_count++;
            }
        }
    }
}

static void oxphp_decorator_end(zend_execute_data *execute_data, zval *retval) {
    oxphp_decorator_ctx_t *dctx = oxphp_decorator_ctx_peek();
    if (!dctx) return;

    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t now_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
    uint64_t elapsed_ns = now_ns - dctx->timestamp_ns;
    int success = !EG(exception);

    /* Dispatch to Rust decorators (reverse order handled in Rust) */
    oxphp_decorator_end_fn_t end_fn = oxphp_bridge_get_decorator_end();
    if (end_fn) {
        const char *exc_class = NULL;
        if (!success && EG(exception)) {
            exc_class = ZSTR_VAL(EG(exception)->ce->name);
        }
        end_fn(dctx->fn_id, elapsed_ns, success, exc_class);
    }

    /* Dispatch to PHP decorators in reverse order: call after() */
    {
        oxphp_php_dec_count_fn_t count_fn = oxphp_bridge_get_php_decorator_count();
        oxphp_php_dec_class_fn_t class_fn = oxphp_bridge_get_php_decorator_class();

        if (count_fn && class_fn && dctx->decorator_count > 0) {
            /* Create context with result for after() */
            zval ctx_zval;
            int has_result = success && retval && Z_TYPE_P(retval) != IS_UNDEF;
            oxphp_create_decorator_context(&ctx_zval, dctx, has_result, retval);

            zend_function *func = execute_data->func;
            for (int i = (int)dctx->decorator_count - 1; i >= 0; i--) {
                const char *cls = class_fn(dctx->fn_id, (uint32_t)i);
                if (!cls) continue;

                zend_ulong key = oxphp_dec_cache_key(func, (uint32_t)i);
                zval *cached = zend_hash_index_find(
                    &decorator_instance_cache_ht, key);
                if (!cached || Z_TYPE_P(cached) != IS_OBJECT) continue;

                zval after_ret;
                ZVAL_UNDEF(&after_ret);
                zend_call_method_with_1_params(
                    Z_OBJ_P(cached),
                    Z_OBJCE_P(cached),
                    NULL, "after", &after_ret, &ctx_zval);
                zval_ptr_dtor(&after_ret);

                /* If after() throws, stop dispatching remaining decorators */
                if (EG(exception)) break;
            }

            zval_ptr_dtor(&ctx_zval);
        }
    }

    oxphp_decorator_ctx_pop();
}
/* }}} */

/* SAPI-side predicate for `oxphp_bridge_in_fiber`. Returns 1 iff the
 * calling thread is inside an oxphp scheduler fiber — i.e. a fiber
 * that `oxphp_fiber_suspend_for_await` can suspend. User-level
 * `Fiber::start()` does NOT touch `oxphp_current_fiber`, so it
 * correctly reports 0. */
int oxphp_in_oxphp_fiber(void) {
    return oxphp_current_fiber != NULL ? 1 : 0;
}

/* Fiber-aware await helper. Called from Rust handler via FFI.
 * Returns: 0 = fiber handled it (retval populated), 1 = not in fiber (caller does blocking),
 *         -1 = error (exception details in bridge TLS), -2 = timeout */
int oxphp_fiber_suspend_for_await(int64_t promise_id, double timeout, void *retval) {
    if (oxphp_current_fiber == NULL) {
        return 1; /* Not in fiber — caller should do blocking await */
    }

    oxphp_current_fiber->suspend_reason = OXPHP_SUSPEND_AWAIT;
    oxphp_current_fiber->suspend_data.promise_id = promise_id;

    zend_fiber_transfer transfer = {
        .context = oxphp_current_fiber->scheduler,
        .flags = 0
    };
    ZVAL_NULL(&transfer.value);

    oxphp_current_fiber = NULL;
    zend_fiber_switch_context(&transfer);
    /* --- RESUMED by scheduler when promise result is ready --- */

    int rc = oxphp_bridge_await_dispatch(promise_id, 0.0, (zval *)retval);
    return rc; /* 0 = success, -1 = error, -2 = timeout */
}

/* ─── Request arginfo ────────────────────────────────────── */
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_method, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_path, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_fullUri, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_scheme, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_host, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_port, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_queryString, 0, 0, IS_STRING, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_isSecure, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_isMethod, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO(0, method, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_httpProtocol, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_httpProtocolVersion, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_query, 0, 0, IS_MIXED, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, key, IS_STRING, 1, "null")
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, default, IS_MIXED, 0, "null")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_payload, 0, 0, IS_MIXED, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, key, IS_STRING, 1, "null")
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, default, IS_MIXED, 0, "null")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_header, 0, 1, IS_STRING, 1)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, default, IS_STRING, 1, "null")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_headers, 0, 0, IS_ARRAY, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_hasHeader, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_cookie, 0, 1, IS_STRING, 1)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, default, IS_STRING, 1, "null")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_cookies, 0, 0, IS_ARRAY, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_body, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_contentType, 0, 0, IS_STRING, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_ip, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_startTime, 0, 0, IS_MIXED, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, asFloat, _IS_BOOL, 0, "false")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_OBJ_INFO_EX(arginfo_req_attributes, 0, 0,
    OxPHP\\Http\\AttributesInterface, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_OBJ_INFO_EX(arginfo_req_session, 0, 0,
    OxPHP\\Http\\SessionInterface, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_OBJ_INFO_EX(arginfo_req_file, 0, 1,
    OxPHP\\Http\\UploadedFileInterface, 1)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_req_files, 0, 0, IS_ARRAY, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, name, IS_STRING, 1, "null")
ZEND_END_ARG_INFO()

/* ─── Request method entries ─────────────────────────────── */
static const zend_function_entry oxphp_http_request_methods[] = {
    ZEND_ME(OxPHP_Http_Request, method,              arginfo_req_method,              ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, path,                arginfo_req_path,                ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, fullUri,             arginfo_req_fullUri,             ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, scheme,              arginfo_req_scheme,              ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, host,                arginfo_req_host,                ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, port,                arginfo_req_port,                ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, queryString,         arginfo_req_queryString,         ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, isSecure,            arginfo_req_isSecure,            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, isMethod,            arginfo_req_isMethod,            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, httpProtocol,        arginfo_req_httpProtocol,        ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, httpProtocolVersion, arginfo_req_httpProtocolVersion, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, query,               arginfo_req_query,               ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, payload,             arginfo_req_payload,             ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, header,              arginfo_req_header,              ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, headers,             arginfo_req_headers,             ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, hasHeader,           arginfo_req_hasHeader,           ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, cookie,              arginfo_req_cookie,              ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, cookies,             arginfo_req_cookies,             ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, body,                arginfo_req_body,                ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, contentType,         arginfo_req_contentType,         ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, ip,                  arginfo_req_ip,                  ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, startTime,           arginfo_req_startTime,           ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, attributes,          arginfo_req_attributes,          ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, session,             arginfo_req_session,             ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, file,                arginfo_req_file,                ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Request, files,               arginfo_req_files,               ZEND_ACC_PUBLIC)
    PHP_FE_END
};

/* ─── Attributes arginfo and method entries ────────────────── */

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_attr_get, 0, 1, IS_MIXED, 0)
    ZEND_ARG_TYPE_INFO(0, key, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, default, IS_MIXED, 0, "null")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_attr_set, 0, 2, IS_VOID, 0)
    ZEND_ARG_TYPE_INFO(0, key, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO(0, value, IS_MIXED, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_attr_has, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO(0, key, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_attr_remove, 0, 1, IS_VOID, 0)
    ZEND_ARG_TYPE_INFO(0, key, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_attr_all, 0, 0, IS_ARRAY, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_http_attributes_methods[] = {
    ZEND_ME(OxPHP_Http_Attributes, get,    arginfo_attr_get,    ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Attributes, set,    arginfo_attr_set,    ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Attributes, has,    arginfo_attr_has,    ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Attributes, remove, arginfo_attr_remove, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Attributes, all,    arginfo_attr_all,    ZEND_ACC_PUBLIC)
    PHP_FE_END
};

/* ─── Session arginfo and method entries ───────────────────── */

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_session_id, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_session_name, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_http_session_methods[] = {
    ZEND_ME(OxPHP_Http_Session, id,   arginfo_session_id,   ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Session, name, arginfo_session_name, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Session, get,  arginfo_attr_get,     ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Session, has,  arginfo_attr_has,     ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_Session, all,  arginfo_attr_all,     ZEND_ACC_PUBLIC)
    PHP_FE_END
};

/* ─── UploadedFile arginfo and method entries ──────────────── */

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_name, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_clientType, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_type, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_size, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_tmpPath, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_error, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_isValid, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_uf_moveTo, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO(0, destination, IS_STRING, 0)
ZEND_END_ARG_INFO()

static const zend_function_entry oxphp_http_uploaded_file_methods[] = {
    ZEND_ME(OxPHP_Http_UploadedFile, name,       arginfo_uf_name,       ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_UploadedFile, clientType, arginfo_uf_clientType, ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_UploadedFile, type,       arginfo_uf_type,       ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_UploadedFile, size,       arginfo_uf_size,       ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_UploadedFile, tmpPath,    arginfo_uf_tmpPath,    ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_UploadedFile, error,      arginfo_uf_error,      ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_UploadedFile, isValid,    arginfo_uf_isValid,    ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Http_UploadedFile, moveTo,     arginfo_uf_moveTo,    ZEND_ACC_PUBLIC)
    PHP_FE_END
};

/* ─── Interface method entries ──────────────────────────── */

static const zend_function_entry oxphp_http_attributes_iface_methods[] = {
    ZEND_ABSTRACT_ME(AttributesInterface, get,    arginfo_attr_get)
    ZEND_ABSTRACT_ME(AttributesInterface, set,    arginfo_attr_set)
    ZEND_ABSTRACT_ME(AttributesInterface, has,    arginfo_attr_has)
    ZEND_ABSTRACT_ME(AttributesInterface, remove, arginfo_attr_remove)
    ZEND_ABSTRACT_ME(AttributesInterface, all,    arginfo_attr_all)
    PHP_FE_END
};

static const zend_function_entry oxphp_http_session_iface_methods[] = {
    ZEND_ABSTRACT_ME(SessionInterface, id,   arginfo_session_id)
    ZEND_ABSTRACT_ME(SessionInterface, name, arginfo_session_name)
    ZEND_ABSTRACT_ME(SessionInterface, get,  arginfo_attr_get)
    ZEND_ABSTRACT_ME(SessionInterface, has,  arginfo_attr_has)
    ZEND_ABSTRACT_ME(SessionInterface, all,  arginfo_attr_all)
    PHP_FE_END
};

static const zend_function_entry oxphp_http_uploaded_file_iface_methods[] = {
    ZEND_ABSTRACT_ME(UploadedFileInterface, name,       arginfo_uf_name)
    ZEND_ABSTRACT_ME(UploadedFileInterface, clientType, arginfo_uf_clientType)
    ZEND_ABSTRACT_ME(UploadedFileInterface, type,       arginfo_uf_type)
    ZEND_ABSTRACT_ME(UploadedFileInterface, size,       arginfo_uf_size)
    ZEND_ABSTRACT_ME(UploadedFileInterface, tmpPath,    arginfo_uf_tmpPath)
    ZEND_ABSTRACT_ME(UploadedFileInterface, error,      arginfo_uf_error)
    ZEND_ABSTRACT_ME(UploadedFileInterface, isValid,    arginfo_uf_isValid)
    ZEND_ABSTRACT_ME(UploadedFileInterface, moveTo,     arginfo_uf_moveTo)
    PHP_FE_END
};

static const zend_function_entry oxphp_http_request_iface_methods[] = {
    ZEND_ABSTRACT_ME(RequestInterface, method,              arginfo_req_method)
    ZEND_ABSTRACT_ME(RequestInterface, path,                arginfo_req_path)
    ZEND_ABSTRACT_ME(RequestInterface, fullUri,             arginfo_req_fullUri)
    ZEND_ABSTRACT_ME(RequestInterface, scheme,              arginfo_req_scheme)
    ZEND_ABSTRACT_ME(RequestInterface, host,                arginfo_req_host)
    ZEND_ABSTRACT_ME(RequestInterface, port,                arginfo_req_port)
    ZEND_ABSTRACT_ME(RequestInterface, queryString,         arginfo_req_queryString)
    ZEND_ABSTRACT_ME(RequestInterface, isSecure,            arginfo_req_isSecure)
    ZEND_ABSTRACT_ME(RequestInterface, isMethod,            arginfo_req_isMethod)
    ZEND_ABSTRACT_ME(RequestInterface, httpProtocol,        arginfo_req_httpProtocol)
    ZEND_ABSTRACT_ME(RequestInterface, httpProtocolVersion, arginfo_req_httpProtocolVersion)
    ZEND_ABSTRACT_ME(RequestInterface, query,               arginfo_req_query)
    ZEND_ABSTRACT_ME(RequestInterface, payload,             arginfo_req_payload)
    ZEND_ABSTRACT_ME(RequestInterface, header,              arginfo_req_header)
    ZEND_ABSTRACT_ME(RequestInterface, headers,             arginfo_req_headers)
    ZEND_ABSTRACT_ME(RequestInterface, hasHeader,           arginfo_req_hasHeader)
    ZEND_ABSTRACT_ME(RequestInterface, cookie,              arginfo_req_cookie)
    ZEND_ABSTRACT_ME(RequestInterface, cookies,             arginfo_req_cookies)
    ZEND_ABSTRACT_ME(RequestInterface, body,                arginfo_req_body)
    ZEND_ABSTRACT_ME(RequestInterface, contentType,         arginfo_req_contentType)
    ZEND_ABSTRACT_ME(RequestInterface, ip,                  arginfo_req_ip)
    ZEND_ABSTRACT_ME(RequestInterface, startTime,           arginfo_req_startTime)
    ZEND_ABSTRACT_ME(RequestInterface, attributes,          arginfo_req_attributes)
    ZEND_ABSTRACT_ME(RequestInterface, session,             arginfo_req_session)
    ZEND_ABSTRACT_ME(RequestInterface, file,                arginfo_req_file)
    ZEND_ABSTRACT_ME(RequestInterface, files,               arginfo_req_files)
    PHP_FE_END
};

/* ─── OxPHP\Server\Worker — ZEND_METHOD implementations ─────── */

/* {{{ OxPHP\Server\Worker::current(): OxPHP\Server\Worker
 * Per-thread cached singleton, lazily allocated on first call. */
ZEND_METHOD(OxPHP_Server_Worker, current) {
    ZEND_PARSE_PARAMETERS_NONE();

    if (oxphp_worker_singleton == NULL) {
        oxphp_worker_singleton = (zval *)pemalloc(sizeof(zval), 1);
        object_init_ex(oxphp_worker_singleton, oxphp_worker_ce);
    }

    /* Return a copy of the cached zval; bump refcount so the caller's
     * eventual zval_ptr_dtor doesn't free our singleton. */
    ZVAL_COPY(return_value, oxphp_worker_singleton);
}
/* }}} */

/* {{{ OxPHP\Server\Worker::isWorkerMode(): bool */
ZEND_METHOD(OxPHP_Server_Worker, isWorkerMode) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_BOOL(oxphp_bridge_is_worker_mode());
}
/* }}} */

/* {{{ OxPHP\Server\Worker::id(): int */
ZEND_METHOD(OxPHP_Server_Worker, id) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_LONG((zend_long)oxphp_bridge_get_worker_id());
}
/* }}} */

/* {{{ OxPHP\Server\Worker::startTime(): float */
ZEND_METHOD(OxPHP_Server_Worker, startTime) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_DOUBLE(oxphp_bridge_get_worker_start_time());
}
/* }}} */

/* {{{ OxPHP\Server\Worker::requestCount(): int */
ZEND_METHOD(OxPHP_Server_Worker, requestCount) {
    ZEND_PARSE_PARAMETERS_NONE();
    /* Single source of truth: bridge counter, incremented at request
     * start by Rust (both modes). 1-based per OS thread. */
    RETURN_LONG((zend_long)oxphp_bridge_get_requests_done());
}
/* }}} */

/* {{{ OxPHP\Server\Worker::memoryUsage(): int */
ZEND_METHOD(OxPHP_Server_Worker, memoryUsage) {
    ZEND_PARSE_PARAMETERS_NONE();
    /* Live Zend allocator usage. Bridge's stored value is updated only
     * post-request — mid-handler we want what the script is using right
     * now, so call zend_memory_usage() directly. */
    RETURN_LONG((zend_long)zend_memory_usage(0));
}
/* }}} */

/* {{{ OxPHP\Server\Worker::rss(): int */
ZEND_METHOD(OxPHP_Server_Worker, rss) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_LONG((zend_long)oxphp_bridge_get_rss_bytes());
}
/* }}} */

/* {{{ OxPHP\Server\Worker::maxMemoryBytes(): int */
ZEND_METHOD(OxPHP_Server_Worker, maxMemoryBytes) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_LONG((zend_long)oxphp_bridge_get_max_memory_bytes());
}
/* }}} */

/* {{{ OxPHP\Server\Worker::scheduleExit(): void
 * Mark the worker for graceful exit after the current request completes.
 * No-op outside worker mode (script is exiting anyway). Idempotent. */
ZEND_METHOD(OxPHP_Server_Worker, scheduleExit) {
    ZEND_PARSE_PARAMETERS_NONE();
    if (!oxphp_bridge_is_worker_mode()) {
        return;
    }
    oxphp_bridge_schedule_exit();
}
/* }}} */

/* {{{ OxPHP\Server\Worker::isExitScheduled(): bool
 * True iff scheduleExit() has been called for the current worker.
 * Always false in traditional mode. */
ZEND_METHOD(OxPHP_Server_Worker, isExitScheduled) {
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_BOOL(oxphp_bridge_is_exit_scheduled());
}
/* }}} */

/* {{{ OxPHP\Server\Worker::exitReason(): ?string
 * Returns null when no exit pending; otherwise one of
 * 'scheduled' | 'max_memory' | 'error'. Always null in traditional mode. */
ZEND_METHOD(OxPHP_Server_Worker, exitReason) {
    ZEND_PARSE_PARAMETERS_NONE();
    uint8_t r = oxphp_bridge_get_exit_reason();
    switch (r) {
        case 1: RETURN_STRING("scheduled");
        case 2: RETURN_STRING("max_memory");
        case 3: RETURN_STRING("error");
        default: RETURN_NULL();
    }
}
/* }}} */

/* {{{ OxPHP\Server\Worker::serve(callable $handler): void
 * Enter the worker request-dispatch loop. Throws InvalidServeContextException
 * when called outside worker mode or re-entered on the same thread. */
ZEND_METHOD(OxPHP_Server_Worker, serve) {
    zend_fcall_info fci;
    zend_fcall_info_cache fcc;

    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_FUNC(fci, fcc)
    ZEND_PARSE_PARAMETERS_END();

    if (!oxphp_bridge_is_worker_mode()) {
        zend_throw_exception(
            oxphp_invalid_serve_ctx_exc_ce,
            "Worker::serve() is only valid in worker mode "
            "(set WORKER_MODE_ENABLED=true and ENTRY_FILE=...)",
            0
        );
        RETURN_THROWS();
    }

    if (oxphp_serve_in_progress) {
        zend_throw_exception(
            oxphp_invalid_serve_ctx_exc_ce,
            "Worker::serve() is already running on this thread "
            "(nested calls are not supported)",
            0
        );
        RETURN_THROWS();
    }

    oxphp_serve_in_progress = true;
    zend_try {
        oxphp_serve_loop(&fci, &fcc);
    } zend_catch {
        oxphp_serve_in_progress = false;
        zend_bailout();
    } zend_end_try();
    oxphp_serve_in_progress = false;
}
/* }}} */

ZEND_BEGIN_ARG_WITH_RETURN_OBJ_INFO_EX(arginfo_oxphp_worker_current, 0, 0,
    OxPHP\\Server\\Worker, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_isWorkerMode, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

/* Class method arginfo; free-function `oxphp_worker_id()` already owns
 * `arginfo_oxphp_worker_id`, so the class entry uses a distinct symbol. */
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_serverworker_id, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_startTime, 0, 0, IS_DOUBLE, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_requestCount, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_memoryUsage, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_rss, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_maxMemoryBytes, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_scheduleExit, 0, 0, IS_VOID, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_isExitScheduled, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_exitReason, 0, 0, IS_STRING, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_serve, 0, 1, IS_VOID, 0)
    ZEND_ARG_TYPE_INFO(0, handler, IS_CALLABLE, 0)
ZEND_END_ARG_INFO()

/* OxPHP\Server\Worker — methods added by subsequent tasks. Kept extensible
 * (file-scope, not static const) so additional method handlers can append
 * entries. */
static zend_function_entry oxphp_worker_methods[] = {
    ZEND_ME(OxPHP_Server_Worker, current,            arginfo_oxphp_worker_current,
            ZEND_ACC_PUBLIC | ZEND_ACC_STATIC)
    ZEND_ME(OxPHP_Server_Worker, isWorkerMode,       arginfo_oxphp_worker_isWorkerMode,
            ZEND_ACC_PUBLIC | ZEND_ACC_STATIC)
    ZEND_ME(OxPHP_Server_Worker, id,                 arginfo_oxphp_serverworker_id,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, startTime,          arginfo_oxphp_worker_startTime,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, requestCount,       arginfo_oxphp_worker_requestCount,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, memoryUsage,        arginfo_oxphp_worker_memoryUsage,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, rss,                arginfo_oxphp_worker_rss,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, maxMemoryBytes,     arginfo_oxphp_worker_maxMemoryBytes,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, scheduleExit,       arginfo_oxphp_worker_scheduleExit,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, isExitScheduled,    arginfo_oxphp_worker_isExitScheduled,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, exitReason,         arginfo_oxphp_worker_exitReason,
            ZEND_ACC_PUBLIC)
    ZEND_ME(OxPHP_Server_Worker, serve,              arginfo_oxphp_worker_serve,
            ZEND_ACC_PUBLIC)
    PHP_FE_END
};

/* Forward-declare clone-disallow handler for OxPHP\Server\Worker. */
static zend_object *oxphp_worker_clone_object(zend_object *object);

/* {{{ arginfo */
ZEND_BEGIN_ARG_WITH_RETURN_OBJ_INFO_EX(arginfo_oxphp_http_request, 0, 0,
    OxPHP\\Http\\RequestInterface, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_superglobals_enabled, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_request_id, 0, 0, IS_STRING, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker_id, 0, 0, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_server_info, 0, 0, IS_ARRAY, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_finish_request, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_is_worker, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_is_streaming, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_stream_flush, 0, 0, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_sleep, 0, 1, IS_VOID, 0)
    ZEND_ARG_TYPE_INFO(0, seconds, IS_DOUBLE, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_usleep, 0, 1, IS_VOID, 0)
    ZEND_ARG_TYPE_INFO(0, microseconds, IS_LONG, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_worker, 0, 1, _IS_BOOL, 0)
    ZEND_ARG_CALLABLE_INFO(0, handler, 0)
ZEND_END_ARG_INFO()

/* }}} */

/* {{{ function entries */
static const zend_function_entry oxphp_sapi_functions[] = {
    PHP_FE(oxphp_http_request,      arginfo_oxphp_http_request)
    PHP_FE(oxphp_superglobals_enabled, arginfo_oxphp_superglobals_enabled)
    PHP_FE(oxphp_request_id,        arginfo_oxphp_request_id)
    PHP_FE(oxphp_worker_id,         arginfo_oxphp_worker_id)
    PHP_FE(oxphp_server_info,       arginfo_oxphp_server_info)
    PHP_FE(oxphp_finish_request,    arginfo_oxphp_finish_request)
    PHP_FE(oxphp_is_worker,          arginfo_oxphp_is_worker)
    PHP_FE(oxphp_is_streaming,      arginfo_oxphp_is_streaming)
    PHP_FE(oxphp_stream_flush,      arginfo_oxphp_stream_flush)
    PHP_FE(oxphp_sleep,             arginfo_oxphp_sleep)
    PHP_FE(oxphp_usleep,            arginfo_oxphp_usleep)
    PHP_FE(oxphp_worker,            arginfo_oxphp_worker)
    PHP_FE(oxphp_register_decorator,     arginfo_oxphp_register_decorator)
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

/* ─── Dynamic arginfo builder for return types ──────────────
 * Maps OXPHP_RT_* bridge constants to PHP's Zend type codes and
 * allocates a one-element zend_internal_arg_info array encoding
 * the return type. Returns NULL for OXPHP_RT_NONE (no type). */
static int oxphp_rt_to_zend(int rt) {
    switch (rt) {
        case OXPHP_RT_NULL:     return IS_NULL;
        case OXPHP_RT_BOOL:     return _IS_BOOL;
        case OXPHP_RT_INT:      return IS_LONG;
        case OXPHP_RT_FLOAT:    return IS_DOUBLE;
        case OXPHP_RT_STRING:   return IS_STRING;
        case OXPHP_RT_ARRAY:    return IS_ARRAY;
        case OXPHP_RT_OBJECT:   return IS_OBJECT;
        case OXPHP_RT_MIXED:    return IS_MIXED;
        case OXPHP_RT_VOID:     return IS_VOID;
        case OXPHP_RT_CALLABLE: return IS_CALLABLE;
        case OXPHP_RT_ITERABLE: return IS_ITERABLE;
        case OXPHP_RT_NEVER:    return IS_NEVER;
        case OXPHP_RT_FALSE:    return IS_FALSE;
        case OXPHP_RT_TRUE:     return IS_TRUE;
        case OXPHP_RT_SELF:     return IS_STATIC; /* self/static → IS_STATIC for internals */
        case OXPHP_RT_STATIC:   return IS_STATIC;
        case OXPHP_RT_PARENT:   return IS_STATIC;
        default:                return -1;
    }
}

static const zend_internal_arg_info *oxphp_build_return_arginfo(int rt, int nullable) {
    if (rt == OXPHP_RT_NONE) return NULL;

    int zend_type = oxphp_rt_to_zend(rt);
    if (zend_type < 0) return NULL;

    /* Compute type_mask the same way ZEND_TYPE_INIT_CODE does */
    uint32_t mask;
    if (zend_type == _IS_BOOL) {
        mask = MAY_BE_FALSE | MAY_BE_TRUE;
    } else {
        mask = (1u << zend_type);
    }
    if (nullable) {
        mask |= MAY_BE_NULL;
    }

    /* Allocate one entry: the return type info (element [0] of arginfo array).
     * Uses calloc (module-level allocation, not request-level). */
    zend_internal_arg_info *info = calloc(1, sizeof(zend_internal_arg_info));
    if (!info) return NULL;

    info[0].name = (const char *)(zend_uintptr_t)(0); /* required_num_args = 0 */
    info[0].type.type_mask = mask;
    /* ptr and ce_cache are zeroed by calloc */

    return info;
}
/* }}} */

/* {{{ Build a full per-method/function arginfo with parameter names.
 *
 * Returns a `(1 + total_params)` array. Slot 0 carries the return type info;
 * slots 1..total_params carry each parameter's name and an MAY_BE_ANY type
 * mask so existing callers that pass mismatched types are not rejected at
 * the PHP layer (the dispatch hop into Rust is what enforces typing today).
 * The variadic bit is set on the last slot when `is_variadic` is true.
 *
 * `param_names[i]` must point to long-lived storage — the bridge strdup's
 * names at registration time, so the pointer can be borrowed directly.
 */
static const zend_internal_arg_info *oxphp_build_method_arginfo(
    int required_params, int total_params, int is_variadic,
    int return_type, int return_nullable,
    const char * const *param_names)
{
    if (total_params <= 0) {
        return oxphp_build_return_arginfo(return_type, return_nullable);
    }

    int slots = 1 + total_params;
    zend_internal_arg_info *info = calloc(slots, sizeof(zend_internal_arg_info));
    if (!info) return NULL;

    /* Slot 0: return-type. The `name` field encodes required_num_args here
     * (per PHP's internal convention used by ZEND_BEGIN_ARG_INFO_EX). */
    info[0].name = (const char *)(zend_uintptr_t)required_params;
    uint32_t return_mask = 0;
    int return_zend_type = oxphp_rt_to_zend(return_type);
    if (return_type != OXPHP_RT_NONE && return_zend_type >= 0) {
        if (return_zend_type == _IS_BOOL) {
            return_mask = MAY_BE_FALSE | MAY_BE_TRUE;
        } else {
            return_mask = (1u << return_zend_type);
        }
        if (return_nullable) {
            return_mask |= MAY_BE_NULL;
        }
    }
    info[0].type.type_mask = return_mask;

    /* Slots 1..total_params: per-parameter info. */
    for (int i = 0; i < total_params; i++) {
        const char *pname = (param_names && param_names[i]) ? param_names[i] : "_";
        info[1 + i].name = pname;
        uint32_t mask = MAY_BE_ANY;
        if (is_variadic && i == total_params - 1) {
            mask |= _ZEND_IS_VARIADIC_BIT;
        }
        info[1 + i].type.type_mask = mask;
    }

    return info;
}
/* }}} */

/* Clone-disallow handler for OxPHP\Server\Worker. */
static zend_object *oxphp_worker_clone_object(zend_object *object) {
    (void)object;
    zend_throw_error(NULL, "Cloning OxPHP\\Server\\Worker is not allowed");
    /* Engine's ZEND_CLONE opcode dereferences the return value
     * unconditionally before checking for the thrown exception on the
     * next opcode. Return a fresh empty object to satisfy the contract;
     * it will be GC'd before any user code observes it. */
    return zend_objects_new(oxphp_worker_ce);
}

/* ── Sub-design A: zend_interrupt_function override ──
 * Centralised cancellation bailout. When a CancelReason is set on
 * the active request and EG(vm_interrupt) becomes 1, Zend calls
 * this handler at the next opcode boundary. We mirror to
 * PG(connection_status), respect ignore_user_abort for ClientAbort,
 * then bail through zend_error_noreturn — the same path as a
 * regular PHP fatal error, so registered shutdown handlers run. */

static void (*orig_zend_interrupt_function)(zend_execute_data*) = NULL;

static const char* oxphp_cancel_reason_label(oxphp_cancel_reason_t r)
{
    switch (r) {
        case OXPHP_CANCEL_CLIENT_ABORT: return "client_abort";
        case OXPHP_CANCEL_TIMEOUT:      return "timeout";
        case OXPHP_CANCEL_SHUTDOWN:     return "shutdown";
        case OXPHP_CANCEL_STUCK:        return "stuck";
        case OXPHP_CANCEL_USER:         return "user_cancel";
        default:                        return "unknown";
    }
}

static void oxphp_zend_interrupt_handler(zend_execute_data *execute_data)
{
    /* SIGALRM-driven max_execution_time: Zend sets EG(timed_out)=1
     * alongside vm_interrupt. Convert it to the unified cancellation
     * reason and claim the flag so zend_timeout()'s default
     * "Maximum execution time exceeded" path doesn't also fire. */
    if (zend_atomic_bool_load_ex(&EG(timed_out))) {
        oxphp_bridge_set_cancel_reason(OXPHP_CANCEL_TIMEOUT);
        zend_atomic_bool_store_ex(&EG(timed_out), false);
    }

    oxphp_cancel_reason_t reason = oxphp_bridge_get_cancel_reason();

    if (reason == OXPHP_CANCEL_NONE) {
        if (orig_zend_interrupt_function) {
            orig_zend_interrupt_function(execute_data);
        }
        return;
    }

    if (reason == OXPHP_CANCEL_CLIENT_ABORT) {
        PG(connection_status) |= PHP_CONNECTION_ABORTED;
        if (PG(ignore_user_abort)) {
            return;
        }
    } else if (reason == OXPHP_CANCEL_TIMEOUT) {
        PG(connection_status) |= PHP_CONNECTION_TIMEOUT;
    } else {
        /* OXPHP_CANCEL_SHUTDOWN / _STUCK / _USER: the connection is no
         * longer being serviced (server going away, supervisor giving up,
         * userland-initiated cancel). Mirror to PHP_CONNECTION_ABORTED so
         * shutdown handlers calling connection_aborted() / connection_status()
         * observe a non-zero state instead of PHP_CONNECTION_NORMAL. */
        PG(connection_status) |= PHP_CONNECTION_ABORTED;
    }

    zend_error_noreturn(E_ERROR,
        "Request cancelled (%s)",
        oxphp_cancel_reason_label(reason));
    /* unreachable: zend_error_noreturn calls zend_bailout() */
}

/* Own the max_execution_time ini handler so future revisions can
 * extend its behaviour without surgical patches. Today this is a thin
 * pass-through; worker-mode and the STARTUP/DEACTIVATE stages are
 * explicit early-exits so the hook is ready for later mirror logic. */
static PHP_INI_MH((*orig_OnUpdateTimeout)) = NULL;

static PHP_INI_MH(oxphp_OnUpdateTimeout)
{
    /* Run upstream first — it updates EG(timeout_seconds) and (re)arms
     * SIGALRM via zend_set_timeout. We never replace its behaviour. */
    int rc = orig_OnUpdateTimeout(entry, new_value, mh_arg1, mh_arg2,
                                  mh_arg3, stage);

    if (oxphp_bridge_is_worker_mode()) {
        return rc;
    }
    if (stage == PHP_INI_STAGE_STARTUP || stage == PHP_INI_STAGE_DEACTIVATE) {
        return rc;
    }
    /* Reserved for future mirror logic. SIGALRM is the source of truth;
     * the interrupt handler converts EG(timed_out) into CancelReason. */
    return rc;
}

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
                /* num_args MUST match arginfo entry count. arginfo_oxphp_native_dispatch
                   is variadic with 0 required args, so num_args must be 0. Setting it to
                   the actual param count causes out-of-bounds read in arginfo → SIGSEGV. */
                entries[i].num_args = 0;
                entries[i].flags = 0;
            }
            /* Sentinel: last entry is all-zeroes (from calloc). */
            zend_register_functions(NULL, entries, NULL, MODULE_PERSISTENT);
            free(entries);
        }
    }

    /* ═══════════════════════════════════════════════════════════
     * Register builder-based plugin functions (new API)
     * ═══════════════════════════════════════════════════════════ */
    {
        int fn_count = oxphp_bridge_get_plugin_function_count();
        if (fn_count > 0) {
            zend_function_entry *fn_entries = calloc(fn_count + 1, sizeof(zend_function_entry));
            if (fn_entries) {
                for (int i = 0; i < fn_count; i++) {
                    fn_entries[i].fname = oxphp_bridge_get_plugin_function_fqn(i);
                    fn_entries[i].handler = ZEND_FN(oxphp_native_dispatch);
                    int rt = oxphp_bridge_get_plugin_function_return_type(i);
                    int rn = oxphp_bridge_get_plugin_function_return_nullable(i);
                    int total = oxphp_bridge_get_plugin_function_total(i);
                    int required = oxphp_bridge_get_plugin_function_required(i);
                    int is_variadic = oxphp_bridge_get_plugin_function_is_variadic(i);
                    const char **pnames = NULL;
                    if (total > 0) {
                        pnames = calloc(total, sizeof(const char *));
                        if (pnames) {
                            for (int p = 0; p < total; p++) {
                                pnames[p] = oxphp_bridge_get_plugin_function_param_name(i, p);
                            }
                        }
                    }
                    const zend_internal_arg_info *info = oxphp_build_method_arginfo(
                        required, total, is_variadic, rt, rn, pnames);
                    free((void *)pnames);
                    fn_entries[i].arg_info = info
                        ? info
                        : (const zend_internal_arg_info *)arginfo_oxphp_native_dispatch;
                    /* num_args MUST equal the number of param slots in arg_info[1..],
                     * otherwise PHP indexes past the array. With param names
                     * present, that's `total`. With no params, fall back to 0. */
                    fn_entries[i].num_args = info ? (uint32_t)total : 0;
                    fn_entries[i].flags = 0;
                }
                zend_register_functions(NULL, fn_entries, NULL, MODULE_PERSISTENT);
                free(fn_entries);
            }
        }
    }

    /* ═══════════════════════════════════════════════════════════
     * Register plugin interfaces
     * ═══════════════════════════════════════════════════════════ */
    {
        int iface_count = oxphp_bridge_get_plugin_interface_count();
        for (int i = 0; i < iface_count; i++) {
            const char *fqn = oxphp_bridge_get_interface_fqn(i);
            const char *parent = oxphp_bridge_get_interface_parent(i);
            if (!fqn) continue;

            /* Build method entries for the interface */
            int mcount = oxphp_bridge_get_interface_method_count(i);
            zend_function_entry *methods = calloc(mcount + 1, sizeof(zend_function_entry));
            if (methods) {
                for (int m = 0; m < mcount; m++) {
                    methods[m].fname = oxphp_bridge_get_interface_method_name(i, m);
                    methods[m].handler = NULL; /* interface methods have no handler */
                    int rt = oxphp_bridge_get_interface_method_return_type(i, m);
                    int rn = oxphp_bridge_get_interface_method_return_nullable(i, m);
                    int total = oxphp_bridge_get_interface_method_total(i, m);
                    int required = oxphp_bridge_get_interface_method_required(i, m);
                    int is_variadic = oxphp_bridge_get_interface_method_is_variadic(i, m);
                    const char **pnames = NULL;
                    if (total > 0) {
                        pnames = calloc(total, sizeof(const char *));
                        if (pnames) {
                            for (int p = 0; p < total; p++) {
                                pnames[p] = oxphp_bridge_get_interface_method_param_name(i, m, p);
                            }
                        }
                    }
                    const zend_internal_arg_info *info = oxphp_build_method_arginfo(
                        required, total, is_variadic, rt, rn, pnames);
                    free((void *)pnames);
                    methods[m].arg_info = info
                        ? info
                        : (const zend_internal_arg_info *)arginfo_oxphp_method_dispatch;
                    methods[m].num_args = info ? (uint32_t)total : 0;
                    methods[m].flags = oxphp_bridge_get_interface_method_flags(i, m)
                                     | ZEND_ACC_ABSTRACT | ZEND_ACC_PUBLIC;
                }
            }

            zend_class_entry tmp_ce;
            INIT_CLASS_ENTRY_EX(tmp_ce, fqn, strlen(fqn), methods);

            zend_class_entry *iface_ce;
            if (parent) {
                size_t plen = strlen(parent);
                char *lc = emalloc(plen + 1);
                zend_str_tolower_copy(lc, parent, plen);
                zend_class_entry *parent_ce = zend_hash_str_find_ptr(CG(class_table), lc, plen);
                efree(lc);
                iface_ce = zend_register_internal_interface(&tmp_ce);
                if (parent_ce) {
                    zend_class_implements(iface_ce, 1, parent_ce);
                }
            } else {
                iface_ce = zend_register_internal_interface(&tmp_ce);
            }

            /* Register interface constants */
            int kcount = oxphp_bridge_get_interface_constant_count(i);
            for (int k = 0; k < kcount; k++) {
                const char *kname = oxphp_bridge_get_interface_constant_name(i, k);
                const char *kval = oxphp_bridge_get_interface_constant_value(i, k);
                if (kname && kval) {
                    zval zv;
                    char *endptr;
                    long lval = strtol(kval, &endptr, 10);
                    if (*endptr == '\0' && kval[0] != '\0') {
                        ZVAL_LONG(&zv, lval);
                    } else {
                        ZVAL_STRING(&zv, kval);
                    }
                    zend_declare_class_constant(iface_ce, kname, strlen(kname), &zv);
                }
            }

            free(methods);
        }
    }

    /* ═══════════════════════════════════════════════════════════
     * Register plugin enums
     * ═══════════════════════════════════════════════════════════ */
    {
        int enum_count = oxphp_bridge_get_plugin_enum_count();
        for (int i = 0; i < enum_count; i++) {
            const char *fqn = oxphp_bridge_get_enum_fqn(i);
            int backing = oxphp_bridge_get_enum_backing_type(i);
            if (!fqn) continue;

            /* Build method entries */
            int mcount = oxphp_bridge_get_enum_method_count(i);
            zend_function_entry *methods = calloc(mcount + 1, sizeof(zend_function_entry));
            if (methods) {
                for (int m = 0; m < mcount; m++) {
                    methods[m].fname = oxphp_bridge_get_enum_method_name(i, m);
                    methods[m].handler = ZEND_FN(oxphp_method_dispatch);
                    int rt = oxphp_bridge_get_enum_method_return_type(i, m);
                    int rn = oxphp_bridge_get_enum_method_return_nullable(i, m);
                    int total = oxphp_bridge_get_enum_method_total(i, m);
                    int required = oxphp_bridge_get_enum_method_required(i, m);
                    int is_variadic = oxphp_bridge_get_enum_method_is_variadic(i, m);
                    const char **pnames = NULL;
                    if (total > 0) {
                        pnames = calloc(total, sizeof(const char *));
                        if (pnames) {
                            for (int p = 0; p < total; p++) {
                                pnames[p] = oxphp_bridge_get_enum_method_param_name(i, m, p);
                            }
                        }
                    }
                    const zend_internal_arg_info *info = oxphp_build_method_arginfo(
                        required, total, is_variadic, rt, rn, pnames);
                    free((void *)pnames);
                    methods[m].arg_info = info
                        ? info
                        : (const zend_internal_arg_info *)arginfo_oxphp_method_dispatch;
                    methods[m].num_args = info ? (uint32_t)total : 0;
                    methods[m].flags = oxphp_bridge_get_enum_method_flags(i, m);
                }
            }

            /* backing: 0=unit, 4=IS_LONG, 6=IS_STRING */
            zend_class_entry *enum_ce = zend_register_internal_enum(
                fqn, backing == 0 ? IS_UNDEF : (zend_uchar)backing, methods);

            /* Implement interfaces */
            int icount = oxphp_bridge_get_enum_interface_count(i);
            for (int j = 0; j < icount; j++) {
                const char *ifqn = oxphp_bridge_get_enum_interface_fqn(i, j);
                if (ifqn) {
                    size_t ilen = strlen(ifqn);
                    char *lc = emalloc(ilen + 1);
                    zend_str_tolower_copy(lc, ifqn, ilen);
                    zend_class_entry *iface_ce = zend_hash_str_find_ptr(CG(class_table), lc, ilen);
                    efree(lc);
                    if (iface_ce) {
                        zend_class_implements(enum_ce, 1, iface_ce);
                    }
                }
            }

            /* Add cases */
            int ccount = oxphp_bridge_get_enum_case_count(i);
            for (int c = 0; c < ccount; c++) {
                const char *cname = oxphp_bridge_get_enum_case_name(i, c);
                const char *cval = oxphp_bridge_get_enum_case_value(i, c);
                if (!cname) continue;

                if (backing == 0) {
                    /* Unit enum */
                    zend_enum_add_case_cstr(enum_ce, cname, NULL);
                } else if (backing == 4) {
                    /* Int-backed */
                    zval zv;
                    ZVAL_LONG(&zv, cval ? strtol(cval, NULL, 10) : 0);
                    zend_enum_add_case_cstr(enum_ce, cname, &zv);
                } else if (backing == 6) {
                    /* String-backed */
                    zval zv;
                    ZVAL_STRING(&zv, cval ? cval : "");
                    zend_enum_add_case_cstr(enum_ce, cname, &zv);
                    zval_ptr_dtor(&zv);
                }
            }

            free(methods);
        }
    }

    /* Register OxPHP\Shared\Shareable interface BEFORE plugin classes so
     * `.implements("OxPHP\\Shared\\Shareable")` on Counter/Flag/Once can
     * resolve during the plugin class registration loop below. Without
     * this the interface lookup in the loop returns NULL and
     * zend_class_implements is skipped silently. */
    if (oxphp_shareable_register_ce() == FAILURE) {
        return FAILURE;
    }

    /* ═══════════════════════════════════════════════════════════
     * Register plugin classes
     * ═══════════════════════════════════════════════════════════ */
    {
        int cls_count = oxphp_bridge_get_plugin_class_count();
        if (cls_count > 0) {
            /* Initialize custom object infrastructure in the bridge */
            oxphp_plugin_init_custom_objects(cls_count);

            for (int i = 0; i < cls_count; i++) {
                const char *fqn = oxphp_bridge_get_class_fqn(i);
                const char *parent_fqn = oxphp_bridge_get_class_parent(i);
                uint32_t cls_flags = oxphp_bridge_get_class_flags(i);
                int has_custom = oxphp_bridge_get_class_has_custom_object(i);
                if (!fqn) continue;

                /* Build method entries */
                int mcount = oxphp_bridge_get_class_method_count(i);
                zend_function_entry *methods = calloc(mcount + 1, sizeof(zend_function_entry));
                if (methods) {
                    for (int m = 0; m < mcount; m++) {
                        methods[m].fname = oxphp_bridge_get_class_method_name(i, m);
                        methods[m].handler = ZEND_FN(oxphp_method_dispatch);
                        int rt = oxphp_bridge_get_class_method_return_type(i, m);
                        int rn = oxphp_bridge_get_class_method_return_nullable(i, m);
                        int total = oxphp_bridge_get_class_method_total(i, m);
                        int required = oxphp_bridge_get_class_method_required(i, m);
                        int is_variadic = oxphp_bridge_get_class_method_is_variadic(i, m);
                        const char **pnames = NULL;
                        if (total > 0) {
                            pnames = calloc(total, sizeof(const char *));
                            if (pnames) {
                                for (int p = 0; p < total; p++) {
                                    pnames[p] = oxphp_bridge_get_class_method_param_name(i, m, p);
                                }
                            }
                        }
                        const zend_internal_arg_info *info = oxphp_build_method_arginfo(
                            required, total, is_variadic, rt, rn, pnames);
                        free((void *)pnames);
                        methods[m].arg_info = info
                            ? info
                            : (const zend_internal_arg_info *)arginfo_oxphp_method_dispatch;
                        methods[m].num_args = info ? (uint32_t)total : 0;
                        methods[m].flags = oxphp_bridge_get_class_method_visibility(i, m)
                                         | oxphp_bridge_get_class_method_flags(i, m);
                    }
                }

                zend_class_entry tmp_ce;

                /* Look up parent class entry if specified.
                 * During MINIT, zend_lookup_class() is unsafe (triggers autoload/executor init).
                 * Use direct class_table lookup instead. Names must be lowercased for the
                 * hash lookup (PHP class names are case-insensitive). */
                zend_class_entry *parent_ce = NULL;
                if (parent_fqn) {
                    size_t parent_len = strlen(parent_fqn);
                    char *lc_parent = emalloc(parent_len + 1);
                    zend_str_tolower_copy(lc_parent, parent_fqn, parent_len);
                    parent_ce = zend_hash_str_find_ptr(CG(class_table), lc_parent, parent_len);
                    efree(lc_parent);
                }

                INIT_CLASS_ENTRY_EX(tmp_ce, fqn, strlen(fqn), methods);
                zend_class_entry *cls_ce = zend_register_internal_class_ex(&tmp_ce, parent_ce);

                /* Wire BorrowedProxy CE for async borrow mechanism */
                if (strcmp(fqn, "OxPHP\\Async\\BorrowedProxy") == 0) {
                    oxphp_bridge_set_borrow_proxy_ce(cls_ce);
                }

                /* Apply class flags */
                cls_ce->ce_flags |= cls_flags;

                /* Store CE in our lookup array */
                oxphp_plugin_set_class_ce(i, cls_ce);

                /* Set up object handlers */
                zend_object_handlers *handlers = oxphp_plugin_get_handlers(i);
                if (has_custom) {
                    cls_ce->create_object = oxphp_plugin_create_object;
                    handlers->free_obj = oxphp_plugin_free_object;
                    handlers->clone_obj = oxphp_plugin_clone_object;
                    cls_ce->default_object_handlers = handlers;
                }
                /* No `else`: for classes without custom storage, leave
                 * default_object_handlers inherited from the parent.
                 * Overriding with our custom-handlers slot (which has
                 * offset = XtOffsetOf(oxphp_custom_object, std) = 16 to
                 * reach the outer wrapper) would poison plain zend_object
                 * allocations (e.g. `new TypeException()` created via
                 * zend_throw_exception), since those objects have no
                 * oxphp_custom_object prefix — PHP's property/GC paths
                 * read `handlers->offset` and compute a wrong outer ptr,
                 * causing SIGSEGV once the exception is freed. */

                /* Implement interfaces */
                int icount = oxphp_bridge_get_class_interface_count(i);
                for (int j = 0; j < icount; j++) {
                    const char *ifqn = oxphp_bridge_get_class_interface_fqn(i, j);
                    if (ifqn) {
                        size_t ilen = strlen(ifqn);
                        char *lc = emalloc(ilen + 1);
                        zend_str_tolower_copy(lc, ifqn, ilen);
                        zend_class_entry *iface_ce = zend_hash_str_find_ptr(CG(class_table), lc, ilen);
                        efree(lc);
                        if (iface_ce) {
                            zend_class_implements(cls_ce, 1, iface_ce);
                        }
                    }
                }

                /* Declare properties */
                int pcount = oxphp_bridge_get_class_property_count(i);
                for (int p = 0; p < pcount; p++) {
                    const char *pname = oxphp_bridge_get_class_property_name(i, p);
                    uint32_t pvis = oxphp_bridge_get_class_property_visibility(i, p);
                    uint32_t pmods = oxphp_bridge_get_class_property_modifiers(i, p);
                    const char *pdefault = oxphp_bridge_get_class_property_default(i, p);
                    if (!pname) continue;

                    uint32_t access = pvis | pmods;
                    if (pdefault) {
                        /* Try to parse as int, float, bool, null, or fall back to string */
                        char *endptr;
                        long lval = strtol(pdefault, &endptr, 10);
                        if (*endptr == '\0' && pdefault[0] != '\0') {
                            zend_declare_property_long(cls_ce, pname, strlen(pname), lval, access);
                        } else if (strcmp(pdefault, "true") == 0) {
                            zend_declare_property_bool(cls_ce, pname, strlen(pname), 1, access);
                        } else if (strcmp(pdefault, "false") == 0) {
                            zend_declare_property_bool(cls_ce, pname, strlen(pname), 0, access);
                        } else if (strcmp(pdefault, "null") == 0) {
                            zend_declare_property_null(cls_ce, pname, strlen(pname), access);
                        } else {
                            double dval = strtod(pdefault, &endptr);
                            if (*endptr == '\0' && pdefault[0] != '\0') {
                                zend_declare_property_double(cls_ce, pname, strlen(pname), dval, access);
                            } else {
                                zend_declare_property_string(cls_ce, pname, strlen(pname), pdefault, access);
                            }
                        }
                    } else {
                        zend_declare_property_null(cls_ce, pname, strlen(pname), access);
                    }
                }

                /* Declare class constants */
                int kcount = oxphp_bridge_get_class_constant_count(i);
                for (int k = 0; k < kcount; k++) {
                    const char *kname = oxphp_bridge_get_class_constant_name(i, k);
                    const char *kval = oxphp_bridge_get_class_constant_value(i, k);
                    if (!kname || !kval) continue;

                    zval zv;
                    char *endptr;
                    long lval = strtol(kval, &endptr, 10);
                    if (*endptr == '\0' && kval[0] != '\0') {
                        ZVAL_LONG(&zv, lval);
                    } else if (strcmp(kval, "true") == 0) {
                        ZVAL_TRUE(&zv);
                    } else if (strcmp(kval, "false") == 0) {
                        ZVAL_FALSE(&zv);
                    } else if (strcmp(kval, "null") == 0) {
                        ZVAL_NULL(&zv);
                    } else {
                        ZVAL_STRING(&zv, kval);
                    }
                    zend_declare_class_constant(cls_ce, kname, strlen(kname), &zv);
                }

                free(methods);
            }
        }
    }

    /* ═══════════════════════════════════════════════════════════
     * Register plugin attributes
     * ═══════════════════════════════════════════════════════════ */
    {
        int attr_count = oxphp_bridge_get_plugin_attribute_count();
        for (int i = 0; i < attr_count; i++) {
            const char *fqn = oxphp_bridge_get_attribute_fqn(i);
            uint32_t targets = oxphp_bridge_get_attribute_targets(i);
            int is_repeatable = oxphp_bridge_get_attribute_is_repeatable(i);
            if (!fqn) continue;

            /* Attributes are registered as internal classes first,
             * then marked as attributes via zend_mark_internal_attribute(). */

            /* Build param entries as constructor arginfo — for now just use variadic */
            zend_function_entry *attr_methods = calloc(1, sizeof(zend_function_entry));
            /* sentinel only — no methods needed for simple attributes */

            zend_class_entry tmp_ce;
            INIT_CLASS_ENTRY_EX(tmp_ce, fqn, strlen(fqn), attr_methods);
            zend_class_entry *attr_ce = zend_register_internal_class(&tmp_ce);

            /* Mark as attribute with targets */
            attr_ce->ce_flags |= ZEND_ACC_NO_DYNAMIC_PROPERTIES;

            /* Build the target flags for registration */
            uint32_t attr_flags = targets;
            if (is_repeatable) {
                attr_flags |= ZEND_ATTRIBUTE_IS_REPEATABLE;
            }

            zend_internal_attribute *attr = zend_internal_attribute_register(attr_ce, attr_flags);
            (void)attr;

            /* Declare attribute properties */
            int pcount = oxphp_bridge_get_attribute_property_count(i);
            for (int p = 0; p < pcount; p++) {
                const char *pname = oxphp_bridge_get_attribute_property_name(i, p);
                uint32_t pvis = oxphp_bridge_get_attribute_property_visibility(i, p);
                if (pname) {
                    zend_declare_property_null(attr_ce, pname, strlen(pname), pvis);
                }
            }

            free(attr_methods);
        }
    }

    zend_class_entry ce;

    /* OxPHP\Http\Exception\NoActiveRequestException */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP\\Http\\Exception", "NoActiveRequestException", NULL);
    oxphp_no_active_request_ce = zend_register_internal_class_ex(&ce, spl_ce_RuntimeException);

    /* OxPHP\Http\Exception\AsyncContextException extends NoActiveRequestException */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP\\Http\\Exception", "AsyncContextException", NULL);
    oxphp_async_context_exc_ce = zend_register_internal_class_ex(&ce, oxphp_no_active_request_ce);

    /* OxPHP\Http\Exception\WorkerIdleException extends NoActiveRequestException */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP\\Http\\Exception", "WorkerIdleException", NULL);
    oxphp_worker_idle_exc_ce = zend_register_internal_class_ex(&ce, oxphp_no_active_request_ce);

    /* OxPHP\Server\Exception\InvalidServeContextException extends \RuntimeException */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP\\Server\\Exception", "InvalidServeContextException", NULL);
    oxphp_invalid_serve_ctx_exc_ce = zend_register_internal_class_ex(&ce, spl_ce_RuntimeException);

    /* OxPHP\Server\Worker — final, non-cloneable. Methods added by subsequent
     * tasks via the file-scope oxphp_worker_methods table. */
    {
        zend_class_entry tmp_worker_ce;
        INIT_CLASS_ENTRY(tmp_worker_ce, "OxPHP\\Server\\Worker", oxphp_worker_methods);
        oxphp_worker_ce = zend_register_internal_class(&tmp_worker_ce);
        oxphp_worker_ce->ce_flags |= ZEND_ACC_FINAL;

        memcpy(&oxphp_worker_object_handlers, zend_get_std_object_handlers(),
               sizeof(zend_object_handlers));
        oxphp_worker_object_handlers.clone_obj = oxphp_worker_clone_object;
        oxphp_worker_ce->default_object_handlers = &oxphp_worker_object_handlers;
    }

    /* ─── HTTP Interfaces (must register before classes) ───── */
    {
        zend_class_entry tmp_ce;

        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "AttributesInterface",
            oxphp_http_attributes_iface_methods);
        oxphp_http_attributes_iface_ce = zend_register_internal_interface(&tmp_ce);

        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "SessionInterface",
            oxphp_http_session_iface_methods);
        oxphp_http_session_iface_ce = zend_register_internal_interface(&tmp_ce);

        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "UploadedFileInterface",
            oxphp_http_uploaded_file_iface_methods);
        oxphp_http_uploaded_file_iface_ce = zend_register_internal_interface(&tmp_ce);

        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "RequestInterface",
            oxphp_http_request_iface_methods);
        oxphp_http_request_iface_ce = zend_register_internal_interface(&tmp_ce);
    }

    /* OxPHP\Http\Request */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "Request",
            oxphp_http_request_methods);
        oxphp_http_request_ce = zend_register_internal_class(&tmp_ce);
        oxphp_http_request_ce->ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
        zend_class_implements(oxphp_http_request_ce, 1, oxphp_http_request_iface_ce);
        memcpy(&oxphp_http_request_handlers, &std_object_handlers, sizeof(zend_object_handlers));
        oxphp_http_request_handlers.clone_obj = NULL;
        oxphp_http_request_ce->default_object_handlers = &oxphp_http_request_handlers;

        /* Internal cache properties (hidden, used for lazy caching) */
        zend_declare_property_null(oxphp_http_request_ce,
            "_query_cache", sizeof("_query_cache")-1, ZEND_ACC_PROTECTED);
        zend_declare_property_null(oxphp_http_request_ce,
            "_payload_cache", sizeof("_payload_cache")-1, ZEND_ACC_PROTECTED);
        zend_declare_property_null(oxphp_http_request_ce,
            "_attributes", sizeof("_attributes")-1, ZEND_ACC_PROTECTED);
    }

    /* OxPHP\Http\Attributes */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "Attributes",
            oxphp_http_attributes_methods);
        oxphp_http_attributes_ce = zend_register_internal_class(&tmp_ce);
        oxphp_http_attributes_ce->ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
        zend_class_implements(oxphp_http_attributes_ce, 1, oxphp_http_attributes_iface_ce);
        memcpy(&oxphp_http_attributes_handlers, &std_object_handlers, sizeof(zend_object_handlers));
        oxphp_http_attributes_handlers.clone_obj = NULL;
        oxphp_http_attributes_ce->default_object_handlers = &oxphp_http_attributes_handlers;
        zend_declare_property_null(oxphp_http_attributes_ce,
            "_store", sizeof("_store")-1, ZEND_ACC_PROTECTED);
    }

    /* OxPHP\Http\Session */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "Session",
            oxphp_http_session_methods);
        oxphp_http_session_ce = zend_register_internal_class(&tmp_ce);
        oxphp_http_session_ce->ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
        zend_class_implements(oxphp_http_session_ce, 1, oxphp_http_session_iface_ce);
        memcpy(&oxphp_http_session_handlers, &std_object_handlers, sizeof(zend_object_handlers));
        oxphp_http_session_handlers.clone_obj = NULL;
        oxphp_http_session_ce->default_object_handlers = &oxphp_http_session_handlers;
    }

    /* OxPHP\Http\UploadedFile */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "UploadedFile",
            oxphp_http_uploaded_file_methods);
        oxphp_http_uploaded_file_ce = zend_register_internal_class(&tmp_ce);
        oxphp_http_uploaded_file_ce->ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
        zend_class_implements(oxphp_http_uploaded_file_ce, 1, oxphp_http_uploaded_file_iface_ce);
        memcpy(&oxphp_http_uploaded_file_handlers, &std_object_handlers, sizeof(zend_object_handlers));
        oxphp_http_uploaded_file_handlers.clone_obj = NULL;
        oxphp_http_uploaded_file_ce->default_object_handlers = &oxphp_http_uploaded_file_handlers;

        zend_declare_property_string(oxphp_http_uploaded_file_ce,
            "name", sizeof("name")-1, "", ZEND_ACC_PROTECTED);
        zend_declare_property_string(oxphp_http_uploaded_file_ce,
            "clientType", sizeof("clientType")-1, "", ZEND_ACC_PROTECTED);
        zend_declare_property_long(oxphp_http_uploaded_file_ce,
            "size", sizeof("size")-1, 0, ZEND_ACC_PROTECTED);
        zend_declare_property_string(oxphp_http_uploaded_file_ce,
            "tmpPath", sizeof("tmpPath")-1, "", ZEND_ACC_PROTECTED);
        zend_declare_property_long(oxphp_http_uploaded_file_ce,
            "error", sizeof("error")-1, 4 /* UPLOAD_ERR_NO_FILE */, ZEND_ACC_PROTECTED);
        zend_declare_property_null(oxphp_http_uploaded_file_ce,
            "_type", sizeof("_type")-1, ZEND_ACC_PROTECTED);
    }

    /* OxPHP\Shared\Shareable interface is registered earlier in MINIT
     * (before the plugin class loop) so .implements() can resolve during
     * plugin class registration — see the earlier oxphp_shareable_register_ce
     * call for rationale. */

    /* OxPHP\Decorator\AttributeInterface */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Decorator", "AttributeInterface",
            oxphp_decorator_interface_methods);
        oxphp_decorator_interface_ce = zend_register_internal_interface(&tmp_ce);
    }

    /* OxPHP\Decorator\Context */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Decorator", "Context",
            oxphp_decorator_context_methods);
        oxphp_decorator_context_ce = zend_register_internal_class(&tmp_ce);
        oxphp_decorator_context_ce->ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
        memcpy(&oxphp_decorator_context_handlers, &std_object_handlers, sizeof(zend_object_handlers));
        oxphp_decorator_context_handlers.clone_obj = NULL;
        oxphp_decorator_context_ce->default_object_handlers = &oxphp_decorator_context_handlers;

        /* Declare public properties with string defaults */
        zend_declare_property_string(oxphp_decorator_context_ce, "target", sizeof("target")-1, "", ZEND_ACC_PUBLIC);
        zend_declare_property_string(oxphp_decorator_context_ce, "class", sizeof("class")-1, "", ZEND_ACC_PUBLIC);
        zend_declare_property_string(oxphp_decorator_context_ce, "method", sizeof("method")-1, "", ZEND_ACC_PUBLIC);
        zend_declare_property_string(oxphp_decorator_context_ce, "function", sizeof("function")-1, "", ZEND_ACC_PUBLIC);
        zend_declare_property_long(oxphp_decorator_context_ce, "objectId", sizeof("objectId")-1, 0, ZEND_ACC_PUBLIC);
        zend_declare_property_string(oxphp_decorator_context_ce, "requestId", sizeof("requestId")-1, "", ZEND_ACC_PUBLIC);
        zend_declare_property_string(oxphp_decorator_context_ce, "traceId", sizeof("traceId")-1, "", ZEND_ACC_PUBLIC);

        /* Internal properties (hidden from userland reflection, prefixed with _) */
        zend_declare_property_null(oxphp_decorator_context_ce, "_result", sizeof("_result")-1, ZEND_ACC_PROTECTED);
        zend_declare_property_bool(oxphp_decorator_context_ce, "_has_result", sizeof("_has_result")-1, 0, ZEND_ACC_PROTECTED);
    }

    /* OxPHP\Decorator\RejectedException */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Decorator", "RejectedException", NULL);
        oxphp_decorator_rejected_ce = zend_register_internal_class_ex(&tmp_ce, zend_ce_exception);
    }

    /* Register decorator observer */
    zend_observer_fcall_register(oxphp_decorator_observer_init);

    /* Register tick observer — increments per-worker heartbeat counter
     * once per PHP function call so the supervisor can classify
     * long-running workers (io / c_call / cpu). */
    zend_observer_fcall_register(oxphp_tick_observer_init);

    /* Register profiler observer. The init callback always returns
     * handlers for user functions; the begin/end pair early-returns
     * when g_prof.mode != PROFILE_ALL.
     *
     * Gated at compile time on OXPHP_WITH_PROFILER (Cargo feature
     * plugin-profiler) and at runtime on PROFILER_ENABLED. Because the
     * Zend Observer API freezes init results per-function after first
     * observation, the runtime gate is read once at MINIT — toggling
     * PROFILER_ENABLED at runtime is not supported (process restart
     * required). */
#ifdef OXPHP_WITH_PROFILER
    {
        const char *profiler_enabled = getenv("PROFILER_ENABLED");
        if (profiler_enabled != NULL
            && (strcmp(profiler_enabled, "true") == 0
                || strcmp(profiler_enabled, "1") == 0)) {
            zend_observer_fcall_register(oxphp_profiler_observer_init);
        }
    }
#endif

    /* APM hook approval — validates targets against loaded extensions.
       No handler replacement here; that happens per-thread in RINIT. */
    oxphp_apm_approve_registered_hooks();

    /* Register fiber-await callback so Rust can call it via the bridge. */
    oxphp_bridge_set_fiber_await(oxphp_fiber_suspend_for_await);
    oxphp_bridge_set_in_fiber_check(oxphp_in_oxphp_fiber);

    /* Sub-design A: chain into Zend's interrupt mechanism so cancellation
     * reasons cause clean bailout at the next opcode boundary. */
    orig_zend_interrupt_function = zend_interrupt_function;
    zend_interrupt_function = oxphp_zend_interrupt_handler;

    /* Install the max_execution_time ini hook. */
    zend_ini_entry *me_entry = zend_hash_str_find_ptr(
        EG(ini_directives),
        "max_execution_time",
        sizeof("max_execution_time") - 1);
    if (me_entry) {
        orig_OnUpdateTimeout = me_entry->on_modify;
        me_entry->on_modify = oxphp_OnUpdateTimeout;
    } else {
        php_log_err("oxphp: max_execution_time directive not found at startup; "
                    "set_time_limit() integration disabled");
        /* Continue startup — PHP's default OnUpdateTimeout still works. */
    }

    return SUCCESS;
}
/* }}} */

/* {{{ MSHUTDOWN — clear ox_shared class_entry cache */
PHP_MSHUTDOWN_FUNCTION(oxphp_sapi)
{
    oxphp_shareable_unregister_ce();
    return SUCCESS;
}
/* }}} */

/* {{{ RINIT — per-thread APM hook installation */
PHP_RINIT_FUNCTION(oxphp_sapi)
{
    oxphp_apm_install_on_thread();  /* no-op after first call per thread */
    return SUCCESS;
}
/* }}} */

/* {{{ RSHUTDOWN — cleanup outstanding async promises */
PHP_RSHUTDOWN_FUNCTION(oxphp_sapi)
{
    /* Cleanup any outstanding promises not awaited by user code. */
    oxphp_bridge_cleanup_outstanding_promises();

    /* Clear decorator instance cache — zvals are dtor'd by the HT's
     * registered destructor, we just need to empty the table. */
    if (decorator_instance_cache_initialized) {
        zend_hash_clean(&decorator_instance_cache_ht);
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
    PHP_MSHUTDOWN(oxphp_sapi),
    PHP_RINIT(oxphp_sapi),   /* RINIT */
    PHP_RSHUTDOWN(oxphp_sapi),   /* RSHUTDOWN */
    PHP_MINFO(oxphp_sapi),
    PHP_OXPHP_SAPI_VERSION,
    STANDARD_MODULE_PROPERTIES
};
/* }}} */

#ifdef COMPILE_DL_OXPHP_SAPI
ZEND_GET_MODULE(oxphp_sapi)
#endif
