---
title: Async Promises
description: Run PHP closures in background threads and await results without blocking the worker pool.
---

# Async Promises

OxPHP provides an async execution system that runs PHP closures on a dedicated thread pool, separate from the HTTP worker pool. This prevents long-running background tasks from blocking request handling.

## How It Works

1. **Dispatch** — call `oxphp_async()` with a closure and optional arguments. OxPHP serializes the closure's `use`-variables and arguments, sends them to the async pool, and returns a promise ID immediately
2. **Execute** — a dedicated async worker thread deserializes the data, runs the closure, and serializes the result
3. **Await** — call `oxphp_async_await()` with the promise ID. In worker mode with fibers, the current fiber suspends and other requests continue on the same thread. In traditional mode, the worker thread blocks until the result is ready
4. **Cleanup** — any promises not explicitly awaited are automatically cancelled and cleaned up at the end of the request

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0` (disabled) | Number of dedicated async worker threads. Set to `0` to disable the async pool entirely |
| `ASYNC_QUEUE_CAPACITY` | `0` (auto) | Maximum pending async tasks. When `0`, defaults to `ASYNC_WORKERS × 64` |

> **Note:** The async pool is disabled by default (`ASYNC_WORKERS=0`). With the pool disabled, all four async functions exist but throw `OxPHP\Async\Exception` when called. Set `ASYNC_WORKERS` to a value greater than `0` to enable background execution.

## Dispatching Tasks

Pass a closure and optional arguments to `oxphp_async()`. It returns a promise ID (integer) immediately:

```php
<?php
$promise = oxphp_async(function (string $url) {
    return file_get_contents($url);
}, 'https://api.example.com/data');

// The closure is running in the background.
// Do other work here...

$result = oxphp_async_await($promise);
echo $result;
```

### Passing Data to Closures

Use `use`-variables or function arguments to pass data. Only scalar types and arrays are supported:

```php
<?php
$apiKey = 'sk-abc123';
$ids = [1, 2, 3];

$promise = oxphp_async(function () use ($apiKey, $ids) {
    // $apiKey and $ids are available here
    return count($ids);
});
```

## Awaiting Results

### Single Promise

```php
<?php
$result = oxphp_async_await($promise);           // Wait indefinitely
$result = oxphp_async_await($promise, 5.0);      // Wait up to 5 seconds
```

A timeout of `0.0` (the default) waits indefinitely. On timeout, `OxPHP\Async\TimeoutException` is thrown.

### All Promises

`oxphp_async_await_all()` waits for every promise and returns an associative array keyed by promise ID:

```php
<?php
$p1 = oxphp_async(fn() => file_get_contents('https://api.example.com/users'));
$p2 = oxphp_async(fn() => file_get_contents('https://api.example.com/orders'));

$results = oxphp_async_await_all([$p1, $p2], 10.0);

$users  = $results[$p1];
$orders = $results[$p2];
```

> **Note:** `oxphp_async_await_all()` awaits promises sequentially in array order. All closures run concurrently on the async pool, but the calling thread collects results one at a time.

### First Promise (Race)

`oxphp_async_await_any()` returns as soon as one promise completes:

```php
<?php
$p1 = oxphp_async(fn() => fetch_from_primary_db());
$p2 = oxphp_async(fn() => fetch_from_replica_db());

$winner = oxphp_async_await_any([$p1, $p2], 5.0);
// $winner = ['id' => int, 'value' => mixed]

echo "Promise {$winner['id']} won: {$winner['value']}";
```

Non-winning promises remain awaitable individually after `oxphp_async_await_any()` returns.

## Error Handling

Exceptions thrown inside an async closure are captured and re-thrown at await time as `OxPHP\Async\Exception`:

```php
<?php
$promise = oxphp_async(function () {
    throw new \RuntimeException('Something failed');
});

try {
    $result = oxphp_async_await($promise);
} catch (\OxPHP\Async\Exception $e) {
    // "Async task failed: [RuntimeException] Something failed"
    echo $e->getMessage();
}
```

`exit()` and `die()` inside an async closure are also caught and converted to `OxPHP\Async\Exception`. The async worker survives and continues processing new tasks.

### Exception Hierarchy

```text
\Exception
  └── OxPHP\Async\Exception              # All async errors
        └── OxPHP\Async\TimeoutException  # Timeout-specific
```

## Fiber Integration

In worker mode, `oxphp_async_await()` cooperates with OxPHP's fiber scheduler. Instead of blocking the worker thread, the current fiber suspends while waiting for the result. The scheduler resumes it when the result is ready, allowing other requests to be processed on the same thread.

In traditional mode (no worker file), `oxphp_async_await()` blocks the worker thread synchronously. This means the worker cannot handle other requests while waiting.

For best performance, combine async promises with worker mode:

```php
<?php
// worker.php
require __DIR__ . '/../vendor/autoload.php';

oxphp_worker(function () {
    // These two API calls run concurrently on the async pool
    // while the fiber suspends — the worker thread is free for other requests
    $p1 = oxphp_async(fn() => file_get_contents('https://api.example.com/users'));
    $p2 = oxphp_async(fn() => file_get_contents('https://api.example.com/orders'));

    $results = oxphp_async_await_all([$p1, $p2]);
    echo json_encode($results);
});
```

## Limitations

Async closures run on separate threads. This imposes restrictions on what data can cross the thread boundary:

| Allowed | Not allowed |
|---------|-------------|
| `null`, `bool`, `int`, `float`, `string` | Objects (any class instance) |
| Arrays of scalar types | Resources (file handles, DB connections, streams) |
| Nested scalar arrays | Closures referencing objects in `use` |

Additional constraints:

- **No nested async** — calling `oxphp_async()` from inside an async closure throws `OxPHP\Async\Exception`
- **User functions only** — the closure must be user-defined, not a wrapper around a built-in function
- **Serialization overhead** — arguments and return values are serialized across the thread boundary. Large arrays or strings add latency
- **No shared state** — each async worker has its own PHP environment. There are no shared variables between the dispatching thread and the async thread

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
    ports:
      - "80:80"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - WORKER_FILE=worker.php
      - ASYNC_WORKERS=4
      - ASYNC_QUEUE_CAPACITY=256
```

## Troubleshooting

### "Async pool is disabled. Set ASYNC_WORKERS > 0 to enable."

The async pool is not configured. When `ASYNC_WORKERS=0` (the default), the async functions are registered but throw `OxPHP\Async\Exception` on every call.

**Fix:** Set `ASYNC_WORKERS` to a positive value:

```bash
ASYNC_WORKERS=4
```

### "Failed to dispatch async task (pool full)"

The async pool is running but all queue slots are occupied.

**Check:** Verify the pool is accepting tasks:

```bash
curl -s http://localhost:9090/config | jq '.async_workers'
```

**Fix:** Increase `ASYNC_WORKERS` or `ASYNC_QUEUE_CAPACITY`:

### "Cannot pass object values in use-vars to async closure"

Objects cannot be serialized across thread boundaries.

**Fix:** Extract the scalar data you need before dispatching:

```php
<?php
// Wrong: passing an object
$promise = oxphp_async(function () use ($user) { ... });

// Correct: passing scalar data extracted from the object
$userId = $user->getId();
$userName = $user->getName();
$promise = oxphp_async(function () use ($userId, $userName) { ... });
```

### Await hangs in traditional mode

In traditional mode (no `WORKER_FILE`), `oxphp_async_await()` blocks the worker thread. If all PHP workers are blocked waiting for async results, the server stops processing requests.

**Fix:** Use worker mode (`WORKER_FILE`) so that `oxphp_async_await()` suspends the fiber instead of blocking the thread.

### Async timeout does not kill the running task

`OxPHP\Async\TimeoutException` is thrown at the await side — the closure continues running on the async pool until it finishes or the request ends. Tasks are cancelled at the end of the request during cleanup.

## Best Practices

- **Always set timeouts** on `oxphp_async_await()` calls in production to prevent indefinite waits
- **Use worker mode** to get non-blocking fiber-based awaiting instead of blocking the worker thread
- **Keep closures small** — dispatch focused units of work, not entire request handlers
- **Extract scalars before dispatch** — pull IDs, strings, and config values out of objects before passing to the closure
- **Monitor the async pool** — check `oxphp_async_tasks_rejected_total` in Prometheus metrics. If rejections are rising, increase `ASYNC_WORKERS` or `ASYNC_QUEUE_CAPACITY`

## See Also

- [Worker Mode](worker-mode.md) -- persistent PHP processes with fiber-based concurrency
- [PHP Functions](../php/functions.md) -- `oxphp_async()`, `oxphp_async_await()`, and related function reference
- [Metrics](../operations/metrics.md) -- async pool Prometheus metrics
- [Configuration Reference](../operations/configuration.md) -- `ASYNC_WORKERS` and `ASYNC_QUEUE_CAPACITY`
