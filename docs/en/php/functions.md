---
title: PHP Extension Functions
description: API reference for the oxphp_sapi PHP extension
---

The `oxphp_sapi` PHP extension registers six built-in functions that give your PHP code access to OxPHP server internals. These functions are available in every PHP script executed by OxPHP --- no `extension=` directive is needed because the extension is compiled into the custom SAPI.

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
- Streaming mode is controlled by the server runtime, not by PHP code.
- This function is primarily useful for scripts that need to adapt their output behavior depending on the transport mode.

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
// )
```

## See Also

- [Superglobals](superglobals.md) --- how OxPHP populates `$_SERVER`, `$_GET`, `$_POST`, and other superglobals
- [OPcache Compatibility](opcache.md) --- how the `request_time` callback enables OPcache
- [Request IDs](/features/request-ids.md) --- how request IDs are generated and propagated
- [SAPI Bridge](/architecture/sapi-bridge.md) --- the C bridge that connects Rust and PHP
