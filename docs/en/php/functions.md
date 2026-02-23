---
title: PHP Extension Functions
description: API reference for the oxphp_sapi PHP extension
---

The `oxphp_sapi` PHP extension registers eight built-in functions that give your PHP code access to OxPHP server internals. These functions are available in every PHP script executed by OxPHP --- no `extension=` directive is needed because the extension is compiled into the custom SAPI.

Plugins can register additional PHP functions at startup. These plugin-provided functions are registered during `MINIT` via the C bridge and appear alongside the built-in ones.

## `oxphp_request_id`

Returns the unique request identifier assigned to the current request.

```php
oxphp_request_id(): string
```

**Parameters:** None.

**Return value:** A 16-character hexadecimal string. The format is `{timestamp_hex}{counter_hex}`, where the first 8 characters are the Unix timestamp and the last 8 are a monotonic counter. Returns an empty string if no request ID has been set (should not happen during normal request processing).

**Example:**

```php
<?php
$requestId = oxphp_request_id();
// "67a3b1c400000042"

header("X-Request-Id: $requestId");

// Use in application logging
error_log("[$requestId] Processing payment for order #1234");
```

**Notes:**
- The request ID is set by the server before PHP execution begins.
- The same ID is available on the Rust side for access logging and response headers.
- The ID is unique within a single server process lifetime.

---

## `oxphp_worker_id`

Returns the index of the PHP worker thread handling the current request.

```php
oxphp_worker_id(): int
```

**Parameters:** None.

**Return value:** A zero-based integer identifying the worker thread. Values range from `0` to `PHP_WORKERS - 1` for static workers. Dynamic workers receive IDs above the initial range.

**Example:**

```php
<?php
$workerId = oxphp_worker_id();
// 3

// Use for worker-specific temp directories or debugging
$tmpDir = "/tmp/oxphp-worker-$workerId";
```

**Notes:**
- Worker IDs are stable for the lifetime of the worker thread.
- In dynamic scaling mode, workers spawned after startup receive incrementing IDs above the initial pool size.
- Useful for debugging concurrency issues or partitioning worker-specific resources.

---

## `oxphp_server_info`

Returns an associative array with server and worker metadata.

```php
oxphp_server_info(): array
```

**Parameters:** None.

**Return value:** An associative array with the following keys:

| Key | Type | Description |
|-----|------|-------------|
| `sapi` | `string` | Always `"oxphp"` |
| `version` | `string` | Server version (currently `"0.1.0"`) |
| `worker_id` | `int` | Same value as `oxphp_worker_id()` |
| `request_time` | `float` | Unix timestamp with microsecond precision when the request started |

**Example:**

```php
<?php
$info = oxphp_server_info();
// [
//     "sapi"         => "oxphp",
//     "version"      => "0.1.0",
//     "worker_id"    => 3,
//     "request_time" => 1738800000.123456,
// ]

// Calculate elapsed time
$elapsed = microtime(true) - $info['request_time'];
echo "Request processing took {$elapsed}s so far";
```

**Notes:**
- `request_time` reads from the C bridge library's thread-local storage, which is set before `php_request_startup()`.
- This value is also used by OPcache's `file_update_protection` check.

---

## `oxphp_request_heartbeat`

Placeholder for a future watchdog timeout extension mechanism. Currently a no-op.

```php
oxphp_request_heartbeat(int $time = 10): bool
```

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `$time` | `int` | `10` | Requested timeout extension in seconds |

**Return value:** Always returns `true` in the current implementation.

**Example:**

```php
<?php
// Long-running data import
foreach ($records as $record) {
    process($record);
    oxphp_request_heartbeat(30); // Signal that the script is still alive
}
```

**Notes:**
- This function exists to provide forward compatibility. In a future release, it will reset the request timeout watchdog timer.
- Calling it today is safe and has no performance cost.

---

## `oxphp_finish_request`

Marks the current request as finished, allowing the server to send the response to the client while the PHP script continues executing in the background. This is the OxPHP equivalent of `fastcgi_finish_request()`.

```php
oxphp_finish_request(): bool
```

**Parameters:** None.

**Return value:** Returns `true` on the first call. Returns `false` if the request was already finished (i.e., the function was called more than once in the same request).

**Example:**

```php
<?php
// Send response to client immediately
echo json_encode(['status' => 'accepted']);
oxphp_finish_request();

// Continue with background work — the client has already received the response
send_notification_email($userId);
update_analytics($eventData);
cleanup_temp_files();
```

**Notes:**
- After calling this function, any further output from `echo` or `print` is discarded from the client response.
- The PHP worker thread remains occupied until the script finishes executing, so long background tasks reduce the available worker pool.
- Calling this function twice in the same request returns `false` on the second call.

---

## `oxphp_is_streaming`

Checks whether the current request is in streaming mode.

```php
oxphp_is_streaming(): bool
```

**Parameters:** None.

**Return value:** `true` if streaming mode is active, `false` otherwise.

**Example:**

```php
<?php
if (oxphp_is_streaming()) {
    // Flush output incrementally
    echo "data: " . json_encode($event) . "\n\n";
    flush();
} else {
    // Buffer the complete response
    echo json_encode($allData);
}
```

**Notes:**
- Streaming mode is activated automatically when the `Content-Type: text/event-stream` header is set, or manually via `oxphp_stream_flush()`.
- This function is useful for scripts that need to adapt their output behavior depending on the transport mode.

---

## `oxphp_stream_flush`

Activates streaming mode (if not already active) and flushes the current output buffer to the client as a chunk. This is the primary function for implementing Server-Sent Events (SSE) in OxPHP.

```php
oxphp_stream_flush(): bool
```

**Parameters:** None.

**Return value:** Returns `true` on success. Returns `false` if the request has already been finished via `oxphp_finish_request()`.

**Example:**

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

for ($i = 0; $i < 10; $i++) {
    echo "id: $i\n";
    echo "data: " . json_encode(['counter' => $i, 'time' => microtime(true)]) . "\n\n";
    oxphp_stream_flush();
    sleep(1);
}
```

**How it works:**

1. On the first call, activates streaming mode via the C bridge (`oxphp_bridge_set_stream_mode`)
2. Flushes all PHP output buffers (`php_output_flush_all`)
3. Triggers the SAPI flush callback, which sends buffered output as an HTTP chunk to the client

**Notes:**
- Headers are sent to the client on the first flush. Subsequent calls only send body chunks.
- You can also use native PHP `flush()` with `Content-Type: text/event-stream` — OxPHP auto-detects the SSE content type and activates streaming mode. In that case, call `ob_end_flush()` first to disable PHP's output buffering layer.
- If `oxphp_finish_request()` was called before, this function returns `false` and does nothing.
- The HTTP connection closes automatically when the PHP script ends and the streaming channel is closed.
- Backpressure is applied via a bounded channel (capacity 64) — if the client is slow, `oxphp_stream_flush()` blocks until the client catches up.

---

## `oxphp_worker`

Enters the persistent worker mode loop. Calls the provided handler callback for each incoming HTTP request. Between requests, a soft reset cleans per-request state (superglobals, output buffers, `$_SESSION`) without destroying the PHP heap, so bootstrap state (autoloaders, database connections, cached config) persists across requests.

```php
oxphp_worker(callable $handler): bool
```

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `$handler` | `callable` | A callback invoked once per HTTP request. Receives no arguments. |

**Return value:** Returns `true` on graceful shutdown (server stopping). Returns `false` immediately if worker mode is not enabled (i.e., `WORKER_FILE` is not set).

**Example:**

```php
<?php
// worker.php — persistent worker entry point

// Bootstrap: runs once per worker lifetime
require __DIR__ . '/vendor/autoload.php';
$db = new PDO('mysql:host=localhost;dbname=app', 'root', '');
$config = json_decode(file_get_contents(__DIR__ . '/config.json'), true);

// Handle requests in a loop
oxphp_worker(function () use ($db, $config) {
    $uri = $_SERVER['REQUEST_URI'];
    $method = $_SERVER['REQUEST_METHOD'];

    // Route and handle the request
    if ($uri === '/api/users' && $method === 'GET') {
        $users = $db->query('SELECT id, name FROM users')->fetchAll();
        header('Content-Type: application/json');
        echo json_encode($users);
    } else {
        http_response_code(404);
        echo 'Not Found';
    }
});
```

**How it works:**

1. The handler callback is called for each HTTP request received from the Rust layer.
2. Between requests, a soft reset occurs:
   - Superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`) are repopulated with the new request data
   - Output buffers are cleared
   - HTTP response headers are reset
   - Shutdown functions registered with `register_shutdown_function()` are called and cleared
3. Garbage collection runs periodically (every 100 requests) to reclaim cyclic references without impacting per-request latency.
4. The loop exits when:
   - The server shuts down (graceful shutdown signal)
   - The handler throws an uncaught exception or fatal error (exit reason: `error`)
   - The worker hits `WORKER_MAX_REQUESTS` (exit reason: `max_requests`)
   - The worker exceeds `WORKER_MAX_MEMORY` (exit reason: `max_memory`)

**Notes:**
- This function only works when `WORKER_FILE` is set. Calling it from a regular PHP script emits an `E_WARNING` and returns `false`.
- Variables declared outside the handler closure persist across requests. Use this for database connections, configuration, and other expensive initialization.
- The handler's `use` clause captures variables by reference or value as usual. Variables captured by reference share state across requests.
- Worker recycling (via `WORKER_MAX_REQUESTS` or `WORKER_MAX_MEMORY`) causes the worker process to exit and respawn, which re-executes the entire worker script including bootstrap.
- Worker mode metrics (`oxphp_worker_requests_handled_total`, `oxphp_worker_recycles_total`, etc.) are available on the `/metrics` endpoint when the internal server is running.

---

## Plugin Functions

Plugins can register custom PHP functions that are callable from your scripts. These functions are registered during PHP module initialization (`MINIT`) and dispatched through the C bridge to Rust handler code.

Plugin functions use the native bridge for zero-serialization dispatch. Arguments and return values are passed as raw `zval` pointers — Rust reads and writes them directly through C accessor functions, with no JSON encoding overhead. If the handler returns an error, a PHP `E_WARNING` is emitted and `NULL` is returned.

```php
<?php
// Example: calling a plugin-registered function
$result = some_plugin_function('arg1', 42, ['key' => 'value']);
```

Plugin functions are listed alongside built-in functions in `phpinfo()` output, but they are registered globally (not under the `oxphp_sapi` extension), so they do not appear in `get_extension_funcs('oxphp_sapi')`.

## Extension Information

The extension metadata is visible in `phpinfo()` output:

| Field | Value |
|-------|-------|
| Extension name | `oxphp_sapi` |
| Version | `0.1.0` |

You can verify the extension is loaded:

```php
<?php
var_dump(extension_loaded('oxphp_sapi'));
// bool(true)

// List built-in extension functions
print_r(get_extension_funcs('oxphp_sapi'));
// Array
// (
//     [0] => oxphp_request_id
//     [1] => oxphp_worker_id
//     [2] => oxphp_server_info
//     [3] => oxphp_request_heartbeat
//     [4] => oxphp_finish_request
//     [5] => oxphp_is_streaming
//     [6] => oxphp_stream_flush
//     [7] => oxphp_worker
// )
```

## See Also

- [Superglobals](superglobals.md) --- how OxPHP populates `$_SERVER`, `$_GET`, `$_POST`, and other superglobals
- [OPcache Compatibility](opcache.md) --- how the `request_time` callback enables OPcache
- [Request IDs](/features/request-ids.md) --- how request IDs are generated and propagated
- [SAPI Bridge](/architecture/sapi-bridge.md) --- the C bridge that connects Rust and PHP
- [Worker Pool](/architecture/worker-pool.md#worker-mode-persistent-php) --- worker mode architecture, recycling, and metrics
- [Configuration](/operations/configuration.md#worker-mode) --- `WORKER_FILE`, `WORKER_MAX_REQUESTS`, `WORKER_MAX_MEMORY`
