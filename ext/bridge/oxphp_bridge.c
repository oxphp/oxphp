#include "oxphp_bridge.h"
#include <stdint.h>
#include <stdbool.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <time.h>
#include <pthread.h>
#include <inttypes.h>
#include <sys/resource.h>
#include <unistd.h>
#include <fcntl.h>

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

void oxphp_bridge_set_worker_start_time(double time) {
    ctx.worker_start_time = time;
}

double oxphp_bridge_get_worker_start_time(void) {
    return ctx.worker_start_time;
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

/* ═══════════════════════════════════════════════════════════
 *  Plugin Class Registry (global, NOT __thread)
 *
 *  Same thread-safety model as the function registry: written once
 *  from the main thread during startup, read during MINIT on the
 *  same thread. No concurrent access.
 * ═══════════════════════════════════════════════════════════ */

#define MAGIC_METHOD_COUNT 17

/* Sub-entries for class properties, constants, and methods. */

typedef struct {
    char *name;
    uint32_t visibility;
    uint32_t modifiers;
    int type_info;
    char *default_value;    /* NULL = no default */
} oxphp_class_property_t;

typedef struct {
    char *name;
    uint32_t visibility;
    char *value;
} oxphp_class_constant_t;

typedef struct {
    char *name;
    uint32_t visibility;
    uint32_t flags;
    int required_params;
    int total_params;
    int is_variadic;
    int return_type;
    int return_nullable;
    /* Per-param metadata (length = total_params, NULL when total_params == 0). */
    char **param_names;
    int *param_types;
    int *param_optional;
} oxphp_class_method_t;

typedef struct {
    char *fqn;
    char *parent_fqn;           /* NULL if no parent */
    uint32_t flags;             /* ZEND_ACC_FINAL, ZEND_ACC_ABSTRACT, etc. */
    int has_custom_object;

    /* Interfaces */
    char **interface_fqns;
    int interface_count;
    int interface_capacity;

    /* Properties */
    oxphp_class_property_t *properties;
    int property_count;
    int property_capacity;

    /* Constants */
    oxphp_class_constant_t *constants;
    int constant_count;
    int constant_capacity;

    /* Methods */
    oxphp_class_method_t *methods;
    int method_count;
    int method_capacity;

    /* Magic method flags (1 = has handler, indexed by MagicMethod enum) */
    int magic_handlers[MAGIC_METHOD_COUNT];
} oxphp_plugin_class_entry_t;

static oxphp_plugin_class_entry_t *plugin_classes = NULL;
static int plugin_class_count = 0;
static int plugin_class_capacity = 0;

int oxphp_bridge_register_class(const char *fqn, const char *parent_fqn, uint32_t flags) {
    if (!fqn) return -1;
    if (plugin_class_count >= plugin_class_capacity) {
        int new_cap = plugin_class_capacity == 0 ? 8 : plugin_class_capacity * 2;
        oxphp_plugin_class_entry_t *new_arr = realloc(plugin_classes, new_cap * sizeof(*plugin_classes));
        if (!new_arr) return -1;
        plugin_classes = new_arr;
        plugin_class_capacity = new_cap;
    }
    int idx = plugin_class_count++;
    oxphp_plugin_class_entry_t *e = &plugin_classes[idx];
    memset(e, 0, sizeof(*e));
    e->fqn = strdup(fqn);
    e->parent_fqn = parent_fqn ? strdup(parent_fqn) : NULL;
    e->flags = flags;
    return idx;
}

void oxphp_bridge_class_implements(int h, const char *interface_fqn) {
    if (h < 0 || h >= plugin_class_count || !interface_fqn) return;
    oxphp_plugin_class_entry_t *e = &plugin_classes[h];
    if (e->interface_count >= e->interface_capacity) {
        int new_cap = e->interface_capacity == 0 ? 4 : e->interface_capacity * 2;
        char **new_arr = realloc(e->interface_fqns, new_cap * sizeof(char*));
        if (!new_arr) return;
        e->interface_fqns = new_arr;
        e->interface_capacity = new_cap;
    }
    e->interface_fqns[e->interface_count++] = strdup(interface_fqn);
}

void oxphp_bridge_class_add_property(int h, const char *name,
    uint32_t visibility, uint32_t modifiers, int type_info, const char *default_value)
{
    if (h < 0 || h >= plugin_class_count || !name) return;
    oxphp_plugin_class_entry_t *e = &plugin_classes[h];
    if (e->property_count >= e->property_capacity) {
        int new_cap = e->property_capacity == 0 ? 4 : e->property_capacity * 2;
        oxphp_class_property_t *new_arr = realloc(e->properties, new_cap * sizeof(*e->properties));
        if (!new_arr) return;
        e->properties = new_arr;
        e->property_capacity = new_cap;
    }
    oxphp_class_property_t *p = &e->properties[e->property_count++];
    p->name = strdup(name);
    p->visibility = visibility;
    p->modifiers = modifiers;
    p->type_info = type_info;
    p->default_value = default_value ? strdup(default_value) : NULL;
}

void oxphp_bridge_class_add_constant(int h, const char *name,
    uint32_t visibility, const char *value)
{
    if (h < 0 || h >= plugin_class_count || !name || !value) return;
    oxphp_plugin_class_entry_t *e = &plugin_classes[h];
    if (e->constant_count >= e->constant_capacity) {
        int new_cap = e->constant_capacity == 0 ? 4 : e->constant_capacity * 2;
        oxphp_class_constant_t *new_arr = realloc(e->constants, new_cap * sizeof(*e->constants));
        if (!new_arr) return;
        e->constants = new_arr;
        e->constant_capacity = new_cap;
    }
    oxphp_class_constant_t *c = &e->constants[e->constant_count++];
    c->name = strdup(name);
    c->visibility = visibility;
    c->value = strdup(value);
}

void oxphp_bridge_class_add_method(int h, const char *name,
    uint32_t visibility, uint32_t flags, int required_params, int total_params, int is_variadic,
    int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional)
{
    if (h < 0 || h >= plugin_class_count || !name) return;
    oxphp_plugin_class_entry_t *e = &plugin_classes[h];
    if (e->method_count >= e->method_capacity) {
        int new_cap = e->method_capacity == 0 ? 8 : e->method_capacity * 2;
        oxphp_class_method_t *new_arr = realloc(e->methods, new_cap * sizeof(*e->methods));
        if (!new_arr) return;
        e->methods = new_arr;
        e->method_capacity = new_cap;
    }
    oxphp_class_method_t *m = &e->methods[e->method_count++];
    memset(m, 0, sizeof(*m));
    m->name = strdup(name);
    m->visibility = visibility;
    m->flags = flags;
    m->required_params = required_params;
    m->total_params = total_params;
    m->is_variadic = is_variadic;
    m->return_type = return_type;
    m->return_nullable = return_nullable;
    if (total_params > 0) {
        m->param_names = calloc(total_params, sizeof(char *));
        m->param_types = calloc(total_params, sizeof(int));
        m->param_optional = calloc(total_params, sizeof(int));
        if (m->param_names && m->param_types && m->param_optional) {
            for (int i = 0; i < total_params; i++) {
                const char *pn = (param_names && param_names[i]) ? param_names[i] : "";
                m->param_names[i] = strdup(pn);
                m->param_types[i] = param_types ? param_types[i] : 0;
                m->param_optional[i] = param_optional ? param_optional[i] : 0;
            }
        }
    }
}

void oxphp_bridge_class_set_magic(int h, int magic_type, int has_handler) {
    if (h < 0 || h >= plugin_class_count) return;
    if (magic_type < 0 || magic_type >= MAGIC_METHOD_COUNT) return;
    plugin_classes[h].magic_handlers[magic_type] = has_handler;
}

void oxphp_bridge_class_enable_custom_object(int h) {
    if (h < 0 || h >= plugin_class_count) return;
    plugin_classes[h].has_custom_object = 1;
}

int oxphp_bridge_get_plugin_class_count(void) { return plugin_class_count; }

const char *oxphp_bridge_get_class_fqn(int i) {
    if (i < 0 || i >= plugin_class_count) return NULL;
    return plugin_classes[i].fqn;
}
const char *oxphp_bridge_get_class_parent(int i) {
    if (i < 0 || i >= plugin_class_count) return NULL;
    return plugin_classes[i].parent_fqn;
}
uint32_t oxphp_bridge_get_class_flags(int i) {
    if (i < 0 || i >= plugin_class_count) return 0;
    return plugin_classes[i].flags;
}
int oxphp_bridge_get_class_has_custom_object(int i) {
    if (i < 0 || i >= plugin_class_count) return 0;
    return plugin_classes[i].has_custom_object;
}
int oxphp_bridge_get_class_interface_count(int i) {
    if (i < 0 || i >= plugin_class_count) return 0;
    return plugin_classes[i].interface_count;
}
const char *oxphp_bridge_get_class_interface_fqn(int ci, int ii) {
    if (ci < 0 || ci >= plugin_class_count) return NULL;
    if (ii < 0 || ii >= plugin_classes[ci].interface_count) return NULL;
    return plugin_classes[ci].interface_fqns[ii];
}
int oxphp_bridge_get_class_property_count(int i) {
    if (i < 0 || i >= plugin_class_count) return 0;
    return plugin_classes[i].property_count;
}
const char *oxphp_bridge_get_class_property_name(int ci, int pi) {
    if (ci < 0 || ci >= plugin_class_count) return NULL;
    if (pi < 0 || pi >= plugin_classes[ci].property_count) return NULL;
    return plugin_classes[ci].properties[pi].name;
}
uint32_t oxphp_bridge_get_class_property_visibility(int ci, int pi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (pi < 0 || pi >= plugin_classes[ci].property_count) return 0;
    return plugin_classes[ci].properties[pi].visibility;
}
uint32_t oxphp_bridge_get_class_property_modifiers(int ci, int pi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (pi < 0 || pi >= plugin_classes[ci].property_count) return 0;
    return plugin_classes[ci].properties[pi].modifiers;
}
const char *oxphp_bridge_get_class_property_default(int ci, int pi) {
    if (ci < 0 || ci >= plugin_class_count) return NULL;
    if (pi < 0 || pi >= plugin_classes[ci].property_count) return NULL;
    return plugin_classes[ci].properties[pi].default_value;
}
int oxphp_bridge_get_class_constant_count(int i) {
    if (i < 0 || i >= plugin_class_count) return 0;
    return plugin_classes[i].constant_count;
}
const char *oxphp_bridge_get_class_constant_name(int ci, int ki) {
    if (ci < 0 || ci >= plugin_class_count) return NULL;
    if (ki < 0 || ki >= plugin_classes[ci].constant_count) return NULL;
    return plugin_classes[ci].constants[ki].name;
}
uint32_t oxphp_bridge_get_class_constant_visibility(int ci, int ki) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (ki < 0 || ki >= plugin_classes[ci].constant_count) return 0;
    return plugin_classes[ci].constants[ki].visibility;
}
const char *oxphp_bridge_get_class_constant_value(int ci, int ki) {
    if (ci < 0 || ci >= plugin_class_count) return NULL;
    if (ki < 0 || ki >= plugin_classes[ci].constant_count) return NULL;
    return plugin_classes[ci].constants[ki].value;
}
int oxphp_bridge_get_class_method_count(int i) {
    if (i < 0 || i >= plugin_class_count) return 0;
    return plugin_classes[i].method_count;
}
const char *oxphp_bridge_get_class_method_name(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return NULL;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return NULL;
    return plugin_classes[ci].methods[mi].name;
}
uint32_t oxphp_bridge_get_class_method_visibility(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    return plugin_classes[ci].methods[mi].visibility;
}
uint32_t oxphp_bridge_get_class_method_flags(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    return plugin_classes[ci].methods[mi].flags;
}
int oxphp_bridge_get_class_method_required(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    return plugin_classes[ci].methods[mi].required_params;
}
int oxphp_bridge_get_class_method_total(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    return plugin_classes[ci].methods[mi].total_params;
}
int oxphp_bridge_get_class_method_is_variadic(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    return plugin_classes[ci].methods[mi].is_variadic;
}
int oxphp_bridge_get_class_method_return_type(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    return plugin_classes[ci].methods[mi].return_type;
}
int oxphp_bridge_get_class_method_return_nullable(int ci, int mi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    return plugin_classes[ci].methods[mi].return_nullable;
}
const char *oxphp_bridge_get_class_method_param_name(int ci, int mi, int pi) {
    if (ci < 0 || ci >= plugin_class_count) return NULL;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return NULL;
    oxphp_class_method_t *m = &plugin_classes[ci].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_names) return NULL;
    return m->param_names[pi];
}
int oxphp_bridge_get_class_method_param_type(int ci, int mi, int pi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    oxphp_class_method_t *m = &plugin_classes[ci].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_types) return 0;
    return m->param_types[pi];
}
int oxphp_bridge_get_class_method_param_optional(int ci, int mi, int pi) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mi < 0 || mi >= plugin_classes[ci].method_count) return 0;
    oxphp_class_method_t *m = &plugin_classes[ci].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_optional) return 0;
    return m->param_optional[pi];
}
int oxphp_bridge_get_class_magic(int ci, int mt) {
    if (ci < 0 || ci >= plugin_class_count) return 0;
    if (mt < 0 || mt >= MAGIC_METHOD_COUNT) return 0;
    return plugin_classes[ci].magic_handlers[mt];
}

/* ═══════════════════════════════════════════════════════════
 *  Plugin Interface Registry
 * ═══════════════════════════════════════════════════════════ */

typedef struct {
    char *name;
    uint32_t flags;
    int required_params;
    int total_params;
    int is_variadic;
    int return_type;
    int return_nullable;
    char **param_names;
    int *param_types;
    int *param_optional;
} oxphp_iface_method_t;

typedef struct {
    char *name;
    uint32_t visibility;
    char *value;
} oxphp_iface_constant_t;

typedef struct {
    char *fqn;
    char *parent_fqn;   /* NULL if no parent */

    oxphp_iface_method_t *methods;
    int method_count;
    int method_capacity;

    oxphp_iface_constant_t *constants;
    int constant_count;
    int constant_capacity;
} oxphp_plugin_iface_entry_t;

static oxphp_plugin_iface_entry_t *plugin_interfaces = NULL;
static int plugin_interface_count = 0;
static int plugin_interface_capacity = 0;

int oxphp_bridge_register_interface(const char *fqn, const char *parent_fqn) {
    if (!fqn) return -1;
    if (plugin_interface_count >= plugin_interface_capacity) {
        int new_cap = plugin_interface_capacity == 0 ? 8 : plugin_interface_capacity * 2;
        oxphp_plugin_iface_entry_t *new_arr = realloc(plugin_interfaces, new_cap * sizeof(*plugin_interfaces));
        if (!new_arr) return -1;
        plugin_interfaces = new_arr;
        plugin_interface_capacity = new_cap;
    }
    int idx = plugin_interface_count++;
    oxphp_plugin_iface_entry_t *e = &plugin_interfaces[idx];
    memset(e, 0, sizeof(*e));
    e->fqn = strdup(fqn);
    e->parent_fqn = parent_fqn ? strdup(parent_fqn) : NULL;
    return idx;
}

void oxphp_bridge_interface_add_method(int h, const char *name,
    uint32_t flags, int required_params, int total_params, int is_variadic,
    int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional)
{
    if (h < 0 || h >= plugin_interface_count || !name) return;
    oxphp_plugin_iface_entry_t *e = &plugin_interfaces[h];
    if (e->method_count >= e->method_capacity) {
        int new_cap = e->method_capacity == 0 ? 4 : e->method_capacity * 2;
        oxphp_iface_method_t *new_arr = realloc(e->methods, new_cap * sizeof(*e->methods));
        if (!new_arr) return;
        e->methods = new_arr;
        e->method_capacity = new_cap;
    }
    oxphp_iface_method_t *m = &e->methods[e->method_count++];
    memset(m, 0, sizeof(*m));
    m->name = strdup(name);
    m->flags = flags;
    m->required_params = required_params;
    m->total_params = total_params;
    m->is_variadic = is_variadic;
    m->return_type = return_type;
    m->return_nullable = return_nullable;
    if (total_params > 0) {
        m->param_names = calloc(total_params, sizeof(char *));
        m->param_types = calloc(total_params, sizeof(int));
        m->param_optional = calloc(total_params, sizeof(int));
        if (m->param_names && m->param_types && m->param_optional) {
            for (int i = 0; i < total_params; i++) {
                const char *pn = (param_names && param_names[i]) ? param_names[i] : "";
                m->param_names[i] = strdup(pn);
                m->param_types[i] = param_types ? param_types[i] : 0;
                m->param_optional[i] = param_optional ? param_optional[i] : 0;
            }
        }
    }
}

void oxphp_bridge_interface_add_constant(int h, const char *name,
    uint32_t visibility, const char *value)
{
    if (h < 0 || h >= plugin_interface_count || !name || !value) return;
    oxphp_plugin_iface_entry_t *e = &plugin_interfaces[h];
    if (e->constant_count >= e->constant_capacity) {
        int new_cap = e->constant_capacity == 0 ? 4 : e->constant_capacity * 2;
        oxphp_iface_constant_t *new_arr = realloc(e->constants, new_cap * sizeof(*e->constants));
        if (!new_arr) return;
        e->constants = new_arr;
        e->constant_capacity = new_cap;
    }
    oxphp_iface_constant_t *c = &e->constants[e->constant_count++];
    c->name = strdup(name);
    c->visibility = visibility;
    c->value = strdup(value);
}

int oxphp_bridge_get_plugin_interface_count(void) { return plugin_interface_count; }

const char *oxphp_bridge_get_interface_fqn(int i) {
    if (i < 0 || i >= plugin_interface_count) return NULL;
    return plugin_interfaces[i].fqn;
}
const char *oxphp_bridge_get_interface_parent(int i) {
    if (i < 0 || i >= plugin_interface_count) return NULL;
    return plugin_interfaces[i].parent_fqn;
}
int oxphp_bridge_get_interface_method_count(int i) {
    if (i < 0 || i >= plugin_interface_count) return 0;
    return plugin_interfaces[i].method_count;
}
const char *oxphp_bridge_get_interface_method_name(int ii, int mi) {
    if (ii < 0 || ii >= plugin_interface_count) return NULL;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return NULL;
    return plugin_interfaces[ii].methods[mi].name;
}
uint32_t oxphp_bridge_get_interface_method_flags(int ii, int mi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    return plugin_interfaces[ii].methods[mi].flags;
}
int oxphp_bridge_get_interface_method_required(int ii, int mi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    return plugin_interfaces[ii].methods[mi].required_params;
}
int oxphp_bridge_get_interface_method_total(int ii, int mi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    return plugin_interfaces[ii].methods[mi].total_params;
}
int oxphp_bridge_get_interface_method_is_variadic(int ii, int mi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    return plugin_interfaces[ii].methods[mi].is_variadic;
}
int oxphp_bridge_get_interface_method_return_type(int ii, int mi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    return plugin_interfaces[ii].methods[mi].return_type;
}
int oxphp_bridge_get_interface_method_return_nullable(int ii, int mi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    return plugin_interfaces[ii].methods[mi].return_nullable;
}
const char *oxphp_bridge_get_interface_method_param_name(int ii, int mi, int pi) {
    if (ii < 0 || ii >= plugin_interface_count) return NULL;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return NULL;
    oxphp_iface_method_t *m = &plugin_interfaces[ii].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_names) return NULL;
    return m->param_names[pi];
}
int oxphp_bridge_get_interface_method_param_type(int ii, int mi, int pi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    oxphp_iface_method_t *m = &plugin_interfaces[ii].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_types) return 0;
    return m->param_types[pi];
}
int oxphp_bridge_get_interface_method_param_optional(int ii, int mi, int pi) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (mi < 0 || mi >= plugin_interfaces[ii].method_count) return 0;
    oxphp_iface_method_t *m = &plugin_interfaces[ii].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_optional) return 0;
    return m->param_optional[pi];
}
int oxphp_bridge_get_interface_constant_count(int i) {
    if (i < 0 || i >= plugin_interface_count) return 0;
    return plugin_interfaces[i].constant_count;
}
const char *oxphp_bridge_get_interface_constant_name(int ii, int ki) {
    if (ii < 0 || ii >= plugin_interface_count) return NULL;
    if (ki < 0 || ki >= plugin_interfaces[ii].constant_count) return NULL;
    return plugin_interfaces[ii].constants[ki].name;
}
uint32_t oxphp_bridge_get_interface_constant_visibility(int ii, int ki) {
    if (ii < 0 || ii >= plugin_interface_count) return 0;
    if (ki < 0 || ki >= plugin_interfaces[ii].constant_count) return 0;
    return plugin_interfaces[ii].constants[ki].visibility;
}
const char *oxphp_bridge_get_interface_constant_value(int ii, int ki) {
    if (ii < 0 || ii >= plugin_interface_count) return NULL;
    if (ki < 0 || ki >= plugin_interfaces[ii].constant_count) return NULL;
    return plugin_interfaces[ii].constants[ki].value;
}

/* ═══════════════════════════════════════════════════════════
 *  Plugin Enum Registry
 * ═══════════════════════════════════════════════════════════ */

typedef struct {
    char *name;
    char *value;    /* NULL for unit enums */
} oxphp_enum_case_t;

typedef struct {
    char *name;
    uint32_t flags;
    int required_params;
    int total_params;
    int is_variadic;
    int return_type;
    int return_nullable;
    char **param_names;
    int *param_types;
    int *param_optional;
} oxphp_enum_method_t;

typedef struct {
    char *fqn;
    int backing_type;   /* 0=unit, 4=IS_LONG, 6=IS_STRING */

    char **interface_fqns;
    int interface_count;
    int interface_capacity;

    oxphp_enum_case_t *cases;
    int case_count;
    int case_capacity;

    oxphp_enum_method_t *methods;
    int method_count;
    int method_capacity;
} oxphp_plugin_enum_entry_t;

static oxphp_plugin_enum_entry_t *plugin_enums = NULL;
static int plugin_enum_count = 0;
static int plugin_enum_capacity = 0;

int oxphp_bridge_register_enum(const char *fqn, int backing_type) {
    if (!fqn) return -1;
    if (plugin_enum_count >= plugin_enum_capacity) {
        int new_cap = plugin_enum_capacity == 0 ? 4 : plugin_enum_capacity * 2;
        oxphp_plugin_enum_entry_t *new_arr = realloc(plugin_enums, new_cap * sizeof(*plugin_enums));
        if (!new_arr) return -1;
        plugin_enums = new_arr;
        plugin_enum_capacity = new_cap;
    }
    int idx = plugin_enum_count++;
    oxphp_plugin_enum_entry_t *e = &plugin_enums[idx];
    memset(e, 0, sizeof(*e));
    e->fqn = strdup(fqn);
    e->backing_type = backing_type;
    return idx;
}

void oxphp_bridge_enum_implements(int h, const char *interface_fqn) {
    if (h < 0 || h >= plugin_enum_count || !interface_fqn) return;
    oxphp_plugin_enum_entry_t *e = &plugin_enums[h];
    if (e->interface_count >= e->interface_capacity) {
        int new_cap = e->interface_capacity == 0 ? 4 : e->interface_capacity * 2;
        char **new_arr = realloc(e->interface_fqns, new_cap * sizeof(char*));
        if (!new_arr) return;
        e->interface_fqns = new_arr;
        e->interface_capacity = new_cap;
    }
    e->interface_fqns[e->interface_count++] = strdup(interface_fqn);
}

void oxphp_bridge_enum_add_case(int h, const char *name, const char *value) {
    if (h < 0 || h >= plugin_enum_count || !name) return;
    oxphp_plugin_enum_entry_t *e = &plugin_enums[h];
    if (e->case_count >= e->case_capacity) {
        int new_cap = e->case_capacity == 0 ? 8 : e->case_capacity * 2;
        oxphp_enum_case_t *new_arr = realloc(e->cases, new_cap * sizeof(*e->cases));
        if (!new_arr) return;
        e->cases = new_arr;
        e->case_capacity = new_cap;
    }
    oxphp_enum_case_t *c = &e->cases[e->case_count++];
    c->name = strdup(name);
    c->value = value ? strdup(value) : NULL;
}

void oxphp_bridge_enum_add_method(int h, const char *name,
    uint32_t flags, int required_params, int total_params, int is_variadic,
    int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional)
{
    if (h < 0 || h >= plugin_enum_count || !name) return;
    oxphp_plugin_enum_entry_t *e = &plugin_enums[h];
    if (e->method_count >= e->method_capacity) {
        int new_cap = e->method_capacity == 0 ? 4 : e->method_capacity * 2;
        oxphp_enum_method_t *new_arr = realloc(e->methods, new_cap * sizeof(*e->methods));
        if (!new_arr) return;
        e->methods = new_arr;
        e->method_capacity = new_cap;
    }
    oxphp_enum_method_t *m = &e->methods[e->method_count++];
    memset(m, 0, sizeof(*m));
    m->name = strdup(name);
    m->flags = flags;
    m->required_params = required_params;
    m->total_params = total_params;
    m->is_variadic = is_variadic;
    m->return_type = return_type;
    m->return_nullable = return_nullable;
    if (total_params > 0) {
        m->param_names = calloc(total_params, sizeof(char *));
        m->param_types = calloc(total_params, sizeof(int));
        m->param_optional = calloc(total_params, sizeof(int));
        if (m->param_names && m->param_types && m->param_optional) {
            for (int i = 0; i < total_params; i++) {
                const char *pn = (param_names && param_names[i]) ? param_names[i] : "";
                m->param_names[i] = strdup(pn);
                m->param_types[i] = param_types ? param_types[i] : 0;
                m->param_optional[i] = param_optional ? param_optional[i] : 0;
            }
        }
    }
}

int oxphp_bridge_get_plugin_enum_count(void) { return plugin_enum_count; }

const char *oxphp_bridge_get_enum_fqn(int i) {
    if (i < 0 || i >= plugin_enum_count) return NULL;
    return plugin_enums[i].fqn;
}
int oxphp_bridge_get_enum_backing_type(int i) {
    if (i < 0 || i >= plugin_enum_count) return 0;
    return plugin_enums[i].backing_type;
}
int oxphp_bridge_get_enum_interface_count(int i) {
    if (i < 0 || i >= plugin_enum_count) return 0;
    return plugin_enums[i].interface_count;
}
const char *oxphp_bridge_get_enum_interface_fqn(int ei, int ii) {
    if (ei < 0 || ei >= plugin_enum_count) return NULL;
    if (ii < 0 || ii >= plugin_enums[ei].interface_count) return NULL;
    return plugin_enums[ei].interface_fqns[ii];
}
int oxphp_bridge_get_enum_case_count(int i) {
    if (i < 0 || i >= plugin_enum_count) return 0;
    return plugin_enums[i].case_count;
}
const char *oxphp_bridge_get_enum_case_name(int ei, int ci) {
    if (ei < 0 || ei >= plugin_enum_count) return NULL;
    if (ci < 0 || ci >= plugin_enums[ei].case_count) return NULL;
    return plugin_enums[ei].cases[ci].name;
}
const char *oxphp_bridge_get_enum_case_value(int ei, int ci) {
    if (ei < 0 || ei >= plugin_enum_count) return NULL;
    if (ci < 0 || ci >= plugin_enums[ei].case_count) return NULL;
    return plugin_enums[ei].cases[ci].value;
}
int oxphp_bridge_get_enum_method_count(int i) {
    if (i < 0 || i >= plugin_enum_count) return 0;
    return plugin_enums[i].method_count;
}
const char *oxphp_bridge_get_enum_method_name(int ei, int mi) {
    if (ei < 0 || ei >= plugin_enum_count) return NULL;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return NULL;
    return plugin_enums[ei].methods[mi].name;
}
uint32_t oxphp_bridge_get_enum_method_flags(int ei, int mi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    return plugin_enums[ei].methods[mi].flags;
}
int oxphp_bridge_get_enum_method_required(int ei, int mi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    return plugin_enums[ei].methods[mi].required_params;
}
int oxphp_bridge_get_enum_method_total(int ei, int mi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    return plugin_enums[ei].methods[mi].total_params;
}
int oxphp_bridge_get_enum_method_is_variadic(int ei, int mi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    return plugin_enums[ei].methods[mi].is_variadic;
}
int oxphp_bridge_get_enum_method_return_type(int ei, int mi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    return plugin_enums[ei].methods[mi].return_type;
}
int oxphp_bridge_get_enum_method_return_nullable(int ei, int mi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    return plugin_enums[ei].methods[mi].return_nullable;
}
const char *oxphp_bridge_get_enum_method_param_name(int ei, int mi, int pi) {
    if (ei < 0 || ei >= plugin_enum_count) return NULL;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return NULL;
    oxphp_enum_method_t *m = &plugin_enums[ei].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_names) return NULL;
    return m->param_names[pi];
}
int oxphp_bridge_get_enum_method_param_type(int ei, int mi, int pi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    oxphp_enum_method_t *m = &plugin_enums[ei].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_types) return 0;
    return m->param_types[pi];
}
int oxphp_bridge_get_enum_method_param_optional(int ei, int mi, int pi) {
    if (ei < 0 || ei >= plugin_enum_count) return 0;
    if (mi < 0 || mi >= plugin_enums[ei].method_count) return 0;
    oxphp_enum_method_t *m = &plugin_enums[ei].methods[mi];
    if (pi < 0 || pi >= m->total_params || !m->param_optional) return 0;
    return m->param_optional[pi];
}

/* ═══════════════════════════════════════════════════════════
 *  Plugin Attribute Registry
 * ═══════════════════════════════════════════════════════════ */

typedef struct {
    char *name;
    int type_info;
    int is_required;
    char *default_value;    /* NULL if none */
} oxphp_attr_param_t;

typedef struct {
    char *name;
    int type_info;
    uint32_t visibility;
} oxphp_attr_property_t;

typedef struct {
    char *fqn;
    uint32_t targets;
    int is_repeatable;

    oxphp_attr_param_t *params;
    int param_count;
    int param_capacity;

    oxphp_attr_property_t *properties;
    int property_count;
    int property_capacity;
} oxphp_plugin_attr_entry_t;

static oxphp_plugin_attr_entry_t *plugin_attributes = NULL;
static int plugin_attribute_count = 0;
static int plugin_attribute_capacity = 0;

int oxphp_bridge_register_attribute(const char *fqn, uint32_t targets, int is_repeatable) {
    if (!fqn) return -1;
    if (plugin_attribute_count >= plugin_attribute_capacity) {
        int new_cap = plugin_attribute_capacity == 0 ? 4 : plugin_attribute_capacity * 2;
        oxphp_plugin_attr_entry_t *new_arr = realloc(plugin_attributes, new_cap * sizeof(*plugin_attributes));
        if (!new_arr) return -1;
        plugin_attributes = new_arr;
        plugin_attribute_capacity = new_cap;
    }
    int idx = plugin_attribute_count++;
    oxphp_plugin_attr_entry_t *e = &plugin_attributes[idx];
    memset(e, 0, sizeof(*e));
    e->fqn = strdup(fqn);
    e->targets = targets;
    e->is_repeatable = is_repeatable;
    return idx;
}

void oxphp_bridge_attribute_add_param(int h, const char *name,
    int type_info, int is_required, const char *default_value)
{
    if (h < 0 || h >= plugin_attribute_count || !name) return;
    oxphp_plugin_attr_entry_t *e = &plugin_attributes[h];
    if (e->param_count >= e->param_capacity) {
        int new_cap = e->param_capacity == 0 ? 4 : e->param_capacity * 2;
        oxphp_attr_param_t *new_arr = realloc(e->params, new_cap * sizeof(*e->params));
        if (!new_arr) return;
        e->params = new_arr;
        e->param_capacity = new_cap;
    }
    oxphp_attr_param_t *p = &e->params[e->param_count++];
    p->name = strdup(name);
    p->type_info = type_info;
    p->is_required = is_required;
    p->default_value = default_value ? strdup(default_value) : NULL;
}

void oxphp_bridge_attribute_add_property(int h, const char *name,
    int type_info, uint32_t visibility)
{
    if (h < 0 || h >= plugin_attribute_count || !name) return;
    oxphp_plugin_attr_entry_t *e = &plugin_attributes[h];
    if (e->property_count >= e->property_capacity) {
        int new_cap = e->property_capacity == 0 ? 4 : e->property_capacity * 2;
        oxphp_attr_property_t *new_arr = realloc(e->properties, new_cap * sizeof(*e->properties));
        if (!new_arr) return;
        e->properties = new_arr;
        e->property_capacity = new_cap;
    }
    oxphp_attr_property_t *p = &e->properties[e->property_count++];
    p->name = strdup(name);
    p->type_info = type_info;
    p->visibility = visibility;
}

int oxphp_bridge_get_plugin_attribute_count(void) { return plugin_attribute_count; }

const char *oxphp_bridge_get_attribute_fqn(int i) {
    if (i < 0 || i >= plugin_attribute_count) return NULL;
    return plugin_attributes[i].fqn;
}
uint32_t oxphp_bridge_get_attribute_targets(int i) {
    if (i < 0 || i >= plugin_attribute_count) return 0;
    return plugin_attributes[i].targets;
}
int oxphp_bridge_get_attribute_is_repeatable(int i) {
    if (i < 0 || i >= plugin_attribute_count) return 0;
    return plugin_attributes[i].is_repeatable;
}
int oxphp_bridge_get_attribute_param_count(int i) {
    if (i < 0 || i >= plugin_attribute_count) return 0;
    return plugin_attributes[i].param_count;
}
const char *oxphp_bridge_get_attribute_param_name(int ai, int pi) {
    if (ai < 0 || ai >= plugin_attribute_count) return NULL;
    if (pi < 0 || pi >= plugin_attributes[ai].param_count) return NULL;
    return plugin_attributes[ai].params[pi].name;
}
int oxphp_bridge_get_attribute_param_is_required(int ai, int pi) {
    if (ai < 0 || ai >= plugin_attribute_count) return 0;
    if (pi < 0 || pi >= plugin_attributes[ai].param_count) return 0;
    return plugin_attributes[ai].params[pi].is_required;
}
const char *oxphp_bridge_get_attribute_param_default(int ai, int pi) {
    if (ai < 0 || ai >= plugin_attribute_count) return NULL;
    if (pi < 0 || pi >= plugin_attributes[ai].param_count) return NULL;
    return plugin_attributes[ai].params[pi].default_value;
}
int oxphp_bridge_get_attribute_property_count(int i) {
    if (i < 0 || i >= plugin_attribute_count) return 0;
    return plugin_attributes[i].property_count;
}
const char *oxphp_bridge_get_attribute_property_name(int ai, int pi) {
    if (ai < 0 || ai >= plugin_attribute_count) return NULL;
    if (pi < 0 || pi >= plugin_attributes[ai].property_count) return NULL;
    return plugin_attributes[ai].properties[pi].name;
}
uint32_t oxphp_bridge_get_attribute_property_visibility(int ai, int pi) {
    if (ai < 0 || ai >= plugin_attribute_count) return 0;
    if (pi < 0 || pi >= plugin_attributes[ai].property_count) return 0;
    return plugin_attributes[ai].properties[pi].visibility;
}

/* ═══════════════════════════════════════════════════════════
 *  Plugin Function Registry (new builder-based)
 * ═══════════════════════════════════════════════════════════ */

typedef struct {
    char *fqn;
    int required_params;
    int total_params;
    int is_variadic;
    int return_type;
    int return_nullable;
    char **param_names;
    int *param_types;
    int *param_optional;
} oxphp_plugin_func_entry_t;

static oxphp_plugin_func_entry_t *plugin_builder_functions = NULL;
static int plugin_builder_function_count = 0;
static int plugin_builder_function_capacity = 0;

int oxphp_bridge_register_plugin_function(const char *fqn, int required_params,
    int total_params, int is_variadic, int return_type, int return_nullable,
    const char * const *param_names,
    const int *param_types,
    const int *param_optional)
{
    if (!fqn) return -1;
    if (plugin_builder_function_count >= plugin_builder_function_capacity) {
        int new_cap = plugin_builder_function_capacity == 0 ? 16 : plugin_builder_function_capacity * 2;
        oxphp_plugin_func_entry_t *new_arr = realloc(plugin_builder_functions, new_cap * sizeof(*plugin_builder_functions));
        if (!new_arr) return -1;
        plugin_builder_functions = new_arr;
        plugin_builder_function_capacity = new_cap;
    }
    int idx = plugin_builder_function_count++;
    oxphp_plugin_func_entry_t *e = &plugin_builder_functions[idx];
    memset(e, 0, sizeof(*e));
    e->fqn = strdup(fqn);
    e->required_params = required_params;
    e->total_params = total_params;
    e->is_variadic = is_variadic;
    e->return_type = return_type;
    e->return_nullable = return_nullable;
    if (total_params > 0) {
        e->param_names = calloc(total_params, sizeof(char *));
        e->param_types = calloc(total_params, sizeof(int));
        e->param_optional = calloc(total_params, sizeof(int));
        if (e->param_names && e->param_types && e->param_optional) {
            for (int i = 0; i < total_params; i++) {
                const char *pn = (param_names && param_names[i]) ? param_names[i] : "";
                e->param_names[i] = strdup(pn);
                e->param_types[i] = param_types ? param_types[i] : 0;
                e->param_optional[i] = param_optional ? param_optional[i] : 0;
            }
        }
    }
    return idx;
}

int oxphp_bridge_get_plugin_function_count(void) { return plugin_builder_function_count; }

const char *oxphp_bridge_get_plugin_function_fqn(int i) {
    if (i < 0 || i >= plugin_builder_function_count) return NULL;
    return plugin_builder_functions[i].fqn;
}
int oxphp_bridge_get_plugin_function_required(int i) {
    if (i < 0 || i >= plugin_builder_function_count) return 0;
    return plugin_builder_functions[i].required_params;
}
int oxphp_bridge_get_plugin_function_total(int i) {
    if (i < 0 || i >= plugin_builder_function_count) return 0;
    return plugin_builder_functions[i].total_params;
}
int oxphp_bridge_get_plugin_function_is_variadic(int i) {
    if (i < 0 || i >= plugin_builder_function_count) return 0;
    return plugin_builder_functions[i].is_variadic;
}
int oxphp_bridge_get_plugin_function_return_type(int i) {
    if (i < 0 || i >= plugin_builder_function_count) return 0;
    return plugin_builder_functions[i].return_type;
}
int oxphp_bridge_get_plugin_function_return_nullable(int i) {
    if (i < 0 || i >= plugin_builder_function_count) return 0;
    return plugin_builder_functions[i].return_nullable;
}
const char *oxphp_bridge_get_plugin_function_param_name(int i, int pi) {
    if (i < 0 || i >= plugin_builder_function_count) return NULL;
    oxphp_plugin_func_entry_t *e = &plugin_builder_functions[i];
    if (pi < 0 || pi >= e->total_params || !e->param_names) return NULL;
    return e->param_names[pi];
}
int oxphp_bridge_get_plugin_function_param_type(int i, int pi) {
    if (i < 0 || i >= plugin_builder_function_count) return 0;
    oxphp_plugin_func_entry_t *e = &plugin_builder_functions[i];
    if (pi < 0 || pi >= e->total_params || !e->param_types) return 0;
    return e->param_types[pi];
}
int oxphp_bridge_get_plugin_function_param_optional(int i, int pi) {
    if (i < 0 || i >= plugin_builder_function_count) return 0;
    oxphp_plugin_func_entry_t *e = &plugin_builder_functions[i];
    if (pi < 0 || pi >= e->total_params || !e->param_optional) return 0;
    return e->param_optional[pi];
}

/* ═══════════════════════════════════════════════════════════
 *  Method Dispatch + Storage Callbacks
 * ═══════════════════════════════════════════════════════════ */

static oxphp_method_dispatch_fn_t method_dispatch_fn = NULL;

void oxphp_bridge_set_method_dispatch(oxphp_method_dispatch_fn_t fn) { method_dispatch_fn = fn; }
oxphp_method_dispatch_fn_t oxphp_bridge_get_method_dispatch(void)    { return method_dispatch_fn; }

static oxphp_storage_create_fn_t storage_create_fn = NULL;
static oxphp_storage_drop_fn_t   storage_drop_fn   = NULL;
static oxphp_storage_clone_fn_t  storage_clone_fn  = NULL;

void oxphp_bridge_set_storage_callbacks(
    oxphp_storage_create_fn_t create_fn,
    oxphp_storage_drop_fn_t drop_fn,
    oxphp_storage_clone_fn_t clone_fn)
{
    storage_create_fn = create_fn;
    storage_drop_fn   = drop_fn;
    storage_clone_fn  = clone_fn;
}

oxphp_storage_create_fn_t oxphp_bridge_get_storage_create(void) { return storage_create_fn; }
oxphp_storage_drop_fn_t   oxphp_bridge_get_storage_drop(void)   { return storage_drop_fn; }
oxphp_storage_clone_fn_t  oxphp_bridge_get_storage_clone(void)  { return storage_clone_fn; }

/* ═══════════════════════════════════════════════════════════
 *  Native Bridge API — Zero-Serialization Value Access
 * ═══════════════════════════════════════════════════════════ */

#include "php.h"
#include "SAPI.h"
#include "Zend/zend_API.h"
#include "Zend/zend_hash.h"
#include "Zend/zend_closures.h"
#include "Zend/zend_exceptions.h"
#include "Zend/zend_attributes.h"
#include "Zend/zend_execute.h"
#include "Zend/zend_interfaces.h"
#include "Zend/zend_enum.h"

/* Custom object struct — defined here because it depends on zend_object from php.h.
 * oxphp_bridge.h declares this inside #ifdef PHP_H, but the header is included
 * before php.h, so PHP_H is not defined at that point.  Define it locally with
 * an include-guard so the two definitions don't conflict on compilers that allow
 * duplicate identical typedefs (C11 §6.7p3). */
#ifndef OXPHP_CUSTOM_OBJECT_DEFINED
#define OXPHP_CUSTOM_OBJECT_DEFINED
typedef struct {
    void       *rust_data;
    uint32_t    class_index;
    zend_object std;   /* MUST be last */
} oxphp_custom_object;

#define OXPHP_OBJ(zobj) \
    ((oxphp_custom_object *)((char *)(zobj) - XtOffsetOf(oxphp_custom_object, std)))
#endif /* OXPHP_CUSTOM_OBJECT_DEFINED */

/* Define thread-local TSRM cache for this compilation unit.
 * SG()/CG()/EG() macros expand to use TSRMLS_CACHE (_tsrm_ls_cache).
 * Each .so gets its own copy of this TLS variable.
 * Must call oxphp_bridge_tsrm_update() on each worker thread after ts_resource_ex(). */
#ifdef ZTS
TSRMLS_CACHE_DEFINE()
#endif

/* ── Type mapping (branchless lookup table) ── */

/* PHP IS_* constants → OXPHP_TYPE_* (max IS_* value is ~12, pad to 16). */
static const uint8_t ztype_to_oxphp[16] = {
    [IS_NULL]     = OXPHP_TYPE_NULL,
    [IS_FALSE]    = OXPHP_TYPE_FALSE,
    [IS_TRUE]     = OXPHP_TYPE_TRUE,
    [IS_LONG]     = OXPHP_TYPE_LONG,
    [IS_DOUBLE]   = OXPHP_TYPE_DOUBLE,
    [IS_STRING]   = OXPHP_TYPE_STRING,
    [IS_ARRAY]    = OXPHP_TYPE_ARRAY,
    [IS_OBJECT]   = OXPHP_TYPE_OBJECT,
    [IS_RESOURCE] = OXPHP_TYPE_RESOURCE,
    /* remaining slots default to 0 = OXPHP_TYPE_NULL */
};

static inline uint8_t type_to_oxphp(uint8_t ztype) {
    return (ztype < 16) ? ztype_to_oxphp[ztype] : OXPHP_TYPE_NULL;
}

uint8_t oxphp_val_type(void *zv) {
    return type_to_oxphp(Z_TYPE_P((zval*)zv));
}

uint8_t oxphp_val_arg_type(void *args, uint32_t idx) {
    return type_to_oxphp(Z_TYPE_P(((zval*)args) + idx));
}

/* ── Scalar reading from args[idx] ── */

int64_t oxphp_arg_long(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    return Z_TYPE_P(z) == IS_LONG ? Z_LVAL_P(z) : 0;
}

double oxphp_arg_double(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) == IS_DOUBLE) return Z_DVAL_P(z);
    if (Z_TYPE_P(z) == IS_LONG) return (double)Z_LVAL_P(z);
    return 0.0;
}

int oxphp_arg_bool(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) == IS_TRUE) return 1;
    if (Z_TYPE_P(z) == IS_FALSE) return 0;
    return zend_is_true(z);
}

const uint8_t *oxphp_arg_str(void *args, uint32_t idx, size_t *out_len) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_STRING) {
        *out_len = 0;
        return NULL;
    }
    *out_len = Z_STRLEN_P(z);
    return (const uint8_t*)Z_STRVAL_P(z);
}

int64_t oxphp_arg_enum_long(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_OBJECT) {
        return 0;
    }
    zend_object *obj = Z_OBJ_P(z);
    zend_class_entry *ce = obj->ce;
    if ((ce->ce_flags & ZEND_ACC_ENUM) == 0) {
        return 0;
    }
    if (ce->enum_backing_type != IS_LONG) {
        return 0;
    }
    zval *backing = zend_enum_fetch_case_value(obj);
    if (Z_TYPE_P(backing) != IS_LONG) {
        return 0;
    }
    return Z_LVAL_P(backing);
}

/* ── Array reading ── */

uint32_t oxphp_arg_array_count(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_ARRAY) return 0;
    return zend_hash_num_elements(Z_ARRVAL_P(z));
}

void *oxphp_arg_array(void *args, uint32_t idx) {
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_ARRAY) return NULL;
    return z;
}

/* ── Exception extraction ── */

void oxphp_exception_capture(void *ex_v,
    const char **out_class, size_t *out_class_len,
    char **out_message, size_t *out_message_len,
    char **out_trace, size_t *out_trace_len)
{
    zend_object *ex = (zend_object *)ex_v;
    /* Class name is length-delimited too: an anonymous class name is
     * "<parent>@anonymous\0<file>:<line>$<hash>" with an embedded NUL, so a
     * NUL-terminated read would collapse every anonymous class to the same
     * "<parent>@anonymous". Borrowed interned identifier, stable for the call. */
    *out_class = ZSTR_VAL(ex->ce->name);
    *out_class_len = ZSTR_LEN(ex->ce->name);
    *out_message = NULL;
    *out_message_len = 0;
    *out_trace = NULL;
    *out_trace_len = 0;

    /* message property: a plain read, safe with an exception pending (mirrors
     * the fiber-side capture). Copied into a malloc'd buffer (like the trace
     * below), so both out-strings are caller-owned and stable — the API hands
     * out no borrows into `ex` whose validity would depend on the object
     * outliving the call or $this->message not being reassigned. Length-
     * delimited: a message may hold binary/latin1 bytes or NULs. */
    zval rv;
    zval *msg = zend_read_property(ex->ce, ex, "message", sizeof("message") - 1, 1, &rv);
    if (msg && Z_TYPE_P(msg) == IS_STRING && Z_STRLEN_P(msg) > 0) {
        size_t mlen = Z_STRLEN_P(msg);
        char *mbuf = malloc(mlen);
        if (mbuf) {
            memcpy(mbuf, Z_STRVAL_P(msg), mlen);
            *out_message = mbuf;
            *out_message_len = mlen;
        }
    }

    /* getTraceAsString() dispatches a method; Zend refuses to call with an
     * exception in flight, so always save/clear/restore EG(exception) around it
     * (unconditionally — symmetric across paths, so a live exception is never
     * released instead of restored). The call re-enters the VM and allocates,
     * so it can zend_bailout (longjmp) if it exhausts memory_limit or trips
     * max_execution_time. The zend_try exists only to run our own cleanup on
     * that path — restore EG(exception), free the message copy — and then
     * re-raise the bailout: swallowing it would let the request keep executing
     * past a resource limit the engine meant to be fatal. zend_catch runs with
     * EG(bailout) already restored to the outer target, so zend_bailout() there
     * propagates to request shutdown.
     * NOTE: on a stack of N decorated frames the exception fires this end hook
     * per frame, so the trace is formatted O(N) times on the (cold) error path. */
    zend_object *saved = EG(exception);
    EG(exception) = NULL;

    zval trace_ret;
    ZVAL_UNDEF(&trace_ret);
    zend_try {
        zend_call_method_with_0_params(ex, ex->ce, NULL, "gettraceasstring", &trace_ret);
    } zend_catch {
        free(*out_message);
        *out_message = NULL;
        *out_message_len = 0;
        EG(exception) = saved;
        zend_bailout();
    } zend_end_try();

    if (Z_TYPE(trace_ret) == IS_STRING && Z_STRLEN(trace_ret) > 0) {
        size_t len = Z_STRLEN(trace_ret);
        char *buf = malloc(len);
        if (buf) {
            memcpy(buf, Z_STRVAL(trace_ret), len);
            *out_trace = buf;
            *out_trace_len = len;
        }
    }
    zval_ptr_dtor(&trace_ret); /* safe on UNDEF (bailout path) */

    /* getTraceAsString() must not leave an exception; clear defensively before
     * restoring the caller's original exception (NULL on the non-decorator
     * paths, the in-flight exception on the decorator path). */
    if (EG(exception)) { OBJ_RELEASE(EG(exception)); EG(exception) = NULL; }
    EG(exception) = saved;
}

void oxphp_arg_exception_capture(
    void *args, uint32_t idx, oxphp_exc_capture_cb cb, void *user_data)
{
    zval *z = ((zval*)args) + idx;
    if (Z_TYPE_P(z) != IS_OBJECT) return;
    zend_object *ex = Z_OBJ_P(z);
    if (!instanceof_function(ex->ce, zend_ce_throwable)) return;

    const char *cls = NULL;
    char *msg = NULL, *trace = NULL;
    size_t cls_len = 0, msg_len = 0, trace_len = 0;
    /* The exception was caught in PHP and passed as an argument, so EG(exception)
     * is not in flight here; the capture saves/restores it regardless. */
    oxphp_exception_capture(ex, &cls, &cls_len, &msg, &msg_len, &trace, &trace_len);
    cb(cls, cls_len, msg, msg_len, trace, trace_len, user_data);
    free(msg);   /* free(NULL) is safe */
    free(trace);
}

/* ── Array iteration ── */

void oxphp_array_foreach(void *zv_array, oxphp_array_iter_fn cb, void *user_data) {
    zval *arr = (zval*)zv_array;
    if (Z_TYPE_P(arr) != IS_ARRAY) return;

    HashTable *ht = Z_ARRVAL_P(arr);
    zend_ulong num_idx;
    zend_string *str_key;
    zval *val;

    ZEND_HASH_FOREACH_KEY_VAL(ht, num_idx, str_key, val) {
        if (str_key) {
            cb((const uint8_t*)ZSTR_VAL(str_key), ZSTR_LEN(str_key),
               (int64_t)num_idx, val, user_data);
        } else {
            cb(NULL, 0, (int64_t)num_idx, val, user_data);
        }
    } ZEND_HASH_FOREACH_END();
}

/* ── Direct value reading ── */

int64_t oxphp_val_long(void *zv)  { return Z_TYPE_P((zval*)zv) == IS_LONG ? Z_LVAL_P((zval*)zv) : 0; }
double  oxphp_val_double(void *zv){ return Z_TYPE_P((zval*)zv) == IS_DOUBLE ? Z_DVAL_P((zval*)zv) : 0.0; }
int     oxphp_val_bool(void *zv)  { return zend_is_true((zval*)zv); }

const uint8_t *oxphp_val_str(void *zv, size_t *out_len) {
    if (Z_TYPE_P((zval*)zv) != IS_STRING) { *out_len = 0; return NULL; }
    *out_len = Z_STRLEN_P((zval*)zv);
    return (const uint8_t*)Z_STRVAL_P((zval*)zv);
}

uint32_t oxphp_val_array_count(void *zv) {
    if (Z_TYPE_P((zval*)zv) != IS_ARRAY) return 0;
    return zend_hash_num_elements(Z_ARRVAL_P((zval*)zv));
}

/* ── Value writing ── */

void oxphp_ret_null(void *rv)                   { ZVAL_NULL((zval*)rv); }
void oxphp_ret_bool(void *rv, int val)          { ZVAL_BOOL((zval*)rv, val); }
void oxphp_ret_long(void *rv, int64_t val)      { ZVAL_LONG((zval*)rv, (zend_long)val); }
void oxphp_ret_double(void *rv, double val)     { ZVAL_DOUBLE((zval*)rv, val); }

void oxphp_ret_str(void *rv, const uint8_t *s, size_t len) {
    ZVAL_STRINGL((zval*)rv, (const char*)s, len);
}

void oxphp_ret_array_init(void *rv, uint32_t size_hint) {
    array_init_size((zval*)rv, size_hint);
}

/* ── Array builders (keyed) ── */

void oxphp_arr_add_null(void *arr, const char *key, size_t klen) {
    add_assoc_null_ex((zval*)arr, key, klen);
}
void oxphp_arr_add_bool(void *arr, const char *key, size_t klen, int val) {
    add_assoc_bool_ex((zval*)arr, key, klen, val);
}
void oxphp_arr_add_long(void *arr, const char *key, size_t klen, int64_t val) {
    add_assoc_long_ex((zval*)arr, key, klen, (zend_long)val);
}
void oxphp_arr_add_double(void *arr, const char *key, size_t klen, double val) {
    add_assoc_double_ex((zval*)arr, key, klen, val);
}
void oxphp_arr_add_str(void *arr, const char *key, size_t klen, const uint8_t *s, size_t slen) {
    add_assoc_stringl_ex((zval*)arr, key, klen, (const char*)s, slen);
}

void *oxphp_arr_add_array(void *arr, const char *key, size_t klen, uint32_t size_hint) {
    zval sub;
    array_init_size(&sub, size_hint);
    /* Single hash op: insert and return the stored zval pointer. */
    return zend_hash_str_update(Z_ARRVAL_P((zval*)arr), key, klen, &sub);
}

/* ── Array builders (indexed / push) ── */

void oxphp_arr_push_null(void *arr)                { add_next_index_null((zval*)arr); }
void oxphp_arr_push_bool(void *arr, int val)       { add_next_index_bool((zval*)arr, val); }
void oxphp_arr_push_long(void *arr, int64_t val)   { add_next_index_long((zval*)arr, (zend_long)val); }
void oxphp_arr_push_double(void *arr, double val)  { add_next_index_double((zval*)arr, val); }
void oxphp_arr_push_str(void *arr, const uint8_t *s, size_t len) {
    add_next_index_stringl((zval*)arr, (const char*)s, len);
}

void *oxphp_arr_push_array(void *arr, uint32_t size_hint) {
    zval sub;
    array_init_size(&sub, size_hint);
    /* Single hash op: append and return the stored zval pointer. */
    return zend_hash_next_index_insert(Z_ARRVAL_P((zval*)arr), &sub);
}

/* ── Native dispatch callback ── */

static oxphp_native_dispatch_fn_t native_dispatch_fn = NULL;

void oxphp_bridge_set_native_dispatch(oxphp_native_dispatch_fn_t fn) { native_dispatch_fn = fn; }
oxphp_native_dispatch_fn_t oxphp_bridge_get_native_dispatch(void)    { return native_dispatch_fn; }

/* ── Decorator system — global callbacks (set once before worker threads) ── */
static void *decorator_registry_ptr = NULL;
static oxphp_decorator_resolve_fn_t decorator_resolve_fn = NULL;
static oxphp_decorator_begin_fn_t decorator_begin_fn = NULL;
static oxphp_decorator_end_fn_t decorator_end_fn = NULL;
static oxphp_decorator_register_php_fn_t decorator_register_php_fn = NULL;

void oxphp_bridge_set_decorator_registry(void *ptr) { decorator_registry_ptr = ptr; }
void *oxphp_bridge_get_decorator_registry(void) { return decorator_registry_ptr; }

void oxphp_bridge_set_decorator_resolve(oxphp_decorator_resolve_fn_t fn) { decorator_resolve_fn = fn; }
oxphp_decorator_resolve_fn_t oxphp_bridge_get_decorator_resolve(void) { return decorator_resolve_fn; }

void oxphp_bridge_set_decorator_begin(oxphp_decorator_begin_fn_t fn) { decorator_begin_fn = fn; }
oxphp_decorator_begin_fn_t oxphp_bridge_get_decorator_begin(void) { return decorator_begin_fn; }

void oxphp_bridge_set_decorator_end(oxphp_decorator_end_fn_t fn) { decorator_end_fn = fn; }
oxphp_decorator_end_fn_t oxphp_bridge_get_decorator_end(void) { return decorator_end_fn; }

void oxphp_bridge_set_decorator_register_php(oxphp_decorator_register_php_fn_t fn) { decorator_register_php_fn = fn; }
oxphp_decorator_register_php_fn_t oxphp_bridge_get_decorator_register_php(void) { return decorator_register_php_fn; }

/* ── Reject reason — per-thread TLS ── */
static __thread char decorator_reject_buf[256];
static __thread size_t decorator_reject_len = 0;

void oxphp_bridge_set_decorator_reject_reason(const char *reason, size_t len) {
    if (len > sizeof(decorator_reject_buf) - 1) len = sizeof(decorator_reject_buf) - 1;
    memcpy(decorator_reject_buf, reason, len);
    decorator_reject_buf[len] = '\0';
    decorator_reject_len = len;
}

const char *oxphp_bridge_get_decorator_reject_reason(size_t *out_len) {
    if (out_len) *out_len = decorator_reject_len;
    return decorator_reject_buf;
}

void oxphp_bridge_clear_decorator_reject_reason(void) {
    decorator_reject_len = 0;
    decorator_reject_buf[0] = '\0';
}

/* ── PHP decorator query callbacks ── */
static oxphp_php_dec_count_fn_t php_dec_count_fn = NULL;
static oxphp_php_dec_class_fn_t php_dec_class_fn = NULL;
static oxphp_php_dec_cache_key_fn_t php_dec_cache_key_fn = NULL;

void oxphp_bridge_set_php_decorator_count(oxphp_php_dec_count_fn_t fn) { php_dec_count_fn = fn; }
oxphp_php_dec_count_fn_t oxphp_bridge_get_php_decorator_count(void) { return php_dec_count_fn; }

void oxphp_bridge_set_php_decorator_class(oxphp_php_dec_class_fn_t fn) { php_dec_class_fn = fn; }
oxphp_php_dec_class_fn_t oxphp_bridge_get_php_decorator_class(void) { return php_dec_class_fn; }

void oxphp_bridge_set_php_decorator_cache_key(oxphp_php_dec_cache_key_fn_t fn) { php_dec_cache_key_fn = fn; }
oxphp_php_dec_cache_key_fn_t oxphp_bridge_get_php_decorator_cache_key(void) { return php_dec_cache_key_fn; }

/* TLS buffer for passing class name strings from Rust to C */
static __thread char decorator_class_buf[256];

void oxphp_bridge_set_decorator_class_buf(const char *s, size_t len) {
    if (len > sizeof(decorator_class_buf) - 1) len = sizeof(decorator_class_buf) - 1;
    memcpy(decorator_class_buf, s, len);
    decorator_class_buf[len] = '\0';
}

const char *oxphp_bridge_get_decorator_class_buf(void) {
    return decorator_class_buf;
}

/* ── PHP decorator registration pass-through ── */
void oxphp_bridge_register_php_decorator(const char *class_name, uint32_t targets) {
    if (decorator_register_php_fn) {
        decorator_register_php_fn(class_name, targets);
    }
}

/* ── Decorator context stack — per-thread TLS ── */
static __thread oxphp_decorator_ctx_t decorator_ctx_stack[OXPHP_DECORATOR_CTX_STACK_MAX];
static __thread int decorator_ctx_depth = 0;

oxphp_decorator_ctx_t *oxphp_decorator_ctx_push(void) {
    /* Always advance depth — even past the array bound — so the matching
     * pop() unwinds it and begin/end stay balanced. On overflow we return
     * NULL; the caller throws OxPHP\Decorator\StackOverflowException instead
     * of silently reusing the top slot and corrupting outer frames' context. */
    int idx = decorator_ctx_depth++;
    if (idx >= OXPHP_DECORATOR_CTX_STACK_MAX) {
        return NULL;
    }
    return &decorator_ctx_stack[idx];
}

oxphp_decorator_ctx_t *oxphp_decorator_ctx_peek(void) {
    int idx = decorator_ctx_depth - 1;
    if (idx < 0 || idx >= OXPHP_DECORATOR_CTX_STACK_MAX) return NULL;
    return &decorator_ctx_stack[idx];
}

void oxphp_decorator_ctx_pop(void) {
    if (decorator_ctx_depth > 0) decorator_ctx_depth--;
}

/* ── PHP → Rust native call ── */

int oxphp_call_php_native(const char *func_name, void *args, uint32_t argc, void *result) {
    /* Resolve function entry directly — avoids ZVAL_STRING allocation +
     * zval_ptr_dtor for the function name zval on every call. */
    size_t name_len = strlen(func_name);
    zend_function *fbc = zend_hash_str_find_ptr(CG(function_table), func_name, name_len);
    if (!fbc) {
        ZVAL_NULL((zval*)result);
        return -1;
    }

    ZVAL_NULL((zval*)result);
    zend_call_known_function(fbc, NULL, NULL, (zval*)result, argc, (zval*)args, NULL);
    return 0;
}

/* ── TSRM cache update ── */

void oxphp_bridge_tsrm_update(void) {
#ifdef ZTS
    TSRMLS_CACHE_UPDATE();
#endif
}

/* ── SAPI request_info ── */

void oxphp_bridge_set_request_info(
    const char *method,
    const char *query_string,
    const char *content_type,
    long content_length
) {
    /* Reset response code from the bridge's TSRM context.
     * sapi_activate() (called by php_request_startup) resets this in libphp's
     * TSRM context, but the bridge library has its own _tsrm_ls_cache that
     * may resolve to stale memory. Explicitly resetting here ensures
     * collect_response_code() reads 200 (not a leaked value from the
     * previous request) when called after script execution. */
    SG(sapi_headers).http_response_code = 200;

    /* Set a non-NULL server_context — PHP checks this in sapi_activate()
     * to decide whether to read POST data and cookies. Without it,
     * $_POST/$_FILES/$_COOKIE are never populated. */
    SG(server_context) = (void*)(intptr_t)(method ? 1 : 0);
    SG(request_info).request_method = method;
    SG(request_info).query_string = (char*)query_string;
    SG(request_info).content_type = content_type;
    SG(request_info).content_length = content_length;
}

/* ── CLI one-shot (oxphp run) ── */

void oxphp_bridge_set_cli_args(int argc, char **argv) {
    /* php_build_argv() (called during php_request_startup when
     * register_argc_argv is on) reads these to build $argv/$argc. The
     * caller owns the array and must keep it alive past php_request_startup. */
    SG(request_info).argc = argc;
    SG(request_info).argv = argv;
}

int oxphp_bridge_get_exit_status(void) {
    return EG(exit_status);
}

/* Compile + execute a PHP snippet under zend_try protection. Used to define
 * the CLI STDIN/STDOUT/STDERR constants. Returns 1 on success, 0 on bailout. */
int oxphp_bridge_eval(const char *code) {
    int ok = 0;
    zend_try {
        zend_eval_string(code, NULL, "oxphp-cli-bootstrap");
        ok = 1;
    } zend_catch {
        ok = 0;
    } zend_end_try();
    return ok;
}

/* Skip a leading `#!` shebang line when compiling the next script (php-cli
 * parity, sapi/cli/php_cli.c). Without this the shebang line is emitted as
 * inline text and leaks into the script's output. */
void oxphp_bridge_skip_shebang(void) {
    CG(skip_shebang) = 1;
}

/* ── SAPI response code ── */

int oxphp_bridge_get_response_code(void) {
#ifdef ZTS
    TSRMLS_CACHE_UPDATE();
#endif
    return SG(sapi_headers).http_response_code;
}

/* ── Zval lifecycle ── */

void oxphp_zval_dtor(void *zv) {
    zval_ptr_dtor((zval*)zv);
}

void oxphp_zval_addref(void *zv) {
    Z_TRY_ADDREF_P((zval*)zv);
}

/* ── Object property access ── */

void *oxphp_object_read_property(void *object_zval, const char *property_name) {
    /* Caller contract: property is plain private/protected, no __get / hooks /
     * readonly. zend_std_read_property's rv-write branches are not handled —
     * for those branches Zend writes into the stack-allocated `rv` below and
     * returns its address, which would dangle once we leave this frame. */
    zval *obj = (zval *)object_zval;
    if (!obj || Z_TYPE_P(obj) != IS_OBJECT || !property_name) return NULL;
    size_t len = strlen(property_name);
    zval rv;
    /* zend_read_property returns &EG(uninitialized_zval) when the property is
     * unset; the caller must check via oxphp_zval_is_null_or_unset. */
    zval *result = zend_read_property(
        Z_OBJCE_P(obj), Z_OBJ_P(obj), property_name, len, /* silent */ 1, &rv);
    return (void *)result;
}

int oxphp_zval_is_null_or_unset(const void *zval_ptr) {
    if (!zval_ptr) return 1;
    const zval *z = (const zval *)zval_ptr;
    return (Z_ISUNDEF_P(z) || Z_ISNULL_P(z)) ? 1 : 0;
}

void oxphp_zval_copy_to_retval(const void *src_zval, void *dst_zval) {
    if (!src_zval || !dst_zval) return;
    zval *src = (zval *)src_zval;
    zval *dst = (zval *)dst_zval;
    ZVAL_COPY(dst, src);
}

void *oxphp_closure_addref(void *closure_zv) {
    zval *zv = (zval*)closure_zv;
    if (Z_TYPE_P(zv) == IS_OBJECT) {
        zend_object *obj = Z_OBJ_P(zv);
        GC_ADDREF(obj);
        return obj;
    }
    return NULL;
}

void oxphp_closure_release(void *obj_ptr) {
    if (obj_ptr) {
        OBJ_RELEASE((zend_object*)obj_ptr);
    }
}

size_t oxphp_zval_size(void) {
    return sizeof(zval);
}

size_t oxphp_op_array_size(void) {
    return sizeof(zend_op_array);
}

/* ─── Worker Mode ─────────────────────────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. All workers share the same callback pointers.
 */
/* ─── Superglobals Configuration ─── */
static bool g_superglobals_enabled = true;

void oxphp_bridge_set_superglobals_enabled(bool enabled) {
    g_superglobals_enabled = enabled;
}

bool oxphp_bridge_get_superglobals_enabled(void) {
    return g_superglobals_enabled;
}

/* ─── HTTP Request Data Accessors ─── */
static oxphp_req_str_fn_t     g_req_method_fn = NULL;
static oxphp_req_str_fn_t     g_req_path_fn = NULL;
static oxphp_req_str_fn_t     g_req_full_uri_fn = NULL;
static oxphp_req_str_fn_t     g_req_scheme_fn = NULL;
static oxphp_req_str_fn_t     g_req_host_fn = NULL;
static oxphp_req_u16_fn_t     g_req_port_fn = NULL;
static oxphp_req_str_fn_t     g_req_query_string_fn = NULL;
static oxphp_req_str_key_fn_t g_req_header_fn = NULL;
static oxphp_req_str_key_fn_t g_req_cookie_fn = NULL;
static oxphp_req_str_fn_t     g_req_ip_fn = NULL;
static oxphp_req_str_fn_t     g_req_protocol_version_fn = NULL;
static oxphp_req_double_fn_t  g_req_start_time_fn = NULL;
static oxphp_req_bool_fn_t    g_req_is_secure_fn = NULL;
static oxphp_req_str_fn_t     g_req_content_type_fn = NULL;
static oxphp_req_str_key_fn_t g_req_query_param_fn = NULL;
static oxphp_req_pairs_fn_t   g_req_headers_all_fn = NULL;
static oxphp_req_pairs_fn_t   g_req_cookies_all_fn = NULL;
static oxphp_req_pairs_fn_t   g_req_query_params_all_fn = NULL;
static oxphp_req_body_fn_t    g_req_body_fn = NULL;
static oxphp_req_bool_fn_t    g_req_is_active_fn = NULL;

void oxphp_bridge_set_request_accessors(
    oxphp_req_str_fn_t      method_fn,
    oxphp_req_str_fn_t      path_fn,
    oxphp_req_str_fn_t      full_uri_fn,
    oxphp_req_str_fn_t      scheme_fn,
    oxphp_req_str_fn_t      host_fn,
    oxphp_req_u16_fn_t      port_fn,
    oxphp_req_str_fn_t      query_string_fn,
    oxphp_req_str_key_fn_t  header_fn,
    oxphp_req_str_key_fn_t  cookie_fn,
    oxphp_req_str_fn_t      ip_fn,
    oxphp_req_str_fn_t      protocol_version_fn,
    oxphp_req_double_fn_t   start_time_fn,
    oxphp_req_bool_fn_t     is_secure_fn,
    oxphp_req_str_fn_t      content_type_fn,
    oxphp_req_str_key_fn_t  query_param_fn,
    oxphp_req_pairs_fn_t    headers_all_fn,
    oxphp_req_pairs_fn_t    cookies_all_fn,
    oxphp_req_pairs_fn_t    query_params_all_fn,
    oxphp_req_body_fn_t     body_fn,
    oxphp_req_bool_fn_t     is_active_fn
) {
    g_req_method_fn = method_fn;
    g_req_path_fn = path_fn;
    g_req_full_uri_fn = full_uri_fn;
    g_req_scheme_fn = scheme_fn;
    g_req_host_fn = host_fn;
    g_req_port_fn = port_fn;
    g_req_query_string_fn = query_string_fn;
    g_req_header_fn = header_fn;
    g_req_cookie_fn = cookie_fn;
    g_req_ip_fn = ip_fn;
    g_req_protocol_version_fn = protocol_version_fn;
    g_req_start_time_fn = start_time_fn;
    g_req_is_secure_fn = is_secure_fn;
    g_req_content_type_fn = content_type_fn;
    g_req_query_param_fn = query_param_fn;
    g_req_headers_all_fn = headers_all_fn;
    g_req_cookies_all_fn = cookies_all_fn;
    g_req_query_params_all_fn = query_params_all_fn;
    g_req_body_fn = body_fn;
    g_req_is_active_fn = is_active_fn;
}

/* Convenience wrappers — call through registered function pointers */
const char* oxphp_req_method(size_t *out_len) {
    return g_req_method_fn ? g_req_method_fn(out_len) : NULL;
}
const char* oxphp_req_path(size_t *out_len) {
    return g_req_path_fn ? g_req_path_fn(out_len) : NULL;
}
const char* oxphp_req_full_uri(size_t *out_len) {
    return g_req_full_uri_fn ? g_req_full_uri_fn(out_len) : NULL;
}
const char* oxphp_req_scheme(size_t *out_len) {
    return g_req_scheme_fn ? g_req_scheme_fn(out_len) : NULL;
}
const char* oxphp_req_host(size_t *out_len) {
    return g_req_host_fn ? g_req_host_fn(out_len) : NULL;
}
uint16_t oxphp_req_port(void) {
    return g_req_port_fn ? g_req_port_fn() : 0;
}
const char* oxphp_req_query_string(size_t *out_len) {
    return g_req_query_string_fn ? g_req_query_string_fn(out_len) : NULL;
}
const char* oxphp_req_header(const char *name, size_t name_len, size_t *out_len) {
    return g_req_header_fn ? g_req_header_fn(name, name_len, out_len) : NULL;
}
const char* oxphp_req_cookie(const char *name, size_t name_len, size_t *out_len) {
    return g_req_cookie_fn ? g_req_cookie_fn(name, name_len, out_len) : NULL;
}
const char* oxphp_req_ip(size_t *out_len) {
    return g_req_ip_fn ? g_req_ip_fn(out_len) : NULL;
}
const char* oxphp_req_protocol_version(size_t *out_len) {
    return g_req_protocol_version_fn ? g_req_protocol_version_fn(out_len) : NULL;
}
double oxphp_req_start_time(void) {
    return g_req_start_time_fn ? g_req_start_time_fn() : 0.0;
}
int oxphp_req_is_secure(void) {
    return g_req_is_secure_fn ? g_req_is_secure_fn() : 0;
}
const char* oxphp_req_content_type(size_t *out_len) {
    return g_req_content_type_fn ? g_req_content_type_fn(out_len) : NULL;
}
const char* oxphp_req_query_param(const char *key, size_t key_len, size_t *out_len) {
    return g_req_query_param_fn ? g_req_query_param_fn(key, key_len, out_len) : NULL;
}
void oxphp_req_headers_all(oxphp_req_pairs_cb_t cb, void *user_data) {
    if (g_req_headers_all_fn) g_req_headers_all_fn(cb, user_data);
}
void oxphp_req_cookies_all(oxphp_req_pairs_cb_t cb, void *user_data) {
    if (g_req_cookies_all_fn) g_req_cookies_all_fn(cb, user_data);
}
void oxphp_req_query_params_all(oxphp_req_pairs_cb_t cb, void *user_data) {
    if (g_req_query_params_all_fn) g_req_query_params_all_fn(cb, user_data);
}
const uint8_t* oxphp_req_body(size_t *out_len) {
    return g_req_body_fn ? g_req_body_fn(out_len) : NULL;
}
int oxphp_req_is_active(void) {
    return g_req_is_active_fn ? g_req_is_active_fn() : 0;
}

/* ─── Worker Mode ─── */
static oxphp_worker_wait_fn_t rust_worker_wait = NULL;
static oxphp_worker_send_fn_t rust_worker_send = NULL;

void oxphp_bridge_set_worker_callbacks(oxphp_worker_wait_fn_t wait_fn, oxphp_worker_send_fn_t send_fn) {
    rust_worker_wait = wait_fn;
    rust_worker_send = send_fn;
}

void oxphp_bridge_set_worker_mode(uint64_t max_memory_mib) {
    ctx.worker_mode = 1;
    ctx.max_memory_bytes = max_memory_mib * 1024 * 1024;  /* pre-compute to avoid per-request mul */
    ctx.requests_done = 0;
    ctx.exit_reason = 0;
    ctx.exit_scheduled = false;
    ctx.current_memory_bytes = 0;
}

bool oxphp_bridge_is_worker_mode(void) {
    return ctx.worker_mode != 0;
}

void oxphp_bridge_reset_request_ctx(void) {
    ctx.request_id[0] = '\0';
    ctx.request_time = 0.0;
    ctx.cancel_ptr = NULL;
    ctx.stream_mode = false;
    ctx.headers_sent = false;
    ctx.finished = false;
    /* Decorator context stack: balanced begin/end leaves depth at 0, but
     * push() now advances depth unconditionally (returning NULL past the
     * bound), so a hypothetical begin-without-end imbalance would otherwise
     * accumulate across requests on a long-lived worker thread and could
     * eventually overflow the signed counter into a negative array index.
     * Resetting here — the single per-request choke point shared by both
     * traditional and worker modes — bounds depth to a single request and
     * keeps the stack index structurally in range. */
    decorator_ctx_depth = 0;
    /* Note: requests_done and worker_start_time are deliberately NOT
     * touched here — they are thread-persistent and only mutated by
     * oxphp_bridge_increment_requests_done() / set_worker_start_time(),
     * called from Rust at request start / thread boot respectively. */

    /* Free any unhandled-exception capture not popped by the send path (e.g. a
     * request whose status was forced below 500). */
    oxphp_bridge_clear_unhandled();

    /* Drop any thrown-class snapshot left by this request's throws. The Rust
     * error callback consumes it on a genuine Uncaught E_ERROR, but a request
     * that only threw-and-caught (or threw a chained/hookless exception) leaves a
     * live snapshot behind. Clearing it at this per-request choke point bounds
     * the slot to a single request, so a later request whose Uncaught fatal did
     * NOT fire the throw hook — e.g. an exception thrown without a stack frame,
     * which the engine sends straight to zend_exception_error — cannot borrow a
     * stale class from an earlier request on this thread. */
    oxphp_bridge_clear_thrown_class();
}

int oxphp_bridge_worker_wait(void) {
    /* Callbacks are guaranteed non-NULL in worker mode (set once at startup).
     * Use __builtin_expect to hint the branch predictor. */
    if (__builtin_expect(rust_worker_wait != NULL, 1)) {
        return rust_worker_wait();
    }
    return -1;
}

int oxphp_bridge_worker_send_response(void) {
    if (__builtin_expect(rust_worker_send != NULL, 1)) {
        return rust_worker_send();
    }
    return -1;
}

/* ─── Fiber Scheduler Callbacks ────────────────────────── */

static oxphp_worker_try_recv_fn_t rust_worker_try_recv = NULL;
static oxphp_prepare_request_fn_t rust_prepare_request = NULL;

void oxphp_bridge_set_fiber_callbacks(
    oxphp_worker_try_recv_fn_t try_recv_fn,
    oxphp_prepare_request_fn_t prepare_fn
) {
    rust_worker_try_recv = try_recv_fn;
    rust_prepare_request = prepare_fn;
}

int oxphp_bridge_worker_try_recv(void) {
    if (__builtin_expect(rust_worker_try_recv != NULL, 1)) {
        return rust_worker_try_recv();
    }
    return 1; /* empty, not shutdown — safe fallback if callbacks not registered */
}

int oxphp_bridge_prepare_request(void) {
    if (__builtin_expect(rust_prepare_request != NULL, 1)) {
        return rust_prepare_request();
    }
    return 0;
}

void oxphp_bridge_set_cancel_ptr(_Atomic(uint8_t)* ptr) {
    ctx.cancel_ptr = ptr;
}

_Atomic(uint8_t)* oxphp_bridge_get_cancel_ptr(void) {
    return ctx.cancel_ptr;
}

oxphp_cancel_reason_t oxphp_bridge_get_cancel_reason(void) {
    if (!ctx.cancel_ptr) return OXPHP_CANCEL_NONE;
    uint8_t v = atomic_load_explicit(ctx.cancel_ptr, memory_order_relaxed);
    return (oxphp_cancel_reason_t)v;
}

bool oxphp_bridge_set_cancel_reason(oxphp_cancel_reason_t reason) {
    return oxphp_bridge_set_cancel_reason_at(ctx.cancel_ptr, reason);
}

/* CAS a specific request's cancel cell to `reason` (first-writer-wins, so an
 * in-flight ClientAbort/Timeout is not clobbered). Used by the fiber
 * scheduler's drain sweep to mark each suspended fiber's OWN cell, rather than
 * the single per-thread ctx pointer. */
bool oxphp_bridge_set_cancel_reason_at(_Atomic(uint8_t)* ptr, oxphp_cancel_reason_t reason) {
    if (!ptr || reason == OXPHP_CANCEL_NONE) return false;
    uint8_t expected = OXPHP_CANCEL_NONE;
    return atomic_compare_exchange_strong_explicit(
        ptr, &expected, (uint8_t)reason,
        memory_order_relaxed, memory_order_relaxed);
}

/* Process-global graceful-shutdown drain latch (NOT __thread — every worker
 * and the scheduler must observe it). One-way: set true once, never reset. */
static atomic_bool g_draining = false;

void oxphp_bridge_set_draining(void) {
    atomic_store_explicit(&g_draining, true, memory_order_relaxed);
}

bool oxphp_bridge_is_draining(void) {
    return atomic_load_explicit(&g_draining, memory_order_relaxed);
}

/* Second drain phase, latched when the drain deadline passes. Read by the
 * interrupt handler to self-cancel whatever request is running when the
 * broadcast vm_interrupt kick lands. One-way, like g_draining. */
static atomic_bool g_drain_hard = false;

void oxphp_bridge_set_drain_hard(void) {
    atomic_store_explicit(&g_drain_hard, true, memory_order_release);
}

bool oxphp_bridge_is_drain_hard(void) {
    return atomic_load_explicit(&g_drain_hard, memory_order_acquire);
}

void* oxphp_bridge_vm_interrupt_addr(void) {
    return ctx.vm_interrupt_addr;
}

_Thread_local _Atomic(uint64_t)* g_tick_ptr = NULL;

void oxphp_bridge_set_tick_ptr(_Atomic(uint64_t)* ptr) {
    g_tick_ptr = ptr;
}


void oxphp_bridge_set_vm_interrupt_addr(void* addr) {
    ctx.vm_interrupt_addr = addr;
}

void oxphp_bridge_request_interrupt(void) {
    oxphp_bridge_request_interrupt_at(ctx.vm_interrupt_addr);
}

void oxphp_bridge_request_interrupt_at(void* addr) {
    /* EG(vm_interrupt) is `zend_atomic_bool` (struct wrapping _Atomic(bool)
     * under HAVE_C11_ATOMICS). Aliasing it as plain uint8_t* and writing
     * with __atomic_store_n is C11 strict-aliasing UB — works on current
     * GCC/Clang on x86_64 but not guaranteed. The Zend public API
     * zend_atomic_bool_store_ex is the supported way to mutate it
     * cross-thread (used by pcntl_signal, win32/signal.c). */
    if (addr) {
        zend_atomic_bool_store_ex((zend_atomic_bool*)addr, true);
    }
}

void oxphp_bridge_schedule_exit(void) {
    ctx.exit_scheduled = true;
    ctx.exit_reason = 1;  /* 'scheduled' */
}

bool oxphp_bridge_is_exit_scheduled(void) {
    return ctx.exit_scheduled;
}

uint8_t oxphp_bridge_get_exit_reason(void) {
    return ctx.exit_reason;
}

uint64_t oxphp_bridge_get_requests_done(void) {
    return ctx.requests_done;
}

uint64_t oxphp_bridge_increment_requests_done(void) {
    return ++ctx.requests_done;
}

uint64_t oxphp_bridge_get_memory_usage(void) {
    return ctx.current_memory_bytes;
}

/* Per-thread cached fd for /proc/self/statm. -1 = not opened, -2 = unavailable
 * (open failed once, don't retry). Never closed: lives until thread exits, at
 * which point process exit closes it. */
static __thread int rss_statm_fd = -1;

uint64_t oxphp_bridge_get_rss_bytes(void) {
#if defined(__linux__)
    static long page_size = 0;  /* never changes within a process */
    if (page_size == 0) {
        page_size = sysconf(_SC_PAGESIZE);
        if (page_size <= 0) page_size = 4096;  /* defensive default */
    }

    if (rss_statm_fd == -1) {
        rss_statm_fd = open("/proc/self/statm", O_RDONLY | O_CLOEXEC);
        if (rss_statm_fd < 0) rss_statm_fd = -2;  /* permanently unavailable */
    }

    if (rss_statm_fd >= 0) {
        /* /proc/self/statm is ~30 bytes, 7 space-separated page counts.
         * Field #2 is resident pages. */
        char buf[64];
        if (lseek(rss_statm_fd, 0, SEEK_SET) == 0) {
            ssize_t n = read(rss_statm_fd, buf, sizeof(buf) - 1);
            if (n > 0) {
                buf[n] = '\0';
                const char *p = strchr(buf, ' ');
                if (p) {
                    unsigned long resident_pages = strtoul(p + 1, NULL, 10);
                    if (resident_pages > 0) {
                        return (uint64_t)resident_pages * (uint64_t)page_size;
                    }
                }
            }
        }
        /* read/parse failed — fall through to getrusage */
    }

    /* Fallback when /proc/self/statm is unavailable or unparseable
     * (restrictive seccomp, certain container runtimes). */
    struct rusage ru;
    if (getrusage(RUSAGE_SELF, &ru) != 0) return 0;
    return (uint64_t)ru.ru_maxrss * 1024ULL;  /* Linux: ru_maxrss in KiB */
#elif defined(__APPLE__) && defined(__MACH__)
    struct rusage ru;
    if (getrusage(RUSAGE_SELF, &ru) != 0) return 0;
    /* Darwin: ru_maxrss is bytes. */
    return (uint64_t)ru.ru_maxrss;
#else
    /* Unsupported OS — log once per process, then return 0 on every call.
     * Callers (Worker::rss()) treat 0 as "no info available". */
    static bool warned = false;
    if (!warned) {
        fprintf(stderr,
                "oxphp_bridge_get_rss_bytes: not supported on this OS — "
                "returning 0\n");
        warned = true;
    }
    return 0;
#endif
}

bool oxphp_bridge_get_handler_failed(void) {
    return ctx.handler_failed;
}

uint64_t oxphp_bridge_get_max_memory_bytes(void) {
    return ctx.max_memory_bytes;
}

/* ── Bailout wrapper ── */

void oxphp_bridge_bailout(void) {
    zend_bailout();
}

/* ═══════════════════════════════════════════════════════════
 *  Exception Bridge (PHP-dependent)
 * ═══════════════════════════════════════════════════════════ */

void oxphp_throw_exception(const char *class_fqn, const char *message, int64_t code) {
    zend_class_entry *ce = NULL;
    if (class_fqn && class_fqn[0] != '\0') {
        zend_string *name = zend_string_init(class_fqn, strlen(class_fqn), 0);
        ce = zend_lookup_class(name);
        zend_string_release(name);
    }
    if (!ce) {
        /* zend_ce_runtime_exception does not exist in PHP 8.x — look up by name. */
        zend_string *rt = zend_string_init("RuntimeException", sizeof("RuntimeException") - 1, 0);
        ce = zend_lookup_class(rt);
        zend_string_release(rt);
    }
    if (!ce) {
        ce = zend_ce_exception; /* last resort */
    }
    zend_throw_exception(ce, message, (zend_long)code);
}

int oxphp_exception_pending(void) {
    return EG(exception) != NULL ? 1 : 0;
}

/* Sub-design A: capture &EG(vm_interrupt) for this worker thread.
 * The address is stable for the thread's lifetime once TSRM is
 * initialised. Subsequent calls re-store the same pointer (no-op). */
void oxphp_capture_vm_interrupt(void) {
    oxphp_bridge_set_vm_interrupt_addr((void*)&EG(vm_interrupt));
}

/* Thread-local buffers for exception class name (avoids allocation) */
static __thread char exc_class_buf[256];
static __thread char exc_message_buf[4096];

void oxphp_exception_get(const char **class_out, const char **message_out, int64_t *code_out) {
    if (!EG(exception)) {
        if (class_out) *class_out = NULL;
        if (message_out) *message_out = NULL;
        if (code_out) *code_out = 0;
        return;
    }

    zend_object *exc = EG(exception);

    /* Class name */
    if (class_out) {
        const char *name = ZSTR_VAL(exc->ce->name);
        size_t len = ZSTR_LEN(exc->ce->name);
        if (len >= sizeof(exc_class_buf)) len = sizeof(exc_class_buf) - 1;
        memcpy(exc_class_buf, name, len);
        exc_class_buf[len] = '\0';
        *class_out = exc_class_buf;
    }

    /* Message — read the 'message' property directly */
    if (message_out) {
        zval rv;
        zval *msg_prop = zend_read_property(
            zend_ce_exception, exc, "message", sizeof("message") - 1, 1, &rv);
        if (msg_prop && Z_TYPE_P(msg_prop) == IS_STRING) {
            size_t len = Z_STRLEN_P(msg_prop);
            if (len >= sizeof(exc_message_buf)) len = sizeof(exc_message_buf) - 1;
            memcpy(exc_message_buf, Z_STRVAL_P(msg_prop), len);
            exc_message_buf[len] = '\0';
            *message_out = exc_message_buf;
        } else {
            exc_message_buf[0] = '\0';
            *message_out = exc_message_buf;
        }
    }

    /* Code */
    if (code_out) {
        zval rv;
        zval *code_prop = zend_read_property(
            zend_ce_exception, exc, "code", sizeof("code") - 1, 1, &rv);
        if (code_prop && Z_TYPE_P(code_prop) == IS_LONG) {
            *code_out = (int64_t)Z_LVAL_P(code_prop);
        } else {
            *code_out = 0;
        }
    }
}

void oxphp_exception_clear(void) {
    if (EG(exception)) {
        zend_clear_exception();
    }
}

/* ═══════════════════════════════════════════════════════════
 *  Custom Object Handlers (PHP-dependent)
 *  One set of handlers per class with custom storage.
 * ═══════════════════════════════════════════════════════════ */

/* Per-class handlers array (allocated during MINIT). */
static zend_object_handlers *oxphp_custom_handlers_arr = NULL;
static int oxphp_custom_handler_capacity = 0;

/* Per-class zend_class_entry* array to map class_index -> ce.
 * Populated during MINIT to enable create_object reverse lookup. */
static zend_class_entry **oxphp_plugin_class_ces = NULL;
static int oxphp_plugin_class_ce_count = 0;

void oxphp_plugin_init_custom_objects(int class_count) {
    if (class_count <= 0) return;
    oxphp_custom_handlers_arr = calloc(class_count, sizeof(zend_object_handlers));
    oxphp_custom_handler_capacity = class_count;
    oxphp_plugin_class_ces = calloc(class_count, sizeof(zend_class_entry *));
    oxphp_plugin_class_ce_count = class_count;

    /* Initialize all handler sets from std_object_handlers */
    for (int i = 0; i < class_count; i++) {
        memcpy(&oxphp_custom_handlers_arr[i], &std_object_handlers, sizeof(zend_object_handlers));
        oxphp_custom_handlers_arr[i].offset = XtOffsetOf(oxphp_custom_object, std);
    }
}

void oxphp_plugin_set_class_ce(int index, zend_class_entry *ce) {
    if (index < 0 || index >= oxphp_plugin_class_ce_count) return;
    oxphp_plugin_class_ces[index] = ce;
}

zend_object_handlers *oxphp_plugin_get_handlers(int index) {
    if (index < 0 || index >= oxphp_custom_handler_capacity) return NULL;
    return &oxphp_custom_handlers_arr[index];
}

zend_object *oxphp_plugin_create_object(zend_class_entry *ce) {
    oxphp_custom_object *intern = zend_object_alloc(sizeof(oxphp_custom_object), ce);

    /* Find class_index by matching ce against the registered class_entry array */
    intern->class_index = 0;
    for (int i = 0; i < oxphp_plugin_class_ce_count; i++) {
        if (oxphp_plugin_class_ces[i] == ce) {
            intern->class_index = (uint32_t)i;
            break;
        }
    }

    /* Allocate Rust storage immediately so __construct can use it */
    if (storage_create_fn) {
        intern->rust_data = storage_create_fn(intern->class_index);
    } else {
        intern->rust_data = NULL;
    }

    zend_object_std_init(&intern->std, ce);
    object_properties_init(&intern->std, ce);
    intern->std.handlers = &oxphp_custom_handlers_arr[intern->class_index];
    return &intern->std;
}

void oxphp_plugin_free_object(zend_object *obj) {
    oxphp_custom_object *intern = OXPHP_OBJ(obj);
    if (intern->rust_data && storage_drop_fn) {
        storage_drop_fn(intern->class_index, intern->rust_data);
        intern->rust_data = NULL;
    }
    zend_object_std_dtor(&intern->std);
}

zend_object *oxphp_plugin_clone_object(zend_object *obj) {
    oxphp_custom_object *old = OXPHP_OBJ(obj);
    zend_object *new_obj = oxphp_plugin_create_object(obj->ce);
    oxphp_custom_object *new_intern = OXPHP_OBJ(new_obj);
    zend_objects_clone_members(&new_intern->std, &old->std);
    if (old->rust_data && storage_clone_fn) {
        /* Drop the default-created storage and replace with clone */
        if (new_intern->rust_data && storage_drop_fn) {
            storage_drop_fn(new_intern->class_index, new_intern->rust_data);
        }
        new_intern->rust_data = storage_clone_fn(old->class_index, old->rust_data);
    }
    return new_obj;
}

/* ── SAPI callback wrappers with cooperative cancellation check ── */

/*
 * Global (not __thread) — set once at startup by build_sapi_module() BEFORE
 * any worker threads are spawned, so no data race.  All workers share the
 * same Rust function pointers; per-request state lives in __thread ctx.
 */
static oxphp_ub_write_fn_t rust_ub_write = NULL;
static oxphp_flush_fn_t    rust_flush    = NULL;

void oxphp_bridge_set_sapi_callbacks(oxphp_ub_write_fn_t ub_write, oxphp_flush_fn_t flush) {
    rust_ub_write = ub_write;
    rust_flush    = flush;
}

/**
 * Check cancellation and bailout if needed.
 * Called from C — longjmp stays within C frames, never crosses Rust FFI.
 */
static inline void check_cancelled_c(void) {
    bool cancelled = ctx.cancel_ptr &&
        atomic_load_explicit(ctx.cancel_ptr, memory_order_relaxed) != OXPHP_CANCEL_NONE;
    if (cancelled) {
        zend_bailout();
    }
}

size_t oxphp_bridge_ub_write(const char *str, size_t str_length) {
    check_cancelled_c();
    if (rust_ub_write) {
        return rust_ub_write(str, str_length);
    }
    return 0;
}

void oxphp_bridge_flush(void *server_context) {
    check_cancelled_c();
    if (rust_flush) {
        rust_flush(server_context);
    }
}

/* ── Safe script execution with zend_try ── */

#include "main/php_main.h"  /* php_execute_script() */

/* Takes void* to avoid exposing zend_file_handle in the Rust FFI bindings.
 * Caller is responsible for passing a valid zend_file_handle pointer. */
int oxphp_execute_script_safe(void *file_handle) {
    int result = 0;
    zend_try {
        php_execute_script((zend_file_handle *)file_handle);
        result = 1;  /* success */
    } zend_catch {
        result = 0;  /* bailout occurred */
    } zend_end_try();
    return result;
}

/* ─── Async Dispatch Function Pointers ─────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. Same pattern as worker callbacks.
 */
static oxphp_async_dispatch_fn_t rust_async_dispatch = NULL;
static oxphp_await_dispatch_fn_t rust_await_dispatch = NULL;
static oxphp_await_race_dispatch_fn_t rust_await_race_dispatch = NULL;
static oxphp_await_any_dispatch_fn_t rust_await_any_dispatch = NULL;

void oxphp_bridge_set_async_dispatch(oxphp_async_dispatch_fn_t fn) {
    rust_async_dispatch = fn;
}

void oxphp_bridge_set_await_dispatch(oxphp_await_dispatch_fn_t fn) {
    rust_await_dispatch = fn;
}

void oxphp_bridge_set_await_race_dispatch(oxphp_await_race_dispatch_fn_t fn) {
    rust_await_race_dispatch = fn;
}

void oxphp_bridge_set_await_any_dispatch(oxphp_await_any_dispatch_fn_t fn) {
    rust_await_any_dispatch = fn;
}

static oxphp_fiber_await_fn_t sapi_fiber_await = NULL;

void oxphp_bridge_set_fiber_await(oxphp_fiber_await_fn_t fn) {
    sapi_fiber_await = fn;
}

int oxphp_bridge_fiber_await(int64_t promise_id, double timeout, void *retval) {
    if (sapi_fiber_await != NULL) {
        return sapi_fiber_await(promise_id, timeout, retval);
    }
    return 1; /* not in fiber — caller should do blocking await */
}

static oxphp_fiber_yield_fn_t sapi_fiber_yield = NULL;

void oxphp_bridge_set_fiber_yield(oxphp_fiber_yield_fn_t fn) {
    sapi_fiber_yield = fn;
}

int oxphp_bridge_fiber_yield(void) {
    if (sapi_fiber_yield != NULL) {
        return sapi_fiber_yield();
    }
    return 0; /* not in fiber — caller should fall back to blocking */
}

static oxphp_in_fiber_check_fn_t sapi_in_fiber_check = NULL;

void oxphp_bridge_set_in_fiber_check(oxphp_in_fiber_check_fn_t fn) {
    sapi_in_fiber_check = fn;
}

int oxphp_bridge_in_fiber(void) {
    /* The only authoritative answer lives in the SAPI: it owns the
     * `oxphp_current_fiber` __thread pointer that `oxphp_bridge_fiber_await`
     * keys off. Bridge-only callers (unit tests, bare CLI) get 0. */
    if (sapi_in_fiber_check != NULL) {
        return sapi_in_fiber_check();
    }
    return 0;
}

/* ─── Async-task fiber scheduler callbacks ─────────────────── */

/* Set once at startup (MINIT) before any worker threads spawn — same
 * non-__thread, no-data-race pattern as the other SAPI callbacks. */
static oxphp_async_sched_spawn_fn_t   sched_async_spawn   = NULL;
static oxphp_async_sched_tick_fn_t    sched_async_tick    = NULL;
static oxphp_async_sched_poll_fn_t    sched_async_poll    = NULL;
static oxphp_async_sched_release_fn_t sched_async_release = NULL;
static oxphp_async_sched_cancel_fn_t  sched_async_cancel  = NULL;
static oxphp_async_sched_drain_fn_t   sched_async_drain   = NULL;

void oxphp_bridge_set_async_sched_callbacks(
    oxphp_async_sched_spawn_fn_t spawn,
    oxphp_async_sched_tick_fn_t tick,
    oxphp_async_sched_poll_fn_t poll,
    oxphp_async_sched_release_fn_t release,
    oxphp_async_sched_cancel_fn_t cancel,
    oxphp_async_sched_drain_fn_t drain
) {
    sched_async_spawn   = spawn;
    sched_async_tick    = tick;
    sched_async_poll    = poll;
    sched_async_release = release;
    sched_async_cancel  = cancel;
    sched_async_drain   = drain;
}

int64_t oxphp_bridge_async_spawn(
    void *op_array, void *static_vars, void *this_ptr,
    uint32_t argc, void *args, void *cancel_cell
) {
    if (sched_async_spawn != NULL) {
        return sched_async_spawn(op_array, static_vars, this_ptr, argc, args, cancel_cell);
    }
    return -1; /* no scheduler registered */
}

int oxphp_bridge_async_tick(void) {
    if (sched_async_tick != NULL) {
        return sched_async_tick();
    }
    return -1;
}

int64_t oxphp_bridge_async_poll_completed(
    void **out_retval, const char **out_exc_class, const char **out_exc_message
) {
    if (sched_async_poll != NULL) {
        return sched_async_poll(out_retval, out_exc_class, out_exc_message);
    }
    return -1;
}

void oxphp_bridge_async_release(int64_t fiber_id) {
    if (sched_async_release != NULL) {
        sched_async_release(fiber_id);
    }
}

int oxphp_bridge_async_cancel(int64_t fiber_id) {
    if (sched_async_cancel != NULL) {
        return sched_async_cancel(fiber_id);
    }
    return 0;
}

uint64_t oxphp_bridge_async_drain_output(void) {
    if (sched_async_drain != NULL) {
        return sched_async_drain();
    }
    return 0; /* no scheduler registered */
}

int64_t oxphp_bridge_async_dispatch(
    void *op_array, void *static_vars, void *this_ptr,
    uint32_t argc, void *args, void *closure_zval
) {
    if (__builtin_expect(rust_async_dispatch != NULL, 1)) {
        return rust_async_dispatch(op_array, static_vars, this_ptr, argc, args, closure_zval);
    }
    return -1;
}

int oxphp_bridge_await_dispatch(int64_t promise_id, double timeout, void *retval) {
    if (__builtin_expect(rust_await_dispatch != NULL, 1)) {
        return rust_await_dispatch(promise_id, timeout, retval);
    }
    return -1;
}

int oxphp_bridge_await_race_dispatch(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
) {
    if (__builtin_expect(rust_await_race_dispatch != NULL, 1)) {
        return rust_await_race_dispatch(promise_ids, count, timeout, out_winner_id, retval);
    }
    return -1;
}

int oxphp_bridge_await_any_dispatch(
    const int64_t *promise_ids, uint32_t count, double timeout,
    int64_t *out_winner_id, void *retval
) {
    if (__builtin_expect(rust_await_any_dispatch != NULL, 1)) {
        return rust_await_any_dispatch(promise_ids, count, timeout, out_winner_id, retval);
    }
    return -1;
}

/* ─── Non-Blocking Await Poll ──────────────────────────────── */
static oxphp_await_poll_fn_t rust_await_poll = NULL;

void oxphp_bridge_set_await_poll(oxphp_await_poll_fn_t fn) {
    rust_await_poll = fn;
}

int oxphp_bridge_await_poll(int64_t promise_id) {
    if (__builtin_expect(rust_await_poll != NULL, 1)) {
        return rust_await_poll(promise_id);
    }
    return 0;
}

/* ─── Async Promise Cleanup ─────────────────────────────────── */
static oxphp_cleanup_promises_fn_t rust_cleanup_promises = NULL;

void oxphp_bridge_set_cleanup_promises(oxphp_cleanup_promises_fn_t fn) {
    rust_cleanup_promises = fn;
}

void oxphp_bridge_cleanup_outstanding_promises(void) {
    if (__builtin_expect(rust_cleanup_promises != NULL, 1)) {
        rust_cleanup_promises();
    }
}

static oxphp_cleanup_promises_for_fiber_fn_t rust_cleanup_promises_for_fiber = NULL;

void oxphp_bridge_set_cleanup_promises_for_fiber(oxphp_cleanup_promises_for_fiber_fn_t fn) {
    rust_cleanup_promises_for_fiber = fn;
}

void oxphp_bridge_cleanup_promises_for_fiber(uint64_t fiber_id) {
    if (__builtin_expect(rust_cleanup_promises_for_fiber != NULL, 1)) {
        rust_cleanup_promises_for_fiber(fiber_id);
    }
}

/* ─── Current Request Fiber Identity ────────────────────────── */
static oxphp_current_fiber_id_fn_t ext_current_fiber_id = NULL;

void oxphp_bridge_set_current_fiber_id_fn(oxphp_current_fiber_id_fn_t fn) {
    ext_current_fiber_id = fn;
}

uint64_t oxphp_bridge_current_fiber_id(void) {
    if (ext_current_fiber_id != NULL) {
        return ext_current_fiber_id();
    }
    return 0;
}

/* ─── Deferred Promise Drain (worker mode) ────────────────────── */
static oxphp_poll_deferred_drains_fn_t rust_poll_deferred_drains = NULL;
static oxphp_has_deferred_drains_fn_t  rust_has_deferred_drains  = NULL;

void oxphp_bridge_set_deferred_drain_callbacks(oxphp_poll_deferred_drains_fn_t poll,
                                               oxphp_has_deferred_drains_fn_t pending) {
    rust_poll_deferred_drains = poll;
    rust_has_deferred_drains  = pending;
}

void oxphp_bridge_poll_deferred_drains(void) {
    if (__builtin_expect(rust_poll_deferred_drains != NULL, 1)) {
        rust_poll_deferred_drains();
    }
}

int oxphp_bridge_has_deferred_drains(void) {
    if (__builtin_expect(rust_has_deferred_drains != NULL, 1)) {
        return rust_has_deferred_drains();
    }
    return 0;
}

/* ─── Async Task Scheduler: descriptor-aware idle backoff ────── */
static oxphp_async_io_backoff_fn_t ext_async_io_backoff = NULL;

void oxphp_bridge_set_async_io_backoff_fn(oxphp_async_io_backoff_fn_t fn) {
    ext_async_io_backoff = fn;
}

int oxphp_bridge_async_io_backoff(uint64_t ns) {
    if (ext_async_io_backoff != NULL) {
        return ext_async_io_backoff(ns);
    }
    return 0;
}

/* ─── Async Task Scheduler: nearest deadline it is holding ───── */
static oxphp_async_next_deadline_fn_t ext_async_next_deadline = NULL;

void oxphp_bridge_set_async_next_deadline_fn(oxphp_async_next_deadline_fn_t fn) {
    ext_async_next_deadline = fn;
}

uint64_t oxphp_bridge_async_next_deadline_ns(void) {
    if (ext_async_next_deadline != NULL) {
        return ext_async_next_deadline();
    }
    return 0; /* no extension: nothing to bound the driver's pause with */
}

/* === Async Promise: Freeze/Unfreeze === */

static void oxphp_freeze_zval_recursive(zval *zv);

int oxphp_freeze_zval(zval *zv, uint32_t *out_orig_refcount, uint32_t *out_orig_gc_flags, uint32_t *out_orig_type_flags) {
    /* Unwrap references */
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }

    switch (Z_TYPE_P(zv)) {
        case IS_ARRAY: {
            /* Separate COW-shared arrays before freezing */
            SEPARATE_ARRAY(zv);
            HashTable *ht = Z_ARRVAL_P(zv);
            *out_orig_refcount = GC_REFCOUNT(ht);
            *out_orig_gc_flags = GC_FLAGS(ht);
            *out_orig_type_flags = Z_TYPE_FLAGS_P(zv);

            GC_ADD_FLAGS(ht, IS_ARRAY_IMMUTABLE);
            GC_SET_REFCOUNT(ht, 2); /* GC_IMMUTABLE_REFCOUNT */

            zval *val;
            ZEND_HASH_FOREACH_VAL(ht, val) {
                oxphp_freeze_zval_recursive(val);
            } ZEND_HASH_FOREACH_END();
            return 0;
        }
        case IS_STRING: {
            *out_orig_refcount = 0;
            *out_orig_gc_flags = 0;
            *out_orig_type_flags = Z_TYPE_FLAGS_P(zv);
            /* Clear refcounted flag — engine skips refcount ops */
            Z_TYPE_FLAGS_P(zv) &= ~(IS_TYPE_REFCOUNTED | IS_TYPE_COLLECTABLE);
            return 0;
        }
        case IS_LONG:
        case IS_DOUBLE:
        case IS_TRUE:
        case IS_FALSE:
        case IS_NULL:
            /* Value types — no freeze needed */
            *out_orig_refcount = 0;
            *out_orig_gc_flags = 0;
            *out_orig_type_flags = 0;
            return 0;
        default:
            /* Objects, resources — cannot freeze */
            return -1;
    }
}

/* Recursive freeze for array elements */
static void oxphp_freeze_zval_recursive(zval *zv) {
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }
    if (Z_TYPE_P(zv) == IS_ARRAY) {
        HashTable *ht = Z_ARRVAL_P(zv);
        GC_ADD_FLAGS(ht, IS_ARRAY_IMMUTABLE);
        GC_SET_REFCOUNT(ht, 2);
        zval *val;
        ZEND_HASH_FOREACH_VAL(ht, val) {
            oxphp_freeze_zval_recursive(val);
        } ZEND_HASH_FOREACH_END();
    } else if (Z_TYPE_P(zv) == IS_STRING) {
        Z_TYPE_FLAGS_P(zv) &= ~(IS_TYPE_REFCOUNTED | IS_TYPE_COLLECTABLE);
    }
}

/* === Async Promise: Unfreeze === */

static void oxphp_unfreeze_zval_recursive(zval *zv);

void oxphp_unfreeze_zval(zval *zv, uint32_t orig_refcount, uint32_t orig_gc_flags, uint32_t orig_type_flags) {
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }
    switch (Z_TYPE_P(zv)) {
        case IS_ARRAY: {
            HashTable *ht = Z_ARRVAL_P(zv);
            GC_SET_REFCOUNT(ht, orig_refcount);
            /* Clear all flags then restore originals (GC_FLAGS is not an lvalue in PHP 8.4) */
            GC_DEL_FLAGS(ht, GC_FLAGS(ht));
            GC_ADD_FLAGS(ht, orig_gc_flags);
            Z_TYPE_FLAGS_P(zv) = (uint8_t)orig_type_flags;

            zval *val;
            ZEND_HASH_FOREACH_VAL(ht, val) {
                oxphp_unfreeze_zval_recursive(val);
            } ZEND_HASH_FOREACH_END();
            break;
        }
        case IS_STRING:
            Z_TYPE_FLAGS_P(zv) = (uint8_t)orig_type_flags;
            break;
        default:
            break;
    }
}

static void oxphp_unfreeze_zval_recursive(zval *zv) {
    if (Z_TYPE_P(zv) == IS_REFERENCE) {
        zv = Z_REFVAL_P(zv);
    }
    if (Z_TYPE_P(zv) == IS_ARRAY) {
        HashTable *ht = Z_ARRVAL_P(zv);
        GC_DEL_FLAGS(ht, IS_ARRAY_IMMUTABLE);
        GC_SET_REFCOUNT(ht, 1);
        zval *val;
        ZEND_HASH_FOREACH_VAL(ht, val) {
            oxphp_unfreeze_zval_recursive(val);
        } ZEND_HASH_FOREACH_END();
    } else if (Z_TYPE_P(zv) == IS_STRING) {
        Z_TYPE_FLAGS_P(zv) |= IS_TYPE_REFCOUNTED;
    }
}

/* === Async Promise: Deep Copy === */

void oxphp_deep_copy_zval(zval *dst, const zval *src) {
    switch (Z_TYPE_P(src)) {
        case IS_NULL:
        case IS_TRUE:
        case IS_FALSE:
        case IS_LONG:
        case IS_DOUBLE:
            ZVAL_COPY_VALUE(dst, src);
            break;
        case IS_STRING: {
            size_t len = Z_STRLEN_P(src);
            ZVAL_STRINGL(dst, Z_STRVAL_P(src), len);
            break;
        }
        case IS_ARRAY: {
            uint32_t count = zend_hash_num_elements(Z_ARRVAL_P(src));
            array_init_size(dst, count);
            zend_ulong idx;
            zend_string *key;
            zval *val;
            ZEND_HASH_FOREACH_KEY_VAL(Z_ARRVAL_P(src), idx, key, val) {
                zval copied;
                oxphp_deep_copy_zval(&copied, val);
                if (key) {
                    zend_string *key_copy = zend_string_init(
                        ZSTR_VAL(key), ZSTR_LEN(key), 0
                    );
                    zend_hash_add_new(Z_ARRVAL_P(dst), key_copy, &copied);
                    zend_string_release(key_copy);
                } else {
                    zend_hash_index_add_new(Z_ARRVAL_P(dst), idx, &copied);
                }
            } ZEND_HASH_FOREACH_END();
            break;
        }
        case IS_REFERENCE:
            oxphp_deep_copy_zval(dst, Z_REFVAL_P(src));
            break;
        default:
            /* Objects, resources — cannot deep copy across threads */
            ZVAL_NULL(dst);
            break;
    }
}

void oxphp_deep_free_zval(zval *zv) {
    zval_ptr_dtor(zv);
}

/* === Portable (cross-thread) serialization ===
 *
 * Serializes zvals into a flat byte buffer allocated via system malloc().
 * The buffer can cross ZTS thread boundaries safely.  The receiver calls
 * oxphp_portable_deserialize() which allocates strings/arrays via emalloc
 * on ITS OWN thread's zend_mm_heap — avoiding the cross-heap corruption
 * that oxphp_deep_copy_zval/oxphp_deep_free_zval cause.
 *
 * Wire format per zval:
 *   [1 byte type tag] [payload …]
 *
 * Type tags:
 *   0 = null, 1 = true, 2 = false, 3 = long (8 bytes),
 *   4 = double (8 bytes), 5 = string (4 bytes length + N bytes data),
 *   6 = array (4 bytes count + N×(1 byte key_type + key + value) entries),
 *       key_type: 0 = index (8 bytes ulong), 1 = string key (4 bytes len + data)
 */

/* Growable buffer using system malloc */
typedef struct {
    unsigned char *data;
    size_t len;
    size_t cap;
} portbuf_t;

static void portbuf_init(portbuf_t *b) {
    b->cap = 256;
    b->data = (unsigned char *)malloc(b->cap);
    b->len = 0;
}

static int portbuf_ensure(portbuf_t *b, size_t extra) {
    if (b->len + extra <= b->cap) return 0;
    size_t need = b->len + extra;
    size_t ncap = b->cap * 2;
    while (ncap < need) ncap *= 2;
    unsigned char *p = (unsigned char *)realloc(b->data, ncap);
    if (!p) return -1;
    b->data = p;
    b->cap = ncap;
    return 0;
}

static void portbuf_put(portbuf_t *b, const void *src, size_t n) {
    memcpy(b->data + b->len, src, n);
    b->len += n;
}

static void portbuf_u8(portbuf_t *b, uint8_t v) {
    b->data[b->len++] = v;
}

static void portbuf_u32(portbuf_t *b, uint32_t v) {
    memcpy(b->data + b->len, &v, 4);
    b->len += 4;
}

static void portbuf_u64(portbuf_t *b, uint64_t v) {
    memcpy(b->data + b->len, &v, 8);
    b->len += 8;
}

/* Forward declaration for recursive serialization */
static int portbuf_ser_zval(portbuf_t *b, const zval *zv);

/* Forward declarations for Shared\* wrapper helpers (tag 7).
 * Full definitions live near the bottom of this file alongside
 * oxphp_is_shareable. The Rust-side FFI exports below provide the
 * pointer-based handle lifecycle used by the serializer and by
 * cross-thread (de)serialization.
 *
 * Weak linkage: the bridge library is also dlopen()'d by a bare `php`
 * CLI invocation (via /usr/local/etc/php/conf.d/extension.ini), with
 * no oxphp Rust binary in the process to provide these symbols. musl
 * does eager relocation on dlopen, so non-weak references would abort
 * the load. With weak linkage the unresolved refs become NULL; the
 * call sites below are guarded by NULL checks or by semantic
 * invariants (cross-thread serialization only runs from oxphp's Rust
 * workers, never from CLI). */
extern const void *oxphp_shared_handle_clone(const void *entry_ptr) __attribute__((weak));
extern void        oxphp_shared_handle_drop(const void *entry_ptr) __attribute__((weak));
extern uint64_t    oxphp_shared_entry_id(const void *entry_ptr) __attribute__((weak));
extern const void *oxphp_shared_handle_from_id(uint64_t id) __attribute__((weak));
int oxphp_plugin_get_shared_handle(zval *obj,
                                   uint8_t *out_type_tag,
                                   const void **out_entry_ptr);
int oxphp_shared_wrapper_new(zval *out,
                             uint8_t type_tag,
                             const void *entry_ptr);

static int portbuf_ser_zval(portbuf_t *b, const zval *zv) {
    if (portbuf_ensure(b, 16) != 0) return -1;

    switch (Z_TYPE_P(zv)) {
        case IS_NULL:
        case IS_UNDEF:
            portbuf_u8(b, 0);
            break;
        case IS_TRUE:
            portbuf_u8(b, 1);
            break;
        case IS_FALSE:
            portbuf_u8(b, 2);
            break;
        case IS_LONG: {
            portbuf_u8(b, 3);
            int64_t v = (int64_t)Z_LVAL_P(zv);
            if (portbuf_ensure(b, 8) != 0) return -1;
            memcpy(b->data + b->len, &v, 8);
            b->len += 8;
            break;
        }
        case IS_DOUBLE: {
            portbuf_u8(b, 4);
            double v = Z_DVAL_P(zv);
            if (portbuf_ensure(b, 8) != 0) return -1;
            memcpy(b->data + b->len, &v, 8);
            b->len += 8;
            break;
        }
        case IS_STRING: {
            size_t slen = Z_STRLEN_P(zv);
            uint32_t slen32 = (uint32_t)slen;
            if (portbuf_ensure(b, 1 + 4 + slen) != 0) return -1;
            portbuf_u8(b, 5);
            portbuf_u32(b, slen32);
            portbuf_put(b, Z_STRVAL_P(zv), slen);
            break;
        }
        case IS_ARRAY: {
            HashTable *ht = Z_ARRVAL_P(zv);
            uint32_t count = zend_hash_num_elements(ht);
            if (portbuf_ensure(b, 1 + 4) != 0) return -1;
            portbuf_u8(b, 6);
            portbuf_u32(b, count);

            zend_ulong idx;
            zend_string *key;
            zval *val;
            ZEND_HASH_FOREACH_KEY_VAL(ht, idx, key, val) {
                if (key) {
                    size_t klen = ZSTR_LEN(key);
                    if (portbuf_ensure(b, 1 + 4 + klen) != 0) return -1;
                    portbuf_u8(b, 1); /* string key */
                    portbuf_u32(b, (uint32_t)klen);
                    portbuf_put(b, ZSTR_VAL(key), klen);
                } else {
                    if (portbuf_ensure(b, 1 + 8) != 0) return -1;
                    portbuf_u8(b, 0); /* index key */
                    portbuf_u64(b, (uint64_t)idx);
                }
                if (portbuf_ser_zval(b, val) != 0) return -1;
            } ZEND_HASH_FOREACH_END();
            break;
        }
        case IS_REFERENCE:
            return portbuf_ser_zval(b, Z_REFVAL_P(zv));
        case IS_OBJECT: {
            /* Only OxPHP\Shared\* wrappers cross the wire. Closures,
             * stdClass, user objects, etc. are non-Shareable — refuse
             * the value so the caller can surface a precise diagnostic
             * (BAD_RETURN / TypeException "non-serialisable value
             * (closure / resource / non-Shareable object)") instead of
             * silently coercing it to null and losing data. */
            if (!oxphp_is_shareable((void *)zv)) {
                return -1;
            }
            uint8_t     type_tag;
            const void *entry_ptr;
            if (oxphp_plugin_get_shared_handle((zval *)zv, &type_tag, &entry_ptr) != 0) {
                /* Shared\* wrapper exists but its handle is unbound
                 * (constructor not finished) — cannot encode an id. */
                return -1;
            }
            /* Need the wire id. If the Rust binary is absent (bare CLI
             * dlopen), entry_id is unresolved — can't encode. */
            if (oxphp_shared_entry_id == NULL) {
                return -1;
            }
            uint64_t shared_id = oxphp_shared_entry_id(entry_ptr);
            if (portbuf_ensure(b, 1 + 1 + 8) != 0) return -1;
            portbuf_u8(b, 7);
            portbuf_u8(b, type_tag);
            /* shared_id: u64 host-endian — intra-host transient buffer. */
            memcpy(b->data + b->len, &shared_id, 8);
            b->len += 8;
            /* No sender-side retain: the source PHP object is alive for
             * the whole serialization, so its Arc keeps the Weak in the
             * registry upgradable. The receiver gets its own strong ref
             * via oxphp_shared_handle_from_id. */
            break;
        }
        default:
            /* IS_RESOURCE and any other non-encodable type — refuse so
             * BAD_RETURN points at the value, not at the callable. */
            return -1;
    }
    return 0;
}

int oxphp_portable_serialize(const zval *args, uint32_t argc,
                             unsigned char **out_buf, size_t *out_len) {
    portbuf_t b;
    portbuf_init(&b);
    if (!b.data) return -1;

    for (uint32_t i = 0; i < argc; i++) {
        if (portbuf_ser_zval(&b, &args[i]) != 0) {
            free(b.data);
            return -1;
        }
    }
    *out_buf = b.data;
    *out_len = b.len;
    return 0;
}

/* Reader state for deserialization */
typedef struct {
    const unsigned char *data;
    size_t len;
    size_t pos;
} portrd_t;

static int portrd_u8(portrd_t *r, uint8_t *out) {
    if (r->pos >= r->len) return -1;
    *out = r->data[r->pos++];
    return 0;
}

static int portrd_u32(portrd_t *r, uint32_t *out) {
    if (r->pos + 4 > r->len) return -1;
    memcpy(out, r->data + r->pos, 4);
    r->pos += 4;
    return 0;
}

static int portrd_u64(portrd_t *r, uint64_t *out) {
    if (r->pos + 8 > r->len) return -1;
    memcpy(out, r->data + r->pos, 8);
    r->pos += 8;
    return 0;
}

static int portrd_bytes(portrd_t *r, size_t n, const unsigned char **out) {
    if (r->pos + n > r->len) return -1;
    *out = r->data + r->pos;
    r->pos += n;
    return 0;
}

/* Forward declaration for recursive deserialization */
static int portrd_deser_zval(portrd_t *r, zval *out);

static int portrd_deser_zval(portrd_t *r, zval *out) {
    uint8_t tag;
    if (portrd_u8(r, &tag) != 0) return -1;

    switch (tag) {
        case 0: /* null */
            ZVAL_NULL(out);
            break;
        case 1: /* true */
            ZVAL_TRUE(out);
            break;
        case 2: /* false */
            ZVAL_FALSE(out);
            break;
        case 3: { /* long */
            int64_t v;
            if (r->pos + 8 > r->len) return -1;
            memcpy(&v, r->data + r->pos, 8);
            r->pos += 8;
            ZVAL_LONG(out, (zend_long)v);
            break;
        }
        case 4: { /* double */
            double v;
            if (r->pos + 8 > r->len) return -1;
            memcpy(&v, r->data + r->pos, 8);
            r->pos += 8;
            ZVAL_DOUBLE(out, v);
            break;
        }
        case 5: { /* string */
            uint32_t slen;
            if (portrd_u32(r, &slen) != 0) return -1;
            const unsigned char *sdata;
            if (portrd_bytes(r, slen, &sdata) != 0) return -1;
            /* ZVAL_STRINGL uses emalloc on the CURRENT thread's heap — correct! */
            ZVAL_STRINGL(out, (const char *)sdata, slen);
            break;
        }
        case 6: { /* array */
            uint32_t count;
            if (portrd_u32(r, &count) != 0) return -1;
            /* array_init_size uses emalloc on the CURRENT thread's heap — correct! */
            array_init_size(out, count);
            for (uint32_t i = 0; i < count; i++) {
                uint8_t key_type;
                if (portrd_u8(r, &key_type) != 0) return -1;

                zval elem;
                ZVAL_UNDEF(&elem);

                if (key_type == 1) {
                    /* string key */
                    uint32_t klen;
                    if (portrd_u32(r, &klen) != 0) return -1;
                    const unsigned char *kdata;
                    if (portrd_bytes(r, klen, &kdata) != 0) return -1;
                    if (portrd_deser_zval(r, &elem) != 0) {
                        zval_ptr_dtor(&elem);
                        return -1;
                    }
                    zend_string *zkey = zend_string_init(
                        (const char *)kdata, klen, 0
                    );
                    zend_hash_add_new(Z_ARRVAL_P(out), zkey, &elem);
                    zend_string_release(zkey);
                } else {
                    /* index key */
                    uint64_t idx;
                    if (portrd_u64(r, &idx) != 0) return -1;
                    if (portrd_deser_zval(r, &elem) != 0) {
                        zval_ptr_dtor(&elem);
                        return -1;
                    }
                    zend_hash_index_add_new(Z_ARRVAL_P(out), (zend_ulong)idx, &elem);
                }
            }
            break;
        }
        case 7: { /* Shared\* wrapper */
            uint8_t  type_tag;
            uint64_t shared_id;
            if (portrd_u8(r, &type_tag) != 0) return -1;
            if (portrd_u64(r, &shared_id) != 0) return -1;
            /* Resolve the wire id to a fresh strong ref. NULL means the
             * entry died in the transfer window — a correct StaleHandle,
             * deserialize as null. Also NULL if the Rust binary is
             * absent (weak-linked symbol unresolved). */
            const void *entry_ptr = NULL;
            if (oxphp_shared_handle_from_id != NULL) {
                entry_ptr = oxphp_shared_handle_from_id(shared_id);
            }
            if (entry_ptr == NULL) {
                ZVAL_NULL(out);
                break;
            }
            if (oxphp_shared_wrapper_new(out, type_tag, entry_ptr) != 0) {
                /* Could not build the wrapper — drop the strong ref
                 * handle_from_id handed us, leave null. */
                if (oxphp_shared_handle_drop != NULL) {
                    oxphp_shared_handle_drop(entry_ptr);
                }
                ZVAL_NULL(out);
                break;
            }
            /* On success, oxphp_shared_wrapper_new moved `entry_ptr`
             * into the new wrapper's handle storage — do NOT drop it
             * here. */
            break;
        }
        default:
            ZVAL_NULL(out);
            break;
    }
    return 0;
}

int oxphp_portable_deserialize(const unsigned char *buf, size_t len,
                               uint32_t argc, zval *out) {
    portrd_t r = { buf, len, 0 };
    for (uint32_t i = 0; i < argc; i++) {
        if (portrd_deser_zval(&r, &out[i]) != 0) {
            /* Cleanup already-deserialized zvals on error */
            for (uint32_t j = 0; j < i; j++) {
                zval_ptr_dtor(&out[j]);
            }
            return -1;
        }
    }
    return 0;
}

int oxphp_portable_serialize_ht(HashTable *ht,
                                unsigned char **out_buf, size_t *out_len) {
    /* Wrap the HashTable in a temporary IS_ARRAY zval and serialize as 1 zval */
    zval tmp;
    ZVAL_ARR(&tmp, ht);
    return oxphp_portable_serialize(&tmp, 1, out_buf, out_len);
}

int oxphp_portable_deserialize_ht(const unsigned char *buf, size_t len,
                                  HashTable **out_ht) {
    /* Deserialize as 1 zval, then extract the HashTable */
    zval tmp;
    ZVAL_UNDEF(&tmp);
    if (oxphp_portable_deserialize(buf, len, 1, &tmp) != 0) {
        return -1;
    }
    if (Z_TYPE(tmp) != IS_ARRAY) {
        zval_ptr_dtor(&tmp);
        return -1;
    }
    /* Separate the HashTable from the zval — caller owns it.
     * Increment refcount so the zval_ptr_dtor below doesn't free it. */
    *out_ht = Z_ARRVAL(tmp);
    GC_ADDREF(*out_ht);
    zval_ptr_dtor(&tmp);
    return 0;
}

void oxphp_portable_free(unsigned char *buf) {
    free(buf);
}

void oxphp_portable_free_ht(HashTable *ht) {
    if (ht) {
        zend_array_destroy(ht);
    }
}

/* Iterate a PHP array and portbuf-serialize each element independently.
 * Produces:
 *   (a) one libc::malloc'd buffer holding every per-element portbuf
 *       concatenated (NULL when the total length is zero, e.g. empty
 *       array or all-null elements);
 *   (b) a libc::malloc'd [size_t; n+1] offsets array whose i-th entry is
 *       the byte offset of payload i inside the concat buffer, with
 *       offsets[n] == total length.
 *
 * On any failure partial allocations are freed and all out-params are
 * zeroed, so the caller can early-return on non-zero.
 *
 * String keys are ignored — this is intentionally `array_values()`-style
 * for batch channel send. */
int oxphp_iter_array_to_portbufs(const zval *arr,
                                  unsigned char **out_concat,
                                  size_t *out_concat_len,
                                  size_t **out_offsets,
                                  size_t *out_n) {
    if (!out_concat || !out_concat_len || !out_offsets || !out_n) return -1;
    *out_concat = NULL;
    *out_concat_len = 0;
    *out_offsets = NULL;
    *out_n = 0;
    if (!arr || Z_TYPE_P(arr) != IS_ARRAY) return -3;

    HashTable *ht = Z_ARRVAL_P(arr);
    uint32_t count = zend_hash_num_elements(ht);
    if (count == 0) return 0;

    /* Phase 1: serialize each element to a transient per-element buffer.
     * We keep every per-element buffer via its (ptr, len) tuple, then
     * concatenate in a single malloc at the end. */
    unsigned char **bufs = (unsigned char **)calloc(count, sizeof(unsigned char *));
    size_t *lens = (size_t *)calloc(count, sizeof(size_t));
    if (!bufs || !lens) {
        free(bufs);
        free(lens);
        return -1;
    }

    uint32_t i = 0;
    zval *val;
    int err = 0;
    ZEND_HASH_FOREACH_VAL(ht, val) {
        if (i >= count) break;
        /* Dereference IS_REFERENCE wrappers so the serializer sees the
         * underlying scalar/array/object. */
        zval *z = val;
        if (Z_TYPE_P(z) == IS_REFERENCE) {
            z = Z_REFVAL_P(z);
        }
        if (oxphp_portable_serialize(z, 1, &bufs[i], &lens[i]) != 0) {
            err = -1;
            break;
        }
        i++;
    } ZEND_HASH_FOREACH_END();

    if (err != 0) {
        for (uint32_t j = 0; j < i; j++) {
            if (bufs[j]) free(bufs[j]);
        }
        free(bufs);
        free(lens);
        return -1;
    }

    /* Adjust count if iteration stopped short for any reason. */
    count = i;

    /* Phase 2: compute total length, allocate final buffers. */
    size_t total = 0;
    for (uint32_t j = 0; j < count; j++) {
        total += lens[j];
    }

    unsigned char *concat = NULL;
    if (total > 0) {
        concat = (unsigned char *)malloc(total);
        if (!concat) {
            for (uint32_t j = 0; j < count; j++) {
                if (bufs[j]) free(bufs[j]);
            }
            free(bufs);
            free(lens);
            return -1;
        }
    }

    size_t *offsets = (size_t *)malloc((count + 1) * sizeof(size_t));
    if (!offsets) {
        if (concat) free(concat);
        for (uint32_t j = 0; j < count; j++) {
            if (bufs[j]) free(bufs[j]);
        }
        free(bufs);
        free(lens);
        return -1;
    }

    size_t cursor = 0;
    offsets[0] = 0;
    for (uint32_t j = 0; j < count; j++) {
        if (lens[j] > 0 && concat && bufs[j]) {
            memcpy(concat + cursor, bufs[j], lens[j]);
        }
        cursor += lens[j];
        offsets[j + 1] = cursor;
        if (bufs[j]) free(bufs[j]);
    }
    free(bufs);
    free(lens);

    *out_concat = concat;
    *out_concat_len = total;
    *out_offsets = offsets;
    *out_n = count;
    return 0;
}

/* Deserialize a portbuf slice and append the resulting zval to `arr` via
 * zend_hash_next_index_insert. `arr` must already be IS_ARRAY. */
int oxphp_arr_push_portbuf(zval *arr, const unsigned char *buf, size_t len) {
    if (!arr || Z_TYPE_P(arr) != IS_ARRAY) return -1;
    zval tmp;
    ZVAL_UNDEF(&tmp);
    if (oxphp_portable_deserialize(buf, len, 1, &tmp) != 0) {
        zval_ptr_dtor(&tmp);
        return -1;
    }
    /* add_next_index_zval transfers ownership — no extra addref needed. */
    zend_hash_next_index_insert(Z_ARRVAL_P(arr), &tmp);
    return 0;
}

/* === Async Promise: Closure Inspection === */

/* PHP 8.4: zend_closure struct is opaque — use public API only */

void *oxphp_closure_get_op_array(zval *closure) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        return NULL;
    }
    const zend_function *func = zend_get_closure_method_def(Z_OBJ_P(closure));
    if (!func || func->type != ZEND_USER_FUNCTION) {
        return NULL; /* Internal function — cannot transfer */
    }
    return (void *)&func->op_array;
}

int oxphp_closure_get_static_vars(zval *closure, HashTable **out_ht) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        *out_ht = NULL;
        return -1;
    }
    const zend_function *func = zend_get_closure_method_def(Z_OBJ_P(closure));
    if (!func || func->type != ZEND_USER_FUNCTION) {
        *out_ht = NULL;
        return -1;
    }
    /* The bound use-var / static-var values live in the per-closure HT
     * reachable via the static_variables MAP_PTR slot: zend_create_closure()
     * dups the template into that slot and ZEND_BIND_LEXICAL writes each
     * captured value there (see zend_closure_bind_var_ex). The direct
     * `static_variables` field is only the compile-time template whose
     * use-var entries are IS_UNDEF placeholders — reading it loses every
     * captured value. Read the slot first; fall back to the template only
     * when the slot is unset (closures with no captures at all). */
    HashTable *bound = ZEND_MAP_PTR_GET(func->op_array.static_variables_ptr);
    *out_ht = bound ? bound : func->op_array.static_variables;
    return 0;
}

int oxphp_closure_has_this(zval *closure) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        return 0;
    }
    zval *this_ptr = zend_get_closure_this_ptr(closure);
    return (this_ptr && Z_TYPE_P(this_ptr) != IS_UNDEF) ? 1 : 0;
}

zval *oxphp_closure_get_this(zval *closure) {
    if (Z_TYPE_P(closure) != IS_OBJECT || !instanceof_function(Z_OBJCE_P(closure), zend_ce_closure)) {
        return NULL;
    }
    zval *this_ptr = zend_get_closure_this_ptr(closure);
    return (this_ptr && Z_TYPE_P(this_ptr) != IS_UNDEF) ? this_ptr : NULL;
}

/* ─── Async Exception Details ────────────────────────────── */
static __thread char *async_exc_class = NULL;
static __thread char *async_exc_msg = NULL;

void oxphp_bridge_set_async_exception(const char *cls, const char *msg) {
    free(async_exc_class);
    free(async_exc_msg);
    async_exc_class = cls ? strdup(cls) : NULL;
    async_exc_msg = msg ? strdup(msg) : NULL;
}

const char *oxphp_bridge_get_async_exc_class(void) { return async_exc_class; }
const char *oxphp_bridge_get_async_exc_message(void) { return async_exc_msg; }

void oxphp_bridge_clear_async_exception(void) {
    free(async_exc_class);
    free(async_exc_msg);
    async_exc_class = NULL;
    async_exc_msg = NULL;
}

/* ─── Async Aggregate Exception (multi-error) ──────────────── */

typedef struct aggregate_entry_s {
    char *exception_class;   /* strdup'd, free in clear() */
    char *message;           /* strdup'd, free in clear() */
    int64_t promise_id;
} aggregate_entry_t;

static __thread aggregate_entry_t *aggregate_buf = NULL;
static __thread size_t aggregate_len = 0;
static __thread size_t aggregate_cap = 0;

void oxphp_bridge_aggregate_clear(void) {
    for (size_t i = 0; i < aggregate_len; i++) {
        free(aggregate_buf[i].exception_class);
        free(aggregate_buf[i].message);
    }
    aggregate_len = 0;
    /* Keep allocation across requests; reset length only. */
}

void oxphp_bridge_aggregate_push(
    const char *exception_class,
    const char *message,
    int64_t promise_id
) {
    if (aggregate_len == aggregate_cap) {
        size_t new_cap = aggregate_cap ? aggregate_cap * 2 : 8;
        aggregate_entry_t *grown = realloc(aggregate_buf, new_cap * sizeof(*grown));
        if (!grown) {
            php_error_docref(NULL, E_WARNING,
                "oxphp_bridge_aggregate_push: realloc(%zu bytes) failed; "
                "dropping aggregate entry for promise_id=%" PRId64,
                new_cap * sizeof(*grown), promise_id);
            return;
        }
        aggregate_buf = grown;
        aggregate_cap = new_cap;
    }
    aggregate_buf[aggregate_len].exception_class =
        exception_class ? strdup(exception_class) : NULL;
    aggregate_buf[aggregate_len].message =
        message ? strdup(message) : NULL;
    aggregate_buf[aggregate_len].promise_id = promise_id;
    aggregate_len++;
}

/* Build a Throwable zval for one aggregate entry. Caller owns the zval.
 * Walks a fallback chain: entry's class → OxPHP\Async\AsyncException →
 * builtin \Exception. The last step is always reachable (zend_ce_exception
 * is provided by Zend itself), so the only remaining -1 path is
 * object_init_ex failure (catastrophic OOM). */
static int build_throwable_zval(zval *out, const aggregate_entry_t *entry) {
    const char *cls_name = entry->exception_class
        ? entry->exception_class
        : "OxPHP\\Async\\AsyncException";
    size_t cls_len = strlen(cls_name);

    zend_string *name = zend_string_init(cls_name, cls_len, 0);
    zend_class_entry *ce = zend_lookup_class(name);
    zend_string_release(name);
    if (!ce) {
        zend_string *fallback = zend_string_init(
            "OxPHP\\Async\\AsyncException",
            sizeof("OxPHP\\Async\\AsyncException") - 1,
            0
        );
        ce = zend_lookup_class(fallback);
        zend_string_release(fallback);
        if (!ce) {
            /* Last-resort fallback. zend_ce_exception is the global
             * builtin \Exception class entry, registered by Zend during
             * MINIT — always available. Preserves the list<Throwable>
             * contract on getErrors() even if the async plugin's classes
             * somehow weren't registered yet. */
            ce = zend_ce_exception;
        }
    }
    if (object_init_ex(out, ce) != SUCCESS) return -1;
    if (entry->message) {
        zend_update_property_string(
            ce, Z_OBJ_P(out),
            "message", sizeof("message") - 1,
            entry->message
        );
    }
    return 0;
}

int oxphp_bridge_aggregate_throw(void) {
    zend_string *agg_name = zend_string_init(
        "OxPHP\\Async\\AggregateAsyncException",
        sizeof("OxPHP\\Async\\AggregateAsyncException") - 1,
        0
    );
    zend_class_entry *agg_ce = zend_lookup_class(agg_name);
    zend_string_release(agg_name);
    if (!agg_ce) {
        oxphp_bridge_aggregate_clear();
        return -1;
    }
    zval ex;
    ZVAL_UNDEF(&ex);
    if (object_init_ex(&ex, agg_ce) != SUCCESS) {
        oxphp_bridge_aggregate_clear();
        return -1;
    }

    char msg_buf[64];
    snprintf(msg_buf, sizeof(msg_buf),
             "All %zu promises rejected", aggregate_len);
    zend_update_property_string(
        agg_ce, Z_OBJ(ex),
        "message", sizeof("message") - 1,
        msg_buf
    );

    /* __errors: list<Throwable> in push order (Rust pushes in input-array order). */
    zval errors;
    array_init(&errors);
    /* __errorMap: array<int, Throwable> keyed by promise_id. */
    zval err_map;
    array_init(&err_map);
    /* __promiseIds: list<int>. */
    zval ids;
    array_init(&ids);

    for (size_t i = 0; i < aggregate_len; i++) {
        /* __promiseIds always records the id — it's just a long, no
         * allocation can fail per-element. Doing this first keeps the
         * id list complete even if Throwable construction below OOMs. */
        add_next_index_long(&ids, (zend_long)aggregate_buf[i].promise_id);

        zval throwable;
        if (build_throwable_zval(&throwable, &aggregate_buf[i]) != 0) {
            /* OOM during object_init_ex on every fallback class. Skip
             * this entry from __errors/__errorMap rather than pushing a
             * null — the property stubs declare list<Throwable> /
             * array<int, Throwable>, and a null violates that contract
             * (foreach ($e->getErrors() as $err) { $err->...; } would
             * TypeError downstream). The id is still present in
             * __promiseIds, so the rejection isn't silently lost. */
            continue;
        }
        /* Each Throwable goes into both `errors` (list) and `err_map`
         * (id-keyed map). zend_hash_index_update / add_next_index_zval take
         * ownership of one ref; bumping the refcount lets us hand the same
         * zval to two arrays without double-free. */
        Z_TRY_ADDREF(throwable);
        add_next_index_zval(&errors, &throwable);
        zend_hash_index_update(
            Z_ARRVAL(err_map),
            (zend_long)aggregate_buf[i].promise_id,
            &throwable
        );
    }

    zend_update_property(agg_ce, Z_OBJ(ex), "__errors", sizeof("__errors") - 1, &errors);
    zend_update_property(agg_ce, Z_OBJ(ex), "__errorMap", sizeof("__errorMap") - 1, &err_map);
    zend_update_property(agg_ce, Z_OBJ(ex), "__promiseIds", sizeof("__promiseIds") - 1, &ids);
    zval_ptr_dtor(&errors);
    zval_ptr_dtor(&err_map);
    zval_ptr_dtor(&ids);

    zend_throw_exception_object(&ex);
    oxphp_bridge_aggregate_clear();
    return 0;
}

int oxphp_bridge_aggregate_throw_timeout(
    const int64_t *pending_ids,
    uint32_t pending_count
) {
    zend_string *to_name = zend_string_init(
        "OxPHP\\Async\\TimeoutException",
        sizeof("OxPHP\\Async\\TimeoutException") - 1,
        0
    );
    zend_class_entry *to_ce = zend_lookup_class(to_name);
    zend_string_release(to_name);
    if (!to_ce) {
        oxphp_bridge_aggregate_clear();
        return -1;
    }
    zval ex;
    ZVAL_UNDEF(&ex);
    if (object_init_ex(&ex, to_ce) != SUCCESS) {
        oxphp_bridge_aggregate_clear();
        return -1;
    }

    char msg_buf[96];
    snprintf(msg_buf, sizeof(msg_buf),
             "oxphp_async_await_any timed out (%u pending, %zu rejected before timeout)",
             pending_count, aggregate_len);
    zend_update_property_string(
        to_ce, Z_OBJ(ex),
        "message", sizeof("message") - 1,
        msg_buf
    );

    /* __partialErrors: array<int, Throwable> keyed by promise_id. */
    zval partial;
    array_init(&partial);
    for (size_t i = 0; i < aggregate_len; i++) {
        zval throwable;
        if (build_throwable_zval(&throwable, &aggregate_buf[i]) != 0) {
            ZVAL_NULL(&throwable);
        }
        zend_hash_index_update(
            Z_ARRVAL(partial),
            (zend_long)aggregate_buf[i].promise_id,
            &throwable
        );
    }

    /* __cancelledPromiseIds: list<int>. The cancel flag has been set on
     * each of these promises and their receivers stranded — they are
     * not resumable via oxphp_async_await*(). */
    zval ids;
    array_init(&ids);
    for (uint32_t i = 0; i < pending_count; i++) {
        add_next_index_long(&ids, (zend_long)pending_ids[i]);
    }

    zend_update_property(to_ce, Z_OBJ(ex),
                         "__partialErrors", sizeof("__partialErrors") - 1, &partial);
    zend_update_property(to_ce, Z_OBJ(ex),
                         "__cancelledPromiseIds", sizeof("__cancelledPromiseIds") - 1, &ids);
    zval_ptr_dtor(&partial);
    zval_ptr_dtor(&ids);

    zend_throw_exception_object(&ex);
    oxphp_bridge_aggregate_clear();
    return 0;
}

/* === Async Promise: Async Worker State === */

void oxphp_bridge_set_async_worker(int is_async) {
    ctx.is_async_worker = is_async;
}

int oxphp_bridge_is_async_worker(void) {
    return ctx.is_async_worker;
}

/* ─── Async Fatal Error Capture ────────────────────────────── */
/* Thread-local buffer to capture the error message from zend_error_cb
 * before zend_bailout() is called. The Rust error callback writes here
 * for fatal errors on async worker threads; the task scheduler's
 * fatal-capture path (task_capture_fatal in oxphp_fiber.c) reads and
 * parses it via oxphp_bridge_pop_fatal(). */
static __thread char *captured_fatal_msg = NULL;

void oxphp_bridge_capture_fatal(const char *msg, size_t len) {
    free(captured_fatal_msg);
    if (msg && len > 0) {
        captured_fatal_msg = strndup(msg, len);
    } else {
        captured_fatal_msg = NULL;
    }
}

char *oxphp_bridge_pop_fatal(void) {
    char *msg = captured_fatal_msg;
    captured_fatal_msg = NULL;
    return msg; /* caller owns — free with free() */
}

/* ─── Worker-mode unhandled-exception capture ──────────────────
 * The fiber catch site stores the failing request's exception here; the Rust
 * worker send callback pops the fields and pushes a synthetic PhpScriptError.
 * Length-delimited: class names may embed a NUL (anonymous classes); messages
 * may be binary/latin1.
 *
 * There is one "active" slot per worker thread. Under fiber multiplexing the
 * capture happens after the handler returns but before shutdown functions run;
 * a shutdown function that suspends (oxphp_sleep/await) would otherwise let the
 * next request's oxphp_bridge_reset_request_ctx wipe this request's capture. The
 * fiber scheduler therefore moves the slot in and out of the running fiber via
 * oxphp_bridge_take_unhandled / oxphp_bridge_restore_unhandled around every
 * suspend/resume, so the capture travels with its request. Presence is
 * "cls != NULL" — set() requires a class and pop detaches all fields together. */
typedef struct {
    char *cls;   size_t cls_len;
    char *msg;   size_t msg_len;
    char *trace; size_t trace_len;
    char *file;  size_t file_len;
    uint32_t line;
} oxphp_unhandled_slot;

static __thread oxphp_unhandled_slot unhandled_exc = {0};

static char *unhandled_dup_n(const char *p, size_t len) {
    if (!p || len == 0) return NULL;
    char *b = malloc(len);
    if (b) memcpy(b, p, len);
    return b;
}

void oxphp_bridge_set_unhandled_exc(
    const char *cls, size_t cls_len,
    const char *msg, size_t msg_len,
    const char *trace, size_t trace_len,
    const char *file, size_t file_len,
    uint32_t line)
{
    oxphp_bridge_clear_unhandled();
    /* Class first: if it can't be duplicated there is no capture, so bail before
     * allocating the other fields (avoids a headerless orphan the pop path,
     * keyed on cls, could never reach). */
    char *c = unhandled_dup_n(cls, cls_len);
    if (!c) return;
    unhandled_exc.cls = c;
    unhandled_exc.cls_len = cls_len;
    unhandled_exc.msg = unhandled_dup_n(msg, msg_len);
    unhandled_exc.msg_len = unhandled_exc.msg ? msg_len : 0;
    unhandled_exc.trace = unhandled_dup_n(trace, trace_len);
    unhandled_exc.trace_len = unhandled_exc.trace ? trace_len : 0;
    unhandled_exc.file = unhandled_dup_n(file, file_len);
    unhandled_exc.file_len = unhandled_exc.file ? file_len : 0;
    unhandled_exc.line = line;
}

static char *unhandled_pop_field(char **slot, size_t *slot_len, size_t *out_len) {
    char *p = *slot;
    *slot = NULL;
    if (out_len) *out_len = *slot_len;
    *slot_len = 0;
    return p; /* caller frees with free() */
}

char *oxphp_bridge_pop_unhandled_class(size_t *out_len) {
    if (!unhandled_exc.cls) {
        if (out_len) *out_len = 0;
        return NULL;
    }
    return unhandled_pop_field(&unhandled_exc.cls, &unhandled_exc.cls_len, out_len);
}

char *oxphp_bridge_pop_unhandled_message(size_t *out_len) {
    return unhandled_pop_field(&unhandled_exc.msg, &unhandled_exc.msg_len, out_len);
}

char *oxphp_bridge_pop_unhandled_trace(size_t *out_len) {
    return unhandled_pop_field(&unhandled_exc.trace, &unhandled_exc.trace_len, out_len);
}

char *oxphp_bridge_pop_unhandled_file(size_t *out_len) {
    return unhandled_pop_field(&unhandled_exc.file, &unhandled_exc.file_len, out_len);
}

uint32_t oxphp_bridge_get_unhandled_line(void) {
    return unhandled_exc.line;
}

void oxphp_bridge_clear_unhandled(void) {
    free(unhandled_exc.cls);   unhandled_exc.cls = NULL;   unhandled_exc.cls_len = 0;
    free(unhandled_exc.msg);   unhandled_exc.msg = NULL;   unhandled_exc.msg_len = 0;
    free(unhandled_exc.trace); unhandled_exc.trace = NULL; unhandled_exc.trace_len = 0;
    free(unhandled_exc.file);  unhandled_exc.file = NULL;  unhandled_exc.file_len = 0;
    unhandled_exc.line = 0;
}

/* ── Per-fiber save/restore of the active capture ──────────────
 * The fiber scheduler parks a suspended request's capture off the thread-active
 * slot and restores it on resume, so a suspending shutdown function cannot let
 * another request's reset wipe it (and so two requests never cross-attribute). */

/* Move the active slot into a heap container (ownership of the inner strings
 * transfers with it) and clear the active slot. Returns NULL when empty. */
void *oxphp_bridge_take_unhandled(void) {
    if (!unhandled_exc.cls) {
        return NULL;
    }
    oxphp_unhandled_slot *p = malloc(sizeof(*p));
    if (!p) {
        /* OOM: drop the capture rather than leak or misattribute it. */
        oxphp_bridge_clear_unhandled();
        return NULL;
    }
    *p = unhandled_exc;
    memset(&unhandled_exc, 0, sizeof(unhandled_exc));
    return p;
}

/* Move a previously-taken container back into the active slot and free the
 * container. Any capture currently in the active slot is released first (it
 * should already be empty at a resume). NULL is a no-op. */
void oxphp_bridge_restore_unhandled(void *vp) {
    oxphp_bridge_clear_unhandled();
    if (!vp) {
        return;
    }
    oxphp_unhandled_slot *p = (oxphp_unhandled_slot *)vp;
    unhandled_exc = *p;
    free(p);
}

/* Free a taken container that will never be restored (fiber destroyed while
 * suspended with a capture parked). NULL is a no-op. */
void oxphp_bridge_free_unhandled(void *vp) {
    if (!vp) {
        return;
    }
    oxphp_unhandled_slot *p = (oxphp_unhandled_slot *)vp;
    free(p->cls);
    free(p->msg);
    free(p->trace);
    free(p->file);
    free(p);
}

/* ── Traditional-path structural class snapshot ────────────────
 * The worker path reads an escaping exception's class straight from
 * EG(exception) at the fiber-catch site. The traditional path never gets that
 * chance: PHP nulls EG(exception) inside zend_exception_error *before* our
 * zend_error_cb sees the "Uncaught …" text, so the class would otherwise have to
 * be parsed out of the formatted, partly user-controlled message (where a
 * message forging a "\n\nNext <FakeClass>: …" segment could poison the type).
 *
 * Instead, snapshot the class of every thrown exception here via
 * zend_throw_exception_hook; the Rust error callback reads the most-recent
 * snapshot when it records the Uncaught fatal and uses it as the authoritative
 * exception.type. The Rust callback reads it only for a genuine engine
 * `E_ERROR` "Uncaught" fatal and clears it immediately after copying
 * (oxphp_bridge_clear_thrown_class). That consume-on-read is not sufficient on
 * its own: an Uncaught fatal is NOT always preceded by a hook-firing throw in
 * the same request — an exception thrown without a stack frame goes straight to
 * zend_exception_error without the hook running (Zend/zend_exceptions.c), so a
 * snapshot left by an EARLIER request (which threw-and-caught, never clearing)
 * would be read as this request's class. oxphp_bridge_reset_request_ctx() clears
 * the slot per request to close that cross-request window. The class is
 * length-delimited (an anonymous class name carries an embedded NUL), matching
 * the worker capture. */
static __thread char *thrown_class = NULL;
static __thread size_t thrown_class_len = 0;

/* Chained so another extension's hook (if any) still runs. Set once at install. */
static void (*prev_throw_exception_hook)(zend_object *ex) = NULL;

static void oxphp_throw_exception_hook(zend_object *ex) {
    if (ex && ex->ce && ex->ce->name) {
        /* free-old + dup-new; on OOM leave the slot empty so the Rust side
         * falls back to text parsing rather than reporting a stale class. */
        char *dup = unhandled_dup_n(ZSTR_VAL(ex->ce->name), ZSTR_LEN(ex->ce->name));
        free(thrown_class);
        thrown_class = dup;
        thrown_class_len = dup ? ZSTR_LEN(ex->ce->name) : 0;
    }
    if (prev_throw_exception_hook) {
        prev_throw_exception_hook(ex);
    }
}

/* Install the throw hook once, after php_module_startup(). Idempotent. */
void oxphp_bridge_install_throw_hook(void) {
    if (zend_throw_exception_hook == oxphp_throw_exception_hook) {
        return;
    }
    prev_throw_exception_hook = zend_throw_exception_hook;
    zend_throw_exception_hook = oxphp_throw_exception_hook;
}

/* Borrow the most-recent thrown class (length-delimited, NUL-inclusive). NULL
 * when none has been thrown. Valid until the next throw overwrites it — the Rust
 * error callback copies it synchronously while recording the Uncaught fatal. */
const char *oxphp_bridge_peek_thrown_class(size_t *out_len) {
    if (out_len) *out_len = thrown_class_len;
    return thrown_class_len ? thrown_class : NULL;
}

/* Drop the thrown-class snapshot (consume-once). The Rust error callback calls
 * this right after copying the class while recording an Uncaught E_ERROR fatal,
 * so a stale class can never be borrowed by a later request on this thread. */
void oxphp_bridge_clear_thrown_class(void) {
    free(thrown_class);
    thrown_class = NULL;
    thrown_class_len = 0;
}

/* === Async Promise: Async Reset === */

#include "main/php_output.h"

void oxphp_async_reset(void) {
    /* Clear error state */
    CG(unclean_shutdown) = 0;
    if (EG(exception)) {
        zend_clear_exception();
    }

    /* Reset output buffers */
    php_output_end_all();
    php_output_deactivate();
    php_output_activate();

    /* Clear PHP error state */
    if (PG(last_error_message)) {
        zend_string_release(PG(last_error_message));
        PG(last_error_message) = NULL;
    }
    PG(last_error_type) = 0;
    PG(last_error_lineno) = 0;
    if (PG(last_error_file)) {
        zend_string_release(PG(last_error_file));
        PG(last_error_file) = NULL;
    }

    /* Reset execution timer */
    zend_set_timeout(0, 0);
}

/* === Async Promise: Fixup run_time_cache for cross-thread execution === */

/**
 * Ensure the current thread's MAP_PTR table is large enough and allocate a
 * fresh run_time_cache for the given op_array.
 *
 * In PHP ZTS, run_time_cache uses ZEND_MAP_PTR which is an offset into the
 * per-thread CG(map_ptr_base) table. When an op_array is transferred from
 * one thread (PHP worker) to another (async worker), the async worker's
 * MAP_PTR table may be smaller than the offset, causing SIGBUS when the VM
 * tries to access the run_time_cache.
 *
 * This function grows the table if needed, then allocates a fresh cache.
 */
/**
 * In PHP ZTS, run_time_cache uses ZEND_MAP_PTR — a byte offset into the
 * per-thread CG(map_ptr_base) table. When an op_array is transferred across
 * threads, the destination thread's table may be too small for the offset,
 * causing SIGBUS when the VM accesses run_time_cache.
 *
 * Fix: allocate a fresh run_time_cache via emalloc on this thread's heap
 * and store it as a DIRECT POINTER (bypassing MAP_PTR offset indirection).
 * ZEND_MAP_PTR_GET detects direct pointers (low bit clear) vs offsets
 * (low bit set) and handles both correctly.
 *
 * Set ZEND_ACC_HEAP_RT_CACHE so zend_closure_free_storage() will efree()
 * the cache when the closure is destroyed.
 */
static void oxphp_fixup_run_time_cache(zend_op_array *op) {
    if (op->cache_size == 0) {
        return;
    }

    void **cache = ecalloc(op->cache_size / sizeof(void *), sizeof(void *));
    ZEND_MAP_PTR_INIT(op->run_time_cache, cache);
    op->fn_flags |= ZEND_ACC_HEAP_RT_CACHE;
}

/* === Async Promise: Reconstruct closure (shared sync + fiber path) === */

int oxphp_reconstruct_async_closure(
    zend_op_array *op_array,
    HashTable *static_vars,
    zval *this_ptr,
    zval *out_closure,
    zend_fcall_info *fci,
    zend_fcall_info_cache *fcc,
    char **exc_class,
    char **exc_message
) {
    zend_function func;

    /* Reconstruct closure from op_array + static_vars */
    memcpy(&func, op_array, sizeof(zend_op_array));
    func.op_array.static_variables = static_vars;

    /* Fix up run_time_cache for this thread — the MAP_PTR offset from the
     * source thread's op_array may be invalid on the async worker's thread. */
    oxphp_fixup_run_time_cache(&func.op_array);

    /* Re-init the static_variables MAP_PTR for this worker thread, pointing
     * it at our own `static_vars`. The slot offset was memcpy'd from the
     * source thread's op_array; on this worker thread that same offset
     * resolves to an unrelated (foreign or stale) slot, so
     * zend_create_closure()'s `ZEND_MAP_PTR_GET(static_variables_ptr)` returns
     * a dangling HashTable and `zend_array_dup()`s it — a use-after-free that
     * corrupts the heap once a captured Shared\* wrapper's refcount is
     * mismanaged. The GET does not return NULL, so the engine's own NULL-slot
     * fallback to op_array.static_variables never kicks in.
     *
     * Pointing the slot at `static_vars` makes the GET return our HT:
     * zend_create_closure() then dups `static_vars` into a closure-owned copy
     * that the closure frees on destruction, while the async pool retains sole
     * ownership of the original `static_vars` and frees it exactly once after
     * this call returns. */
    if (static_vars) {
        ZEND_MAP_PTR_INIT(func.op_array.static_variables_ptr, static_vars);
    }

    zend_create_closure(out_closure, &func,
        NULL, /* scope */
        NULL, /* called_scope */
        this_ptr /* this_ptr, may be NULL */
    );

    if (zend_fcall_info_init(out_closure, 0, fci, fcc, NULL, NULL) != SUCCESS) {
        zval_ptr_dtor(out_closure);
        *exc_class = strdup("RuntimeException");
        *exc_message = strdup("Failed to initialize async closure call");
        return -1;
    }
    return 0;
}

/* === Async Promise: Borrow Proxy === */

/* CE pointer set by oxphp_sapi.c during MINIT via oxphp_bridge_set_borrow_proxy_ce() */
static zend_class_entry *borrow_proxy_ce = NULL;

void oxphp_bridge_set_borrow_proxy_ce(zend_class_entry *ce) {
    borrow_proxy_ce = ce;
}

int oxphp_ht_has_non_shareable_objects(HashTable *ht) {
    if (!ht) return 0;
    zval *val;
    ZEND_HASH_FOREACH_VAL(ht, val) {
        zval *check = val;
        if (Z_TYPE_P(check) == IS_REFERENCE) {
            check = Z_REFVAL_P(check);
        }
        if (Z_TYPE_P(check) == IS_RESOURCE) return 1;
        if (Z_TYPE_P(check) == IS_OBJECT) {
            if (!oxphp_is_shareable((void *)check)) return 1;
        }
        if (Z_TYPE_P(check) == IS_ARRAY) {
            if (oxphp_ht_has_non_shareable_objects(Z_ARRVAL_P(check))) return 1;
        }
    } ZEND_HASH_FOREACH_END();
    return 0;
}

void oxphp_arr_add_zval(zval *arr, const char *key, zval *val) {
    if (!arr || Z_TYPE_P(arr) != IS_ARRAY || !key || !val) return;
    zval copy;
    ZVAL_COPY(&copy, val);
    zend_hash_str_add_new(Z_ARRVAL_P(arr), key, strlen(key), &copy);
}

void oxphp_arr_add_index_zval(zval *arr, zend_ulong idx, zval *val) {
    if (!arr || Z_TYPE_P(arr) != IS_ARRAY || !val) return;
    zval copy;
    ZVAL_COPY(&copy, val);
    zend_hash_index_add_new(Z_ARRVAL_P(arr), idx, &copy);
}

void oxphp_create_borrow_proxy(zval *dst, uint64_t promise_id) {
    if (!borrow_proxy_ce) {
        ZVAL_NULL(dst);
        return;
    }
    object_init_ex(dst, borrow_proxy_ce);
    zend_update_property_long(borrow_proxy_ce, Z_OBJ_P(dst),
        "promiseId", sizeof("promiseId") - 1, (zend_long)promise_id);
}

/* ─── Fiber TLS Context Callbacks ──────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. Same pattern as worker/async callbacks.
 */
static oxphp_fiber_save_ctx_fn_t    rust_fiber_save_ctx    = NULL;
static oxphp_fiber_restore_ctx_fn_t rust_fiber_restore_ctx = NULL;
static oxphp_fiber_drop_ctx_fn_t    rust_fiber_drop_ctx    = NULL;

void oxphp_bridge_set_fiber_ctx_callbacks(
    oxphp_fiber_save_ctx_fn_t save_fn,
    oxphp_fiber_restore_ctx_fn_t restore_fn,
    oxphp_fiber_drop_ctx_fn_t drop_fn
) {
    rust_fiber_save_ctx    = save_fn;
    rust_fiber_restore_ctx = restore_fn;
    rust_fiber_drop_ctx    = drop_fn;
}

void oxphp_bridge_fiber_save_ctx(uint64_t fiber_id) {
    if (__builtin_expect(rust_fiber_save_ctx != NULL, 1)) {
        rust_fiber_save_ctx(fiber_id);
    }
}

void oxphp_bridge_fiber_restore_ctx(uint64_t fiber_id) {
    if (__builtin_expect(rust_fiber_restore_ctx != NULL, 1)) {
        rust_fiber_restore_ctx(fiber_id);
    }
}

void oxphp_bridge_fiber_drop_ctx(uint64_t fiber_id) {
    if (__builtin_expect(rust_fiber_drop_ctx != NULL, 1)) {
        rust_fiber_drop_ctx(fiber_id);
    }
}

/* ─── Fiber Timer Service ──────────────────────────────── */

/*
 * Global (not __thread) — set once at startup BEFORE any worker threads
 * are spawned, so no data race. Same pattern as worker/async callbacks.
 */
static oxphp_timer_register_fn_t rust_timer_register = NULL;
static oxphp_timer_poll_fn_t     rust_timer_poll     = NULL;
static oxphp_timer_remove_fn_t   rust_timer_remove   = NULL;

void oxphp_bridge_set_timer_callbacks(
    oxphp_timer_register_fn_t reg,
    oxphp_timer_poll_fn_t poll,
    oxphp_timer_remove_fn_t rem
) {
    rust_timer_register = reg;
    rust_timer_poll     = poll;
    rust_timer_remove   = rem;
}

uint64_t oxphp_bridge_timer_register(uint64_t duration_ms) {
    if (__builtin_expect(rust_timer_register != NULL, 1)) {
        return rust_timer_register(duration_ms);
    }
    return 0;
}

uint32_t oxphp_bridge_timer_poll(uint64_t *out_ids, uint32_t max_count) {
    if (__builtin_expect(rust_timer_poll != NULL, 1)) {
        return rust_timer_poll(out_ids, max_count);
    }
    return 0;
}

void oxphp_bridge_timer_remove(uint64_t timer_id) {
    if (__builtin_expect(rust_timer_remove != NULL, 1)) {
        rust_timer_remove(timer_id);
    }
}

/* ═══════════════════════════════════════════════════════════
 *  APM Hook Infrastructure — Two-Phase Design
 *
 *  Phase 1 — Registration + Approval (global, during init):
 *    1) Rust calls oxphp_apm_register_hook() before PHP startup
 *       to record which functions should be hooked (pending list).
 *    2) oxphp_apm_approve_registered_hooks() runs during MINIT —
 *       validates each pending hook against CG(class_table)/CG(function_table)
 *       and copies approved entries to a global read-only list.
 *
 *  Phase 2 — Installation (per-thread, during first RINIT):
 *    3) oxphp_apm_install_on_thread() reads the global approved list
 *       and installs hooks into THIS thread's function tables.
 *       Idempotent — no-op after first call per thread.
 *
 *  Thread safety: pending + approved lists are global, written once
 *  before workers start, read-only after. Installed hooks (apm_hooks,
 *  apm_hook_count) are __thread — each ZTS worker has its own copy.
 * ═══════════════════════════════════════════════════════════ */

/* ── Types ── */

typedef struct {
    char class_name[128];
    char func_name[128];
} oxphp_apm_pending_hook_t;

typedef struct {
    char class_name[128];
    char func_name[128];
    zif_handler original_handler; /* captured during MINIT before any replacement */
} oxphp_apm_approved_hook_t;

typedef struct {
    char class_name[128];
    char func_name[128];
    zif_handler original_handler;
} oxphp_apm_hook_t;

/* ── Global state (written once during init, read-only after) ── */

static oxphp_apm_pending_hook_t apm_pending_hooks[OXPHP_APM_MAX_HOOKS];
static int apm_pending_count = 0;

static oxphp_apm_approved_hook_t approved_hooks[OXPHP_APM_MAX_HOOKS];
static int approved_hook_count = 0;

static oxphp_apm_before_fn_t apm_before_fn = NULL;
static oxphp_apm_after_fn_t  apm_after_fn  = NULL;

/* ── Per-thread state ── */

static __thread oxphp_apm_hook_t apm_hooks[OXPHP_APM_MAX_HOOKS];
static __thread int apm_hook_count = 0;
static __thread int apm_hooks_installed = 0;

/* ── Callbacks (set by Rust before PHP startup) ── */

void oxphp_apm_set_before(oxphp_apm_before_fn_t fn) { apm_before_fn = fn; }
void oxphp_apm_set_after(oxphp_apm_after_fn_t fn)   { apm_after_fn = fn; }

/* ── Registration (called by Rust before PHP startup) ── */

void oxphp_apm_register_hook(const char *class_name, const char *func_name) {
    if (!func_name || apm_pending_count >= OXPHP_APM_MAX_HOOKS) return;

    oxphp_apm_pending_hook_t *entry = &apm_pending_hooks[apm_pending_count];
    strncpy(entry->class_name, class_name ? class_name : "", sizeof(entry->class_name) - 1);
    entry->class_name[sizeof(entry->class_name) - 1] = '\0';
    strncpy(entry->func_name, func_name, sizeof(entry->func_name) - 1);
    entry->func_name[sizeof(entry->func_name) - 1] = '\0';

    apm_pending_count++;
}

/* ── Hook wrapper (per-thread, replaces original handler) ── */

static void oxphp_apm_hook_wrapper(zend_execute_data *execute_data, zval *return_value) {
    const char *fname = (execute_data->func->common.function_name)
        ? ZSTR_VAL(execute_data->func->common.function_name) : "";
    const char *cname = (execute_data->func->common.scope)
        ? ZSTR_VAL(execute_data->func->common.scope->name) : "";

    /* Find the hook entry to get the original handler */
    zif_handler orig = NULL;
    for (int i = 0; i < apm_hook_count; i++) {
        if (strcmp(apm_hooks[i].func_name, fname) == 0 &&
            strcmp(apm_hooks[i].class_name, cname) == 0) {
            orig = apm_hooks[i].original_handler;
            break;
        }
    }

    if (__builtin_expect(orig == NULL, 0)) {
        /* Safety fallback — shouldn't happen. */
        return;
    }

    uint32_t argc = ZEND_CALL_NUM_ARGS(execute_data);
    zval *args = ZEND_CALL_ARG(execute_data, 1);

    /* Object receiver ($this) for method calls — handle 0 / NULL for global
     * functions. The handle keys connection metadata per PDO/mysqli instance;
     * the zval pointer lets the before-callback read a plain property such as
     * PDOStatement::queryString. `&execute_data->This` is stable for the call. */
    int has_this = (Z_TYPE(execute_data->This) == IS_OBJECT);
    uint32_t this_handle = has_this ? Z_OBJ_HANDLE(execute_data->This) : 0;
    void *this_zv = has_this ? (void *)&execute_data->This : NULL;

    if (apm_before_fn) {
        apm_before_fn(cname, fname, argc, (void *)args, this_handle, this_zv);
    }

    /* Call original handler */
    orig(execute_data, return_value);

    if (apm_after_fn) {
        apm_after_fn(cname, fname, argc, (void *)args, (void *)return_value);
    }
}

/* ── Helper: look up a zend_function by class+func name ── */

static zend_function *oxphp_apm_lookup(const char *class_name, const char *func_name) {
    if (!func_name || func_name[0] == '\0') return NULL;

    zend_function *func = NULL;

    if (class_name && class_name[0] != '\0') {
        zend_string *cls_lower = zend_string_init(class_name, strlen(class_name), 0);
        zend_str_tolower(ZSTR_VAL(cls_lower), ZSTR_LEN(cls_lower));
        zend_class_entry *ce = zend_hash_find_ptr(CG(class_table), cls_lower);
        zend_string_release(cls_lower);
        if (!ce) return NULL;

        zend_string *fn_lower = zend_string_init(func_name, strlen(func_name), 0);
        zend_str_tolower(ZSTR_VAL(fn_lower), ZSTR_LEN(fn_lower));
        func = zend_hash_find_ptr(&ce->function_table, fn_lower);
        zend_string_release(fn_lower);
    } else {
        zend_string *fn_lower = zend_string_init(func_name, strlen(func_name), 0);
        zend_str_tolower(ZSTR_VAL(fn_lower), ZSTR_LEN(fn_lower));
        func = zend_hash_find_ptr(CG(function_table), fn_lower);
        zend_string_release(fn_lower);
    }

    return func;
}

/* ── Phase 1: Approve hooks (MINIT, global) ── */

int oxphp_apm_approve_registered_hooks(void) {
    approved_hook_count = 0;

    for (int i = 0; i < apm_pending_count; i++) {
        zend_function *func = oxphp_apm_lookup(
            apm_pending_hooks[i].class_name,
            apm_pending_hooks[i].func_name
        );

        if (!func || func->type != ZEND_INTERNAL_FUNCTION) continue;

        oxphp_apm_approved_hook_t *entry = &approved_hooks[approved_hook_count];
        memcpy(entry->class_name, apm_pending_hooks[i].class_name, sizeof(entry->class_name));
        memcpy(entry->func_name, apm_pending_hooks[i].func_name, sizeof(entry->func_name));
        entry->original_handler = func->internal_function.handler;
        approved_hook_count++;
    }

    return approved_hook_count;
}

int oxphp_apm_hook_count_approved(void) {
    return approved_hook_count;
}

/* ── Phase 2: Install hooks (first RINIT, per-thread) ── */

void oxphp_apm_install_on_thread(void) {
    if (apm_hooks_installed) return;
    apm_hooks_installed = 1;

    apm_hook_count = 0;

    for (int i = 0; i < approved_hook_count; i++) {
        if (apm_hook_count >= OXPHP_APM_MAX_HOOKS) break;

        zend_function *func = oxphp_apm_lookup(
            approved_hooks[i].class_name,
            approved_hooks[i].func_name
        );

        if (!func || func->type != ZEND_INTERNAL_FUNCTION) continue;

        oxphp_apm_hook_t *entry = &apm_hooks[apm_hook_count];
        /* snprintf truncates and NUL-terminates in one call; the strncpy +
         * explicit-NUL pair triggered -Wstringop-truncation when the source
         * could exceed the dest buffer (which the Rust-side approved_hooks
         * table can in theory do). */
        snprintf(entry->class_name, sizeof(entry->class_name), "%s",
                 approved_hooks[i].class_name);
        snprintf(entry->func_name, sizeof(entry->func_name), "%s",
                 approved_hooks[i].func_name);
        /* Use the original handler captured during MINIT (before any replacement),
           not the current handler which may already be our wrapper from another thread. */
        entry->original_handler = approved_hooks[i].original_handler;

        func->internal_function.handler = oxphp_apm_hook_wrapper;
        apm_hook_count++;
    }
}

/* ── Unhook (per-thread) ── */

void oxphp_apm_unhook_all(void) {
    for (int i = 0; i < apm_hook_count; i++) {
        zend_function *func = oxphp_apm_lookup(
            apm_hooks[i].class_name,
            apm_hooks[i].func_name
        );

        if (func && func->type == ZEND_INTERNAL_FUNCTION) {
            func->internal_function.handler = apm_hooks[i].original_handler;
        }
    }
    apm_hook_count = 0;
    apm_hooks_installed = 0;
}

/* ── Diagnostics ── */

int oxphp_apm_hook_count_installed(void) {
    return apm_hook_count;
}

/* ============================================================ *
 * Profiler observer state                                      *
 * ============================================================ *
 *
 * Per-thread state for the ox_profiler plugin's observer hook.
 * Adds the TLS struct and entry points so Rust can read/write the
 * mode flag and drain the ring buffer; the Observer install +
 * begin/end callbacks are defined below.
 */

#define OXPHP_PROF_BUF_DEPTH        256
#define OXPHP_PROF_OPEN_STACK_MAX    32
#define OXPHP_PROF_NAME_ARENA_BYTES 8192
#define OXPHP_PROF_NAME_MAX_BYTES     64

/* Forward declaration. The flush sink is implemented in Rust
 * (src/profiling/flush.rs) and exported as a #[no_mangle]
 * extern "C" symbol; the dynamic linker resolves it at load time
 * when the oxphp Rust binary is the process image.
 *
 * Weak linkage: under a bare `php` CLI invocation (no oxphp binary
 * in the process) musl's eager relocation would otherwise abort the
 * dlopen of liboxphp_bridge.so. See the matching comment near the
 * oxphp_shared_* declarations for the full rationale. Under that
 * configuration g_prof.mode stays at OXPHP_PROFILING_MODE_OFF, so
 * oxphp_prof_flush_buffer() never populates the ring buffer; the
 * NULL check below is belt-and-suspenders. */
extern void oxphp_profiler_flush_span_events(const ox_span_event_t *events,
                                              uint32_t count)
    __attribute__((weak));

static __thread struct {
    uint8_t  mode;
    uint8_t  paused;            /* set/cleared by oxphp_bridge_set_profiling_paused */
    uint8_t  open_depth;
    uint8_t  open_stack_overflow;
    uint16_t buf_len;
    uint16_t name_arena_used;

    uint64_t next_seq;          /* incremented on every BEGIN */

    uint32_t force_profile_fn_count; /* mirrors count of force_profile=1 entries in g_filter_cache; reset only by clear_filter_cache() */
    uint8_t  capture_mem;
    uint8_t  capture_cpu;

    ox_span_event_t buf[OXPHP_PROF_BUF_DEPTH];

    /* Open-stack mirror: 32-bit BEGIN seq tags from root → current.
     * The heap hook will read this; currently only written by the
     * observer begin/end callbacks. */
    uint32_t open_stack[OXPHP_PROF_OPEN_STACK_MAX];

    /* Parallel to open_stack: records the zend_function* whose BEGIN
     * actually pushed the entry. The end callback compares this
     * against execute_data->func and only pops when they match —
     * correctness guard for mixed-capture runs (ApmOnly with
     * force_profile fns, PROFILE_ALL + pause/resume, sample misses,
     * max_spans cap) where begin decides not to push but end still
     * fires for the same function. Without the check, end would
     * unbalance a span that belongs to a different (outer) function. */
    uintptr_t open_fn_stack[OXPHP_PROF_OPEN_STACK_MAX];

    /* Per-flush string arena. Reset whenever buf_len returns to 0. */
    char     name_arena[OXPHP_PROF_NAME_ARENA_BYTES];
} g_prof;

/* Process-wide cap on span count per request. Set once at plugin
 * init from ProfilerConfig.max_spans via oxphp_bridge_set_profiler_max_spans.
 * 0 means "no cap" (we map it to UINT32_MAX internally). */
static uint32_t g_prof_max_spans_cap = UINT32_MAX;

/* Sticky flag — stays 1 once a request was seen with mode==PROFILE_ALL.
 * Used by oxphp_profiler_end so end events for begins captured under
 * the previous mode still drain (avoids unbalanced open_stack at the
 * request boundary). Cleared in oxphp_bridge_set_profiling_mode(OFF). */
static __thread uint8_t oxphp_prof_was_active = 0;

_Static_assert(sizeof(ox_span_event_t) == 64,
               "ox_span_event_t must be exactly 64 bytes (one cache line)");

/* Process-wide capture toggles. Read once from env at library load;
 * each thread copies them into g_prof on first set_profiling_mode. */
static uint8_t oxphp_prof_capture_mem_default = 1;
static uint8_t oxphp_prof_capture_cpu_default = 1;

static uint8_t oxphp_prof_parse_env_bool(const char *val, uint8_t dflt) {
    if (val == NULL) return dflt;
    if (val[0] == '\0') return dflt;
    if (strcmp(val, "true") == 0 || strcmp(val, "1") == 0) return 1;
    if (strcmp(val, "false") == 0 || strcmp(val, "0") == 0) return 0;
    return dflt;
}

__attribute__((constructor))
static void oxphp_prof_init_env(void) {
    oxphp_prof_capture_mem_default =
        oxphp_prof_parse_env_bool(getenv("PROFILER_CAPTURE_MEM"), 1);
    oxphp_prof_capture_cpu_default =
        oxphp_prof_parse_env_bool(getenv("PROFILER_CAPTURE_CPU"), 1);
}

/* --- internal helpers ----------------------------------------- */

/* Drain the ring buffer into Rust, then reset buf_len + name_arena.
 * Called from the begin/end callbacks when the buffer fills, and
 * from the public RSHUTDOWN flush below. */
static void oxphp_prof_flush_buffer(void) {
    if (g_prof.buf_len == 0) return;
    /* Weak-linked — NULL when the bridge is loaded without the oxphp
     * Rust binary (e.g. a standalone `php` CLI session). */
    if (oxphp_profiler_flush_span_events != NULL) {
        oxphp_profiler_flush_span_events(g_prof.buf, g_prof.buf_len);
    }
    g_prof.buf_len = 0;
    g_prof.name_arena_used = 0;
}

/* --- public entry points -------------------------------------- */

void oxphp_bridge_set_profiling_mode(uint8_t mode) {
    g_prof.mode = mode;
    g_prof.capture_mem = oxphp_prof_capture_mem_default;
    g_prof.capture_cpu = oxphp_prof_capture_cpu_default;
    if (mode == OXPHP_PROFILING_MODE_OFF) {
        /* Clean reset between requests so the next worker run
         * starts with empty state. We do not flush here — anything
         * still in the buffer when mode flips to OFF is dropped on
         * purpose (the request that produced it is over). */
        g_prof.buf_len = 0;
        g_prof.name_arena_used = 0;
        g_prof.open_depth = 0;
        g_prof.open_stack_overflow = 0;
        g_prof.next_seq = 0;
        g_prof.paused = 0;          /* Fresh request starts unpaused */
        /* force_profile_fn_count intentionally preserved: mirrors g_filter_cache, not per-request */
        oxphp_prof_was_active = 0;
    }
}

uint8_t oxphp_bridge_get_profiling_mode(void) {
    return g_prof.mode;
}

uint8_t oxphp_bridge_snapshot_open_stack(uint32_t *dst, uint8_t max_depth) {
    if (g_prof.open_stack_overflow) return 255;
    uint8_t n = g_prof.open_depth;
    if (n > max_depth) n = max_depth;
    if (dst != NULL && n > 0) {
        memcpy(dst, g_prof.open_stack, (size_t)n * sizeof(uint32_t));
    }
    return n;
}

void oxphp_bridge_profiler_rshutdown_flush(void) {
    oxphp_prof_flush_buffer();
}

/* ── Profiler paused flag ───────────────────────────── */

void oxphp_bridge_set_profiling_paused(uint8_t paused) {
    g_prof.paused = paused ? 1 : 0;
}

uint8_t oxphp_bridge_is_profiling_paused(void) {
    return g_prof.paused;
}

int64_t oxphp_bridge_get_memory_usage_bytes(void) {
    /* zend_memory_usage(0) returns the current allocated bytes
     * across all opcaches for this request. Outside a PHP request
     * (e.g. between RINIT/RSHUTDOWN) the engine returns 0 by
     * convention. The MemoryThresholdDecorator uses this for
     * delta-based threshold detection. */
    return (int64_t)zend_memory_usage(0);
}

/* ============================================================ *
 * Profiler filter cache                                        *
 * ============================================================ */

#define OXPHP_PROF_FILTER_CACHE_INIT_CAP   256
#define OXPHP_PROF_FILTER_CACHE_MAX_CAP   4096

/* Per-thread cache entry. Holds spec_id (Rust handle) + the four
 * hot-path decision values mirrored from the spec, so begin/end
 * never re-enter Rust to ask "should I create this span?". */
typedef struct {
    uintptr_t fn_id;          /* 0 = empty slot (zend_function* never NULL) */
    uint32_t  spec_id;        /* 0 = no filter (cached fast path) */
    uint8_t   excluded;       /* 1 = skip span creation */
    uint8_t   force_profile;  /* 1 = create even when mode != PROFILE_ALL */
    uint8_t   has_sample;     /* 1 = sample_rate is set */
    uint8_t   reserved;       /* keep zero — alignment */
    float     sample_rate;    /* only meaningful when has_sample == 1 */
} ox_filter_cache_entry_t;

static __thread struct {
    ox_filter_cache_entry_t *entries;
    uint32_t                  cap;        /* power-of-two */
    uint32_t                  size;       /* number of non-empty slots */
} g_filter_cache;

/* Resolver function pointer set by Rust at init. NULL = no
 * filtering (observer creates spans for all functions). */
static oxphp_profiler_resolve_filter_fn_t g_filter_resolver = NULL;

void oxphp_bridge_set_filter_resolver(oxphp_profiler_resolve_filter_fn_t resolver) {
    g_filter_resolver = resolver;
}

/* fxhash-style mix to (uintptr_t)func → bucket index. cap is
 * power-of-two so & cap-1 == mod cap. */
static uint32_t oxphp_filter_cache_hash(uintptr_t fn_id, uint32_t cap) {
    uint64_t h = (uint64_t)fn_id;
    h ^= h >> 33;
    h *= 0xff51afd7ed558ccdULL;
    h ^= h >> 33;
    return (uint32_t)(h & ((uint64_t)cap - 1));
}

static int oxphp_filter_cache_grow(void) {
    uint32_t new_cap = (g_filter_cache.cap == 0)
                       ? OXPHP_PROF_FILTER_CACHE_INIT_CAP
                       : g_filter_cache.cap * 2;
    if (new_cap > OXPHP_PROF_FILTER_CACHE_MAX_CAP) {
        return 0;  /* abort grow; observer init falls back to re-resolve */
    }
    ox_filter_cache_entry_t *new_entries =
        calloc(new_cap, sizeof(ox_filter_cache_entry_t));
    if (!new_entries) return 0;

    uint32_t mask = new_cap - 1;
    for (uint32_t i = 0; i < g_filter_cache.cap; i++) {
        if (g_filter_cache.entries[i].fn_id == 0) continue;
        uint32_t h = oxphp_filter_cache_hash(g_filter_cache.entries[i].fn_id, new_cap);
        while (new_entries[h].fn_id != 0) h = (h + 1) & mask;
        new_entries[h] = g_filter_cache.entries[i];
    }

    free(g_filter_cache.entries);
    g_filter_cache.entries = new_entries;
    g_filter_cache.cap = new_cap;
    return 1;
}

/* Look up an existing entry. Returns NULL on miss. */
static ox_filter_cache_entry_t *oxphp_filter_cache_lookup(uintptr_t fn_id) {
    if (g_filter_cache.cap == 0) return NULL;
    uint32_t mask = g_filter_cache.cap - 1;
    uint32_t h = oxphp_filter_cache_hash(fn_id, g_filter_cache.cap);
    while (g_filter_cache.entries[h].fn_id != 0) {
        if (g_filter_cache.entries[h].fn_id == fn_id) {
            return &g_filter_cache.entries[h];
        }
        h = (h + 1) & mask;
    }
    return NULL;
}

/* Insert / overwrite an entry. Grows if load > 0.75. Silently no-ops
 * past max cap (the function will just be re-resolved every time). */
static void oxphp_filter_cache_put(const ox_filter_cache_entry_t *entry) {
    if (g_filter_cache.cap == 0) {
        if (!oxphp_filter_cache_grow()) return;
    }
    if (g_filter_cache.size * 4 >= g_filter_cache.cap * 3) {
        if (!oxphp_filter_cache_grow()) return;
    }
    uint32_t mask = g_filter_cache.cap - 1;
    uint32_t h = oxphp_filter_cache_hash(entry->fn_id, g_filter_cache.cap);
    while (g_filter_cache.entries[h].fn_id != 0
           && g_filter_cache.entries[h].fn_id != entry->fn_id) {
        h = (h + 1) & mask;
    }
    uint8_t prev_force = (g_filter_cache.entries[h].fn_id == entry->fn_id)
                         ? g_filter_cache.entries[h].force_profile
                         : 0;
    if (g_filter_cache.entries[h].fn_id == 0) g_filter_cache.size++;
    g_filter_cache.entries[h] = *entry;
    if (entry->force_profile && !prev_force) {
        g_prof.force_profile_fn_count++;
    } else if (!entry->force_profile && prev_force) {
        if (g_prof.force_profile_fn_count > 0) g_prof.force_profile_fn_count--;
    }
}

uint32_t oxphp_bridge_get_filter_spec_id_cached(uintptr_t fn_id) {
    ox_filter_cache_entry_t *e = oxphp_filter_cache_lookup(fn_id);
    return e ? e->spec_id : 255;
}

void oxphp_bridge_clear_filter_cache(void) {
    free(g_filter_cache.entries);
    g_filter_cache.entries = NULL;
    g_filter_cache.cap = 0;
    g_filter_cache.size = 0;
    g_prof.force_profile_fn_count = 0;
}

/* `ox_attr_resolver_ctx_t` is declared in oxphp_bridge.h so the SAPI
 * translation unit can build it for the decorator resolve path too. */

/* Look up the `idx`-th attribute named `attr_name` in `attrs`.
 * Repeated attributes (e.g. multiple #[Tag(...)] on one function)
 * are addressed via idx (idx=0 = first occurrence). */
static zend_attribute *
oxphp_lookup_nth_attribute(HashTable *attrs, const char *attr_name, uint32_t idx) {
    if (!attrs) return NULL;
    uint32_t seen = 0;
    zend_attribute *a;
    ZEND_HASH_FOREACH_PTR(attrs, a) {
        if (a && a->name && strcmp(ZSTR_VAL(a->name), attr_name) == 0) {
            if (seen == idx) return a;
            seen++;
        }
    } ZEND_HASH_FOREACH_END();
    return NULL;
}

size_t oxphp_bridge_read_attr_arg_str(
    void *attr_resolver_ctx,
    int is_class_scope,
    const char *attr_name,
    uint32_t attr_idx,
    uint32_t arg_idx,
    char *out, size_t out_cap)
{
    ox_attr_resolver_ctx_t *ctx = (ox_attr_resolver_ctx_t *)attr_resolver_ctx;
    HashTable *attrs = is_class_scope ? ctx->class_attrs : ctx->fn_attrs;
    zend_attribute *attr = oxphp_lookup_nth_attribute(attrs, attr_name, attr_idx);
    if (!attr || arg_idx >= attr->argc || out_cap == 0) return 0;

    zval val;
    if (zend_get_attribute_value(&val, attr, arg_idx, ctx->scope) != SUCCESS) {
        return 0;
    }
    size_t written = 0;
    if (Z_TYPE(val) == IS_STRING) {
        size_t src_len = Z_STRLEN(val);
        size_t copy = src_len < out_cap - 1 ? src_len : out_cap - 1;
        memcpy(out, Z_STRVAL(val), copy);
        out[copy] = '\0';
        written = copy;
    }
    zval_ptr_dtor(&val);
    return written;
}

int oxphp_bridge_read_attr_arg_double(
    void *attr_resolver_ctx,
    int is_class_scope,
    const char *attr_name,
    uint32_t attr_idx,
    uint32_t arg_idx,
    double *out)
{
    ox_attr_resolver_ctx_t *ctx = (ox_attr_resolver_ctx_t *)attr_resolver_ctx;
    HashTable *attrs = is_class_scope ? ctx->class_attrs : ctx->fn_attrs;
    zend_attribute *attr = oxphp_lookup_nth_attribute(attrs, attr_name, attr_idx);
    if (!attr || arg_idx >= attr->argc) return 0;

    zval val;
    if (zend_get_attribute_value(&val, attr, arg_idx, ctx->scope) != SUCCESS) {
        return 0;
    }
    int ok = 0;
    if (Z_TYPE(val) == IS_DOUBLE) {
        *out = Z_DVAL(val);
        ok = 1;
    } else if (Z_TYPE(val) == IS_LONG) {
        *out = (double)Z_LVAL(val);
        ok = 1;
    }
    zval_ptr_dtor(&val);
    return ok;
}

size_t oxphp_bridge_read_attr_arg_name(
    void *attr_resolver_ctx,
    int is_class_scope,
    const char *attr_name,
    uint32_t attr_idx,
    uint32_t arg_idx,
    char *out, size_t out_cap)
{
    ox_attr_resolver_ctx_t *ctx = (ox_attr_resolver_ctx_t *)attr_resolver_ctx;
    HashTable *attrs = is_class_scope ? ctx->class_attrs : ctx->fn_attrs;
    zend_attribute *attr = oxphp_lookup_nth_attribute(attrs, attr_name, attr_idx);
    if (!attr || arg_idx >= attr->argc || out_cap == 0) return 0;

    /* Positional arguments carry a NULL name; only `name:`-style
     * arguments record one. Zend stores args in source-call order, so
     * the name is the only way to map them back to constructor params. */
    zend_string *name = attr->args[arg_idx].name;
    if (!name) return 0;
    size_t src_len = ZSTR_LEN(name);
    size_t copy = src_len < out_cap - 1 ? src_len : out_cap - 1;
    memcpy(out, ZSTR_VAL(name), copy);
    out[copy] = '\0';
    return copy;
}

int oxphp_bridge_attr_arg_count(
    void *attr_resolver_ctx,
    int is_class_scope,
    const char *attr_name,
    uint32_t attr_idx)
{
    ox_attr_resolver_ctx_t *ctx = (ox_attr_resolver_ctx_t *)attr_resolver_ctx;
    HashTable *attrs = is_class_scope ? ctx->class_attrs : ctx->fn_attrs;
    zend_attribute *attr = oxphp_lookup_nth_attribute(attrs, attr_name, attr_idx);
    if (!attr) return -1;
    return (int)attr->argc;
}

int oxphp_bridge_read_attr_arg_variant(
    void *attr_resolver_ctx,
    int is_class_scope,
    const char *attr_name,
    uint32_t attr_idx,
    uint32_t arg_idx,
    int64_t *out_long,
    double *out_double,
    int *out_bool,
    char *out, size_t out_cap)
{
    ox_attr_resolver_ctx_t *ctx = (ox_attr_resolver_ctx_t *)attr_resolver_ctx;
    HashTable *attrs = is_class_scope ? ctx->class_attrs : ctx->fn_attrs;
    zend_attribute *attr = oxphp_lookup_nth_attribute(attrs, attr_name, attr_idx);
    if (!attr || arg_idx >= attr->argc) return 0;

    zval val;
    if (zend_get_attribute_value(&val, attr, arg_idx, ctx->scope) != SUCCESS) {
        return 0;
    }
    int tag = 0;
    switch (Z_TYPE(val)) {
        case IS_LONG:
            *out_long = (int64_t)Z_LVAL(val);
            tag = 1;
            break;
        case IS_DOUBLE:
            *out_double = Z_DVAL(val);
            tag = 2;
            break;
        case IS_STRING:
            if (out_cap > 0) {
                size_t src_len = Z_STRLEN(val);
                size_t copy = src_len < out_cap - 1 ? src_len : out_cap - 1;
                memcpy(out, Z_STRVAL(val), copy);
                out[copy] = '\0';
            }
            tag = 3;
            break;
        case IS_TRUE:
            *out_bool = 1;
            tag = 4;
            break;
        case IS_FALSE:
            *out_bool = 0;
            tag = 4;
            break;
        case IS_NULL:
            tag = 5;
            break;
        default:
            tag = 0;
            break;
    }
    zval_ptr_dtor(&val);
    return tag;
}

/* ============================================================ *
 * Profiler observer init + begin/end callbacks                 *
 * ============================================================ */

#include "Zend/zend_observer.h"
#include "Zend/zend_compile.h"
/* zend_attributes.h is included higher up alongside the other PHP
 * headers (filter cache needs zend_attribute earlier). */

/* Synthesise a span name for a zend_function. Returns a pointer into
 * a per-thread static buffer (overwritten on next call). May return
 * NULL when the function has no name and no scope. Caller copies into
 * the per-flush arena via oxphp_prof_arena_copy(). */
static const char *oxphp_prof_synthesise_name(zend_function *func, size_t *out_len) {
    static __thread char tmp[OXPHP_PROF_NAME_MAX_BYTES + 1];

    if (func == NULL) { *out_len = 0; return NULL; }

    const char *fn  = func->common.function_name
                       ? ZSTR_VAL(func->common.function_name)
                       : "{closure}";
    size_t       fnl = func->common.function_name
                       ? ZSTR_LEN(func->common.function_name)
                       : 9;

    if (func->common.scope) {
        const char *cn  = ZSTR_VAL(func->common.scope->name);
        size_t       cnl = ZSTR_LEN(func->common.scope->name);
        size_t       total = cnl + 2 + fnl;
        if (total > OXPHP_PROF_NAME_MAX_BYTES) total = OXPHP_PROF_NAME_MAX_BYTES;
        size_t cn_take = (cnl < total - 2) ? cnl : (total - 2);
        size_t fn_take = total - 2 - cn_take;
        memcpy(tmp, cn, cn_take);
        tmp[cn_take]     = ':';
        tmp[cn_take + 1] = ':';
        memcpy(tmp + cn_take + 2, fn, fn_take);
        *out_len = total;
        tmp[total] = '\0';
        return tmp;
    }

    if (fnl > OXPHP_PROF_NAME_MAX_BYTES) fnl = OXPHP_PROF_NAME_MAX_BYTES;
    memcpy(tmp, fn, fnl);
    tmp[fnl] = '\0';
    *out_len = fnl;
    return tmp;
}

/* Copy `name` of `name_len` bytes into the per-thread arena. Returns a
 * pointer into the arena, or NULL when the arena is full. Pointer is
 * valid only until the next flush (which resets the arena). */
static const char *oxphp_prof_arena_copy(const char *name, size_t name_len) {
    if (name == NULL || name_len == 0) return NULL;
    if (name_len > OXPHP_PROF_NAME_MAX_BYTES) name_len = OXPHP_PROF_NAME_MAX_BYTES;
    if ((size_t)g_prof.name_arena_used + name_len > OXPHP_PROF_NAME_ARENA_BYTES) {
        return NULL;
    }
    char *dst = &g_prof.name_arena[g_prof.name_arena_used];
    memcpy(dst, name, name_len);
    g_prof.name_arena_used += (uint16_t)name_len;
    return dst;
}

static inline uint64_t oxphp_prof_clock_mono_ns(void) {
    struct timespec ts;
    /* CLOCK_MONOTONIC (not _RAW) goes through the vDSO on Linux, so
     * it's ~10-20 ns instead of ~300 ns for a full syscall. The NTP
     * slew that _RAW avoids is irrelevant for intra-request span
     * durations, and the observer reads the same clock on BEGIN and
     * END so any slew applies equally to both. */
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

static inline uint64_t oxphp_prof_clock_cpu_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &ts) != 0) {
        return 0;
    }
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

/* Forward declaration — implemented in Rust (src/profiling/filter.rs)
 * as #[no_mangle] pub extern "C". Weak for the same reason as
 * oxphp_profiler_flush_span_events above. When the bridge is loaded
 * by a bare `php` CLI, g_prof.mode stays OFF and oxphp_profiler_begin
 * returns before the sample-hit branch is reached. */
extern uint8_t oxphp_profiler_sample_hit(float rate) __attribute__((weak));

static void oxphp_profiler_begin(zend_execute_data *execute_data) {
    /* Paused short-circuit applies regardless of filter spec. */
    if (UNEXPECTED(g_prof.paused)) return;

    /* Mode-first gate: skip the filter cache lookup entirely when
     * mode=OFF and no force_profile attribute has been resolved on
     * this thread. This is the common production path. */
    if (g_prof.mode != OXPHP_PROFILING_MODE_PROFILE_ALL
        && g_prof.force_profile_fn_count == 0) {
        return;
    }

    /* Consult the per-fn filter spec cached at observer init. */
    uintptr_t fn_id = (uintptr_t)execute_data->func;
    ox_filter_cache_entry_t *filter = oxphp_filter_cache_lookup(fn_id);

    /* Decision tree (per spec §6 composition rules):
     * 1. Excluded — never create a span.
     * 2. !PROFILE_ALL && !force_profile — default mode-only check
     *    rejects.
     * 3. has_sample — roll PRNG; skip if uniform > rate. */
    if (filter != NULL && filter->excluded) return;

    if (g_prof.mode != OXPHP_PROFILING_MODE_PROFILE_ALL) {
        if (filter == NULL || !filter->force_profile) return;
    }

    if (filter != NULL && filter->has_sample) {
        /* Weak-linked — NULL under a bare `php` CLI dlopen. The
         * mode-first gate above already prevents this branch in
         * that case, but guard anyway for defense in depth. */
        if (oxphp_profiler_sample_hit == NULL) return;
        if (!oxphp_profiler_sample_hit(filter->sample_rate)) return;
    }

    /* Per-request span cap. next_seq grows monotonically under
     * mode != OFF; once the cap is reached we stop emitting new
     * span events. A sentinel (fn_id=0) is still pushed to
     * open_fn_stack so the matching end pops in LIFO order — this
     * is required for correctness under recursion, where the
     * outer fn_id on the stack equals the inner fn_id and the
     * plain fn_id-mismatch guard would otherwise mis-pop an
     * outer frame. zend_function* is never NULL, so 0 is a safe
     * sentinel value. */
    if (UNEXPECTED(g_prof.next_seq >= g_prof_max_spans_cap)) {
        g_prof.open_stack_overflow = 1;  /* signal truncation to Rust */
        if (g_prof.open_depth < OXPHP_PROF_OPEN_STACK_MAX) {
            g_prof.open_stack[g_prof.open_depth] = 0;
            g_prof.open_fn_stack[g_prof.open_depth] = 0;
            g_prof.open_depth++;
        }
        return;
    }

    oxphp_prof_was_active = 1;

    if (UNEXPECTED(g_prof.buf_len == OXPHP_PROF_BUF_DEPTH)) {
        oxphp_prof_flush_buffer();
    }

    size_t name_len = 0;
    const char *raw = oxphp_prof_synthesise_name(execute_data->func, &name_len);
    const char *arena_name = oxphp_prof_arena_copy(raw, name_len);

    uint64_t seq = ++g_prof.next_seq;
    ox_span_event_t *ev = &g_prof.buf[g_prof.buf_len++];
    ev->kind        = OXPHP_SPAN_EVENT_KIND_BEGIN;
    ev->reserved0   = 0;
    ev->name_len    = arena_name ? (uint16_t)name_len : 0;
    ev->reserved1   = 0;
    ev->seq         = seq;
    ev->ts_ns       = oxphp_prof_clock_mono_ns();
    ev->cpu_ns      = g_prof.capture_cpu ? oxphp_prof_clock_cpu_ns() : 0;
    ev->mem         = g_prof.capture_mem ? (int64_t)zend_memory_usage(0) : 0;
    ev->mem_peak    = g_prof.capture_mem ? (int64_t)zend_memory_peak_usage(0) : 0;
    ev->name_ptr    = arena_name;
    /* Pass spec_id to Rust so apply_events can attach tags after
     * pushing the BEGIN event. spec_id 0 = no tag work. */
    ev->reserved2   = filter != NULL ? (uint64_t)filter->spec_id : 0;

    /* Mirror open-span stack for the heap hook. seq tags are
     * 64-bit but we only keep the low 32 bits in the mirror — heap
     * attribution doesn't need them to be unique across the whole
     * process, only within a request, and 32 bits is enough for any
     * realistic per-request span count.
     *
     * open_fn_stack mirrors fn_id at the same depth so the end
     * callback can distinguish "my begin pushed" from "my begin was
     * skipped but an outer span is still open". */
    if (g_prof.open_depth < OXPHP_PROF_OPEN_STACK_MAX) {
        g_prof.open_stack[g_prof.open_depth] = (uint32_t)(seq & 0xFFFFFFFFu);
        g_prof.open_fn_stack[g_prof.open_depth] = fn_id;
        g_prof.open_depth++;
    } else {
        g_prof.open_stack_overflow = 1;
    }
}

static void oxphp_profiler_end(zend_execute_data *execute_data, zval *retval) {
    (void)retval;
    /* Allow drain when mode just flipped from PROFILE_ALL to OFF
     * (oxphp_prof_was_active stays 1 until the next set_profiling_mode
     * call). Without this, end events for begins captured under the
     * previous mode would be lost and Rust would mark them leaked. */
    if (EXPECTED(g_prof.mode != OXPHP_PROFILING_MODE_PROFILE_ALL)
        && !oxphp_prof_was_active) {
        return;
    }

    if (g_prof.open_depth == 0) return;

    /* Correctness guard: if the begin for THIS function was skipped
     * (paused, sample miss, force_profile-only with fn not in filter),
     * open_fn_stack top belongs to an outer span — popping would
     * unbalance it. Skip silently.
     *
     * The check uses execute_data->func which is the same pointer the
     * observer API keyed the handler-pair cache on, so equality with
     * the value begin stored is guaranteed for well-formed recursion. */
    uintptr_t fn_id = (uintptr_t)(execute_data ? execute_data->func : NULL);
    uintptr_t top_fn = g_prof.open_fn_stack[g_prof.open_depth - 1];

    /* Sentinel path: begin was dropped because max_spans cap was
     * reached. Consume the slot silently; do not emit an END event.
     * Under recursion the outer fn_id equals the dropped begin's
     * fn_id, so this LIFO sentinel is the only way to keep begin/end
     * pairs aligned. */
    if (top_fn == 0) {
        g_prof.open_depth--;
        return;
    }
    if (top_fn != fn_id) {
        return;
    }

    if (UNEXPECTED(g_prof.buf_len == OXPHP_PROF_BUF_DEPTH)) {
        oxphp_prof_flush_buffer();
    }

    g_prof.open_depth--;
    uint64_t seq = (uint64_t)g_prof.open_stack[g_prof.open_depth];

    ox_span_event_t *ev = &g_prof.buf[g_prof.buf_len++];
    ev->kind        = OXPHP_SPAN_EVENT_KIND_END;
    ev->reserved0   = 0;
    ev->name_len    = 0;
    ev->reserved1   = 0;
    ev->seq         = seq;
    ev->ts_ns       = oxphp_prof_clock_mono_ns();
    ev->cpu_ns      = g_prof.capture_cpu ? oxphp_prof_clock_cpu_ns() : 0;
    ev->mem         = g_prof.capture_mem ? (int64_t)zend_memory_usage(0) : 0;
    ev->mem_peak    = g_prof.capture_mem ? (int64_t)zend_memory_peak_usage(0) : 0;
    ev->name_ptr    = NULL;
    ev->reserved2   = 0;
}

/* Set the per-request span cap. 0 is interpreted as "unlimited" and
 * stored as UINT32_MAX so the hot-path comparison stays a simple
 * unsigned less-than. Process-wide; intended to be called once from
 * Rust at plugin init (ProfilerPlugin::init). */
void oxphp_bridge_set_profiler_max_spans(uint32_t cap) {
    g_prof_max_spans_cap = cap ? cap : UINT32_MAX;
}

/* Per-fn-creation init. The Observer API caches the returned handler
 * pair for the lifetime of `execute_data->func`, so we MUST NOT make
 * the result depend on a runtime-mutable flag (the cached result
 * would freeze early-on-first-request decisions for the rest of the
 * process). Instead we always return our handler pair for user
 * functions; the begin/end callbacks themselves consult g_prof.mode. */
zend_observer_fcall_handlers
oxphp_profiler_observer_init(zend_execute_data *execute_data) {
    if (UNEXPECTED(execute_data == NULL || execute_data->func == NULL)) {
        return (zend_observer_fcall_handlers){NULL, NULL};
    }
    if (execute_data->func->common.type != ZEND_USER_FUNCTION) {
        /* Internal functions skipped at the gate (PROFILER_INTERNAL
         * toggle). */
        return (zend_observer_fcall_handlers){NULL, NULL};
    }

    /* Resolve filter spec for this function on first observation,
     * cache the result per (fn, thread). spec_id 0 is "no filter,
     * default behaviour" — still cached so subsequent inits skip
     * the resolve work. */
    uintptr_t fn_id = (uintptr_t)execute_data->func;
    if (g_filter_resolver != NULL && oxphp_filter_cache_lookup(fn_id) == NULL) {
        const char *fn_attr_names[64];
        uint32_t fn_attr_count = 0;
        HashTable *fn_attrs = execute_data->func->common.attributes;
        if (fn_attrs) {
            zend_attribute *a;
            ZEND_HASH_FOREACH_PTR(fn_attrs, a) {
                if (a && a->name && fn_attr_count < 64) {
                    fn_attr_names[fn_attr_count++] = ZSTR_VAL(a->name);
                }
            } ZEND_HASH_FOREACH_END();
        }

        const char *class_attr_names[64];
        uint32_t class_attr_count = 0;
        HashTable *class_attrs = execute_data->func->common.scope
                                 ? execute_data->func->common.scope->attributes
                                 : NULL;
        if (class_attrs) {
            zend_attribute *a;
            ZEND_HASH_FOREACH_PTR(class_attrs, a) {
                if (a && a->name && class_attr_count < 64) {
                    class_attr_names[class_attr_count++] = ZSTR_VAL(a->name);
                }
            } ZEND_HASH_FOREACH_END();
        }

        /* Pre-filter: skip the Rust call when no attribute begins
         * with "OxPHP\Profile\" — the fast path for ~100% of fns
         * in a typical app. The cache still stores spec_id 0 so we
         * don't repeat this work. */
        int has_profile_attr = 0;
        for (uint32_t i = 0; i < fn_attr_count && !has_profile_attr; i++) {
            if (strncmp(fn_attr_names[i], "OxPHP\\Profile\\", 14) == 0) has_profile_attr = 1;
        }
        for (uint32_t i = 0; i < class_attr_count && !has_profile_attr; i++) {
            if (strncmp(class_attr_names[i], "OxPHP\\Profile\\", 14) == 0) has_profile_attr = 1;
        }

        ox_filter_cache_entry_t entry = {0};
        entry.fn_id = fn_id;

        if (has_profile_attr) {
            ox_attr_resolver_ctx_t actx = {
                .scope       = execute_data->func->common.scope,
                .fn_attrs    = fn_attrs,
                .class_attrs = class_attrs,
            };
            entry.spec_id = g_filter_resolver(
                fn_id,
                class_attr_count > 0 ? class_attr_names : NULL,
                class_attr_count,
                fn_attr_count > 0 ? fn_attr_names : NULL,
                fn_attr_count,
                &actx,
                &entry.excluded,
                &entry.force_profile,
                &entry.has_sample,
                &entry.sample_rate);
        }
        oxphp_filter_cache_put(&entry);
    }

    return (zend_observer_fcall_handlers){
        oxphp_profiler_begin,
        oxphp_profiler_end,
    };
}

/* ═══════════════════════════════════════════════════════════
 *  Shareable interface
 * ═══════════════════════════════════════════════════════════ */

zend_class_entry *oxphp_shareable_ce = NULL;

int oxphp_shareable_register_ce(void)
{
    zend_class_entry tmp_ce;
    INIT_NS_CLASS_ENTRY(tmp_ce, "OxPHP\\Shared", "Shareable", NULL);
    oxphp_shareable_ce = zend_register_internal_interface(&tmp_ce);
    if (!oxphp_shareable_ce) {
        return FAILURE;
    }
    return SUCCESS;
}

int oxphp_shareable_unregister_ce(void)
{
    /* The engine frees the class_entry at module shutdown; just clear
     * our cached pointer so a later re-init of the module starts fresh. */
    oxphp_shareable_ce = NULL;
    return SUCCESS;
}

int oxphp_is_shareable(void *z)
{
    zval *zv = (zval *)z;
    if (zv == NULL) return 0;
    if (Z_TYPE_P(zv) != IS_OBJECT) return 0;
    if (oxphp_shareable_ce == NULL) return 0;
    return instanceof_function(Z_OBJCE_P(zv), oxphp_shareable_ce) ? 1 : 0;
}

/* ─── Synthetic promise callbacks ─────── */

static oxphp_async_synth_alloc_fn_t   g_synth_alloc   = NULL;
static oxphp_async_synth_resolve_fn_t g_synth_resolve = NULL;
static oxphp_async_synth_reject_fn_t  g_synth_reject  = NULL;
static oxphp_async_synth_cancel_fn_t  g_synth_cancel  = NULL;

void oxphp_bridge_set_async_synth_alloc(oxphp_async_synth_alloc_fn_t fn) {
    g_synth_alloc = fn;
}
void oxphp_bridge_set_async_synth_resolve(oxphp_async_synth_resolve_fn_t fn) {
    g_synth_resolve = fn;
}
void oxphp_bridge_set_async_synth_reject(oxphp_async_synth_reject_fn_t fn) {
    g_synth_reject = fn;
}
void oxphp_bridge_set_async_synth_cancel(oxphp_async_synth_cancel_fn_t fn) {
    g_synth_cancel = fn;
}

int64_t oxphp_async_synthetic_promise_alloc(void)
{
    return g_synth_alloc ? g_synth_alloc() : 0;
}

int oxphp_async_synthetic_promise_resolve(int64_t id,
                                           const uint8_t *payload_bytes,
                                           size_t payload_len)
{
    return g_synth_resolve ? g_synth_resolve(id, payload_bytes, payload_len) : 0;
}

int oxphp_async_synthetic_promise_reject(int64_t id,
                                          const char *cls_fqn,
                                          const char *message)
{
    return g_synth_reject ? g_synth_reject(id, cls_fqn, message) : 0;
}

int oxphp_async_synthetic_promise_cancel(int64_t id)
{
    return g_synth_cancel ? g_synth_cancel(id) : 0;
}

/* ═══════════════════════════════════════════════════════════
 *  Shared wrapper cross-thread helpers
 *  Used by portbuf_ser_zval (tag 7) and portrd_deser_zval (tag 7)
 *  to cross a Shared\* object between threads. SharedHandle is
 *  #[repr(C)] in Rust so reading offsets 0 (*const Entry) and 8
 *  (u8 type_tag) is well-defined.
 * ═══════════════════════════════════════════════════════════ */

/* See the forward-declared externs near the serializer. Only the
 * tag→FQN mapping needs re-declaring at this site. */
extern const char *oxphp_shared_class_name(uint8_t type_tag) __attribute__((weak));

int oxphp_plugin_get_shared_handle(zval *obj,
                                   uint8_t *out_type_tag,
                                   const void **out_entry_ptr) {
    if (!obj || Z_TYPE_P(obj) != IS_OBJECT) return -1;
    if (!oxphp_is_shareable((void *)obj)) return -1;
    oxphp_custom_object *intern = OXPHP_OBJ(Z_OBJ_P(obj));
    if (intern == NULL || intern->rust_data == NULL) return -1;
    /* SharedHandle layout: *const Entry at offset 0, u8 type_tag at 8 */
    unsigned char *storage = (unsigned char *)intern->rust_data;
    const void *eptr;
    memcpy(&eptr, storage, sizeof(const void *));
    uint8_t tt = storage[8];
    if (eptr == NULL) return -1; /* uninitialised wrapper */
    *out_entry_ptr = eptr;
    *out_type_tag  = tt;
    return 0;
}

int oxphp_shared_wrapper_new(zval *out, uint8_t type_tag, const void *entry_ptr) {
    if (!out || entry_ptr == NULL) return -1;

    /* Look up the PHP class_entry by type_tag. Weak-linked: in a bare
     * PHP CLI dlopen() the Rust binary is absent and the symbol is
     * NULL — fail closed. Cross-thread tag-7 deserialization only runs
     * from oxphp's Rust workers, where the symbol is always present. */
    if (oxphp_shared_class_name == NULL) return -1;
    const char *fqn = oxphp_shared_class_name(type_tag);
    if (fqn == NULL) return -1; /* unknown type tag */
    zend_string *cname = zend_string_init(fqn, strlen(fqn), 0);
    zend_class_entry *ce = zend_lookup_class_ex(cname, NULL, 0);
    zend_string_release(cname);
    if (ce == NULL) return -1;

    /* object_init_ex runs the create_object handler (→ storage_factory)
     * but NOT __construct. The factory populates a fresh SharedHandle
     * { entry_ptr: NULL, type_tag: ... }; we overwrite entry_ptr with
     * the transferred strong-ref pointer below. */
    if (object_init_ex(out, ce) != SUCCESS) return -1;

    oxphp_custom_object *intern = OXPHP_OBJ(Z_OBJ_P(out));
    if (intern == NULL || intern->rust_data == NULL) {
        zval_ptr_dtor(out);
        return -1;
    }
    unsigned char *storage = (unsigned char *)intern->rust_data;
    /* The caller (tag-7 deserializer) obtained `entry_ptr` from
     * oxphp_shared_handle_from_id, which already incremented the strong
     * count. Move it into the wrapper's handle — the wrapper's
     * SharedHandle::drop will balance it. */
    memcpy(storage, &entry_ptr, sizeof(const void *));
    storage[8] = type_tag;
    return 0;
}

/* ─── Shared\* synchronous closure-invoke shims ─────────
 *
 * Same-thread synchronous invocation of a PHP callable, with state
 * crossing the Rust↔C boundary as portbuf bytes (same wire format
 * as oxphp_portable_serialize / oxphp_portable_deserialize).
 *
 * Rust never touches emalloc — all zvals allocated and freed here.
 * On closure throw, EG(exception) stays set; caller returns
 * RETURN_THROWS-style from its plugin method handler.
 */

#define OXPHP_SHARED_INVOKE_OK          0
#define OXPHP_SHARED_INVOKE_PHP_THREW   1
#define OXPHP_SHARED_INVOKE_BAD_CALLABLE -1
#define OXPHP_SHARED_INVOKE_BAD_RETURN  -2

/* Invoke a zero-argument closure/callable. Returns its portbuf-encoded
 * return value in *out_ret_buf (emalloc'd via the same free path as
 * oxphp_portable_serialize — caller calls oxphp_portable_free).
 *
 * On closure throw: returns OXPHP_SHARED_INVOKE_PHP_THREW. EG(exception)
 * stays set for the caller's plugin-method wrapper to surface.
 */
int oxphp_shared_invoke_0_portbuf(zval *callable,
                                  uint8_t **out_ret_buf,
                                  size_t *out_ret_len,
                                  const void **out_retained_entry)
{
    if (!callable || !out_ret_buf || !out_ret_len || !out_retained_entry)
        return OXPHP_SHARED_INVOKE_BAD_CALLABLE;
    *out_ret_buf = NULL;
    *out_ret_len = 0;
    *out_retained_entry = NULL;

    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
    char *err = NULL;
    if (zend_fcall_info_init(callable, 0, &fci, &fcc, NULL, &err) != SUCCESS) {
        if (err) efree(err);
        return OXPHP_SHARED_INVOKE_BAD_CALLABLE;
    }
    if (err) efree(err);

    zval ret_zv;
    ZVAL_UNDEF(&ret_zv);

    zend_call_known_function(fcc.function_handler,
                             fcc.object,
                             fcc.called_scope,
                             &ret_zv,
                             0, NULL, NULL);

    if (EG(exception)) {
        zval_ptr_dtor(&ret_zv);
        return OXPHP_SHARED_INVOKE_PHP_THREW;
    }

    /* Serialise ret_zv into portbuf bytes for Rust to decode. The
     * callable executed successfully — failure here means the *return
     * value* is not portbuf-serialisable (e.g. a closure, a resource,
     * or a non-Shareable PHP object). Surface as BAD_RETURN so the
     * caller can distinguish it from "could not invoke at all" and
     * raise a precise PHP exception. */
    if (oxphp_portable_serialize(&ret_zv, 1, out_ret_buf, out_ret_len) != 0) {
        zval_ptr_dtor(&ret_zv);
        return OXPHP_SHARED_INVOKE_BAD_RETURN;
    }

    /* Top-level Shared return: retain the entry across the wrapper
     * drop. The buffer only carries the id; if the wrapper was the
     * sole strong ref to the Entry, zval_ptr_dtor would fire
     * Entry::drop, the registry's Weak would die, and the Rust
     * receiver's lookup(id) would return StaleHandle — manifesting as
     * "factory must return a Shared\\* instance" even though it did.
     *
     * The +1 strong ref is conceptually owned by *out_ret_buf; the
     * Rust caller MUST drop it (Arc::from_raw + drop) after decoding,
     * symmetric to oxphp_portable_free for the byte buffer.
     *
     * Nested Shared\* (e.g., a Shared inside an array return) is NOT
     * retained here — those values land in the wildcard arm of the
     * Registry/Once factory match and surface a TypeException anyway,
     * so the early Entry::drop has no observable effect. */
    if (Z_TYPE(ret_zv) == IS_OBJECT && oxphp_is_shareable(&ret_zv)
        && oxphp_shared_handle_clone != NULL) {
        uint8_t type_tag;
        const void *entry_ptr;
        if (oxphp_plugin_get_shared_handle(&ret_zv, &type_tag, &entry_ptr) == 0) {
            oxphp_shared_handle_clone(entry_ptr);
            *out_retained_entry = entry_ptr;
        }
    }

    zval_ptr_dtor(&ret_zv);
    return OXPHP_SHARED_INVOKE_OK;
}

/* Invoke a 1-argument closure where arg 0 is a by-reference zval
 * materialised from the caller's SharedValue (encoded in state_buf).
 * After the closure returns, re-serialise the (possibly mutated)
 * state zval into *new_state_buf for Rust to write back.
 *
 * Semantics:
 *   *did_mutate is always set to 1 by this shim — we cannot cheaply
 *   diff pre/post, so callers always write back on INVOKE_OK. Rust
 *   keeps the state lock held across this call.
 *
 * Return codes:
 *   INVOKE_OK            — closure ran, state and return value serialised.
 *   INVOKE_PHP_THREW     — closure threw; EG(exception) set. Partial
 *                          state (mutations before the throw) IS
 *                          serialised into *new_state_buf so callers
 *                          can honour a no-rollback policy.
 *   INVOKE_BAD_CALLABLE  — callable itself is unusable (NULL, fcall_init
 *                          failure, missing required args).
 *   INVOKE_BAD_RETURN    — callable ran to completion but either its
 *                          return value or the post-call state contains
 *                          a non-Shareable zval (closure, resource,
 *                          non-Shareable object) that the portbuf
 *                          serialiser refused. Distinct from
 *                          BAD_CALLABLE so the caller can point the
 *                          PHP-level diagnostic at the return/state
 *                          instead of the callable.
 *
 *                          State-preservation contract (symmetric with
 *                          PHP_THREW): if the *state* serialised cleanly
 *                          and only the return value is the offender,
 *                          *new_state_buf is non-NULL and carries the
 *                          partial mutation for the caller to apply per
 *                          its no-rollback policy. If the state itself
 *                          contained the non-Shareable value (caught at
 *                          the state-serialise step), *new_state_buf
 *                          stays NULL — nothing to apply. The caller
 *                          frees *new_state_buf on every non-OK return.
 *
 * Embedded-Shareable retention (*out_retained_state, *out_retained_ret):
 *   Symmetric pin for both the by-ref state and the return value. If the
 *   closure assigns a fresh Shared\* into the by-ref state (e.g.
 *   `$s = new Map()`) OR returns one (`return new Map()`), that wrapper's
 *   only PHP-level strong ref is `state_zv` / `ret_zv` itself. The
 *   serialiser writes the wire id into `new_state_buf` / `out_ret_buf`,
 *   but the Rust-side decode (`raw_to_owned` → `Weak::upgrade`) runs
 *   AFTER this function returns — and AFTER the `zval_ptr_dtor`s below
 *   would drop the wrapper and GC the Entry, invalidating the wire id.
 *   To bridge that lifetime gap, on every code path where the
 *   corresponding wire buffer is non-NULL we `ZVAL_COPY` the post-call
 *   value into a heap-allocated zval and hand its pointer back via the
 *   out-param. The copy pins every embedded Shareable across the dtor
 *   (ZVAL_COPY on an array bumps refcounts on every nested element);
 *   the caller MUST balance with `oxphp_shared_free_zval()` after
 *   decoding (or on any early exit). NULL when there is nothing to
 *   retain (serialise step failed, ret was IS_UNDEF/IS_NULL, etc.).
 */
int oxphp_shared_invoke_byref_1_portbuf(zval *callable,
                                         const uint8_t *state_buf,
                                         size_t state_len,
                                         uint8_t **new_state_buf,
                                         size_t *new_state_len,
                                         uint8_t **out_ret_buf,
                                         size_t *out_ret_len,
                                         int *did_mutate,
                                         void **out_retained_state,
                                         void **out_retained_ret)
{
    if (!callable || !state_buf || state_len == 0 ||
        !new_state_buf || !new_state_len ||
        !out_ret_buf || !out_ret_len || !did_mutate ||
        !out_retained_state || !out_retained_ret) {
        return OXPHP_SHARED_INVOKE_BAD_CALLABLE;
    }
    *new_state_buf = NULL;
    *new_state_len = 0;
    *out_ret_buf = NULL;
    *out_ret_len = 0;
    *did_mutate = 0;
    *out_retained_state = NULL;
    *out_retained_ret = NULL;

    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
    char *err = NULL;
    if (zend_fcall_info_init(callable, 0, &fci, &fcc, NULL, &err) != SUCCESS) {
        if (err) efree(err);
        return OXPHP_SHARED_INVOKE_BAD_CALLABLE;
    }
    if (err) efree(err);

    /* Materialise state_buf into a stack zval on the invoking thread. */
    zval state_zv;
    ZVAL_UNDEF(&state_zv);
    if (oxphp_portable_deserialize(state_buf, state_len, 1, &state_zv) != 0) {
        zval_ptr_dtor(&state_zv);
        return OXPHP_SHARED_INVOKE_BAD_CALLABLE;
    }

    /* Wrap as a by-reference zval so mutations inside the closure
     * persist on the outer state_zv. ZVAL_MAKE_REF upgrades in
     * place to IS_REFERENCE. */
    ZVAL_MAKE_REF(&state_zv);

    zval ret_zv;
    ZVAL_UNDEF(&ret_zv);

    zval args[1];
    ZVAL_COPY_VALUE(&args[0], &state_zv);

    zend_call_known_function(fcc.function_handler,
                             fcc.object,
                             fcc.called_scope,
                             &ret_zv,
                             1, args, NULL);

    /* Unwrap the reference so serialise sees the underlying value.
     * Do this BEFORE checking EG(exception) so partial mutations made
     * by the closure before it threw are preserved in new_state_buf.
     * Callers check the return code to decide whether to apply the
     * new state; they always free new_state_buf when rc != INVOKE_OK. */
    zval *state_inner = Z_ISREF(state_zv) ? Z_REFVAL(state_zv) : &state_zv;

    if (oxphp_portable_serialize(state_inner, 1, new_state_buf, new_state_len) != 0) {
        zval_ptr_dtor(&state_zv);
        zval_ptr_dtor(&ret_zv);
        /* Closure completed (or threw); the serialiser refused the
         * (possibly partially-mutated) state — e.g. the closure assigned
         * a non-Shareable value into the by-ref state. If a PHP exception
         * is also pending, report PHP_THREW so the caller surfaces it
         * unchanged; otherwise BAD_RETURN distinguishes "callable is
         * fine, return/state value is unusable" from "callable itself
         * is unusable" (BAD_CALLABLE).
         *
         * No state to retain: serialise itself rejected the value,
         * `*new_state_buf` is NULL, so embedded Shareables (if any)
         * never reached the wire format. */
        return EG(exception) ? OXPHP_SHARED_INVOKE_PHP_THREW : OXPHP_SHARED_INVOKE_BAD_RETURN;
    }

    /* State serialised cleanly. Pin embedded Shareables across the
     * upcoming `zval_ptr_dtor(&state_zv)` so the Rust-side decode can
     * resolve their wire ids. The caller MUST balance via
     * `oxphp_shared_free_zval()` after decoding. */
    {
        zval *retained = (zval *)emalloc(sizeof(zval));
        ZVAL_COPY(retained, state_inner);
        *out_retained_state = retained;
    }

    if (EG(exception)) {
        /* State serialised above for callers that want to keep partial
         * mutations (Mutex "no rollback on PHP throw" policy).
         * Caller is responsible for freeing *new_state_buf. */
        zval_ptr_dtor(&state_zv);
        zval_ptr_dtor(&ret_zv);
        return OXPHP_SHARED_INVOKE_PHP_THREW;
    }

    if (oxphp_portable_serialize(&ret_zv, 1, out_ret_buf, out_ret_len) != 0) {
        /* Callable ran to completion without throwing; the serialiser
         * refused the returned value (closure, resource, non-Shareable
         * object). The by-ref STATE serialised cleanly at the call
         * above and lives in *new_state_buf — hand it back so the
         * caller can apply the partial mutation per Mutex's no-rollback
         * policy, symmetric with PHP_THREW. The caller frees
         * *new_state_buf on every non-OK return.
         *
         * No ret to retain: the serialiser rejected the value, so the
         * wire bytes carry nothing for the Rust decoder to upgrade. */
        zval_ptr_dtor(&state_zv);
        zval_ptr_dtor(&ret_zv);
        return OXPHP_SHARED_INVOKE_BAD_RETURN;
    }

    /* Ret serialised cleanly. Symmetric to the state-retention block
     * above: pin any Shareables embedded in the return value across the
     * upcoming `zval_ptr_dtor(&ret_zv)` so the Rust caller's
     * `raw_to_owned` can resolve the wire ids. Without this, a closure
     * returning a freshly-constructed `new Shared\*()` (whose only
     * strong ref is `ret_zv` itself) would have its Entry GC'd before
     * the receiver decodes the wire bytes — manifesting as a silent
     * PHP-side `null` return. `ZVAL_COPY` on an array bumps refcounts
     * on every nested element too, so nested Shareables are covered.
     * The caller MUST balance via `oxphp_shared_free_zval()` after
     * decoding (or on any early exit). */
    if (Z_TYPE(ret_zv) != IS_UNDEF && Z_TYPE(ret_zv) != IS_NULL) {
        zval *retained = (zval *)emalloc(sizeof(zval));
        ZVAL_COPY(retained, &ret_zv);
        *out_retained_ret = retained;
    }

    *did_mutate = 1;
    zval_ptr_dtor(&state_zv);
    zval_ptr_dtor(&ret_zv);
    return OXPHP_SHARED_INVOKE_OK;
}

/* Release a retained-state zval produced by
 * `oxphp_shared_invoke_byref_1_portbuf` (`*out_retained_state`). NULL
 * is a no-op (callers don't have to gate themselves). */
void oxphp_shared_free_zval(void *p)
{
    if (!p) return;
    zval *z = (zval *)p;
    zval_ptr_dtor(z);
    efree(z);
}

/* Invoke $fn(key, value) for Shared\Map::forEach. The key is the tagged
 * tuple (key_kind: 0=int → IS_LONG, 1=str → IS_STRING); the value is
 * portbuf bytes deserialised into a zval on this thread. No Map lock is
 * held across this call (the Rust side snapshots keys, releases the
 * shard, then calls here per key).
 *
 * Returns 1 to STOP iteration (callback returned bool false), 0 to
 * continue, <0 on a bad callable, a deserialise failure, or a PHP throw
 * (EG(exception) stays set for the caller's method wrapper to surface). */
int oxphp_shared_invoke_2_ret_stop(zval *callable,
                                   int key_kind,
                                   int64_t key_int,
                                   const char *key_ptr,
                                   size_t key_len,
                                   const char *val_buf,
                                   size_t val_len)
{
    if (!callable) return -1;

    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
    char *err = NULL;
    if (zend_fcall_info_init(callable, 0, &fci, &fcc, NULL, &err) != SUCCESS) {
        if (err) efree(err);
        return -1;
    }
    if (err) efree(err);

    zval args[2];
    /* arg 0: key */
    if (key_kind == 0) {
        ZVAL_LONG(&args[0], (zend_long)key_int);
    } else {
        ZVAL_STRINGL(&args[0], key_ptr ? key_ptr : "", key_len);
    }
    /* arg 1: value, materialised from portbuf */
    ZVAL_UNDEF(&args[1]);
    if (oxphp_portable_deserialize((const uint8_t *)val_buf, val_len, 1, &args[1]) != 0) {
        zval_ptr_dtor(&args[0]);
        zval_ptr_dtor(&args[1]);
        return -1;
    }

    zval ret_zv;
    ZVAL_UNDEF(&ret_zv);
    zend_call_known_function(fcc.function_handler,
                             fcc.object,
                             fcc.called_scope,
                             &ret_zv,
                             2, args, NULL);

    zval_ptr_dtor(&args[0]);
    zval_ptr_dtor(&args[1]);

    if (EG(exception)) {
        zval_ptr_dtor(&ret_zv);
        return -1;
    }

    int stop = (Z_TYPE(ret_zv) == IS_FALSE) ? 1 : 0;
    zval_ptr_dtor(&ret_zv);
    return stop;
}

/* ═══════════════════════════════════════════════════════════
 *  Cross-thread fcc spike
 *
 *  Probes whether a `zend_fcall_info_cache` captured on thread A
 *  is safely invokable from thread B under ZTS. If not, Pool's
 *  factory path has to store the callable's function NAME and
 *  re-resolve per invoking thread (extra indirection). Temporary
 *  probe — superseded by the real Pool FFI path below.
 * ═══════════════════════════════════════════════════════════ */

typedef struct {
    zend_fcall_info_cache fcc;
    zval callable_zv;        /* keeps the Closure/callable alive */
    uint64_t captured_tid;
    int in_use;
} spike_pool_slot_t;

/* Process-global: populated on capture, read on invoke. No mutex:
 * the spike's PHP test is strictly request-at-a-time. Real Pool
 * will use proper synchronisation. */
static spike_pool_slot_t spike_pool_slot = {0};

static inline uint64_t spike_current_tid(void) {
    /* pthread_self() returns an opaque handle — on Linux/musl it's
     * a pointer; cast to uintptr_t for a stable-per-thread id. */
    return (uint64_t)(uintptr_t)pthread_self();
}

void oxphp_pool_spike_reset(void) {
    if (spike_pool_slot.in_use) {
        zval_ptr_dtor(&spike_pool_slot.callable_zv);
        memset(&spike_pool_slot, 0, sizeof(spike_pool_slot));
    }
}

int oxphp_pool_spike_capture(void *callable_zval, uint64_t *out_tid) {
    if (!callable_zval || !out_tid) return -1;
    zval *callable = (zval *)callable_zval;

    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
    char *err = NULL;
    if (zend_fcall_info_init(callable, 0, &fci, &fcc, NULL, &err) != SUCCESS) {
        if (err) efree(err);
        return -1;
    }
    if (err) efree(err);

    /* Drop any previous capture. Runs on the current thread, which
     * may not be the prior capturer — zval_ptr_dtor on a closure
     * allocated elsewhere is exactly the kind of hazard the spike
     * is meant to flush out. For this probe we accept the risk and
     * cover it with explicit `reset()` calls in the test runner. */
    oxphp_pool_spike_reset();

    ZVAL_COPY(&spike_pool_slot.callable_zv, callable);
    spike_pool_slot.fcc = fcc;
    spike_pool_slot.captured_tid = spike_current_tid();
    spike_pool_slot.in_use = 1;

    *out_tid = spike_pool_slot.captured_tid;
    return 0;
}

int oxphp_pool_spike_invoke(
    uint64_t *out_captured_tid,
    uint64_t *out_current_tid,
    uint8_t **out_ret_buf,
    size_t *out_ret_len)
{
    if (!out_captured_tid || !out_current_tid || !out_ret_buf || !out_ret_len) return -1;
    *out_captured_tid = 0;
    *out_current_tid = spike_current_tid();
    *out_ret_buf = NULL;
    *out_ret_len = 0;

    if (!spike_pool_slot.in_use) return -1;
    *out_captured_tid = spike_pool_slot.captured_tid;

    zval ret_zv;
    ZVAL_UNDEF(&ret_zv);

    zend_call_known_function(
        spike_pool_slot.fcc.function_handler,
        spike_pool_slot.fcc.object,
        spike_pool_slot.fcc.called_scope,
        &ret_zv,
        0, NULL, NULL);

    if (EG(exception)) {
        zval_ptr_dtor(&ret_zv);
        return -2;
    }

    if (oxphp_portable_serialize(&ret_zv, 1, out_ret_buf, out_ret_len) != 0) {
        zval_ptr_dtor(&ret_zv);
        return -3;
    }
    zval_ptr_dtor(&ret_zv);
    return 0;
}

/* ═══════════════════════════════════════════════════════════
 *  Shared\Pool helpers
 *
 *  Factory/body closure invocation + slot-zval lifecycle.
 *  Called from Rust's oxphp_shared_pool_* FFI.
 *
 *  oxphp_pool_fcc_t is emalloc'd at pool_create, holds a
 *  ZVAL_COPY of the callable so its op_array outlives any
 *  one thread — the spike above verified `zend_call_known_function`
 *  on the stored fcc is safe to invoke from any worker under ZTS.
 *
 *  Per-resource zvals are emalloc'd once at factory invocation
 *  and owned by the pool; acquire ZVAL_COPYs them into user
 *  out-zvals; release does not touch the slot zval (it stays
 *  refcounted by the pool). Only pool-drop should efree the
 *  slot; v1 leaks on drop.
 * ═══════════════════════════════════════════════════════════ */

typedef struct {
    zend_fcall_info_cache fcc;
    zval callable_zv; /* keeps the Closure/callable op_array alive */
} oxphp_pool_fcc_t;

/* Allocate a fcc_heap from a user callable. Returns 0 on success
 * with `*out_fcc_heap` populated, -1 if `callable` is not a valid
 * PHP callable. The caller must pair with oxphp_pool_fcc_free. */
int oxphp_pool_fcc_new(void *callable_zval, void **out_fcc_heap) {
    if (!callable_zval || !out_fcc_heap) return -1;
    *out_fcc_heap = NULL;

    zval *callable = (zval *)callable_zval;
    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
    char *err = NULL;
    if (zend_fcall_info_init(callable, 0, &fci, &fcc, NULL, &err) != SUCCESS) {
        if (err) efree(err);
        return -1;
    }
    if (err) efree(err);

    oxphp_pool_fcc_t *heap = emalloc(sizeof(oxphp_pool_fcc_t));
    heap->fcc = fcc;
    ZVAL_COPY(&heap->callable_zv, callable);
    *out_fcc_heap = (void *)heap;
    return 0;
}

/* Free a fcc_heap. In v1 the pool leaks these at registry drop;
 * later shutdown-drain wiring will invoke this explicitly on the
 * creating worker. zval_ptr_dtor from a non-Zend-initialised
 * thread is unsafe — callers MUST ensure they run inside
 * `php_request_startup` bounds. */
void oxphp_pool_fcc_free(void *fcc_heap) {
    if (!fcc_heap) return;
    oxphp_pool_fcc_t *heap = (oxphp_pool_fcc_t *)fcc_heap;
    zval_ptr_dtor(&heap->callable_zv);
    efree(heap);
}

/* Invoke the factory with 0 args. On success, emalloc a fresh
 * zval and ZVAL_COPY the factory's return into it; the heap
 * pointer is written to `*out_slot_zv_heap`. The pool owns this
 * allocation from here on.
 *
 * Status codes:
 *   0  — OK, `*out_slot_zv_heap` populated (IS_OBJECT guaranteed).
 *  -1  — factory threw; EG(exception) set, nothing allocated.
 *  -2  — factory returned non-object. v1 requires objects so the
 *        release path can match via spl_object_id-style identity
 *        stored in the Shared\Pool\Handle wrapper's rust_data slot.
 *        Caller surfaces TypeException; nothing allocated. */
int oxphp_pool_factory_invoke(void *fcc_heap, void **out_slot_zv_heap) {
    if (!fcc_heap || !out_slot_zv_heap) return -1;
    *out_slot_zv_heap = NULL;

    oxphp_pool_fcc_t *heap = (oxphp_pool_fcc_t *)fcc_heap;

    zval ret_zv;
    ZVAL_UNDEF(&ret_zv);

    zend_call_known_function(heap->fcc.function_handler,
                              heap->fcc.object,
                              heap->fcc.called_scope,
                              &ret_zv,
                              0, NULL, NULL);

    if (EG(exception)) {
        zval_ptr_dtor(&ret_zv);
        return -1;
    }
    if (Z_TYPE(ret_zv) != IS_OBJECT) {
        zval_ptr_dtor(&ret_zv);
        return -2;
    }

    zval *slot_zv = emalloc(sizeof(zval));
    ZVAL_COPY(slot_zv, &ret_zv);
    zval_ptr_dtor(&ret_zv);
    *out_slot_zv_heap = (void *)slot_zv;
    return 0;
}

/* Invoke a 1-arg body: body($resource). The resource is the
 * pool's slot_zv — the body receives a ZVAL_COPY'd reference,
 * so object-method calls on it affect the underlying resource
 * naturally (Z_OBJ identity preserved).
 *
 * On success, ZVAL_COPY the return into `*user_out_zv`. On
 * throw, EG(exception) stays set and `user_out_zv` is untouched.
 *
 * Status codes:
 *   0  — OK, user_out_zv filled.
 *  -1  — body is not a valid callable.
 *  -2  — body threw; EG(exception) set. */
int oxphp_pool_body_invoke(void *body_callable_zv,
                            void *slot_zv_heap,
                            void *user_out_zv)
{
    if (!body_callable_zv || !slot_zv_heap || !user_out_zv) return -1;
    zval *body = (zval *)body_callable_zv;
    zval *slot = (zval *)slot_zv_heap;
    zval *out = (zval *)user_out_zv;

    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
    char *err = NULL;
    if (zend_fcall_info_init(body, 0, &fci, &fcc, NULL, &err) != SUCCESS) {
        if (err) efree(err);
        return -1;
    }
    if (err) efree(err);

    zval args[1];
    ZVAL_COPY(&args[0], slot);

    zval ret_zv;
    ZVAL_UNDEF(&ret_zv);

    zend_call_known_function(fcc.function_handler,
                              fcc.object,
                              fcc.called_scope,
                              &ret_zv,
                              1, args, NULL);

    zval_ptr_dtor(&args[0]);

    if (EG(exception)) {
        zval_ptr_dtor(&ret_zv);
        return -2;
    }

    ZVAL_COPY(out, &ret_zv);
    zval_ptr_dtor(&ret_zv);
    return 0;
}

/* ZVAL_COPY a heap slot-zval into a user out-zval. Used by
 * pool_acquire (hand resource to user) and by Handle::get()
 * (read-only accessor — pool retains ownership). */
void oxphp_pool_slot_to_user(void *slot_zv_heap, void *user_out_zv) {
    if (!slot_zv_heap || !user_out_zv) return;
    zval *slot = (zval *)slot_zv_heap;
    zval *out = (zval *)user_out_zv;
    ZVAL_COPY(out, slot);
}

/* zval_ptr_dtor + efree on a slot-zval heap allocation. Same
 * thread-safety caveat as oxphp_pool_fcc_free: must run on a
 * Zend-initialised worker thread. */
void oxphp_pool_slot_free(void *slot_zv_heap) {
    if (!slot_zv_heap) return;
    zval *slot = (zval *)slot_zv_heap;
    zval_ptr_dtor(slot);
    efree(slot);
}

/* Best-effort $destroy($resource) invocation, then slot teardown.
 *
 * Called by `PoolInner::on_shutdown_notify` and `on_drop` on the
 * thread that drains the pool — which may not be the thread that
 * minted the resource, and may not even be inside a live PHP
 * request. We guard both hazards:
 *
 *  - If `destroy_fcc_heap == NULL` (user opted out by passing
 *    `$destroy: null`), we skip invocation entirely.
 *  - If `EG(current_execute_data) == NULL` we skip invocation
 *    too: `zend_call_known_function` requires a request context
 *    (symbol tables, VM stack). The slot-zval is still released
 *    — `zval_ptr_dtor` is refcount arithmetic and safe anywhere
 *    the bridge is loaded.
 *  - If $destroy throws, we serialise the message via
 *    `oxphp_bridge_capture_fatal` (operator-visible via the
 *    thread-local pop path) and clear the exception so the
 *    drain loop can continue. Drain callers have no PHP frame
 *    to propagate into.
 *
 * Always frees `slot_zv_heap` (zval_ptr_dtor + efree). Always
 * returns 0 — best-effort, never signals failure upward. */
int oxphp_pool_destroy_invoke(void *destroy_fcc_heap, void *slot_zv_heap) {
    if (!slot_zv_heap) return 0;
    zval *slot = (zval *)slot_zv_heap;

    if (destroy_fcc_heap && EG(current_execute_data)) {
        oxphp_pool_fcc_t *heap = (oxphp_pool_fcc_t *)destroy_fcc_heap;

        zval args[1];
        ZVAL_COPY(&args[0], slot);

        zval ret_zv;
        ZVAL_UNDEF(&ret_zv);

        zend_call_known_function(heap->fcc.function_handler,
                                  heap->fcc.object,
                                  heap->fcc.called_scope,
                                  &ret_zv,
                                  1, args, NULL);

        zval_ptr_dtor(&args[0]);

        if (EG(exception)) {
            zend_object *ex = EG(exception);
            zend_class_entry *ce = ex->ce;
            zval rv;
            zval *msg_zv = zend_read_property(
                ce, ex, "message", sizeof("message") - 1, 1, &rv);
            if (msg_zv && Z_TYPE_P(msg_zv) == IS_STRING) {
                oxphp_bridge_capture_fatal(
                    Z_STRVAL_P(msg_zv), Z_STRLEN_P(msg_zv));
            }
            zend_clear_exception();
        }

        zval_ptr_dtor(&ret_zv);
    }

    zval_ptr_dtor(slot);
    efree(slot);
    return 0;
}

/* ─── Shared\Pool\Handle rust_data wrapper helpers ──────────────
 * Handle's storage struct (Rust `#[repr(C)] PoolHandleStorage`):
 *   u64    pool_id       @ offset 0
 *   u64    owner_tid     @ offset 8
 *   void * slot_zv_heap  @ offset 16
 * Total: 24 bytes on LP64.
 *
 * The Rust side registers `OxPHP\Shared\Pool\Handle` via
 * `register_class` + `with_storage(PoolHandleStorage::default)` —
 * the storage_factory zero-inits the slot when object_init_ex
 * runs, so a Handle before alloc-fill has pool_id=0 and
 * slot_zv_heap=NULL. We treat NULL slot as "cleared" to make
 * the release / auto-release paths idempotent.
 */

static zend_class_entry *oxphp_pool_handle_ce_lookup(void) {
    zend_string *cname = zend_string_init(
        "OxPHP\\Shared\\Pool\\Handle",
        sizeof("OxPHP\\Shared\\Pool\\Handle") - 1,
        0);
    zend_class_entry *ce = zend_lookup_class_ex(cname, NULL, 0);
    zend_string_release(cname);
    return ce;
}

/* Allocate a new Handle object into out_zv and populate its
 * storage. Returns 0 on success, -1 if the class is not
 * registered or object_init_ex fails. */
int oxphp_shared_pool_handle_alloc(void *out_zv,
                                    uint64_t pool_id,
                                    uint64_t owner_tid,
                                    void *slot_zv_heap) {
    if (!out_zv) return -1;
    zval *out = (zval *)out_zv;

    zend_class_entry *ce = oxphp_pool_handle_ce_lookup();
    if (!ce) return -1;
    if (object_init_ex(out, ce) != SUCCESS) return -1;

    oxphp_custom_object *intern = OXPHP_OBJ(Z_OBJ_P(out));
    if (!intern || !intern->rust_data) {
        zval_ptr_dtor(out);
        return -1;
    }
    unsigned char *storage = (unsigned char *)intern->rust_data;
    memcpy(storage,       &pool_id,       sizeof(uint64_t));
    memcpy(storage + 8,   &owner_tid,     sizeof(uint64_t));
    memcpy(storage + 16,  &slot_zv_heap,  sizeof(void *));
    return 0;
}

/* ─── Shared\Map\KeyCursor rust_data wrapper helper ─────────────
 * KeyCursor's storage struct (Rust `#[repr(C)] KeyCursorStorage`):
 *   void * state  @ offset 0   (Box::into_raw(Box<KeyCursorState>))
 * The storage_factory zero-inits the slot (state = NULL) when
 * object_init_ex runs; this helper stamps the real state pointer. */
static zend_class_entry *oxphp_map_cursor_ce_lookup(void) {
    zend_string *cname = zend_string_init(
        "OxPHP\\Shared\\Map\\KeyCursor",
        sizeof("OxPHP\\Shared\\Map\\KeyCursor") - 1,
        0);
    zend_class_entry *ce = zend_lookup_class_ex(cname, NULL, 0);
    zend_string_release(cname);
    return ce;
}

/* Construct a KeyCursor into out_zv and stamp state_ptr into its
 * rust_data slot (offset 0). Returns 0 on success, -1 if the class is
 * not registered or object_init_ex fails (caller reclaims the box). */
int oxphp_shared_map_cursor_alloc(void *out_zv, void *state_ptr) {
    if (!out_zv) return -1;
    zval *out = (zval *)out_zv;

    zend_class_entry *ce = oxphp_map_cursor_ce_lookup();
    if (!ce) return -1;
    if (object_init_ex(out, ce) != SUCCESS) return -1;

    oxphp_custom_object *intern = OXPHP_OBJ(Z_OBJ_P(out));
    if (!intern || !intern->rust_data) {
        zval_ptr_dtor(out);
        return -1;
    }
    memcpy(intern->rust_data, &state_ptr, sizeof(void *));
    return 0;
}

/* ═══════════════════════════════════════════════════════════
 *  Generic PHP object construction helpers
 *  See oxphp_bridge.h for the contract. Used by Rust handlers
 *  for value-typed return classes (RecvResult / SendResult).
 * ═══════════════════════════════════════════════════════════ */

int oxphp_bridge_make_object(void *out, const char *cls_fqn, size_t cls_len) {
    if (!out || !cls_fqn || cls_len == 0) return -1;
    zend_string *name = zend_string_init(cls_fqn, cls_len, 0);
    zend_class_entry *ce = zend_lookup_class(name);
    zend_string_release(name);
    if (!ce) return -1;
    if (object_init_ex((zval *)out, ce) != SUCCESS) return -1;
    return 0;
}

int oxphp_bridge_object_set_property_long(void *obj,
                                          const char *name,
                                          size_t name_len,
                                          long val) {
    if (!obj) return -1;
    zval *z = (zval *)obj;
    if (Z_TYPE_P(z) != IS_OBJECT) return -1;
    zend_update_property_long(Z_OBJCE_P(z), Z_OBJ_P(z), name, name_len, (zend_long)val);
    return 0;
}

int oxphp_bridge_object_set_property_zval(void *obj,
                                          const char *name,
                                          size_t name_len,
                                          void *src) {
    if (!obj || !src) return -1;
    zval *z = (zval *)obj;
    if (Z_TYPE_P(z) != IS_OBJECT) return -1;
    zend_update_property(Z_OBJCE_P(z), Z_OBJ_P(z), name, name_len, (zval *)src);
    return 0;
}

int oxphp_bridge_get_enum_case(void *out,
                               const char *cls_fqn,
                               size_t cls_len,
                               const char *case_name,
                               size_t case_len) {
    if (!out || !cls_fqn || cls_len == 0 || !case_name || case_len == 0) return -1;
    zend_string *cname = zend_string_init(cls_fqn, cls_len, 0);
    zend_class_entry *ce = zend_lookup_class(cname);
    zend_string_release(cname);
    if (!ce) return -1;
    zend_string *case_str = zend_string_init(case_name, case_len, 0);
    zend_object *case_obj = zend_enum_get_case(ce, case_str);
    zend_string_release(case_str);
    if (!case_obj) return -1;
    /* Enum cases are process-global singletons; bump refcount so the
     * caller's zval owns one strong reference (zval_ptr_dtor will balance). */
    GC_ADDREF(case_obj);
    ZVAL_OBJ((zval *)out, case_obj);
    return 0;
}

int oxphp_bridge_wrap_result_ok_inplace(void *retval,
                                        const char *cls_fqn,
                                        size_t cls_len,
                                        const char *value_prop,
                                        size_t value_prop_len,
                                        const char *status_prop,
                                        size_t status_prop_len,
                                        long status_val) {
    if (!retval || !cls_fqn || cls_len == 0) return -1;

    zend_string *name = zend_string_init(cls_fqn, cls_len, 0);
    zend_class_entry *ce = zend_lookup_class(name);
    zend_string_release(name);
    if (!ce) return -1;

    zval *rv = (zval *)retval;

    /* Snapshot the payload WITHOUT touching its refcount — `tmp` and `rv`
     * briefly alias the same value. */
    zval tmp;
    ZVAL_COPY_VALUE(&tmp, rv);

    /* Overwrite rv with a fresh result object. object_init_ex writes into
     * rv, so the payload now lives only in `tmp`. */
    if (object_init_ex(rv, ce) != SUCCESS) {
        ZVAL_COPY_VALUE(rv, &tmp); /* restore; caller still owns the payload */
        return -1;
    }

    /* value_prop = payload. zend_update_property does ZVAL_DEREF + ZVAL_COPY
     * (refcount++ on the payload). */
    zend_update_property(ce, Z_OBJ_P(rv), value_prop, value_prop_len, &tmp);

    /* Balance the reference we moved out of rv: the property now holds its
     * own ref, so drop ours. Net payload refcount is unchanged across the
     * whole call. */
    zval_ptr_dtor(&tmp);

    zend_update_property_long(ce, Z_OBJ_P(rv), status_prop, status_prop_len,
                              (zend_long)status_val);
    return 0;
}
