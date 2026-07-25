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
#include "main/php_network.h"
#include <limits.h>
#include <stdlib.h>
#include <strings.h>
#include <stdatomic.h>
#include <time.h>
#include <poll.h>
#include <unistd.h>
#include <sys/mman.h>

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
static zend_class_entry *oxphp_decorator_stack_overflow_ce = NULL;

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

/* Construct an OxPHP\Http\UploadedFile from one rfc1867 entry's scalar parts.
 * Each zval is a slot of the $_FILES sub-arrays (name/type/tmp_name/error/size);
 * zend_update_property copies them, so the object owns its own refs. */
static void oxphp_new_uploaded_file(zval *out, zval *z_name, zval *z_type,
        zval *z_tmp, zval *z_err, zval *z_size) {
    object_init_ex(out, oxphp_http_uploaded_file_ce);
    zend_object *obj = Z_OBJ_P(out);
    if (z_name)  zend_update_property(oxphp_http_uploaded_file_ce, obj, "name", sizeof("name")-1, z_name);
    if (z_type)  zend_update_property(oxphp_http_uploaded_file_ce, obj, "clientType", sizeof("clientType")-1, z_type);
    if (z_tmp)   zend_update_property(oxphp_http_uploaded_file_ce, obj, "tmpPath", sizeof("tmpPath")-1, z_tmp);
    if (z_err)   zend_update_property(oxphp_http_uploaded_file_ce, obj, "error", sizeof("error")-1, z_err);
    if (z_size)  zend_update_property(oxphp_http_uploaded_file_ce, obj, "size", sizeof("size")-1, z_size);
}

/* Resolve one slot of a parallel $_FILES sub-array (name/type/error/size),
 * keyed the same way as the tmp_name slot being iterated. PHP keys these arrays
 * by integer for name="field[]" and by string for name="field[key]"; pick
 * whichever the current tmp_name key uses. */
static zval *oxphp_field_slot(zval *arr, zend_string *str_key, zend_ulong num_key) {
    if (!arr || Z_TYPE_P(arr) != IS_ARRAY) {
        return NULL;
    }
    return str_key ? zend_hash_find(Z_ARRVAL_P(arr), str_key)
                   : zend_hash_index_find(Z_ARRVAL_P(arr), num_key);
}

/* Append every UploadedFile of one $_FILES field to `out`. Handles the scalar
 * shape (single file) and both array shapes — sequential (name="field[]") and
 * associative (name="field[key]") — by pairing each tmp_name slot with the
 * same-keyed slot of the parallel name/type/error/size arrays. */
static void oxphp_append_field_files(zval *out, zval *entry) {
    zval *z_name = zend_hash_str_find(Z_ARRVAL_P(entry), "name", sizeof("name")-1);
    zval *z_type = zend_hash_str_find(Z_ARRVAL_P(entry), "type", sizeof("type")-1);
    zval *z_tmp  = zend_hash_str_find(Z_ARRVAL_P(entry), "tmp_name", sizeof("tmp_name")-1);
    zval *z_err  = zend_hash_str_find(Z_ARRVAL_P(entry), "error", sizeof("error")-1);
    zval *z_size = zend_hash_str_find(Z_ARRVAL_P(entry), "size", sizeof("size")-1);
    if (!z_tmp) {
        return;
    }
    if (Z_TYPE_P(z_tmp) == IS_ARRAY) {
        zend_ulong idx;
        zend_string *key;
        zval *f_tmp;
        ZEND_HASH_FOREACH_KEY_VAL(Z_ARRVAL_P(z_tmp), idx, key, f_tmp) {
            zval *f_name = oxphp_field_slot(z_name, key, idx);
            zval *f_type = oxphp_field_slot(z_type, key, idx);
            zval *f_err  = oxphp_field_slot(z_err, key, idx);
            zval *f_size = oxphp_field_slot(z_size, key, idx);
            zval file_obj;
            oxphp_new_uploaded_file(&file_obj, f_name, f_type, f_tmp, f_err, f_size);
            add_next_index_zval(out, &file_obj);
        } ZEND_HASH_FOREACH_END();
    } else {
        zval file_obj;
        oxphp_new_uploaded_file(&file_obj, z_name, z_type, z_tmp, z_err, z_size);
        add_next_index_zval(out, &file_obj);
    }
}

/* {{{ OxPHP\Http\Request::file(string $name): ?UploadedFileInterface
 * Returns the uploaded file for `$name`, or the first file for an array field
 * (name="$name[]" / name="$name[key]"), or null when the field is absent or
 * carries no files. */
ZEND_METHOD(OxPHP_Http_Request, file) {
    zend_string *name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(name)
    ZEND_PARSE_PARAMETERS_END();

    zval *files = &PG(http_globals)[TRACK_VARS_FILES];
    if (Z_TYPE_P(files) != IS_ARRAY) {
        RETURN_NULL();
    }
    zval *entry = zend_hash_find(Z_ARRVAL_P(files), name);
    if (!entry || Z_TYPE_P(entry) != IS_ARRAY) {
        RETURN_NULL();
    }

    /* Expand the field once, then hand back its first file (or null when the
     * field carries none). Sharing oxphp_append_field_files() keeps the scalar,
     * sequential-array and associative-array shapes resolved in one place. */
    zval list;
    array_init(&list);
    oxphp_append_field_files(&list, entry);
    zval *first = zend_hash_index_find(Z_ARRVAL(list), 0);
    if (first) {
        ZVAL_COPY(return_value, first);
    } else {
        ZVAL_NULL(return_value);
    }
    zval_ptr_dtor(&list);
}
/* }}} */

/* {{{ OxPHP\Http\Request::files(?string $name = null): array
 * Without an argument, returns every uploaded file as a flat list. With a field
 * name, returns all files for that field (supports name="$name[]" and
 * name="$name[key]"). */
ZEND_METHOD(OxPHP_Http_Request, files) {
    zend_string *name = NULL;
    ZEND_PARSE_PARAMETERS_START(0, 1)
        Z_PARAM_OPTIONAL
        Z_PARAM_STR_OR_NULL(name)
    ZEND_PARSE_PARAMETERS_END();

    array_init(return_value);

    zval *files = &PG(http_globals)[TRACK_VARS_FILES];
    if (Z_TYPE_P(files) != IS_ARRAY) {
        return;
    }

    if (name) {
        zval *entry = zend_hash_find(Z_ARRVAL_P(files), name);
        if (entry && Z_TYPE_P(entry) == IS_ARRAY) {
            oxphp_append_field_files(return_value, entry);
        }
        return;
    }

    zval *entry;
    ZEND_HASH_FOREACH_VAL(Z_ARRVAL_P(files), entry) {
        if (Z_TYPE_P(entry) == IS_ARRAY) {
            oxphp_append_field_files(return_value, entry);
        }
    } ZEND_HASH_FOREACH_END();
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
    /* Use mime_content_type() for magic-bytes detection. Skip it for an empty
     * tmpPath (e.g. an UPLOAD_ERR_NO_FILE entry): mime_content_type('') throws a
     * ValueError, so fall straight through to the default below instead. */
    zval *tmp_path = zend_read_property(oxphp_http_uploaded_file_ce, Z_OBJ_P(ZEND_THIS),
        "tmpPath", sizeof("tmpPath")-1, 1, NULL);
    if (tmp_path && Z_TYPE_P(tmp_path) == IS_STRING && Z_STRLEN_P(tmp_path) > 0) {
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

/* Uncatchable drain bail for a fiber force-resumed by the scheduler's drain
 * sweep: mark the connection aborted (so shutdown handlers calling
 * connection_aborted()/connection_status() observe it), then unwind via
 * zend_error_noreturn — registered shutdown functions still run, a userland
 * try/catch cannot swallow it. Shared by every suspend point's resume path. */
static ZEND_COLD ZEND_NORETURN void oxphp_fiber_drain_bail(void)
{
    PG(connection_status) |= PHP_CONNECTION_ABORTED;
    zend_error_noreturn(E_ERROR, "Request cancelled (shutdown)");
}

/* Whether the code calling a suspend point is running on this fiber's own
 * context. It is not once a PHP script starts a userland Fiber (AMPHP, Revolt,
 * a bare `new Fiber`) inside an oxphp fiber: the userland fiber executes on its
 * own context while oxphp_current_fiber still names the outer one, since only
 * the oxphp scheduler ever maintains that pointer.
 *
 * Switching away from there would be silent corruption, not a degraded wait.
 * zend_fiber_switch_context() saves the continuation of whatever context is
 * actually running — the userland fiber's — while the scheduler later resumes
 * the outer fiber's now-stale handle, which still points inside Fiber::start().
 * The outer fiber would return from start() as though the userland fiber had
 * suspended, with no Suspension ever registered for it.
 *
 * So a suspend point checks this and takes its blocking path instead. Losing
 * cooperative scheduling under a userland fiber scheduler is a real cost, but a
 * predictable one. */
static bool oxphp_fiber_owns_current_context(oxphp_request_fiber *self)
{
    return EG(current_fiber_context) == &self->context;
}

/* Internal: register timer and suspend current fiber.
 * duration_us is the sleep duration in microseconds.
 * Returns 1 if fiber-suspended (timer expired), 0 if no fiber (use blocking
 * fallback), -1 if the task was cancelled while sleeping (caller throws). */
static int oxphp_fiber_sleep_us(uint64_t duration_us)
{
    if (oxphp_current_fiber == NULL) return 0;
    if (!oxphp_fiber_owns_current_context(oxphp_current_fiber)) return 0;

    uint64_t duration_ms = (duration_us + 999) / 1000; /* round up */
    if (duration_ms == 0) duration_ms = 1;

    oxphp_request_fiber *self = oxphp_current_fiber;
    uint64_t timer_id = oxphp_bridge_timer_register(duration_ms);

    self->suspend_reason = OXPHP_SUSPEND_SLEEP;
    self->suspend_data.timer_id = timer_id;

    zend_fiber_transfer transfer = {
        .context = self->scheduler,
        .flags = 0
    };
    ZVAL_NULL(&transfer.value);

    oxphp_current_fiber = NULL;
    zend_fiber_switch_context(&transfer);
    /* --- RESUMED on timer expiry, OR force-resumed by the scheduler when the
     * task was cancelled (awaiter gave up) or the server is draining. */
    if (self->drain_kill) {
        oxphp_fiber_drain_bail();
    }
    if (self->cancel_requested) {
        self->cancel_requested = false;
        return -1; /* cancelled */
    }
    return 1;
}

/* Suspend the current fiber until one of `fds` is ready for the events it asks
 * for, its deadline elapses, or the fiber is cancelled. The scheduler owns the
 * readiness poll (oxphp_io_collect_ready), so the worker thread keeps serving
 * other fibers while this one waits. Mirrors oxphp_fiber_sleep_us(), including
 * its resume contract: an uncatchable bail on drain, a cancellation return
 * otherwise.
 *
 * `fds` is borrowed for the whole suspension and written back into: the
 * scheduler fills each entry's revents before resuming, so the caller reads
 * which of its descriptors fired. It must therefore live in the caller's frame,
 * which by construction outlives the wait. `owners` is borrowed on the same
 * terms and holds the identity each registration carries back; the caller only
 * has to supply the room, this fills it in.
 *
 * timeout_ns < 0 waits indefinitely. Returns 1 when a descriptor is ready,
 * 0 when not called from a fiber (or the set is unusable, or the scheduler
 * cannot watch it), -1 when the fiber was cancelled, and -2 when the deadline
 * elapsed first.
 *
 * A 0 means the caller must do the wait itself, blocking its thread — so the
 * set being unusable has to stay a caller's bug rather than a size a real
 * program reaches: OXPHP_MAX_WAIT_FDS is set at the ceiling PHP's own
 * multiplexed wait enforces, and a wider set is one PHP would have rejected
 * before ever calling a hook. */
static int oxphp_fiber_io_wait(struct pollfd *fds, struct oxphp_io_owner *owners,
                               uint32_t nfds, int64_t timeout_ns)
{
    if (oxphp_current_fiber == NULL) return 0;
    if (!oxphp_fiber_owns_current_context(oxphp_current_fiber)) return 0;
    if (fds == NULL || owners == NULL || nfds == 0 || nfds > OXPHP_MAX_WAIT_FDS) return 0;

    oxphp_request_fiber *self = oxphp_current_fiber;

    uint64_t deadline_ns = 0;
    if (timeout_ns >= 0) {
        struct timespec ts;
        clock_gettime(CLOCK_MONOTONIC, &ts);
        deadline_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec
                      + (uint64_t)timeout_ns;
    }

    for (uint32_t i = 0; i < nfds; i++) {
        fds[i].revents = 0;
        owners[i].fiber = self;
        owners[i].idx = i;
    }

    /* Before the suspension is recorded, so a set the scheduler cannot watch
     * leaves nothing half-written to undo — the fiber simply never parked. */
    if (!oxphp_io_park(self, fds, owners, nfds)) return 0;

    self->suspend_reason = OXPHP_SUSPEND_IO_WAIT;
    self->suspend_data.io.fds = fds;
    self->suspend_data.io.owners = owners;
    self->suspend_data.io.nfds = nfds;
    self->suspend_data.io.expired = false;
    self->suspend_data.io.deadline_ns = deadline_ns;

    zend_fiber_transfer transfer = {
        .context = self->scheduler,
        .flags = 0
    };
    ZVAL_NULL(&transfer.value);

    oxphp_current_fiber = NULL;
    zend_fiber_switch_context(&transfer);
    /* --- RESUMED once one descriptor is ready or the deadline passed, OR
     * force-resumed by the scheduler when the task was cancelled (awaiter gave
     * up) or the server is draining. */
    if (self->drain_kill) {
        oxphp_fiber_drain_bail();
    }
    if (self->cancel_requested) {
        self->cancel_requested = false;
        return -1; /* cancelled */
    }
    if (self->suspend_data.io.expired) {
        return -2; /* deadline */
    }
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
    int rc = oxphp_fiber_sleep_us(duration_us);
    if (rc < 0) {
        oxphp_throw_exception("OxPHP\\Async\\AsyncException",
                              "Async task cancelled", 0);
        return;
    }
    if (rc) return;

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

    int rc = oxphp_fiber_sleep_us((uint64_t)microseconds);
    if (rc < 0) {
        oxphp_throw_exception("OxPHP\\Async\\AsyncException",
                              "Async task cancelled", 0);
        return;
    }
    if (rc) return;

    usleep((useconds_t)microseconds);
}
/* }}} */

/* ─── Runtime Hooks (RUNTIME_HOOKS) ──────────────────────────
 * Opt-in replacement of blocking native builtins with fiber-suspending
 * implementations. Handler pointers are swapped in the master function
 * table during MINIT — on the startup thread, before worker threads copy
 * the table; under ZTS the per-thread copies alias the same
 * zend_internal_function structs, so swapping after threads spawn would
 * be a data race and is never done. Outside a fiber the hooks delegate
 * to the saved original handler, byte-identical to native behavior. */

static zif_handler oxphp_orig_sleep = NULL;
static zif_handler oxphp_orig_usleep = NULL;

/* Drop surrounding blanks from one comma-separated token, so a list written
 * the way a person writes one ("sleep, streams") names the same categories as
 * a list written without spaces. */
static void oxphp_hooks_trim_token(const char **tok, size_t *len)
{
    while (*len > 0 && ((*tok)[0] == ' ' || (*tok)[0] == '\t')) {
        (*tok)++;
        (*len)--;
    }
    while (*len > 0 && ((*tok)[*len - 1] == ' ' || (*tok)[*len - 1] == '\t')) {
        (*len)--;
    }
}

static bool oxphp_hooks_token_is(const char *tok, size_t len, const char *word)
{
    return len == strlen(word) && strncasecmp(tok, word, len) == 0;
}

/* The RUNTIME_HOOKS value as a (pointer, length) pair with surrounding blanks
 * removed, or NULL when unset, empty, or nothing but blanks. Trimming here
 * rather than per token is what makes " all" mean the same as "all". */
static const char *oxphp_hooks_env(size_t *out_len)
{
    const char *env = getenv("RUNTIME_HOOKS");
    if (!env || !*env) return NULL;

    size_t len = strlen(env);
    oxphp_hooks_trim_token(&env, &len);
    if (len == 0) return NULL;

    *out_len = len;
    return env;
}

/* RUNTIME_HOOKS grammar: unset/""/"0"/"false" = off; "1"/"true"/"all" = every
 * category; otherwise a comma-separated category list, blanks around each entry
 * ignored. This answers only "did the operator ask for this one"; the set of
 * categories that actually exist is oxphp_hook_categories[], which is what
 * startup validation reports against — keep the two in step when adding one. */
static bool oxphp_hooks_category_enabled(const char *category)
{
    size_t env_len = 0;
    const char *env = oxphp_hooks_env(&env_len);
    if (env == NULL) return false;

    if (oxphp_hooks_token_is(env, env_len, "0")
        || oxphp_hooks_token_is(env, env_len, "false")) {
        return false;
    }
    if (oxphp_hooks_token_is(env, env_len, "1")
        || oxphp_hooks_token_is(env, env_len, "true")
        || oxphp_hooks_token_is(env, env_len, "all")) {
        return true;
    }

    const char *p = env;
    const char *end = env + env_len;
    while (p < end) {
        const char *comma = memchr(p, ',', (size_t)(end - p));
        size_t len = comma ? (size_t)(comma - p) : (size_t)(end - p);
        const char *tok = p;
        p = comma ? comma + 1 : end;

        oxphp_hooks_trim_token(&tok, &len);
        if (oxphp_hooks_token_is(tok, len, category)) {
            return true;
        }
    }
    return false;
}

/* Hooked sleep(): native argument contract (mirrors ext/standard
 * PHP_FUNCTION(sleep)), cooperative suspend inside a fiber. Returns 0 —
 * the cooperative timer always runs to completion, so the "seconds left
 * on signal interrupt" case of the native builtin does not arise.
 * Cancellation of a task fiber unwinds via AsyncException, matching
 * oxphp_sleep(). */
static ZEND_NAMED_FUNCTION(oxphp_hooked_sleep)
{
    if (oxphp_current_fiber == NULL) {
        oxphp_orig_sleep(INTERNAL_FUNCTION_PARAM_PASSTHRU);
        return;
    }

    zend_long num;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_LONG(num)
    ZEND_PARSE_PARAMETERS_END();

    if (num < 0 || (zend_ulong) num > UINT_MAX) {
        zend_argument_value_error(1, "must be between 0 and %u", UINT_MAX);
        RETURN_THROWS();
    }

    if (num > 0 && oxphp_fiber_sleep_us((uint64_t) num * 1000000) < 0) {
        oxphp_throw_exception("OxPHP\\Async\\AsyncException",
                              "Async task cancelled", 0);
        return;
    }
    RETURN_LONG(0);
}

static ZEND_NAMED_FUNCTION(oxphp_hooked_usleep)
{
    if (oxphp_current_fiber == NULL) {
        oxphp_orig_usleep(INTERNAL_FUNCTION_PARAM_PASSTHRU);
        return;
    }

    zend_long num;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_LONG(num)
    ZEND_PARSE_PARAMETERS_END();

    if (num < 0 || (zend_ulong) num > UINT_MAX) {
        zend_argument_value_error(1, "must be between 0 and %u", UINT_MAX);
        RETURN_THROWS();
    }

    if (num > 0 && oxphp_fiber_sleep_us((uint64_t) num) < 0) {
        oxphp_throw_exception("OxPHP\\Async\\AsyncException",
                              "Async task cancelled", 0);
    }
}

static bool oxphp_hook_swap(const char *name, size_t name_len,
                            zif_handler hook, zif_handler *orig_out)
{
    zend_function *fn = zend_hash_str_find_ptr(CG(function_table), name, name_len);
    if (!fn || fn->type != ZEND_INTERNAL_FUNCTION) {
        return false;
    }
    *orig_out = fn->internal_function.handler;
    fn->internal_function.handler = hook;
    return true;
}

static void oxphp_hook_restore(const char *name, size_t name_len, zif_handler orig)
{
    if (!orig) return;
    zend_function *fn = zend_hash_str_find_ptr(CG(function_table), name, name_len);
    if (fn && fn->type == ZEND_INTERNAL_FUNCTION) {
        fn->internal_function.handler = orig;
    }
}

/* ─── Hooked socket reads (category "streams") ───────────────
 * php_sockop_read() parks the calling thread inside php_pollfd_for() whenever a
 * blocking socket stream has no data yet. It is reached through
 * php_stream_socket_ops — the ops table every tcp:// stream carries — so
 * replacing that one entry is enough to make fsockopen(),
 * stream_socket_client() and everything layered on php_streams (mysqlnd and
 * phpredis included) suspend the current fiber instead of pinning the worker
 * thread while waiting for an answer.
 *
 * The table is patched in place rather than replaced per stream so that
 * php_stream_is(stream, PHP_STREAM_IS_SOCKET) keeps matching: a private ops
 * table would silently break socket_import_stream() and every other identity
 * check, and would miss sockets that never pass through the transport factory
 * (stream_socket_accept() results among them). unix://, udp:// and udg://
 * carry their own tables, which php-src keeps static and out of reach, and
 * ssl:// / tls:// layer the openssl table on top — none of them are hooked.
 *
 * Writes are deliberately NOT hooked. Read readiness is stable: the descriptor
 * belongs to this fiber, nothing else drains it, so data that arrived is still
 * there when the delegate runs. Write readiness is not — the room in the send
 * buffer is created and taken away by the peer, so between waking on POLLOUT
 * and the delegate's send() the window can close again, after which
 * php_sockop_write() blocks the thread for its whole timeout anyway. Measured
 * on a saturated socket, hooking writes made a blocking write cost its timeout
 * twice (the fiber's wait plus the delegate's) without ever preventing the
 * block. Serving that correctly would mean reimplementing the native retry and
 * warning path, which is exactly what this design refuses to do. */

static ssize_t (*oxphp_orig_sockop_read)(php_stream *, char *, size_t) = NULL;

/* Nanoseconds left on the stream's own timeout, mirroring how php_sockop_read
 * reads sock->timeout; tv_sec == -1 means "wait indefinitely". Nanoseconds
 * rather than milliseconds because the deadline is compared in nanoseconds:
 * rounding up here would coarsen a sub-millisecond timeout for nothing. What
 * still bounds the resolution is how often the deadline is looked at — once per
 * scheduler tick. */
static int64_t oxphp_sock_timeout_ns(const php_netstream_data_t *sock)
{
    if (sock->timeout.tv_sec == -1) return -1;
    return (int64_t) sock->timeout.tv_sec * 1000000000
           + (int64_t) sock->timeout.tv_usec * 1000;
}

static ssize_t oxphp_hooked_sockop_read(php_stream *stream, char *buf, size_t count)
{
    php_netstream_data_t *sock = (php_netstream_data_t *) stream->abstract;

    /* The condition under which php_sockop_read() would call
     * php_sock_stream_wait_for_data(): a blocking socket with a nonzero
     * timeout and nothing buffered from a previous read. Every other case it
     * serves without waiting, so it runs unchanged. */
    if (oxphp_current_fiber != NULL && sock != NULL && sock->socket != -1
        && sock->is_blocked && !stream->has_buffered_data
        && !(sock->timeout.tv_sec == 0 && sock->timeout.tv_usec == 0)) {
        /* PHP_POLLREADABLE is exactly what php_sock_stream_wait_for_data()
         * waits for; matching it keeps this suspension from ending on an
         * event the delegate would go on to block for. */
        int64_t budget_ns = oxphp_sock_timeout_ns(sock);
        struct timespec started;
        clock_gettime(CLOCK_MONOTONIC, &started);

        struct pollfd pfd = {
            .fd = sock->socket,
            .events = PHP_POLLREADABLE,
            .revents = 0,
        };
        struct oxphp_io_owner owner;
        int rc = oxphp_fiber_io_wait(&pfd, &owner, 1, budget_ns);
        if (rc == 0) {
            /* Declined the suspension — a userland fiber scheduler owns the
             * context, so nothing was waited for. Step aside completely: with
             * no time spent there is no budget to subtract, and handing the
             * read over untouched is what this case promises. */
            return oxphp_orig_sockop_read(stream, buf, count);
        }
        if (rc == -1) {
            /* Cancelled, not timed out. php_sock_stream_wait_for_data() clears
             * this flag before every wait, so leaving a previous read's value
             * behind would have stream_get_meta_data() report a timeout that
             * did not happen. */
            sock->timeout_event = false;
            oxphp_throw_exception("OxPHP\\Async\\AsyncException",
                                  "Async task cancelled", 0);
            return -1;
        }
        if (rc == -2) {
            /* Timed out — php_sockop_read's own timeout branch. Nothing is
             * buffered in this branch, so -1 is the value it would return. */
            sock->timeout_event = true;
            return -1;
        }

        /* Ready. The delegate polls the descriptor once more and normally
         * returns from that poll at once — but if the data is gone by then (a
         * connection shared between fibers, or a spurious wake) it would wait
         * the socket's *whole* timeout again, making the worst case twice what
         * the caller asked for. Hand it only what is left of the budget, and
         * put the real value back afterwards. Anything below a microsecond is
         * floored there rather than to zero, which php_sockop_read() reads as
         * "do not wait at all" and answers differently. */
        if (budget_ns > 0) {
            struct timespec now;
            clock_gettime(CLOCK_MONOTONIC, &now);
            int64_t spent_ns = (int64_t)(now.tv_sec - started.tv_sec) * 1000000000
                               + (int64_t)(now.tv_nsec - started.tv_nsec);
            int64_t left_us = (budget_ns - spent_ns) / 1000;
            if (left_us < 1) left_us = 1;

            struct timeval saved = sock->timeout;
            sock->timeout.tv_sec = (time_t)(left_us / 1000000);
            sock->timeout.tv_usec = (suseconds_t)(left_us % 1000000);

            /* php_sockop_read() runs userland code on its way out — a stream
             * context's notification callback — so it can leave through
             * zend_bailout(); exit() in that callback is enough. The shortened
             * timeout must not survive that unwind: on a persistent stream the
             * socket outlives the request, and every later read on that
             * connection would inherit whatever fraction of the budget was
             * left here. */
            volatile ssize_t nr = -1;
            zend_try {
                nr = oxphp_orig_sockop_read(stream, buf, count);
            } zend_catch {
                sock->timeout = saved;
                zend_bailout();
            } zend_end_try();
            sock->timeout = saved;

            return nr;
        }
    }

    return oxphp_orig_sockop_read(stream, buf, count);
}

/* Protection the page holding php_stream_socket_ops carried before the patch,
 * so it can be put back exactly. -1 = not captured. */
static int oxphp_socket_ops_prot = -1;

/* Read a mapping's current protection out of /proc/self/maps. The table is
 * const and normally lands in .data.rel.ro, which the loader maps read-only
 * once relocations are done — but a build without RELRO can leave it on a page
 * it shares with writable data, and blindly restoring PROT_READ there would
 * take write access away from unrelated globals and fault far from the cause.
 * Returns -1 when the mapping cannot be determined. */
static int oxphp_page_protection(uintptr_t addr)
{
    FILE *maps = fopen("/proc/self/maps", "r");
    if (maps == NULL) return -1;

    char line[512];
    int prot = -1;
    while (fgets(line, sizeof(line), maps) != NULL) {
        unsigned long start, end;
        char perms[5];
        if (sscanf(line, "%lx-%lx %4s", &start, &end, perms) != 3) continue;
        if (addr < (uintptr_t) start || addr >= (uintptr_t) end) continue;

        prot = 0;
        if (perms[0] == 'r') prot |= PROT_READ;
        if (perms[1] == 'w') prot |= PROT_WRITE;
        if (perms[2] == 'x') prot |= PROT_EXEC;
        break;
    }
    fclose(maps);

    return prot;
}

static bool oxphp_socket_ops_protect(int prot)
{
    long page_size = sysconf(_SC_PAGESIZE);
    if (page_size <= 0) return false;

    uintptr_t addr = (uintptr_t) &php_stream_socket_ops;
    uintptr_t start = addr & ~((uintptr_t) page_size - 1);
    size_t len = (addr + sizeof(php_stream_ops)) - start;

    return mprotect((void *) start, len, prot) == 0;
}

static bool oxphp_hook_socket_ops(void)
{
    php_stream_ops *ops = (php_stream_ops *) (uintptr_t) &php_stream_socket_ops;

    oxphp_socket_ops_prot = oxphp_page_protection((uintptr_t) ops);
    if (!oxphp_socket_ops_protect(PROT_READ | PROT_WRITE)) {
        return false;
    }
    oxphp_orig_sockop_read = ops->read;
    ops->read = oxphp_hooked_sockop_read;

    /* Restore the protection we found. If it could not be read, leave the page
     * writable rather than guess: the patch itself has already succeeded and
     * nothing about correctness depends on the page being read-only again. Both
     * outcomes are logged — a page of function pointers staying writable weakens
     * RELRO, which an operator should learn from the log and not from a memory
     * dump. */
    if (oxphp_socket_ops_prot < 0) {
        php_log_err("oxphp: could not read the original memory protection of the stream "
                    "ops table (/proc/self/maps unavailable); the page stays writable "
                    "after installing the socket hooks");
    } else if (!oxphp_socket_ops_protect(oxphp_socket_ops_prot)) {
        php_log_err("oxphp: could not restore memory protection on the stream ops "
                    "table after installing the socket hooks; the page stays writable");
    }
    return true;
}

static void oxphp_restore_socket_ops(void)
{
    if (oxphp_orig_sockop_read == NULL) return;

    php_stream_ops *ops = (php_stream_ops *) (uintptr_t) &php_stream_socket_ops;
    if (oxphp_socket_ops_protect(PROT_READ | PROT_WRITE)) {
        ops->read = oxphp_orig_sockop_read;
        if (oxphp_socket_ops_prot >= 0) {
            oxphp_socket_ops_protect(oxphp_socket_ops_prot);
        }
    }
    oxphp_orig_sockop_read = NULL;
    oxphp_socket_ops_prot = -1;
}

/* ─── Hooked stream_select() (category "streams") ────────────
 * stream_select() waits on descriptors directly and never reaches ops->read, so
 * the socket read hook does nothing for it: a select loop pins the worker thread
 * for its whole timeout. This hook reproduces only the wait. Everything the
 * function is actually specified to do — rewriting the three arrays down to the
 * ready streams, the return count, the warnings, the argument errors — stays
 * with the original handler, which is called afterwards with the timeout forced
 * to zero.
 *
 * Anything this hook does not fully understand delegates unchanged, so the worst
 * case is exactly today's behaviour. */

static zif_handler oxphp_orig_stream_select = NULL;

/* Collect the descriptors one stream array contributes to the wait set, merging
 * a descriptor that appears in two arrays into a single entry carrying both
 * events. Returns false when the array holds something this hook must not try
 * to represent, in which case the caller delegates and native behaviour is
 * preserved exactly:
 *
 *   - an element that is not a live stream resource, because native's answer to
 *     that is an exception rather than a wait;
 *   - a read stream with buffered data, because stream_select() answers from
 *     the buffer without looking at any descriptor and a hook that parked
 *     instead would sleep through data the caller already has;
 *   - a stream that does not cast to a descriptor, because native emits its own
 *     warning and selects on the remainder, which is not ours to reproduce;
 *   - a descriptor at or past FD_SETSIZE. Native does not wait on those at all:
 *     it warns and returns false immediately. Parking a fiber on one would
 *     delay an answer that is already decided, and on a null timeout would
 *     never produce it;
 *   - more descriptors than the buffer the caller sized from the arrays.
 *
 * The cast asks for no error output (last argument 0) — native will do the same
 * cast with output enabled, and a warning printed twice is a behaviour change. */
static bool oxphp_select_collect(zval *arr, short events, bool buffered_is_ready,
                                 struct pollfd *fds, uint32_t cap, uint32_t *nfds)
{
    if (arr == NULL || Z_TYPE_P(arr) == IS_NULL) return true;
    if (Z_TYPE_P(arr) != IS_ARRAY) return false;

    zval *elem;
    ZEND_HASH_FOREACH_VAL(Z_ARRVAL_P(arr), elem) {
        ZVAL_DEREF(elem);
        /* The fetch php_stream_from_zval_no_verify() performs, minus the type
         * name that makes it raise a TypeError. Native raises exactly one for an
         * element that is not a live stream and then follows its own error path;
         * a second one from here would chain onto it, and parking a fiber with
         * an exception already pending is worse than that: Zend does not carry
         * EG(exception) across a fiber switch, so it would surface in whichever
         * fiber the scheduler resumes next — another request. */
        php_stream *stream = (php_stream *) zend_fetch_resource2_ex(
            elem, NULL, php_file_le_stream(), php_file_le_pstream());
        if (stream == NULL) return false;

        if (buffered_is_ready && (stream->writepos - stream->readpos) > 0) {
            return false;
        }

        /* The cast can reach a userland wrapper's stream_cast(), which is PHP
         * code and may throw; the rule above applies to that exception too. */
        php_socket_t sock_fd;
        if (php_stream_cast(stream,
                            PHP_STREAM_AS_FD_FOR_SELECT | PHP_STREAM_CAST_INTERNAL,
                            (void *) &sock_fd, 0) != SUCCESS || sock_fd == -1
            || EG(exception) != NULL) {
            return false;
        }
        if ((int) sock_fd >= FD_SETSIZE) {
            return false;
        }

        uint32_t i = 0;
        for (; i < *nfds; i++) {
            if (fds[i].fd == (int) sock_fd) {
                fds[i].events |= events;
                break;
            }
        }
        if (i == *nfds) {
            if (*nfds == cap) return false;
            fds[*nfds].fd = (int) sock_fd;
            fds[*nfds].events = events;
            fds[*nfds].revents = 0;
            (*nfds)++;
        }
    } ZEND_HASH_FOREACH_END();

    return true;
}

/* Call the original handler with the timeout arguments forced to zero, so its
 * own select() answers from the readiness we have already waited for instead of
 * waiting for it a second time — which is what would make the worst case twice
 * the timeout the caller asked for.
 *
 * Dereferenced first, so this writes the same slot the caller's arguments were
 * read from — a by-value parameter holding a reference would otherwise be read
 * through the wrapper and written over it. After the dereference both are longs
 * or nulls taken by value, so the substitution lives in this call frame, is
 * invisible to the caller's variables, and holds nothing refcounted: a
 * zend_bailout out of the delegate leaks nothing and the restore below is
 * hygiene rather than a correctness requirement. */
static void oxphp_select_delegate_now(zend_execute_data *execute_data,
                                      zval *return_value, uint32_t argc)
{
    zval *sec = ZEND_CALL_ARG(execute_data, 4);
    ZVAL_DEREF(sec);
    zval saved_sec = *sec;
    ZVAL_LONG(sec, 0);

    zval *usec = NULL;
    zval saved_usec;
    if (argc >= 5) {
        usec = ZEND_CALL_ARG(execute_data, 5);
        ZVAL_DEREF(usec);
        saved_usec = *usec;
        ZVAL_LONG(usec, 0);
    }

    oxphp_orig_stream_select(execute_data, return_value);

    *sec = saved_sec;
    if (usec != NULL) *usec = saved_usec;
}

static void oxphp_hooked_stream_select(zend_execute_data *execute_data, zval *return_value)
{
    /* Outside a fiber there is nothing to suspend; on a userland fiber's context
     * suspending would store the continuation in the wrong handle and corrupt
     * both schedulers — the same guard every other suspend point carries. */
    if (oxphp_current_fiber == NULL
        || !oxphp_fiber_owns_current_context(oxphp_current_fiber)) {
        oxphp_orig_stream_select(execute_data, return_value);
        return;
    }

    /* Four required parameters, one optional timeout fraction — the whole
     * signature on every supported version. Anything else is either the error
     * native raises for the wrong argument count, which it must raise at once
     * rather than after a wait, or a later PHP whose extra parameter this hook
     * has not been taught to read. Both are native's to answer. */
    uint32_t argc = ZEND_CALL_NUM_ARGS(execute_data);
    if (argc < 4 || argc > 5) {
        oxphp_orig_stream_select(execute_data, return_value);
        return;
    }

    zval *r = ZEND_CALL_ARG(execute_data, 1);
    zval *w = ZEND_CALL_ARG(execute_data, 2);
    zval *e = ZEND_CALL_ARG(execute_data, 3);
    zval *sec = ZEND_CALL_ARG(execute_data, 4);
    zval *usec = argc >= 5 ? ZEND_CALL_ARG(execute_data, 5) : NULL;
    ZVAL_DEREF(r); ZVAL_DEREF(w); ZVAL_DEREF(e);
    ZVAL_DEREF(sec);
    if (usec != NULL) ZVAL_DEREF(usec);

    /* Native builds the timeval from these two and throws on a negative one.
     * A shape this hook does not read the same way is not a shape it may wait
     * on. Ten years stands in for "longer than anyone means"; both arguments
     * are clamped to it, because the nanosecond arithmetic below overflows on
     * either one left unbounded, and a signed overflow that lands on a negative
     * total reads as "wait forever". */
    int64_t timeout_ns = -1;
    if (Z_TYPE_P(sec) == IS_LONG) {
        zend_long s = Z_LVAL_P(sec);
        zend_long us = (usec != NULL && Z_TYPE_P(usec) == IS_LONG) ? Z_LVAL_P(usec) : 0;
        if (s < 0 || us < 0 || (usec != NULL && Z_TYPE_P(usec) != IS_LONG
                                && Z_TYPE_P(usec) != IS_NULL)) {
            oxphp_orig_stream_select(execute_data, return_value);
            return;
        }
        if (s > 315360000) s = 315360000;
        if (us > 315360000000000LL) us = 315360000000000LL;
        timeout_ns = (int64_t) s * 1000000000 + (int64_t) us * 1000;
    } else if (Z_TYPE_P(sec) != IS_NULL) {
        oxphp_orig_stream_select(execute_data, return_value);
        return;
    } else if (usec != NULL && Z_TYPE_P(usec) != IS_NULL
               && !(Z_TYPE_P(usec) == IS_LONG && Z_LVAL_P(usec) == 0)) {
        /* A null $seconds with a non-zero $microseconds is an argument error
         * native raises before waiting at all. Waiting forever on a call that is
         * specified to throw is the one way this hook could hang a fiber that
         * native would not, so it declines instead. */
        oxphp_orig_stream_select(execute_data, return_value);
        return;
    }

    /* A zero timeout is the non-blocking probe idiom, and native answers it from
     * a select() that does not wait at all. Parking for it would hold the caller
     * to the next scheduler tick to learn something already known, making the
     * idiom slower with the hook than without it. */
    if (timeout_ns == 0) {
        oxphp_orig_stream_select(execute_data, return_value);
        return;
    }

    /* Sized from the arrays rather than from OXPHP_MAX_WAIT_FDS: the cap is the
     * ceiling PHP's own select enforces, and a frame reserving all of it would
     * be 24 KiB for a call that almost always watches one or two descriptors.
     * do_alloca keeps the small case on the stack and spills the large one to
     * the request heap; either way the buffer outlives the suspension, which is
     * all oxphp_fiber_io_wait() asks of it. */
    uint32_t cap = 0;
    if (Z_TYPE_P(r) == IS_ARRAY) cap += zend_hash_num_elements(Z_ARRVAL_P(r));
    if (Z_TYPE_P(w) == IS_ARRAY) cap += zend_hash_num_elements(Z_ARRVAL_P(w));
    if (Z_TYPE_P(e) == IS_ARRAY) cap += zend_hash_num_elements(Z_ARRVAL_P(e));
    if (cap == 0 || cap > OXPHP_MAX_WAIT_FDS) {
        oxphp_orig_stream_select(execute_data, return_value);
        return;
    }

    ALLOCA_FLAG(use_heap)
    struct pollfd *fds = do_alloca(
        cap * (sizeof(struct pollfd) + sizeof(struct oxphp_io_owner)), use_heap);
    /* One allocation for both arrays: they are created together, handed to the
     * wait together and released together. The second half starts a whole
     * number of 8-byte pollfds in, which is the alignment an owner needs. */
    struct oxphp_io_owner *owners = (struct oxphp_io_owner *) (fds + cap);
    uint32_t nfds = 0;
    /* One event mask per array, each matching what PHP's own select() reports
     * that array from: a hangup and a socket error to the read set, a socket
     * error to the write set, and out-of-band data alone to the exception set.
     * Waking on more than the array's own mask would resume a caller its
     * delegate then tells nothing happened; see oxphp_io_entry_ready(). */
    if (!oxphp_select_collect(r, PHP_POLLREADABLE, true, fds, cap, &nfds)
        || !oxphp_select_collect(w, POLLOUT | POLLERR, false, fds, cap, &nfds)
        || !oxphp_select_collect(e, POLLPRI, false, fds, cap, &nfds)
        || nfds == 0) {
        free_alloca(fds, use_heap);
        oxphp_orig_stream_select(execute_data, return_value);
        return;
    }

    /* Native rewrites all three arrays down to the streams that were ready, and
     * does so on every select that succeeds — including one that reports
     * nothing. A second delegation would therefore see three empty arrays and
     * raise the error native raises for a caller who passed none. Hold the
     * originals and put them back before any retry; the caller is suspended
     * inside this call and cannot observe the intermediate state. */
    zval saved_r, saved_w, saved_e;
    ZVAL_COPY(&saved_r, r);
    ZVAL_COPY(&saved_w, w);
    ZVAL_COPY(&saved_e, e);

    struct timespec started;
    clock_gettime(CLOCK_MONOTONIC, &started);

    /* Every exit from here on releases the buffer and the saved arrays through
     * the one label. A drain bails out of oxphp_fiber_io_wait() with
     * zend_bailout and skips it, which leaks nothing: the stack form of the
     * buffer has nothing to release, the heap form is emalloc, and the arrays
     * are the caller's own — all three go with the request. */
    /* A caller's own timeout ends the loop on its own. A null one does not, so
     * something has to bound the case where waking and delegating never makes
     * progress. The one way to reach it is the scheduler releasing every waiter
     * because its readiness wait failed outright: the delegate then reports
     * nothing, the fiber parks again, and the same failure releases it again.
     * That path already promises in the log that hooked waits fall back to
     * blocking the worker thread — this is what makes the promise true here
     * instead of spinning. The bound is far above any number of lost races a
     * shared connection can produce, since each of those follows a real wake-up
     * rather than a failed wait. */
    const int max_barren_retries = 64;
    int barren_retries = 0;

    for (;;) {
        int64_t left_ns = -1;
        if (timeout_ns >= 0) {
            struct timespec now;
            clock_gettime(CLOCK_MONOTONIC, &now);
            int64_t spent = (int64_t)(now.tv_sec - started.tv_sec) * 1000000000
                            + (int64_t)(now.tv_nsec - started.tv_nsec);
            left_ns = timeout_ns - spent;
            if (left_ns < 0) left_ns = 0;
        }

        int rc = oxphp_fiber_io_wait(fds, owners, nfds, left_ns);
        if (rc == 0) {
            /* Declined — nothing was waited for, so the call is handed over
             * untouched, arrays included. */
            goto delegate;
        }
        if (rc == -1) {
            oxphp_throw_exception("OxPHP\\Async\\AsyncException",
                                  "Async task cancelled", 0);
            RETVAL_FALSE;
            goto done;
        }

        /* Ready, or the deadline passed. Either way native decides what to
         * report, without being allowed to wait again. */
        oxphp_select_delegate_now(execute_data, return_value, argc);
        if (EG(exception) != NULL) goto done;
        if (Z_TYPE_P(return_value) != IS_LONG || Z_LVAL_P(return_value) != 0) goto done;
        if (rc == -2) goto done;   /* the caller's timeout really did elapse */

        /* Zero with budget left: the readiness we woke on did not survive to
         * the delegate's own select — another fiber sharing this connection
         * drained it. Restore what the delegate consumed and wait out the
         * remainder rather than report a timeout that has not happened. */
        zval_ptr_dtor(r); ZVAL_COPY(r, &saved_r);
        zval_ptr_dtor(w); ZVAL_COPY(w, &saved_w);
        zval_ptr_dtor(e); ZVAL_COPY(e, &saved_e);

        if (++barren_retries >= max_barren_retries) goto delegate;
    }

delegate:
    oxphp_orig_stream_select(execute_data, return_value);
done:
    zval_ptr_dtor(&saved_r);
    zval_ptr_dtor(&saved_w);
    zval_ptr_dtor(&saved_e);
    free_alloca(fds, use_heap);
}

static const char *const oxphp_hook_categories[] = { "sleep", "streams" };

/* A category name that matches nothing disables the hook the operator asked
 * for, silently and identically to not setting the variable at all. Name the
 * unrecognised ones at startup; the boolean spellings carry no category list
 * and are left alone.
 *
 * This warns where the rest of the configuration would abort — a malformed
 * ASYNC_WORKERS is a startup error, deliberately. The difference is that this
 * value is a list of names whose vocabulary grows between releases: a config
 * naming a category a newer build understands would stop an older binary from
 * starting at all, which turns a rollback into an outage. A typo and a
 * from-the-future name are indistinguishable here, so the failure mode that
 * keeps the server up is the right one to pick. */
static void oxphp_hooks_report_unknown_categories(void)
{
    size_t env_len = 0;
    const char *env = oxphp_hooks_env(&env_len);
    if (env == NULL) return;

    if (oxphp_hooks_token_is(env, env_len, "0")
        || oxphp_hooks_token_is(env, env_len, "false")
        || oxphp_hooks_token_is(env, env_len, "1")
        || oxphp_hooks_token_is(env, env_len, "true")
        || oxphp_hooks_token_is(env, env_len, "all")) {
        return;
    }

    const char *p = env;
    const char *end = env + env_len;
    while (p < end) {
        const char *comma = memchr(p, ',', (size_t)(end - p));
        size_t len = comma ? (size_t)(comma - p) : (size_t)(end - p);
        const char *tok = p;
        p = comma ? comma + 1 : end;

        oxphp_hooks_trim_token(&tok, &len);
        if (len == 0) continue;

        bool known = false;
        for (size_t i = 0; i < sizeof(oxphp_hook_categories) / sizeof(oxphp_hook_categories[0]); i++) {
            if (oxphp_hooks_token_is(tok, len, oxphp_hook_categories[i])) {
                known = true;
                break;
            }
        }
        if (!known) {
            /* Plain buffer rather than zend_strpprintf: MINIT has no request
             * arena to lean on, and the message is bounded anyway. */
            char msg[256];
            snprintf(msg, sizeof(msg),
                     "oxphp: RUNTIME_HOOKS lists an unknown category \"%.*s\", which enables "
                     "nothing; known categories are sleep and streams", (int) len, tok);
            php_log_err(msg);
        }
    }
}

static void oxphp_runtime_hooks_install(void)
{
    oxphp_hooks_report_unknown_categories();

    if (oxphp_hooks_category_enabled("sleep")) {
        oxphp_hook_swap("sleep", sizeof("sleep") - 1,
                        oxphp_hooked_sleep, &oxphp_orig_sleep);
        oxphp_hook_swap("usleep", sizeof("usleep") - 1,
                        oxphp_hooked_usleep, &oxphp_orig_usleep);
    }
    if (oxphp_hooks_category_enabled("streams")) {
        if (!oxphp_hook_socket_ops()) {
            /* MINIT has no request context, and this is the only signal that
             * part of the category was dropped, so it goes to the server log
             * rather than wherever error output happens to be pointed. The
             * stream_select() hook needs no writable page and stays installed. */
            php_log_err("oxphp: socket stream hooks unavailable (the stream ops table "
                        "could not be made writable); socket reads stay blocking");
        }
        oxphp_hook_swap("stream_select", sizeof("stream_select") - 1,
                        oxphp_hooked_stream_select, &oxphp_orig_stream_select);
    }
}

static void oxphp_runtime_hooks_restore(void)
{
    oxphp_hook_restore("sleep", sizeof("sleep") - 1, oxphp_orig_sleep);
    oxphp_hook_restore("usleep", sizeof("usleep") - 1, oxphp_orig_usleep);
    oxphp_orig_sleep = NULL;
    oxphp_orig_usleep = NULL;
    oxphp_hook_restore("stream_select", sizeof("stream_select") - 1,
                       oxphp_orig_stream_select);
    oxphp_orig_stream_select = NULL;
    oxphp_restore_socket_ops();
}

/* ─── Worker Mode: soft reset between requests ─────────────── */

/* Discard any output a background async task left in the shared PHP output
 * buffer, freeing the backing allocation, and restart a clean default buffer.
 * Called by the async driver only when the worker is idle (no fiber running),
 * so nothing is mid-write. DISCARDS (does not flush) — the bytes have no
 * client. Returns the bytes discarded from the active buffer (the metric
 * undercounts nested ob_start leftovers; discard_all still frees them). */
static uint64_t oxphp_async_sched_drain_output(void) {
    int lvl = php_output_get_level();
    if (lvl <= 0) {
        return 0;
    }

    zval zlen;
    uint64_t used = 0;
    if (php_output_get_length(&zlen) == SUCCESS && Z_TYPE(zlen) == IS_LONG
        && Z_LVAL(zlen) > 0) {
        used = (uint64_t) Z_LVAL(zlen);
    }

    if (lvl > 1 || used > 0) {
        php_output_discard_all(); /* pop+free all buffers, no flush */
        if (PG(output_buffering)) {
            /* restore the default buffer exactly as php_request_startup does */
            php_output_start_user(
                NULL,
                PG(output_buffering) > 1 ? PG(output_buffering) : 0,
                PHP_OUTPUT_HANDLER_STDFLAGS);
        }
        return used;
    }
    return 0;
}

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
        if (sched.fiber_count == 0 && !oxphp_bridge_has_deferred_drains()) {
            /* ── No active fibers and no deferred promise drains: block-wait
             * for the next request. When deferred drains remain, fall through
             * to the event-loop branch instead — its tick both accepts new
             * requests and polls the drains, so a fire-and-forget promise left
             * by the last request is reclaimed without waiting for the next
             * one to arrive. ──────────────────────────────────────────── */

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

            /* Fresh or recycled, a new request is always a start, never a resume. */
            oxphp_scheduler_start_fiber(&sched, fiber);

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
                /* No work done — pause briefly to avoid busy-wait. 100μs is
                 * short enough for responsive SSE, long enough to avoid CPU
                 * spin. When fibers are parked on sockets, spend that same
                 * 100μs waiting on those descriptors instead of sleeping
                 * blind, so a peer's reply resumes its fiber immediately
                 * rather than on the next tick. */
                if (!oxphp_scheduler_io_backoff(&sched, 100000)) {
                    usleep(100);
                }
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
    void *this_zval = NULL;

    /* For instance methods on classes registered with custom storage,
     * extract rust_data and class_index from the custom object wrapper —
     * O(1) read of the prefix that `oxphp_plugin_create_object` allocated.
     *
     * For instance methods on plugin classes WITHOUT custom storage
     * (e.g. exception classes that just hold PHP-level properties), the
     * underlying zend_object has no oxphp_custom_object prefix, so we
     * resolve class_index from the scope name via a linear scan and leave
     * rust_data NULL. The fast path is selected by checking whether the
     * class entry's `create_object` slot points to our custom hook —
     * that's exactly the bit set during registration for custom-storage
     * classes (see oxphp_register_class flow above).
     *
     * `this_zval` exposes the zval* of `$this` to the dispatch callback
     * so Rust handlers can read PHP-level properties via
     * oxphp_object_read_property. */
    if (Z_TYPE(execute_data->This) == IS_OBJECT) {
        this_zval = (void *)&execute_data->This;
        zend_class_entry *ce = Z_OBJCE(execute_data->This);
        if (ce->create_object == oxphp_plugin_create_object) {
            /* Fast path: O(1) prefix read for custom-storage classes. */
            oxphp_custom_object *intern = OXPHP_OBJ(Z_OBJ(execute_data->This));
            rust_data = intern->rust_data;
            class_index = intern->class_index;
        } else {
            /* Slow fallback: linear scan to resolve class_index from
             * scope name. Only for plugin classes without custom storage
             * (e.g. AggregateAsyncException, TimeoutException). */
            zend_class_entry *scope = execute_data->func->common.scope;
            int cls_count = oxphp_bridge_get_plugin_class_count();
            int found = 0;
            for (int i = 0; i < cls_count; i++) {
                const char *fqn = oxphp_bridge_get_class_fqn(i);
                if (fqn && scope && strcmp(ZSTR_VAL(scope->name), fqn) == 0) {
                    class_index = (uint32_t)i;
                    found = 1;
                    break;
                }
            }
            if (!found) {
                zend_throw_error(NULL,
                    "OxPHP plugin method %s::%s dispatched but class is not registered",
                    scope ? ZSTR_VAL(scope->name) : "?",
                    method_name);
                return;
            }
        }
    } else if (execute_data->func->common.scope) {
        /* Static method — find class_index from the scope CE.
         * Walk the plugin class CE array to find the match. */
        zend_class_entry *scope = execute_data->func->common.scope;
        int cls_count = oxphp_bridge_get_plugin_class_count();
        int found = 0;
        for (int i = 0; i < cls_count; i++) {
            const char *fqn = oxphp_bridge_get_class_fqn(i);
            if (fqn && strcmp(ZSTR_VAL(scope->name), fqn) == 0) {
                class_index = (uint32_t)i;
                found = 1;
                break;
            }
        }
        if (!found) {
            zend_throw_error(NULL,
                "OxPHP plugin static method %s::%s dispatched but class is not registered",
                ZSTR_VAL(scope->name),
                method_name);
            return;
        }
    }

    oxphp_method_dispatch_fn_t dispatch = oxphp_bridge_get_method_dispatch();
    if (!dispatch) {
        zend_throw_error(NULL, "OxPHP method dispatch not initialized");
        return;
    }

    int rc = dispatch(class_index, method_name, args, argc, return_value, rust_data, this_zval);
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

    /* Function/method and class attributes are kept in separate arrays so
     * Rust can count attribute occurrences independently per scope. A flat
     * list would lose the fn/class boundary, causing a repeated or
     * dual-scope attribute name to alias occurrence 0. */
    const char *fn_attr_names[64];
    uint32_t fn_attr_count = 0;
    if (func->common.attributes) {
        zend_attribute *attr;
        ZEND_HASH_FOREACH_PTR(func->common.attributes, attr) {
            if (fn_attr_count < 64) {
                fn_attr_names[fn_attr_count++] = ZSTR_VAL(attr->name);
            }
        } ZEND_HASH_FOREACH_END();
    }

    /* Class attributes (for TARGET_CLASS) */
    const char *class_attr_names[64];
    uint32_t class_attr_count = 0;
    if (func->common.scope && func->common.scope->attributes) {
        zend_attribute *attr;
        ZEND_HASH_FOREACH_PTR(func->common.scope->attributes, attr) {
            if (class_attr_count < 64) {
                class_attr_names[class_attr_count++] = ZSTR_VAL(attr->name);
            }
        } ZEND_HASH_FOREACH_END();
    }

    if (fn_attr_count == 0 && class_attr_count == 0) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    /* Attribute resolver context — lets Rust read each matched
     * decorator's constructor arguments during resolve so per-attribute
     * parameters (e.g. #[SlowThreshold(ms: 250)]) reach the decorator. */
    ox_attr_resolver_ctx_t actx = {
        .scope       = func->common.scope,
        .fn_attrs    = func->common.attributes,
        .class_attrs = func->common.scope ? func->common.scope->attributes : NULL,
    };

    uintptr_t fn_id = (uintptr_t)func;
    int found = resolve(fn_id,
                        fn_attr_count > 0 ? fn_attr_names : NULL, fn_attr_count,
                        class_attr_count > 0 ? class_attr_names : NULL, class_attr_count,
                        &actx);
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
    if (!dctx) {
        /* Stack overflow: more than OXPHP_DECORATOR_CTX_STACK_MAX levels of
         * nested decorated calls. Fail loud instead of silently reusing the
         * top slot and corrupting outer frames' context. The matching end()
         * (always called — see oxphp_decorator_end) unwinds the depth. */
        zend_throw_exception_ex(oxphp_decorator_stack_overflow_ce, 0,
            "Decorator context stack overflow: more than %d levels of "
            "nested decorated calls", OXPHP_DECORATOR_CTX_STACK_MAX);
        oxphp_force_exception_on_current_frame();
        return;
    }
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

                /* before() below runs arbitrary PHP that may instantiate more
                 * decorators and resize decorator_instance_cache_ht, which
                 * reallocates its arData and invalidates this bucket pointer.
                 * Hold an owned copy (refcount-bumped) so the instance survives
                 * a resize for the duration of the call. */
                zval dec_local;
                ZVAL_COPY(&dec_local, dec_instance);

                /* Create context for before() */
                zval ctx_zval;
                oxphp_create_decorator_context(&ctx_zval, dctx, 0, NULL);

                /* Call $dec->before($ctx) */
                zval retval;
                ZVAL_UNDEF(&retval);
                zend_call_method_with_1_params(
                    Z_OBJ_P(&dec_local), Z_OBJCE_P(&dec_local),
                    NULL, "before", &retval, &ctx_zval);
                zval_ptr_dtor(&retval);
                zval_ptr_dtor(&ctx_zval);
                zval_ptr_dtor(&dec_local);

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

                        /* Own a copy — after() may resize the cache and
                         * invalidate this bucket pointer (see before()). */
                        zval prev_local;
                        ZVAL_COPY(&prev_local, prev_cached);

                        zval cleanup_ret;
                        ZVAL_UNDEF(&cleanup_ret);
                        /* Save and clear exception to allow after() to run */
                        zend_object *saved_exception = EG(exception);
                        EG(exception) = NULL;
                        zend_call_method_with_1_params(
                            Z_OBJ_P(&prev_local),
                            Z_OBJCE_P(&prev_local),
                            NULL, "after", &cleanup_ret, &cleanup_ctx);
                        zval_ptr_dtor(&cleanup_ret);
                        zval_ptr_dtor(&prev_local);
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
    if (!dctx) {
        /* peek() is NULL either at depth underflow (no-op pop) or for an
         * overflow frame whose begin() threw StackOverflowException without
         * pushing — unwind the depth counter so begin/end stay balanced. */
        oxphp_decorator_ctx_pop();
        return;
    }

    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t now_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
    uint64_t elapsed_ns = now_ns - dctx->timestamp_ns;
    int success = !EG(exception);

    /* Dispatch to Rust decorators (reverse order handled in Rust) */
    oxphp_decorator_end_fn_t end_fn = oxphp_bridge_get_decorator_end();
    if (end_fn) {
        const char *exc_class = NULL;
        char *exc_msg = NULL, *exc_trace = NULL;
        size_t exc_class_len = 0, exc_msg_len = 0, exc_trace_len = 0;
        if (!success && EG(exception)) {
            oxphp_exception_capture(EG(exception), &exc_class, &exc_class_len,
                                    &exc_msg, &exc_msg_len, &exc_trace, &exc_trace_len);
        }
        end_fn(dctx->fn_id, elapsed_ns, success, exc_class, exc_class_len,
               exc_msg, exc_msg_len, exc_trace, exc_trace_len);
        free(exc_msg);   /* free(NULL) is safe */
        free(exc_trace);
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

                /* Own a copy — after() may resize the cache and invalidate
                 * this bucket pointer (see before()). */
                zval cached_local;
                ZVAL_COPY(&cached_local, cached);

                zval after_ret;
                ZVAL_UNDEF(&after_ret);
                zend_call_method_with_1_params(
                    Z_OBJ_P(&cached_local),
                    Z_OBJCE_P(&cached_local),
                    NULL, "after", &after_ret, &ctx_zval);
                zval_ptr_dtor(&after_ret);
                zval_ptr_dtor(&cached_local);

                /* If after() throws, stop dispatching remaining decorators */
                if (EG(exception)) break;
            }

            zval_ptr_dtor(&ctx_zval);
        }
    }

    oxphp_decorator_ctx_pop();
}
/* }}} */

/* Bridge-shaped adapter for the async driver's idle backoff (see
 * oxphp_bridge_set_async_io_backoff_fn). The budget is clamped rather than
 * cast: a value that overflowed a signed long would arm the backoff's timer
 * with a negative interval, which fails immediately and turns the driver's
 * backoff into a spin. One second is far beyond any sensible idle interval. */
static int oxphp_async_io_backoff_bridge(uint64_t ns) {
    const uint64_t max_backoff_ns = 1000000000ULL;
    if (ns > max_backoff_ns) ns = max_backoff_ns;

    return oxphp_async_sched_io_backoff((int64_t) ns) ? 1 : 0;
}

/* SAPI-side predicate for `oxphp_bridge_in_fiber`. Returns 1 iff the calling
 * code is running on an oxphp scheduler fiber's own context — i.e. one that
 * `oxphp_fiber_suspend_for_await` can actually suspend.
 *
 * A user-level `Fiber::start()` outside any oxphp fiber never sets
 * `oxphp_current_fiber`, so the pointer alone answers that case. It does not
 * answer the nested one: a userland fiber started inside an oxphp fiber runs on
 * its own context while the pointer still names the outer fiber, and suspending
 * from there corrupts both (see oxphp_fiber_owns_current_context). */
int oxphp_in_oxphp_fiber(void) {
    return (oxphp_current_fiber != NULL
            && oxphp_fiber_owns_current_context(oxphp_current_fiber)) ? 1 : 0;
}

/* Fiber-aware await helper. Called from Rust handler via FFI.
 * Returns: 0 = fiber handled it (retval populated), 1 = not in fiber (caller does blocking),
 *         -1 = error (exception details in bridge TLS), -2 = timeout */
int oxphp_fiber_suspend_for_await(int64_t promise_id, double timeout, void *retval) {
    if (oxphp_current_fiber == NULL
        || !oxphp_fiber_owns_current_context(oxphp_current_fiber)) {
        return 1; /* Not on a suspendable fiber — caller does a blocking await */
    }

    oxphp_request_fiber *self = oxphp_current_fiber;
    self->suspend_reason = OXPHP_SUSPEND_AWAIT;
    self->suspend_data.promise_id = promise_id;

    /* Arm a per-call deadline so the scheduler can unwind this await if the
     * awaited promise does not settle in time. timeout <= 0 means "wait
     * forever" (no deadline). Without this the cooperative fiber path would
     * ignore the timeout entirely and block until the promise settles or the
     * outer await budget elapsed — losing the inner timeout in composition. */
    if (timeout > 0.0) {
        struct timespec ts;
        clock_gettime(CLOCK_MONOTONIC, &ts);
        uint64_t now_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
        self->await_deadline_ns = now_ns + (uint64_t)(timeout * 1000000000.0);
    } else {
        self->await_deadline_ns = 0;
    }

    zend_fiber_transfer transfer = {
        .context = self->scheduler,
        .flags = 0
    };
    ZVAL_NULL(&transfer.value);

    oxphp_current_fiber = NULL;
    zend_fiber_switch_context(&transfer);
    /* --- RESUMED by scheduler when promise result is ready, when the task was
     * cancelled (awaiter gave up), or when the per-call deadline elapsed. Disarm
     * the deadline first; then unwind on cancel/timeout by signalling the caller
     * to throw instead of consuming a result. */
    self->await_deadline_ns = 0;
    /* Drain: bail uncatchably before touching the (unsettled) promise result. */
    if (self->drain_kill) {
        oxphp_fiber_drain_bail();
    }
    if (self->cancel_requested) {
        self->cancel_requested = false;
        return -3; /* cancelled */
    }
    if (self->timed_out) {
        self->timed_out = false;
        return -2; /* timed out */
    }

    int rc = oxphp_bridge_await_dispatch(promise_id, 0.0, (zval *)retval);
    return rc; /* 0 = success, -1 = error, -2 = timeout */
}

/* Cooperative yield: suspend the current task fiber for one scheduler cycle
 * (a ~1ms timer) so other fibers / nested tasks can make progress, then
 * resume. Used by the fiber-aware await_race / await_any poll loops to avoid
 * pinning the worker while waiting for one of several promises to settle.
 * Returns 1 if it suspended (in a fiber), 0 if not in a fiber (caller falls
 * back to blocking), -3 if the task was cancelled while yielded. */
int oxphp_fiber_suspend_for_yield(void) {
    if (oxphp_current_fiber == NULL
        || !oxphp_fiber_owns_current_context(oxphp_current_fiber)) {
        return 0; /* not on a suspendable fiber — caller falls back to blocking */
    }

    oxphp_request_fiber *self = oxphp_current_fiber;
    uint64_t timer_id = oxphp_bridge_timer_register(1); /* ~1ms; resumed by tick */
    self->suspend_reason = OXPHP_SUSPEND_SLEEP;
    self->suspend_data.timer_id = timer_id;

    zend_fiber_transfer transfer = {
        .context = self->scheduler,
        .flags = 0
    };
    ZVAL_NULL(&transfer.value);

    oxphp_current_fiber = NULL;
    zend_fiber_switch_context(&transfer);
    /* --- RESUMED on the next scheduler tick once the timer expires --- */
    if (self->drain_kill) {
        oxphp_fiber_drain_bail();
    }
    if (self->cancel_requested) {
        self->cancel_requested = false;
        return -3; /* cancelled */
    }
    return 1;
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
    /* Path B: per-fiber async cancellation. A timed-out awaiter sets the
     * running task fiber's shared cancel cell and kicks this worker thread's
     * vm_interrupt cross-thread (the scheduler loop can't reach a CPU-bound
     * fiber). The kick fires in whichever fiber is running, so unwind only if
     * it is the cancelled one — throwing a PHP exception that the VM propagates
     * up this fiber's stack to its scheduler entry (rejected task), exactly like
     * a suspend-point cancel. cancel_cell is NULL for HTTP request fibers and
     * for the scheduler context, so this is a no-op there.
     *
     * Latency bound: the kick is honoured only at opcode boundaries (loop
     * backedges, call boundaries). A fiber stuck inside a single long C call —
     * a catastrophic preg_match, a large gzuncompress, a blocking
     * fread/fwrite/PDO query — is interrupted only when that call returns,
     * because C functions do not poll vm_interrupt from inside their own loops.
     * This is the same limit as PHP's own max_execution_time (SIGALRM sets
     * EG(timed_out), also acted on at opcode boundaries); a threaded SAPI
     * cannot kill a worker mid-C-call without corrupting Zend allocator state.
     * So for a cancelled task: side effects across opcode boundaries are
     * prevented (the throw lands before the next PHP statement), but those
     * inside the one in-flight C call complete before the fiber unwinds. */
    /* memory_order_acquire pairs with the awaiter's Release store of the cancel
     * flag (Rust side): once we observe it set, the publish is visible without
     * relying on vm_interrupt's barrier to carry it. */
    if (oxphp_current_fiber != NULL
        && oxphp_current_fiber->cancel_cell != NULL
        && atomic_load_explicit(oxphp_current_fiber->cancel_cell,
                                memory_order_acquire)) {
        oxphp_throw_exception("OxPHP\\Async\\AsyncException",
                              "Async task cancelled", 0);
        return; /* VM checks EG(exception) on return → unwinds this fiber only */
    }

    /* SIGALRM-driven max_execution_time: Zend sets EG(timed_out)=1
     * alongside vm_interrupt. Convert it to the unified cancellation
     * reason and claim the flag so zend_timeout()'s default
     * "Maximum execution time exceeded" path doesn't also fire. */
    if (zend_atomic_bool_load_ex(&EG(timed_out))) {
        oxphp_bridge_set_cancel_reason(OXPHP_CANCEL_TIMEOUT);
        zend_atomic_bool_store_ex(&EG(timed_out), false);
    }

    oxphp_cancel_reason_t reason = oxphp_bridge_get_cancel_reason();

    /* Hard-drain broadcast kick: the drain deadline passed and Rust raised
     * vm_interrupt on every worker with requests in flight. Under fiber
     * multiplexing the registry slot names only the most recently prepared
     * request, so no per-request reason may have reached the cell of the
     * request actually running here — self-cancel it (CAS, first writer
     * wins; if the cell already holds a reason, that reason is used). */
    if (reason == OXPHP_CANCEL_NONE && oxphp_bridge_is_drain_hard()) {
        oxphp_bridge_set_cancel_reason(OXPHP_CANCEL_SHUTDOWN);
        reason = oxphp_bridge_get_cancel_reason();
    }

    if (reason == OXPHP_CANCEL_NONE) {
        if (orig_zend_interrupt_function) {
            orig_zend_interrupt_function(execute_data);
        }
        return;
    }

    if (reason == OXPHP_CANCEL_CLIENT_ABORT) {
        PG(connection_status) |= PHP_CONNECTION_ABORTED;
        /* ignore_user_abort() lets a script outlive its client — but not the
         * server. Once the drain deadline has passed, the hard kick must be
         * able to unwind a request whose cell already holds CLIENT_ABORT
         * (first-writer-wins), or it survives until the forced process exit. */
        if (PG(ignore_user_abort) && !oxphp_bridge_is_drain_hard()) {
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

        /* A drain unwind is administrative, not a handler defect — mark it so
         * oxphp_scheduler_finalize_fiber keeps the consecutive-error breaker
         * neutral. The scheduler's sweep sets this itself for the fibers it
         * force-resumes; the two paths that reach a RUNNING request cannot:
         * the streaming flush path's self-cancel (tight flush loops the sweep
         * never sees) and the deadline kick (broadcast vm_interrupt). Three
         * such kills in a row would otherwise trip WORKER_MAX_CONSECUTIVE_ERRORS
         * and error-exit the worker mid-drain, destroying the ordinary in-flight
         * requests the drain exists to protect. */
        if (reason == OXPHP_CANCEL_SHUTDOWN && oxphp_current_fiber != NULL) {
            oxphp_current_fiber->drain_kill = true;
        }
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

    /* OxPHP\Decorator\StackOverflowException */
    {
        zend_class_entry tmp_ce;
        INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Decorator", "StackOverflowException", NULL);
        oxphp_decorator_stack_overflow_ce = zend_register_internal_class_ex(&tmp_ce, zend_ce_exception);
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
    oxphp_bridge_set_fiber_yield(oxphp_fiber_suspend_for_yield);
    oxphp_bridge_set_current_fiber_id_fn(oxphp_fiber_current_id);
    oxphp_bridge_set_async_io_backoff_fn(oxphp_async_io_backoff_bridge);

    /* Register async-task scheduler callbacks (stub bodies for now; the
     * Rust fiber-mode async driver reaches the scheduler through these). */
    oxphp_bridge_set_async_sched_callbacks(
        oxphp_async_sched_spawn,
        oxphp_async_sched_tick,
        oxphp_async_sched_poll_completed,
        oxphp_async_sched_release,
        oxphp_async_sched_cancel,
        oxphp_async_sched_drain_output);

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

    /* Last: all extensions (standard included) have registered their
     * functions by now; worker threads have not started yet. */
    oxphp_runtime_hooks_install();

    return SUCCESS;
}
/* }}} */

/* {{{ MSHUTDOWN — clear ox_shared class_entry cache */
PHP_MSHUTDOWN_FUNCTION(oxphp_sapi)
{
    oxphp_runtime_hooks_restore();
    oxphp_shareable_unregister_ce();
    return SUCCESS;
}
/* }}} */

/* {{{ RINIT — per-thread APM hook installation */
PHP_RINIT_FUNCTION(oxphp_sapi)
{
    oxphp_apm_install_on_thread();  /* no-op after first call per thread */

    /* Defensive: clear the per-thread aggregate-exception buffer in case
     * a prior request faulted between aggregate_push and the trailing
     * aggregate_clear in oxphp_bridge_aggregate_throw{,_timeout} (e.g.
     * a fatal during array_init or zend_throw_exception_object). The
     * normal happy/error paths already clear; this is belt-and-suspenders
     * so a stranded entry can't bleed into a new request. */
    oxphp_bridge_aggregate_clear();

    /* Mark the decorator instance cache uninitialized so the first decorator
     * of this request re-creates it in fresh request-scoped memory.
     * The HashTable is allocated non-persistent (its arData lives in the Zend
     * MM request arena), so php_request_shutdown of the *previous* request
     * freed arData — but the __thread `initialized` flag persists across
     * requests. Without this reset, ensure_init() would skip re-init and
     * zend_hash_index_find() would dereference the freed arData (a
     * use-after-free that crashes under amd64 heap reuse, stays latent on
     * arm64). RSHUTDOWN still runs zend_hash_clean() to dtor cached instances
     * while they are valid; this only forces a fresh table next request. */
    decorator_instance_cache_initialized = 0;

    return SUCCESS;
}
/* }}} */

/* {{{ RSHUTDOWN — cleanup outstanding async promises */
PHP_RSHUTDOWN_FUNCTION(oxphp_sapi)
{
    /* Tear down this thread's async task scheduler (if any). The async worker
     * runs a single long-lived request, so this fires once at thread exit while
     * the heap is still live — freeing fiber C stacks and task payload that
     * would otherwise leak across worker respawns. No-op on HTTP/CLI threads. */
    oxphp_async_sched_shutdown();

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
