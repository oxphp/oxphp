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
#include "main/php_output.h"
#include "main/php_main.h"
#include "ext/standard/basic_functions.h"
#include "ext/json/php_json.h"
#include "ext/spl/spl_exceptions.h"
#include "ext/session/php_session.h"
#include <stdlib.h>
#include <time.h>

/* HTTP Request class */
static zend_class_entry *oxphp_http_request_ce = NULL;

/* Async promise exception and proxy classes */
static zend_class_entry *oxphp_async_exception_ce = NULL;

/* HTTP Object API exception classes */
static zend_class_entry *oxphp_no_active_request_ce = NULL;
static zend_class_entry *oxphp_async_context_exc_ce = NULL;
static zend_class_entry *oxphp_worker_idle_exc_ce = NULL;
static zend_class_entry *oxphp_async_timeout_ce = NULL;
static zend_class_entry *oxphp_async_borrow_ce = NULL;
zend_class_entry *oxphp_borrowed_proxy_ce = NULL;

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

/* Decorator instance cache (TLS) */
#define OXPHP_DEC_CACHE_MAX 256
static __thread zval decorator_instance_cache[OXPHP_DEC_CACHE_MAX];
static __thread int decorator_instance_count = 0;

/* Forward declarations for observer functions */
static zend_observer_fcall_handlers oxphp_decorator_observer_init(zend_execute_data *execute_data);
static void oxphp_decorator_begin(zend_execute_data *execute_data);
static void oxphp_decorator_end(zend_execute_data *execute_data, zval *retval);

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

/* {{{ OxPHP\Http\Request::payload(?string $key = null, mixed $default = null): mixed */
ZEND_METHOD(OxPHP_Http_Request, payload) {
    zend_string *key = NULL;
    zval *def = NULL;
    ZEND_PARSE_PARAMETERS_START(0, 2)
        Z_PARAM_OPTIONAL
        Z_PARAM_STR_OR_NULL(key)
        Z_PARAM_ZVAL_OR_NULL(def)
    ZEND_PARSE_PARAMETERS_END();

    /* Check cached payload */
    zval *cached = zend_read_property(oxphp_http_request_ce, Z_OBJ_P(ZEND_THIS),
        "_payload_cache", sizeof("_payload_cache")-1, 1, NULL);

    if (!cached || Z_TYPE_P(cached) == IS_UNDEF || Z_TYPE_P(cached) == IS_NULL) {
        /* Parse body based on Content-Type */
        size_t ct_len = 0;
        const char *ct = oxphp_req_content_type(&ct_len);
        size_t body_len = 0;
        const uint8_t *body_data = oxphp_req_body(&body_len);

        zval parsed;
        ZVAL_NULL(&parsed);

        if (ct && body_data && body_len > 0) {
            if (ct_len >= 16 && strncasecmp(ct, "application/json", 16) == 0) {
                /* JSON decode */
                zend_string *body_str = zend_string_init((const char *)body_data, body_len, 0);
                php_json_decode(&parsed, ZSTR_VAL(body_str), ZSTR_LEN(body_str), 1, 512);
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

    if (Z_TYPE_P(cached) != IS_NULL) {
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

    oxphp_ctx_t *ctx = oxphp_bridge_get_ctx();
    if (!ctx->worker_mode) {
        php_error_docref(NULL, E_WARNING, "oxphp_worker() only available in worker mode");
        RETURN_FALSE;
    }

    /* Prevent handler closure from being GC'd during worker lifetime */
    zend_fcc_addref(&fcc);

    /* Initialize the fiber scheduler */
    oxphp_fiber_scheduler sched;
    oxphp_scheduler_init(&sched);
    sched.shared_fci = &fci;
    sched.shared_fcc = &fcc;

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

            /* Create or reuse a fiber for the request */
            oxphp_request_fiber *fiber = oxphp_scheduler_create_fiber(&sched, &fci, &fcc);
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
            ctx->requests_done = sched.total_requests_done;

            if (rc == 0) {
                /* No work done — brief sleep to avoid busy-wait.
                 * 100μs is short enough for responsive SSE,
                 * long enough to avoid CPU spin. */
                usleep(100);
            }
        }

        /* ── Check exit conditions (same as current) ────────────── */
        if (consecutive_errors >= WORKER_MAX_CONSECUTIVE_ERRORS) {
            ctx->exit_reason = 3;
            break;
        }
        if (ctx->max_requests > 0 && ctx->requests_done >= ctx->max_requests) {
            ctx->exit_reason = 1;
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
 * cache_key is used as the index into decorator_instance_cache.
 * Returns a pointer to the cached zval (valid for the request lifetime). */
static zval *oxphp_get_cached_decorator(const char *class_name, uint64_t cache_key,
                                         zend_function *decorated_func, uint32_t attr_index)
{
    /* Use cache_key as index — it's unique per (fn_id, decorator_index) */
    int idx = (int)(cache_key % OXPHP_DEC_CACHE_MAX);

    /* Check if already cached */
    if (Z_TYPE(decorator_instance_cache[idx]) == IS_OBJECT) {
        return &decorator_instance_cache[idx];
    }

    /* Look up the decorator class */
    zend_string *cls = zend_string_init(class_name, strlen(class_name), 0);
    zend_class_entry *ce = zend_lookup_class(cls);
    zend_string_release(cls);
    if (!ce) return NULL;

    /* Create instance */
    zval *cached = &decorator_instance_cache[idx];
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

    Z_TRY_ADDREF_P(cached);
    if (idx >= decorator_instance_count) {
        decorator_instance_count = idx + 1;
    }

    return cached;
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
            return;
        }
    }

    /* Dispatch to PHP decorators: call before() on cached instances */
    {
        oxphp_php_dec_count_fn_t count_fn = oxphp_bridge_get_php_decorator_count();
        oxphp_php_dec_class_fn_t class_fn = oxphp_bridge_get_php_decorator_class();
        oxphp_php_dec_cache_key_fn_t key_fn = oxphp_bridge_get_php_decorator_cache_key();

        if (count_fn && class_fn && key_fn) {
            uint32_t php_count = count_fn(dctx->fn_id);

            for (uint32_t i = 0; i < php_count; i++) {
                const char *cls = class_fn(dctx->fn_id, i);
                uint64_t cache_key = key_fn(dctx->fn_id, i);
                if (!cls) continue;

                zval *dec_instance = oxphp_get_cached_decorator(
                    cls, cache_key, func, i);
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
                        uint64_t prev_key = key_fn(dctx->fn_id, (uint32_t)j);
                        if (!prev_cls) continue;
                        int prev_idx = (int)(prev_key % OXPHP_DEC_CACHE_MAX);
                        if (Z_TYPE(decorator_instance_cache[prev_idx]) != IS_OBJECT) continue;

                        zval cleanup_ret;
                        ZVAL_UNDEF(&cleanup_ret);
                        /* Save and clear exception to allow after() to run */
                        zend_object *saved_exception = EG(exception);
                        EG(exception) = NULL;
                        zend_call_method_with_1_params(
                            Z_OBJ(decorator_instance_cache[prev_idx]),
                            Z_OBJCE(decorator_instance_cache[prev_idx]),
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
                    return; /* PHP engine will skip the function */
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
        oxphp_php_dec_cache_key_fn_t key_fn = oxphp_bridge_get_php_decorator_cache_key();

        if (count_fn && class_fn && key_fn && dctx->decorator_count > 0) {
            /* Create context with result for after() */
            zval ctx_zval;
            int has_result = success && retval && Z_TYPE_P(retval) != IS_UNDEF;
            oxphp_create_decorator_context(&ctx_zval, dctx, has_result, retval);

            for (int i = (int)dctx->decorator_count - 1; i >= 0; i--) {
                const char *cls = class_fn(dctx->fn_id, (uint32_t)i);
                uint64_t cache_key = key_fn(dctx->fn_id, (uint32_t)i);
                if (!cls) continue;

                int idx = (int)(cache_key % OXPHP_DEC_CACHE_MAX);
                if (Z_TYPE(decorator_instance_cache[idx]) != IS_OBJECT) continue;

                zval after_ret;
                ZVAL_UNDEF(&after_ret);
                zend_call_method_with_1_params(
                    Z_OBJ(decorator_instance_cache[idx]),
                    Z_OBJCE(decorator_instance_cache[idx]),
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

    /* Fiber-aware path: if inside a fiber, suspend instead of blocking */
    if (oxphp_current_fiber != NULL) {
        oxphp_current_fiber->suspend_reason = OXPHP_SUSPEND_AWAIT;
        oxphp_current_fiber->suspend_data.promise_id = (int64_t)promise_id;

        zend_fiber_transfer transfer = {
            .context = oxphp_current_fiber->scheduler,
            .flags = 0
        };
        ZVAL_NULL(&transfer.value);

        oxphp_current_fiber = NULL;
        zend_fiber_switch_context(&transfer);
        /* --- RESUMED by scheduler when promise result is ready --- */

        /* The result is now in READY_RESULTS (Rust TLS).
         * Call the regular dispatch which has a fast path for ready results. */
        int rc = oxphp_bridge_await_dispatch((int64_t)promise_id, 0.0, return_value);
        if (rc == -2) {
            zend_throw_exception(oxphp_async_timeout_ce, "Unexpected timeout after fiber resume", 0);
            return;
        }
        if (rc == -1) {
            /* Read exception details from bridge TLS */
            const char *cls = oxphp_bridge_get_async_exc_class();
            const char *msg = oxphp_bridge_get_async_exc_message();
            zend_string *zmsg = zend_strpprintf(0, "Async task failed: [%s] %s",
                cls ? cls : "?", msg ? msg : "?");
            zend_throw_exception(oxphp_async_exception_ce, ZSTR_VAL(zmsg), 0);
            zend_string_release(zmsg);
            oxphp_bridge_clear_async_exception();
            return;
        }
        /* rc == 0: return_value already populated */
        return;
    }

    /* Traditional blocking path (non-fiber mode) */
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

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_sleep, 0, 1, IS_VOID, 0)
    ZEND_ARG_TYPE_INFO(0, seconds, IS_DOUBLE, 0)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_oxphp_usleep, 0, 1, IS_VOID, 0)
    ZEND_ARG_TYPE_INFO(0, microseconds, IS_LONG, 0)
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
    PHP_FE(oxphp_http_request,      arginfo_oxphp_http_request)
    PHP_FE(oxphp_superglobals_enabled, arginfo_oxphp_superglobals_enabled)
    PHP_FE(oxphp_request_id,        arginfo_oxphp_request_id)
    PHP_FE(oxphp_worker_id,         arginfo_oxphp_worker_id)
    PHP_FE(oxphp_server_info,       arginfo_oxphp_server_info)
    PHP_FE(oxphp_request_heartbeat, arginfo_oxphp_request_heartbeat)
    PHP_FE(oxphp_finish_request,    arginfo_oxphp_finish_request)
    PHP_FE(oxphp_is_worker,          arginfo_oxphp_is_worker)
    PHP_FE(oxphp_is_streaming,      arginfo_oxphp_is_streaming)
    PHP_FE(oxphp_stream_flush,      arginfo_oxphp_stream_flush)
    PHP_FE(oxphp_sleep,             arginfo_oxphp_sleep)
    PHP_FE(oxphp_usleep,            arginfo_oxphp_usleep)
    PHP_FE(oxphp_worker,            arginfo_oxphp_worker)
    PHP_FE(oxphp_async,             arginfo_oxphp_async)
    PHP_FE(oxphp_async_await,             arginfo_oxphp_async_await)
    PHP_FE(oxphp_async_await_all,         arginfo_oxphp_async_await_all)
    PHP_FE(oxphp_async_await_any,         arginfo_oxphp_async_await_any)
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

    /* OxPHP\Http\Exception\NoActiveRequestException */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP\\Http\\Exception", "NoActiveRequestException", NULL);
    oxphp_no_active_request_ce = zend_register_internal_class_ex(&ce, spl_ce_RuntimeException);

    /* OxPHP\Http\Exception\AsyncContextException extends NoActiveRequestException */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP\\Http\\Exception", "AsyncContextException", NULL);
    oxphp_async_context_exc_ce = zend_register_internal_class_ex(&ce, oxphp_no_active_request_ce);

    /* OxPHP\Http\Exception\WorkerIdleException extends NoActiveRequestException */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP\\Http\\Exception", "WorkerIdleException", NULL);
    oxphp_worker_idle_exc_ce = zend_register_internal_class_ex(&ce, oxphp_no_active_request_ce);

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
        oxphp_http_request_ce->ce_flags |= ZEND_ACC_FINAL;

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
        oxphp_http_attributes_ce->ce_flags |= ZEND_ACC_FINAL;
        zend_declare_property_null(oxphp_http_attributes_ce,
            "_store", sizeof("_store")-1, ZEND_ACC_PROTECTED);
    }

    /* OxPHP\Http\Session */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "Session",
            oxphp_http_session_methods);
        oxphp_http_session_ce = zend_register_internal_class(&tmp_ce);
        oxphp_http_session_ce->ce_flags |= ZEND_ACC_FINAL;
    }

    /* OxPHP\Http\UploadedFile */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Http", "UploadedFile",
            oxphp_http_uploaded_file_methods);
        oxphp_http_uploaded_file_ce = zend_register_internal_class(&tmp_ce);
        oxphp_http_uploaded_file_ce->ce_flags |= ZEND_ACC_FINAL;

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

    /* BorrowedProxy class */
    INIT_NS_CLASS_ENTRY(ce, "OxPHP", "BorrowedProxy", oxphp_borrowed_proxy_methods);
    oxphp_borrowed_proxy_ce = zend_register_internal_class(&ce);
    /* Share CE with bridge library so oxphp_create_borrow_proxy() can use it */
    oxphp_bridge_set_borrow_proxy_ce(oxphp_borrowed_proxy_ce);

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
        oxphp_decorator_context_ce->ce_flags |= ZEND_ACC_FINAL;

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

    return SUCCESS;
}
/* }}} */

/* {{{ RSHUTDOWN — cleanup outstanding async promises */
PHP_RSHUTDOWN_FUNCTION(oxphp_sapi)
{
    /* Cleanup any outstanding promises not awaited by user code. */
    oxphp_bridge_cleanup_outstanding_promises();

    /* Clear decorator instance cache */
    for (int i = 0; i < decorator_instance_count; i++) {
        zval_ptr_dtor(&decorator_instance_cache[i]);
        ZVAL_UNDEF(&decorator_instance_cache[i]);
    }
    decorator_instance_count = 0;

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
