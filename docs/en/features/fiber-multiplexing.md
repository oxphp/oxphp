---
title: Fiber-Based Request Multiplexing
description: Cooperative multitasking that lets PHP worker threads handle multiple concurrent requests
---

OxPHP's fiber scheduler enables each PHP worker thread to handle multiple concurrent HTTP requests. When a request calls a suspend point such as `oxphp_async_await()` or `oxphp_sleep()`, the fiber yields control and the worker picks up other requests instead of blocking.

## How It Works

Every HTTP request runs in its own fiber (`zend_fiber_context` from PHP 8.4's low-level Fiber API). The fiber's C stack is allocated once via `mmap` and reused across requests through a **looping coroutine** — the coroutine processes requests in a loop, suspending between each one. This avoids the overhead of `mmap`/`munmap` on every request.

The worker thread runs an event loop that:

1. **Accepts new requests** from the bounded channel (non-blocking `try_recv`)
2. **Checks completed async results** for fibers waiting on `oxphp_async_await()`
3. **Checks expired timers** for fibers waiting on `oxphp_sleep()` / `oxphp_usleep()`
4. **Resumes ready fibers** by restoring their state and switching to their fiber context

Each fiber has its own isolated state:
- **PHP VM state**: `EG(vm_stack)`, `EG(execute_data)`, `EG(bailout)` — saved and restored on every context switch
- **PHP superglobals**: `$_SERVER`, `$_GET`, `$_POST`, etc. — moved (not copied) between TLS and per-fiber storage
- **SAPI headers**: response headers and HTTP status code
- **C stack limits**: `EG(stack_base)` / `EG(stack_limit)` — set per fiber to prevent false stack overflow detection
- **Rust TLS**: response output buffer, EARLY_TX (response channel), request start time

From the PHP script's perspective, execution is continuous — suspend and resume are invisible.

### Sequential requests (no multiplexing)

When a request handler completes without calling any suspend point, the looping coroutine processes it and immediately waits for the next request. Only one fiber is active, and its C stack is reused for every request. Performance is equivalent to the pre-fiber worker loop — no additional `mmap`, no state save/restore, no event loop polling.

### Multiplexed requests

When a handler calls a suspend point, the fiber suspends and the scheduler enters the event loop. The event loop accepts new requests (creating additional fibers as needed) and resumes fibers whose suspend conditions are satisfied. Each additional fiber allocates its own C stack (once) and reuses it via the looping coroutine.

```
Worker Thread
===========================
                          ┌─ try_recv ──► new request? ──► create/reuse fiber, start handler
                          │
loop ─────────────────────┤─ poll awaits ► result ready? ─► resume waiting fiber
                          │
                          └─ poll timers ► timer expired? ► resume sleeping fiber

                          (fibers that complete are finalized and their response is sent)
```

## Suspend Points

The following functions trigger fiber suspension when called inside a worker handler:

| Function | Behavior |
|----------|----------|
| `oxphp_async_await(int $promise_id, ?float $timeout = null)` | Suspends until the async task completes or times out |
| `oxphp_async_await_all(array $promise_ids, ?float $timeout = null)` | Falls back to blocking in v1 (fiber-aware in a future release) |
| `oxphp_async_await_any(array $promise_ids, ?float $timeout = null)` | Falls back to blocking in v1 (fiber-aware in a future release) |
| `oxphp_sleep(float $seconds)` | Suspends for the given duration (cooperative) |
| `oxphp_usleep(int $microseconds)` | Suspends for the given duration in microseconds (cooperative) |

When called outside a fiber (traditional mode), these functions fall back to their blocking equivalents.

## Configuration

Fiber multiplexing is automatic when worker mode is enabled. No additional environment variables are required.

| Constant | Value | Description |
|----------|-------|-------------|
| `OXPHP_MAX_FIBERS` | `256` | Maximum concurrent fibers per worker thread (compile-time) |

The fiber limit prevents a single worker from accumulating too many suspended requests. When the limit is reached, the event loop stops accepting new requests until an active fiber completes.

### Multi-worker scaling

Each PHP worker thread runs its own independent fiber scheduler. All state (fibers, timers, TLS slots) is thread-local. Multiple workers scale linearly:

| Workers | `/bench` (req/s) | `/sleep?ms=20` c40 (req/s) |
|---------|------------------|---------------------------|
| 1 | 40,144 | 867 |
| 2 | 66,619 | 1,378 |

Set `PHP_WORKERS=N` to control the number of worker threads. Default: `CPU / 2`.

## PHP API

### `oxphp_sleep`

Cooperative sleep that suspends the current fiber, allowing the worker to handle other requests during the wait.

```php
oxphp_sleep(float $seconds): void
```

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `$seconds` | `float` | Duration to sleep in seconds (e.g., `0.5` for 500ms) |

**Behavior:**
- In worker mode: registers a timer and suspends the fiber. The scheduler resumes it after the specified duration.
- Outside worker mode: falls back to blocking `usleep()`.
- Values less than or equal to zero return immediately with no effect.

### `oxphp_usleep`

Cooperative microsecond sleep. Identical to `oxphp_sleep()` but accepts microseconds as an integer, consistent with PHP's built-in `usleep()`.

```php
oxphp_usleep(int $microseconds): void
```

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `$microseconds` | `int` | Duration to sleep in microseconds |

**Behavior:** Same as `oxphp_sleep()` but with microsecond granularity. Values less than or equal to zero return immediately.

## Backward Compatibility

Fiber multiplexing is fully backward compatible:

- **Handlers that don't call suspend points** run through the looping coroutine with zero additional overhead. The fiber's C stack is reused, no state save/restore occurs. Performance is equivalent to the pre-fiber worker loop.
- **`oxphp_sleep()` and `oxphp_usleep()` outside worker mode** fall back to blocking `usleep()`.
- **`oxphp_async_await()` outside a fiber** falls back to blocking on the oneshot channel.
- **No new environment variables** are required.
- **Multi-worker configurations** work identically — each worker has its own independent fiber scheduler.

## Limitations (v1)

- **`ob_start()` with custom callbacks** may behave unexpectedly across suspend points. Output buffers are flushed to the Rust response buffer on suspend, so custom OB callbacks see partial output at suspend boundaries.
- **Shared mutable closure variables** (`use (&$var)`) are subject to interleaving at suspend points. Between two suspend points, your code runs without interruption. At each suspend point, other requests may execute on the same worker. This is the same concurrency model as Node.js `async`/`await`.
- **`oxphp_async_await_all` and `oxphp_async_await_any`** fall back to blocking in fiber mode (v1). A future release will add fiber-aware implementations.
- **Maximum 256 concurrent fibers per worker.** This is a compile-time constant (`OXPHP_MAX_FIBERS`).
- **REQUEST_DATA** (server variables, cookies, request body) is not yet saved/restored per fiber. When multiple fibers are active, they share the most recently loaded request data. This affects `$_SERVER`, `$_GET`, `$_POST` after a suspend point in multiplexed scenarios.

## Example

### SSE with cooperative sleep

This example streams Server-Sent Events while yielding the worker thread during the delay between events. Other requests are handled during each `oxphp_sleep()` call.

```php
<?php
oxphp_worker(function() {
    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');

    for ($i = 0; $i < 10; $i++) {
        echo "data: " . json_encode(['counter' => $i]) . "\n\n";
        oxphp_stream_flush();
        oxphp_sleep(1.0); // yields fiber, worker handles other requests
    }

    echo "event: done\ndata: {}\n\n";
    oxphp_stream_flush();
});
```

### Async await with multiplexing

When `oxphp_async_await()` suspends the fiber, the worker handles other HTTP requests while waiting for the async result:

```php
<?php
oxphp_worker(function() {
    $config = ['dsn' => 'mysql:host=localhost;dbname=app', 'user' => 'root', 'pass' => ''];

    // Dispatch async task
    $promise = oxphp_async(function() use ($config): array {
        $pdo = new PDO($config['dsn'], $config['user'], $config['pass']);
        return $pdo->query('SELECT count(*) as c FROM users')->fetch();
    });

    // Fiber suspends here — worker handles other requests
    $result = oxphp_async_await($promise);

    header('Content-Type: application/json');
    echo json_encode($result);
});
```

## See Also

- [Async Promises](async-promises.md) --- parallel execution with `oxphp_async()` and `oxphp_async_await()`
- [Worker Pool](../architecture/worker-pool.md) --- HTTP worker pool architecture and worker mode
- [PHP Extension Functions](../php/functions.md) --- full function reference including `oxphp_sleep` and `oxphp_usleep`
