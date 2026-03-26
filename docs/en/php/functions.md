---
title: PHP Functions
description: Complete reference for all oxphp_* PHP functions provided by OxPHP, including async, streaming, worker mode, and decorator APIs.
---

# PHP Functions

OxPHP registers its functions through the `oxphp_sapi` extension, which loads automatically for every PHP script the server executes. No `extension=` directive and no manual loading are required — every function listed here is available from the first line of your PHP code.

## Table of Contents

- [oxphp_http_request()](#oxphp_http_request)
- [oxphp_superglobals_enabled()](#oxphp_superglobals_enabled)
- [oxphp_request_id()](#oxphp_request_id)
- [oxphp_worker_id()](#oxphp_worker_id)
- [oxphp_server_info()](#oxphp_server_info)
- [oxphp_request_heartbeat()](#oxphp_request_heartbeat)
- [oxphp_finish_request()](#oxphp_finish_request)
- [oxphp_is_worker()](#oxphp_is_worker)
- [oxphp_worker()](#oxphp_worker)
- [oxphp_is_streaming()](#oxphp_is_streaming)
- [oxphp_stream_flush()](#oxphp_stream_flush)
- [oxphp_sleep()](#oxphp_sleep)
- [oxphp_usleep()](#oxphp_usleep)
- [oxphp_async()](#oxphp_async)
- [oxphp_async_await()](#oxphp_async_await)
- [oxphp_async_await_all()](#oxphp_async_await_all)
- [oxphp_async_await_any()](#oxphp_async_await_any)
- [oxphp_register_decorator()](#oxphp_register_decorator)
- [Classes and Interfaces](#classes-and-interfaces)
- [Exceptions](#exceptions)

---

## oxphp_http_request()

```php
oxphp_http_request(): \OxPHP\Http\Request
```

Returns the request object for the current HTTP request. The object provides typed access to the HTTP method, URI, query parameters, parsed body, headers, cookies, uploaded files, client IP, and request timing.

**Returns:** An `\OxPHP\Http\Request` instance backed by the request data in the current PHP worker thread.

**Throws:** An exception from the `OxPHP\Http\Exception` namespace when called outside an active request:

| Exception | Situation |
|-----------|-----------|
| `\OxPHP\Http\Exception\WorkerIdleException` | Worker mode, between requests |
| `\OxPHP\Http\Exception\AsyncContextException` | Inside an `oxphp_async()` callback |
| `\OxPHP\Http\Exception\NoActiveRequestException` | Any other context without an active request |

In normal request-handling code, no exception handling is required.

**Example:**

```php
<?php
$request = oxphp_http_request();

$method  = $request->method();             // "POST"
$path    = $request->path();               // "/api/users"
$email   = $request->payload('email');     // from JSON or form body
$token   = $request->header('Authorization');
$theme   = $request->cookie('theme', 'light');
```

For the complete interface reference, see the [HTTP Request API](request-api.md) documentation.

---

## oxphp_superglobals_enabled()

```php
oxphp_superglobals_enabled(): bool
```

Returns whether superglobal population is enabled for this server instance. The value reflects the `SUPERGLOBALS_ENABLED` environment variable and does not change during the server's lifetime.

When `false`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, and `$_SERVER` are empty arrays. The HTTP Object API (`oxphp_http_request()`), `php://input`, and PHP session functions are unaffected.

**Returns:** `true` when `SUPERGLOBALS_ENABLED` is `true` (the default), `false` otherwise.

**Example:**

```php
<?php
if (oxphp_superglobals_enabled()) {
    $query = $_GET['page'] ?? 1;
} else {
    $query = oxphp_http_request()->query('page', 1);
}
```

---

## oxphp_request_id()

```php
oxphp_request_id(): string
```

Returns the unique request identifier for the current request. This is the same value sent in the `X-Request-ID` response header. If the client sends an `X-Request-ID` header, OxPHP passes it through unchanged instead of generating a new one.

**Returns:** A 16-character hexadecimal string when OxPHP generates the ID (e.g. `"67b9a3c11a2b0042"`). When the client sends an `X-Request-ID` header, that value is returned as-is (1–64 characters, alphanumeric plus `-`, `_`, `.`).

**Example:**

```php
<?php
$id = oxphp_request_id();
error_log("[$id] Processing order #1234");

// Propagate the ID to downstream services
header("X-Correlation-ID: $id");
```

---

## oxphp_worker_id()

```php
oxphp_worker_id(): int
```

Returns the zero-based index of the PHP worker thread handling the current request. Worker indices range from `0` to `PHP_WORKERS - 1`.

**Returns:** An integer identifying the current worker thread.

**Example:**

```php
<?php
$workerId = oxphp_worker_id();

// Use per-worker temp files to avoid collisions
$tmp = "/tmp/worker_{$workerId}_buffer.dat";

error_log("Worker $workerId handling request");
```

---

## oxphp_server_info()

```php
oxphp_server_info(): array
```

Returns an associative array with server and request metadata.

**Returns:** An array with the following keys:

| Key | Type | Description |
|-----|------|-------------|
| `sapi` | `string` | Always `"oxphp"` |
| `version` | `string` | Server version (e.g. `"0.1.0"`) |
| `worker_id` | `int` | Same value as `oxphp_worker_id()` |
| `request_time` | `float` | Unix timestamp with microsecond precision when the request started |
| `worker_mode` | `bool` | Whether the current process runs in worker mode |

**Example:**

```php
<?php
$info = oxphp_server_info();
// [
//     "sapi"         => "oxphp",
//     "version"      => "0.1.0",
//     "worker_id"    => 3,
//     "request_time" => 1738800000.123456,
//     "worker_mode"  => true,
// ]

$elapsed = microtime(true) - $info['request_time'];
echo "Processing took {$elapsed}s so far";
```

---

## oxphp_finish_request()

```php
oxphp_finish_request(): bool
```

Flushes the response to the client and continues PHP execution in the background. The client receives the complete HTTP response immediately; the script keeps running until it exits naturally. This is the OxPHP equivalent of `fastcgi_finish_request()` in PHP-FPM.

**Returns:** `true` on success, `false` if already called on this request.

> **Note:** The PHP worker thread remains occupied until the script finishes. Keep background work short or offload heavy processing to a queue.

**Example:**

```php
<?php
http_response_code(202);
echo json_encode(['status' => 'accepted']);
oxphp_finish_request();

// The client already has its 202 response; continue working
send_notification_email($user);
update_analytics($event);
```

---

## oxphp_request_heartbeat()

```php
oxphp_request_heartbeat(int $time = 10): bool
```

Extends the `REQUEST_TIMEOUT_SECONDS` deadline by `$time` seconds from the moment of the call. Call this periodically in long-running loops to prevent OxPHP from killing the request mid-processing.

**Parameters:**
- `$time` — Seconds to extend the timeout deadline by. Default: `10`

**Returns:** `true` on success, `false` if `$time` is zero or negative.

> **Note:** Each call sets a new deadline relative to the current time, not the original request start. Calling `oxphp_request_heartbeat(30)` at the 100-second mark of a request sets the deadline 30 seconds from now (130 seconds from request start).

**Example:**

```php
<?php
foreach ($large_dataset as $row) {
    oxphp_request_heartbeat(30); // extend by 30 seconds from now
    process($row);
}
```

---

## oxphp_is_worker()

```php
oxphp_is_worker(): bool
```

Returns whether the server is running in worker mode. Worker mode activates when `WORKER_FILE` is set.

**Returns:** `true` if running in worker mode, `false` in traditional mode.

**Example:**

```php
<?php
if (oxphp_is_worker()) {
    // Reuse persistent connections across requests
    $db = $GLOBALS['db'] ??= new PDO($dsn);
} else {
    // Traditional mode: create a new connection per request
    $db = new PDO($dsn);
}
```

---

## oxphp_worker()

```php
oxphp_worker(callable $handler): bool
```

Enters the persistent worker mode loop. OxPHP calls `$handler` once for each incoming HTTP request. Between requests, a soft reset clears per-request state — output buffers, headers, and superglobals — without destroying the PHP heap, so any variables declared outside the handler persist across requests.

**Parameters:**
- `$handler` — Called once per request. The handler receives no arguments; use superglobals (`$_SERVER`, `$_GET`, `$_POST`, etc.) for request data.

**Returns:** `true` on graceful shutdown, `false` if not in worker mode.

The worker loop exits when any of the following conditions are met:
- The server shuts down gracefully
- The handler raises 3 consecutive uncaught exceptions or fatal errors
- The worker reaches `WORKER_MAX_REQUESTS`
- The worker exceeds `WORKER_MAX_MEMORY_MIB`

> **Note:** `oxphp_worker()` only works when `WORKER_FILE` is configured. In traditional mode it logs a warning and returns `false`.

**Example:**

```php
<?php
// worker.php — runs once per worker process lifetime

// Bootstrap: executed once on startup
require __DIR__ . '/vendor/autoload.php';
$app = new App();

// Handle requests in a loop
oxphp_worker(function () use ($app) {
    $app->handle();
});

// Code after oxphp_worker() runs during shutdown
$app->terminate();
```

---

## oxphp_is_streaming()

```php
oxphp_is_streaming(): bool
```

Returns whether the current request is in streaming mode. Streaming mode activates on the first call to `oxphp_stream_flush()` or automatically when PHP sets `Content-Type: text/event-stream`.

**Returns:** `true` if streaming mode is active, `false` otherwise.

**Example:**

```php
<?php
if (oxphp_is_streaming()) {
    echo "data: " . json_encode($event) . "\n\n";
    oxphp_stream_flush();
} else {
    echo json_encode($allData);
}
```

---

## oxphp_stream_flush()

```php
oxphp_stream_flush(): bool
```

Activates streaming mode and flushes any buffered output to the client as an HTTP chunk. On the first call, HTTP headers are sent immediately and streaming begins. Each subsequent call flushes output written since the last flush.

**Returns:** `true` on success, `false` if `oxphp_finish_request()` was already called.

> **Note:** Streaming mode also activates automatically when PHP sets `Content-Type: text/event-stream`. In that case you can use PHP's built-in `flush()`, but call `ob_end_flush()` first to bypass PHP's output buffering layer.

**Example:**

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

for ($i = 0; $i < 10; $i++) {
    echo "id: $i\n";
    echo "data: " . json_encode(['counter' => $i]) . "\n\n";
    oxphp_stream_flush();
    oxphp_sleep(1.0); // use oxphp_sleep instead of sleep — does not block the worker in fiber mode
}
```

---

## oxphp_sleep()

```php
oxphp_sleep(float $seconds): void
```

Sleeps for the specified duration. Inside a worker mode handler running in a fiber, this call is cooperative — it suspends the current fiber so other requests can be processed during the wait. Outside a fiber, it falls back to a standard blocking `usleep()`.

**Parameters:**
- `$seconds` — Duration to sleep in seconds. Fractional values are accepted (e.g. `0.5` for 500 milliseconds). Values of `0` or less return immediately.

**Returns:** `void`

**Example:**

```php
<?php
oxphp_worker(function () {
    // In worker mode with fiber multiplexing:
    // this suspends the fiber rather than blocking the thread
    oxphp_sleep(1.0);
    echo json_encode(['done' => true]);
});
```

---

## oxphp_usleep()

```php
oxphp_usleep(int $microseconds): void
```

Sleeps for the specified number of microseconds. Like `oxphp_sleep()`, this is cooperative inside a fiber and falls back to blocking `usleep()` otherwise.

**Parameters:**
- `$microseconds` — Duration to sleep in microseconds. Values of `0` or less return immediately.

**Returns:** `void`

**Example:**

```php
<?php
oxphp_worker(function () {
    // Poll for a condition every 100ms without blocking other requests
    while (!$condition_met()) {
        oxphp_usleep(100_000);
    }
    echo "ready";
});
```

---

## oxphp_async()

```php
oxphp_async(Closure $closure, mixed ...$args): int
```

Dispatches a closure for execution on a dedicated async worker thread and returns a promise ID immediately. The caller continues executing without waiting for the closure to finish. Use `oxphp_async_await()` to retrieve the result.

**Parameters:**
- `$closure` — A user-defined `Closure` to run on an async worker thread
- `...$args` — Arguments to pass to the closure. Only scalar values (`null`, `bool`, `int`, `float`, `string`) and arrays of scalars are accepted. Objects and resources cannot be passed across threads.

**Returns:** An integer promise ID. Pass this to `oxphp_async_await()`, `oxphp_async_await_all()`, or `oxphp_async_await_any()`.

**Throws:** `OxPHP\AsyncException` if the closure is not user-defined, if the async pool is full, or if arguments contain objects or resources.

> **Note:** Use-vars captured via `use` in the closure follow the same restrictions — objects and resources are rejected.

**Example:**

```php
<?php
// Dispatch two independent tasks concurrently
$p1 = oxphp_async(function () {
    return fetch_from_api('/users');
});

$p2 = oxphp_async(function () {
    return fetch_from_api('/posts');
});

// Retrieve both results
$users = oxphp_async_await($p1);
$posts = oxphp_async_await($p2);
```

---

## oxphp_async_await()

```php
oxphp_async_await(int $promise_id, float $timeout = 0.0): mixed
```

Blocks until the specified async promise completes and returns its result. Inside a worker mode fiber, this suspends the current fiber cooperatively rather than blocking the thread.

**Parameters:**
- `$promise_id` — A promise ID returned by `oxphp_async()`
- `$timeout` — Maximum seconds to wait. `0.0` means wait indefinitely. Default: `0.0`

**Returns:** The return value of the async closure.

**Throws:**
- `OxPHP\AsyncException` if the async task threw an exception
- `OxPHP\AsyncTimeoutException` if `$timeout` is exceeded

**Example:**

```php
<?php
$promise = oxphp_async(function (int $n) {
    return array_sum(range(1, $n));
}, 1_000_000);

$result = oxphp_async_await($promise);
echo $result; // 500000500000

// With timeout
try {
    $result = oxphp_async_await($promise, 5.0);
} catch (\OxPHP\AsyncTimeoutException $e) {
    echo "Task took too long";
}
```

---

## oxphp_async_await_all()

```php
oxphp_async_await_all(array $promise_ids, float $timeout = 0.0): array
```

Awaits all promises in the array and returns an associative array mapping each promise ID to its result. Promises are awaited in array order.

**Parameters:**
- `$promise_ids` — An array of integer promise IDs returned by `oxphp_async()`
- `$timeout` — Maximum seconds to wait per promise. `0.0` means wait indefinitely. Default: `0.0`

**Returns:** An associative array where each key is a promise ID (integer) and each value is the result of that promise.

**Throws:**
- `OxPHP\AsyncException` if any promise fails
- `OxPHP\AsyncTimeoutException` if any promise exceeds `$timeout`

**Example:**

```php
<?php
$promises = [
    oxphp_async(fn() => slow_query('users')),
    oxphp_async(fn() => slow_query('orders')),
    oxphp_async(fn() => slow_query('products')),
];

$results = oxphp_async_await_all($promises);

foreach ($results as $promiseId => $result) {
    // process $result
}
```

---

## oxphp_async_await_any()

```php
oxphp_async_await_any(array $promise_ids, float $timeout = 0.0): array
```

Races multiple promises and returns the first one to complete. The other promises are not cancelled — they continue running and remain awaitable with `oxphp_async_await()`.

**Parameters:**
- `$promise_ids` — An array of at least one integer promise ID returned by `oxphp_async()`. Must not be empty.
- `$timeout` — Maximum seconds to wait for any promise to complete. `0.0` means wait indefinitely. Default: `0.0`

**Returns:** An associative array with two keys:
- `id` (`int`) — The promise ID of the winner
- `value` (`mixed`) — The return value of the winning promise

**Throws:**
- `OxPHP\AsyncException` if the winning promise failed
- `OxPHP\AsyncTimeoutException` if no promise completes within `$timeout`

**Example:**

```php
<?php
// Try two mirror endpoints; use whichever responds first
$p1 = oxphp_async(fn() => fetch('https://mirror-1.example.com/data'));
$p2 = oxphp_async(fn() => fetch('https://mirror-2.example.com/data'));

$winner = oxphp_async_await_any([$p1, $p2], timeout: 10.0);
echo "Mirror {$winner['id']} won: " . json_encode($winner['value']);
```

---

## oxphp_register_decorator()

```php
oxphp_register_decorator(string $class): bool
```

Registers a PHP class as a decorator that wraps function and method calls. The class must implement `OxPHP\Decorator\AttributeInterface`. Once registered, OxPHP invokes the decorator's `before()` and `after()` hooks around every function or method call that matches the decorator's `#[Attribute]` targets.

**Parameters:**
- `$class` — The fully qualified class name of the decorator to register

**Returns:** `true` on success, `false` if the class does not exist or does not implement `OxPHP\Decorator\AttributeInterface`.

**Example:**

```php
<?php
use OxPHP\Decorator\AttributeInterface;
use OxPHP\Decorator\Context;

#[\Attribute(\Attribute::TARGET_METHOD)]
class LogDecorator implements AttributeInterface
{
    public function before(Context $ctx): void
    {
        error_log("Calling {$ctx->target} (request {$ctx->requestId})");
    }

    public function after(Context $ctx): void
    {
        error_log("Finished {$ctx->target}");
    }
}

// Register once at bootstrap (or worker startup)
oxphp_register_decorator(LogDecorator::class);
```

---

## Classes and Interfaces

The `oxphp_sapi` extension registers the following classes:

### HTTP

| Class | Description |
|-------|-------------|
| `OxPHP\Http\Request` | Request object returned by `oxphp_http_request()`. `final` — cannot be extended. |
| `OxPHP\Http\Attributes` | Mutable request attributes container (for middleware). `final`. |
| `OxPHP\Http\Session` | Session object accessible via `$request->session()`. `final`. |
| `OxPHP\Http\UploadedFile` | Uploaded file object from `$request->files()`. `final`. |

### Decorators

| Class / Interface | Description |
|-------------------|-------------|
| `OxPHP\Decorator\AttributeInterface` | Interface for decorators. Requires `before(Context $ctx)` and `after(Context $ctx)` methods. |
| `OxPHP\Decorator\Context` | Context object passed to decorator hooks. `final`. Contains `target`, `requestId`, arguments, and return value. |

### Async

| Class | Description |
|-------|-------------|
| `OxPHP\BorrowedProxy` | Proxy object for borrowed values between threads. |

---

## Exceptions

All exceptions registered by the extension:

| Exception | Extends | When thrown |
|-----------|---------|------------|
| `OxPHP\AsyncException` | `\Exception` | Error in an async task (`oxphp_async_await()`) or invalid arguments in `oxphp_async()` |
| `OxPHP\AsyncTimeoutException` | `OxPHP\AsyncException` | Timeout exceeded in `oxphp_async_await()`, `oxphp_async_await_all()`, or `oxphp_async_await_any()` |
| `OxPHP\AsyncBorrowException` | `\Exception` | Error borrowing a value between threads |
| `OxPHP\Http\Exception\NoActiveRequestException` | `\RuntimeException` | Calling `oxphp_http_request()` outside an active request |
| `OxPHP\Http\Exception\AsyncContextException` | `NoActiveRequestException` | Calling `oxphp_http_request()` inside an `oxphp_async()` callback |
| `OxPHP\Http\Exception\WorkerIdleException` | `NoActiveRequestException` | Calling `oxphp_http_request()` in worker mode between requests |
| `OxPHP\Decorator\RejectedException` | `\Exception` | A decorator rejected a function/method call |

---

## Extension Verification

You can verify that the OxPHP extension is loaded and inspect all registered functions:

```php
<?php
if (extension_loaded('oxphp_sapi')) {
    echo "OxPHP extension is loaded\n";
}

$functions = get_extension_funcs('oxphp_sapi');
print_r($functions);
// Array
// (
//     [0]  => oxphp_http_request
//     [1]  => oxphp_superglobals_enabled
//     [2]  => oxphp_request_id
//     [3]  => oxphp_worker_id
//     [4]  => oxphp_server_info
//     [5]  => oxphp_request_heartbeat
//     [6]  => oxphp_finish_request
//     [7]  => oxphp_is_worker
//     [8]  => oxphp_is_streaming
//     [9]  => oxphp_stream_flush
//     [10] => oxphp_sleep
//     [11] => oxphp_usleep
//     [12] => oxphp_worker
//     [13] => oxphp_async
//     [14] => oxphp_async_await
//     [15] => oxphp_async_await_all
//     [16] => oxphp_async_await_any
//     [17] => oxphp_register_decorator
// )
```

## Compatibility with PHP-FPM

If your code must run on both OxPHP and PHP-FPM, use fallback wrappers:

```php
<?php
function finish_request(): bool
{
    if (function_exists('oxphp_finish_request')) {
        return oxphp_finish_request();
    }
    if (function_exists('fastcgi_finish_request')) {
        return fastcgi_finish_request();
    }
    return false;
}

// Worker-aware bootstrap
if (function_exists('oxphp_is_worker') && oxphp_is_worker()) {
    // OxPHP worker mode
} else {
    // PHP-FPM or OxPHP traditional mode
}
```

## See Also

- [HTTP Request API](request-api.md) -- object-oriented access to request data via `oxphp_http_request()`
- [Worker Mode](../features/worker-mode.md) -- persistent worker loop and request lifecycle
- [Server-Sent Events](../features/sse.md) -- real-time streaming with `oxphp_stream_flush()`
- [Early Response](../features/early-response.md) -- background processing with `oxphp_finish_request()`
- [Superglobals](superglobals.md) -- how OxPHP populates `$_SERVER`, `$_GET`, `$_POST`, and other superglobals
- [Configuration Reference](../operations/configuration.md) -- `WORKER_FILE`, `PHP_WORKERS`, `REQUEST_TIMEOUT_SECONDS`, and other env vars
