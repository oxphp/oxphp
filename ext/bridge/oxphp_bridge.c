#include "oxphp_bridge.h"
#include <string.h>
#include <stdlib.h>

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

static oxphp_dispatch_fn_t dispatch_fn = NULL;
static oxphp_call_php_fn_t call_php_fn = NULL;

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

void oxphp_bridge_set_dispatch_fn(oxphp_dispatch_fn_t fn) {
    dispatch_fn = fn;
}

oxphp_dispatch_fn_t oxphp_bridge_get_dispatch_fn(void) {
    return dispatch_fn;
}

void oxphp_bridge_set_call_php_fn(oxphp_call_php_fn_t fn) {
    call_php_fn = fn;
}

oxphp_call_php_fn_t oxphp_bridge_get_call_php_fn(void) {
    return call_php_fn;
}

char* oxphp_bridge_dispatch(const char* name, const char* json_args) {
    if (!dispatch_fn) return NULL;
    return dispatch_fn(name, json_args);
}

char* oxphp_bridge_call_php(const char* name, const char* json_args) {
    if (!call_php_fn) return NULL;
    return call_php_fn(name, json_args);
}

char* oxphp_bridge_strdup(const char* s) {
    if (!s) return NULL;
    return strdup(s);
}

void oxphp_bridge_free_string(char* ptr) {
    free(ptr);
}
