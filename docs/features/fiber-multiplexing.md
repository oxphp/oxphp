---
title: Fiber Multiplexing
description: Handle hundreds of concurrent requests on a single PHP worker thread using cooperative multitasking with fibers.
---

# Fiber Multiplexing

OxPHP uses PHP Fibers to handle multiple HTTP requests concurrently on a single worker thread. When a request calls `oxphp_sleep()` or `oxphp_async_await()` (when the async pool is enabled), it suspends and the worker thread immediately picks up the next request. This allows one worker to manage hundreds of in-flight requests without additional threads.

## How It Works

1. A request arrives and the scheduler assigns it to a fiber — a lightweight execution context with its own stack and PHP state
2. The fiber runs the `oxphp_worker()` handler. If the handler completes without suspending, the response is sent and the fiber is recycled — zero overhead compared to a single-request worker
3. If the handler calls a suspending function (`oxphp_sleep()`, `oxphp_usleep()`, `oxphp_async_await()`), the fiber yields back to the scheduler
4. The scheduler picks up new incoming requests (creating new fibers) and resumes suspended fibers whose wait conditions are met (timer expired, async result ready)
5. Each fiber's PHP state — superglobals, response headers, output buffers, VM stack — is saved on suspension and restored on resumption. Fibers are fully isolated from each other

```text
Worker Thread
  │
  ├─ Fiber A: handling /api/users ──── oxphp_sleep(0.5) ──── [suspended] ──── [resumed] ──── response
  │
  ├─ Fiber B: handling /api/orders ──── oxphp_async_await($p) ──── [suspended] ──── [resumed] ──── response
  │
  └─ Fiber C: handling /health ──── response (no suspension, zero overhead)
```

## Configuration

Fiber multiplexing activates automatically when worker mode is enabled. There are no additional environment variables.

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_MODE_ENABLED` | `false` | Set to `true` and point `ENTRY_FILE` at a `.php` bootstrap to enable worker mode and fiber multiplexing |
| `PHP_WORKERS` | CPU / 2 (min 1) | Number of worker threads. Each thread runs its own independent scheduler with up to 256 concurrent fibers |

The maximum number of concurrent fibers per worker thread is 256. With 4 worker threads, OxPHP can handle up to 1,024 in-flight requests simultaneously.

## Suspension Points

These functions suspend the current fiber and let other requests run on the same thread:

| Function | What happens |
|----------|-------------|
| `oxphp_sleep(float $seconds)` | Suspends the fiber for the given duration. Other fibers continue running |
| `oxphp_usleep(int $microseconds)` | Same as `oxphp_sleep()` with microsecond granularity (minimum 1 ms) |
| `oxphp_async_await(int $promise_id)` | Suspends the fiber until the async task completes on the background thread pool |

These functions do **not** suspend the fiber:

| Function | Behavior |
|----------|----------|
| `oxphp_stream_flush()` | Sends a chunk to the client immediately and returns. Use with `oxphp_sleep()` in SSE loops |
| `oxphp_finish_request()` | Sends the complete response and continues PHP execution. Does not yield |

> **Note:** PHP's built-in `sleep()` and `usleep()` block the entire worker thread by default. Either use `oxphp_sleep()` / `oxphp_usleep()`, or set `RUNTIME_HOOKS=sleep` to make the native builtins suspend the fiber automatically — useful when third-party code calls `sleep()` directly. See [Configuration](../operations/configuration.md).

## PHP Examples

### Basic Concurrent Handling

Requests that don't suspend run at full speed with zero fiber overhead:

```php
<?php
oxphp_worker(function () {
    $path = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);

    if ($path === '/health') {
        echo json_encode(['status' => 'ok']);
        return; // No suspension — runs at full speed
    }

    if ($path === '/slow') {
        oxphp_sleep(2.0); // Yields for 2 seconds — other requests run
        echo "Done after 2s delay";
        return;
    }

    echo "Hello";
});
```

### Non-Blocking API Calls

Combine `oxphp_async()` with `oxphp_async_await()` to make external API calls without blocking the worker:

```php
<?php
oxphp_worker(function () {
    // Dispatch two API calls to the async thread pool
    $p1 = oxphp_async(fn() => file_get_contents('https://api.example.com/users'));
    $p2 = oxphp_async(fn() => file_get_contents('https://api.example.com/orders'));

    // Await both — the fiber suspends, other requests run on this thread
    $users  = oxphp_async_await($p1);
    $orders = oxphp_async_await($p2);

    header('Content-Type: application/json');
    echo json_encode(['users' => json_decode($users), 'orders' => json_decode($orders)]);
});
```

### SSE with Cooperative Sleep

Interleave Server-Sent Events with other requests using `oxphp_stream_flush()` and `oxphp_sleep()`:

```php
<?php
oxphp_worker(function () {
    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');

    for ($i = 0; $i < 30; $i++) {
        echo "data: " . json_encode(['count' => $i, 'time' => time()]) . "\n\n";
        oxphp_stream_flush(); // Send chunk now (does not suspend)
        oxphp_sleep(1.0);     // Yield for 1 second (other requests run)
    }
});
```

## Blocking I/O

Fiber multiplexing is **cooperative, not preemptive**. A fiber that calls a blocking function freezes the entire worker thread — no other fibers on that thread can make progress.

### Functions That Block the Worker

- `file_get_contents()`, `fopen()`, `fread()`
- `curl_exec()`, `curl_multi_exec()`
- PDO queries, `mysqli_query()`
- PHP's `sleep()`, `usleep()` (use `oxphp_sleep()` instead, or enable `RUNTIME_HOOKS=sleep`)
- DNS resolution (`gethostbyname()`)
- Any synchronous network or disk I/O

Socket **reads** and **`stream_select()`** are the exception once `RUNTIME_HOOKS=streams` is enabled: both suspend the fiber instead of the thread, which covers clients that block on a PHP stream read (mysqlnd, phpredis, the HTTP stream wrappers) as well as loops waiting on several sockets at once. Writes, `socket_select()`, ext/curl, connecting and DNS resolution still block. See [Configuration](../operations/configuration.md#runtime-hooks).

### How to Avoid Blocking

Wrap blocking operations in `oxphp_async()` to run them on the async thread pool:

```php
<?php
// WRONG — blocks the entire worker thread
$html = file_get_contents('https://example.com');

// CORRECT — runs on async pool, fiber yields
$promise = oxphp_async(fn() => file_get_contents('https://example.com'));
$html = oxphp_async_await($promise);
```

For database queries:

```php
<?php
$db = new PDO('mysql:host=db;dbname=app', 'root', 'secret');

// WRONG — blocks the worker
$users = $db->query('SELECT * FROM users WHERE active = 1')->fetchAll();

// CORRECT — query runs on async thread, fiber yields
$promise = oxphp_async(function () {
    $db = new PDO('mysql:host=db;dbname=app', 'root', 'secret');
    return $db->query('SELECT * FROM users WHERE active = 1')->fetchAll();
});
$users = oxphp_async_await($promise);
```

> **Note:** Database connections cannot be passed to `oxphp_async()` because objects are not serializable across threads. Create the connection inside the async closure, or use the direct query in the fiber if the query is fast enough that blocking is acceptable.

> **Important:** `oxphp_async()` requires `ASYNC_WORKERS > 0`. When the async pool is disabled (the default), calling `oxphp_async()` throws `OxPHP\Async\AsyncException`.

## How Fibers Are Recycled

Fiber C stacks are allocated once and reused across requests. When a fiber finishes handling a request, it is not destroyed — it suspends back to the scheduler and is added to a free list. The next request reuses the existing C stack, avoiding expensive memory allocation.

PHP VM stacks (used for function call frames) are allocated fresh per request and freed when the handler returns.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "80:80"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - WORKER_MODE_ENABLED=true
      - ENTRY_FILE=worker.php
      - PHP_WORKERS=4
      - ASYNC_WORKERS=8
```

With this configuration, each of the 4 worker threads can handle up to 256 concurrent fibers, and blocking I/O is offloaded to 8 async worker threads.

## Troubleshooting

### Requests become slow when one request does heavy I/O

A fiber is calling a blocking function (database query, HTTP request, file read) without `oxphp_async()`. This blocks the entire worker thread.

**Fix:** Wrap blocking calls in `oxphp_async()`:

```php
<?php
$promise = oxphp_async(fn() => file_get_contents($url));
$result = oxphp_async_await($promise);
```

### "Async pool is disabled. Set ASYNC_WORKERS > 0 to enable."

The async pool is not configured. When `ASYNC_WORKERS=0` (the default), all async functions throw `OxPHP\Async\AsyncException`.

**Fix:** Set `ASYNC_WORKERS` to a positive value:

```bash
ASYNC_WORKERS=8
```

### "Failed to dispatch async task" when using oxphp_async()

The async pool is running but at capacity.

**Fix:** Increase `ASYNC_WORKERS` or `ASYNC_QUEUE_CAPACITY`:

```bash
ASYNC_WORKERS=8
ASYNC_QUEUE_CAPACITY=512
```

### oxphp_sleep() not yielding to other requests

Fiber multiplexing only works in worker mode. In traditional mode, `oxphp_sleep()` falls back to a blocking `usleep()`.

**Fix:** Enable worker mode by setting `WORKER_MODE_ENABLED=true`.

### High memory usage with many concurrent requests

Each fiber uses a C stack (default 8 MiB, configured by PHP's `fiber.stack_size` ini setting) plus a PHP VM stack per request. With 256 concurrent fibers, worst-case C stack memory is 2 GiB per worker thread.

**Fix:** Reduce `fiber.stack_size` in `php.ini` if your application does not use deep recursion:

```ini
fiber.stack_size = 512K
```

## Limitations

- **Worker mode only** — fiber multiplexing is not available in traditional mode
- **256 fibers per worker** — hard limit, not configurable at runtime
- **Cooperative only** — CPU-bound code (tight loops, heavy computation) starves other fibers. There is no preemption
- **Blocking I/O blocks the thread** — all blocking calls must be wrapped in `oxphp_async()` for true concurrency
- **PHP's native `sleep()`/`usleep()` are not fiber-aware** — use `oxphp_sleep()`/`oxphp_usleep()`
- **`oxphp_async_await_race()` and `oxphp_async_await_any()` do not yield** — they currently block even inside a fiber. `oxphp_async_await_all()` *does* suspend the fiber while waiting, so it is fiber-friendly; for `race`/`any`, use sequential `oxphp_async_await()` calls if you need the thread to stay cooperative

## See Also

- [Worker Mode](worker-mode.md) -- persistent PHP processes and the `oxphp_worker()` API
- [Async Promises](async-promises.md) -- background thread pool for offloading blocking I/O
- [SSE](sse.md) -- real-time streaming combined with fiber-based cooperative sleep
- [PHP Functions](../php/functions.md) -- `oxphp_sleep()`, `oxphp_usleep()`, and other fiber-aware functions
- [Configuration Reference](../operations/configuration.md) -- `WORKER_MODE_ENABLED`, `ENTRY_FILE`, `PHP_WORKERS`, `ASYNC_WORKERS`
