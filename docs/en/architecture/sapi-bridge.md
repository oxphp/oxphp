---
title: SAPI and Bridge
description: OxPHP's custom PHP SAPI, C bridge library with __thread TLS, and PHP extension API
---

OxPHP uses a custom SAPI (Server API) to integrate with PHP, rather than the standard `php-embed` SAPI. A shared C bridge library provides the mechanism for sharing per-request state between the Rust binary and the PHP extension. This page explains why this architecture exists and how the components interact.

## Why a Custom SAPI?

PHP's SAPI layer is the interface between the web server and the PHP engine. Standard SAPIs (cli, fpm, embed) make assumptions about process lifecycle that do not fit OxPHP's model:

- **php-embed** expects one request per process. It does not support concurrent request handling on multiple threads.
- **php-fpm** is a separate process manager. OxPHP eliminates the need for inter-process communication.
- **php-cli** has no HTTP integration.

OxPHP registers its own `sapi_module_struct` with the name `"oxphp"`. This gives full control over:

- Output capture (intercepting PHP's output buffer)
- Header handling (collecting `header()` calls)
- `php://input` (providing the request body)
- `$_SERVER` population (setting superglobals from Rust-side request data)
- Request timing (via `sapi_get_request_time`)

## The Bridge Problem

When OxPHP's Rust binary is compiled, it links against `libphp.so`. PHP extensions are loaded by `libphp.so` at runtime via `dlopen()`. This creates a visibility problem:

```
┌────────────────────┐         ┌───────────────────┐
│  Rust Binary       │         │  libphp.so        │
│                    │ links   │                   │
│  thread_local! {   │────────▶│  dlopen() ───────▶│ oxphp_sapi.so
│    // Rust TLS     │         │                   │  (PHP extension)
│  }                 │         └───────────────────┘
└────────────────────┘                             │
                                                   │
  Rust thread_local! vars are INVISIBLE            │
  to dlopen'd shared libraries ──────────────────▶ │
```

Rust's `thread_local!` macro uses ELF TLS or a platform-specific mechanism that is resolved at link time. Shared libraries loaded via `dlopen()` at runtime cannot see these symbols. This means a PHP extension cannot directly read request data that Rust stores in thread-local storage.

## The Bridge Library

The solution is `liboxphp_bridge.so` — a small C shared library that both the Rust binary and the PHP extension link against. It uses C `__thread` TLS, which is visible to all `dlopen`'d libraries sharing the same address space.

```
┌────────────────────┐
│  Rust Binary       │──links──┐
└────────────────────┘         │
                               ▼
                    ┌──────────────────────┐
                    │  liboxphp_bridge.so  │
                    │                      │
                    │  static __thread     │
                    │    oxphp_ctx_t ctx;  │
                    │                      │
                    │  static (global)     │
                    │    plugin_functions  │
                    │    dispatch_fn       │
                    │    call_php_fn       │
                    └──────────────────────┘
                               ▲
┌────────────────────┐         │
│  oxphp_sapi.so     │──links──┘
│  (PHP extension)   │
└────────────────────┘
```

Both the Rust binary and the PHP extension call functions in `liboxphp_bridge.so` to read and write the same `__thread` variable. Since they are in the same process and on the same OS thread, they share the same TLS slot.

### Bridge Context

The per-request context is defined in `ext/bridge/oxphp_bridge.h`:

```c
typedef struct {
    char request_id[65];    // Hex request ID (64 chars + null)
    int32_t worker_id;      // Worker thread index
    double request_time;    // Unix epoch, microseconds
    bool stream_mode;       // Streaming mode active
    bool headers_sent;      // Headers sent (streaming)
    bool finished;          // oxphp_finish_request() called
} oxphp_ctx_t;
```

### Bridge API

The bridge exposes getter/setter functions that operate on the `__thread`-local `ctx` variable:

| Function | Purpose |
|---|---|
| `oxphp_bridge_init_ctx()` | Zero-initialize the context (call before `php_request_startup`) |
| `oxphp_bridge_clear_ctx()` | Zero the context after request shutdown |
| `oxphp_bridge_get_ctx()` | Get a pointer to the context struct |
| `oxphp_bridge_set_request_id(id)` | Copy a request ID (up to 64 chars) |
| `oxphp_bridge_get_request_id()` | Get the request ID pointer |
| `oxphp_bridge_set_worker_id(id)` | Set the worker thread index |
| `oxphp_bridge_set_request_time(time)` | Set the request start time |
| `oxphp_bridge_get_request_time()` | Get the request start time |
| `oxphp_bridge_set_stream_mode(mode)` | Enable/disable streaming mode |
| `oxphp_bridge_is_streaming()` | Check if streaming is active |
| `oxphp_bridge_set_finished(bool)` | Mark request as finished |
| `oxphp_bridge_is_finished()` | Check if request is finished |
| `oxphp_bridge_set_headers_sent(bool)` | Mark headers as sent |
| `oxphp_bridge_get_headers_sent()` | Check if headers were sent |

The implementation in `ext/bridge/oxphp_bridge.c` is straightforward — each function reads or writes a field on the `static __thread oxphp_ctx_t ctx` variable.

### Critical Invariant

**`init_ctx()` and `set_request_time()` must be called BEFORE `php_request_startup()`.**

OPcache's RINIT handler reads `sapi_get_request_time()` during `php_request_startup()`. The custom SAPI's `sapi_get_request_time` callback reads from the bridge context. If the bridge returns 0 (uninitialized), OPcache's `file_update_protection` check fails, resulting in a 0% cache hit rate.

The correct call order on each worker thread:

```
1. oxphp_bridge_init_ctx()
2. oxphp_bridge_set_request_id(...)
3. oxphp_bridge_set_request_time(...)
4. sapi::set_request_data(request)    // server vars, cookies, body
5. php_request_startup()              // triggers RINIT for all extensions
6. php_execute_script(...)
7. php_request_shutdown()
8. oxphp_bridge_clear_ctx()
```

## Plugin Function Registry

The bridge also provides a **global** (not `__thread`) plugin function registry. This allows Rust plugins to register functions that PHP scripts can call, and PHP functions that Rust can call.

### Registry API

| Function | Purpose |
|---|---|
| `oxphp_bridge_register_plugin_fn(name, required, total)` | Register a plugin function (called by Rust during startup) |
| `oxphp_bridge_get_plugin_fn_count()` | Get number of registered plugin functions |
| `oxphp_bridge_get_plugin_fn_name(index)` | Get plugin function name by index |
| `oxphp_bridge_get_plugin_fn_required(index)` | Get required param count by index |
| `oxphp_bridge_get_plugin_fn_total(index)` | Get total param count by index |
| `oxphp_bridge_set_dispatch_fn(fn)` | Set the Rust dispatch callback |
| `oxphp_bridge_get_dispatch_fn()` | Get the Rust dispatch callback |
| `oxphp_bridge_set_call_php_fn(fn)` | Set the PHP call callback |
| `oxphp_bridge_get_call_php_fn()` | Get the PHP call callback |
| `oxphp_bridge_dispatch(name, json_args)` | Dispatch to Rust handler |
| `oxphp_bridge_call_php(name, json_args)` | Call a PHP function from Rust |
| `oxphp_bridge_strdup(s)` | Duplicate a string using C `malloc` |
| `oxphp_bridge_free_string(ptr)` | Free a string allocated by `strdup` |

The registry is global (not per-thread) because it is written once from the main thread during startup and read during MINIT — no concurrent access. It is never freed; it lives for the entire process lifetime.

### Cross-Boundary Data Format

All cross-boundary function calls use a JSON envelope:

- **Arguments**: JSON-encoded array of parameters
- **Success result**: `{"ok": value}`
- **Error result**: `{"err": "message"}`

The `oxphp_bridge_strdup`/`oxphp_bridge_free_string` pair uses C's `malloc`/`free` to avoid allocator mismatch between Rust and the C library.

## PHP Extension

The PHP extension (`ext/oxphp_sapi.c`) exposes server-specific functions to PHP scripts. It links against `liboxphp_bridge.so` to read the bridge context.

### Available Functions

| Function | Return Type | Description |
|---|---|---|
| `oxphp_request_id()` | `string` | Returns the hex request ID for the current request |
| `oxphp_worker_id()` | `int` | Returns the worker thread index (0-based) |
| `oxphp_server_info()` | `array` | Returns `sapi`, `version`, `worker_id`, `request_time` |
| `oxphp_request_heartbeat(int $time = 10)` | `bool` | Placeholder for timeout extension (currently returns `true`) |
| `oxphp_finish_request()` | `bool` | Marks the request as finished for background processing |
| `oxphp_is_streaming()` | `bool` | Checks if the current request uses streaming mode |

### Plugin Dispatch Function

The extension also registers `oxphp_plugin_dispatch` — a generic handler for all plugin-registered functions. When a PHP script calls a plugin function (e.g., `oxphp_debug_info()`), the Zend engine dispatches to `oxphp_plugin_dispatch`, which:

1. Reads the function name from `execute_data->func->common.function_name`
2. Collects all arguments into a PHP array and `json_encode`s them
3. Calls `oxphp_bridge_dispatch(func_name, json_args)` to invoke the Rust handler
4. Parses the JSON envelope result (`{"ok": value}` or `{"err": "message"}`)
5. Returns the `ok` value or emits a warning for `err`

### Call PHP from Rust

The extension provides `oxphp_sapi_call_php()` — a callback that Rust can invoke via the bridge to call PHP functions:

1. Rust calls `oxphp_bridge_call_php(func_name, json_args)`
2. The bridge invokes `call_php_fn`, which is set to `oxphp_sapi_call_php` during MINIT
3. `oxphp_sapi_call_php` decodes the JSON args, calls `call_user_function()`, and returns a JSON envelope

### Example Usage

```php
<?php
// Get the request ID assigned by the server
$requestId = oxphp_request_id();
header("X-Debug-Worker: " . oxphp_worker_id());

// Examine SAPI details
$info = oxphp_server_info();
// $info = [
//     'sapi' => 'oxphp',
//     'version' => '0.1.0',
//     'worker_id' => 3,
//     'request_time' => 1707609600.123456,
// ]

// Finish the response but continue processing
oxphp_finish_request();
// ... background work here (logging, cleanup, etc.)
```

### Extension Registration

The extension is registered as a standard PHP module with a MINIT hook that sets up the plugin function bridge:

```c
zend_module_entry oxphp_sapi_module_entry = {
    STANDARD_MODULE_HEADER,
    "oxphp_sapi",
    oxphp_sapi_functions,
    PHP_MINIT(oxphp_sapi),  // sets call_php callback, registers plugin fns
    NULL,                    // MSHUTDOWN
    NULL,                    // RINIT
    NULL,                    // RSHUTDOWN
    PHP_MINFO(oxphp_sapi),
    "0.1.0",
    STANDARD_MODULE_PROPERTIES
};
```

**MINIT** performs two tasks:

1. Sets `oxphp_bridge_set_call_php_fn(oxphp_sapi_call_php)` so Rust can call PHP functions
2. Reads the plugin function registry from the bridge and registers each function with Zend via `zend_register_functions()` — this must happen at module startup (not request startup) so OPcache's compile-time `function_exists()` optimization can see the functions

## Data Flow Summary

```
Rust (Tokio task)                     PHP Worker Thread
─────────────────                     ──────────────────
ScriptRequest ──sync_channel──▶ recv()
                                      │
                                      ├── bridge::init_ctx()
                                      ├── bridge::set_request_id()
                                      ├── bridge::set_request_time()
                                      ├── sapi::set_request_data()
                                      │     ├── server vars → TLS
                                      │     ├── cookies → TLS
                                      │     └── body → TLS
                                      │
                                      ├── php_request_startup()
                                      │     ├── RINIT for all extensions
                                      │     └── OPcache reads request_time
                                      │
                                      ├── php_execute_script()
                                      │     ├── PHP reads $_SERVER, $_GET, etc.
                                      │     ├── PHP calls oxphp_request_id()
                                      │     │     └── bridge::get_request_id()
                                      │     ├── PHP calls plugin function
                                      │     │     └── bridge::dispatch() → Rust
                                      │     └── Output captured by SAPI
                                      │
                                      ├── php_request_shutdown()
                                      │
                                      ├── sapi::take_response()
                                      │     ├── output buffer
                                      │     ├── response headers
                                      │     └── status code
                                      │
                                      └── bridge::clear_ctx()
                                      │
ScriptResponse ◀──oneshot──────────── tx.send()
```

## Building the Bridge and Extension

The bridge library and PHP extension are built as part of the Docker image. For local development:

```bash
# Build the bridge library
cd ext/bridge
make
sudo make install  # installs liboxphp_bridge.so

# Build the PHP extension
cd ext
phpize
./configure --enable-oxphp-sapi
make
sudo make install  # installs oxphp_sapi.so
```

Both artifacts must be available at runtime:
- `liboxphp_bridge.so` in the library search path (`LD_LIBRARY_PATH=/usr/local/lib`)
- `oxphp_sapi.so` in the PHP extensions directory (or loaded via `extension=oxphp_sapi.so` in `php.ini`)

## See Also

- [Architecture Overview](./overview.md) — Component map and startup sequence
- [Worker Pool](./worker-pool.md) — Worker thread lifecycle that calls the bridge
- [Request Lifecycle](./request-lifecycle.md) — Full request pipeline from TCP to response
- [PHP Functions](../php/functions.md) — PHP-callable functions reference
- [Superglobals](../php/superglobals.md) — How `$_SERVER`, `$_GET`, etc. are populated
- [OPcache](../php/opcache.md) — OPcache integration and the `request_time` invariant
