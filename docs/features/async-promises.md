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
| `ASYNC_MAX_FIBERS` | `256` | Per-worker cap on concurrent async task fibers. The process-global in-flight limit (queued + running tasks) is `ASYNC_MAX_FIBERS × ASYNC_WORKERS`; a dispatch past it is rejected immediately (non-blocking) with `OxPHP\Async\AsyncException`, so fan-out composition cannot deadlock waiting on capacity it holds |

> **Note:** The async pool is disabled by default (`ASYNC_WORKERS=0`). With the pool disabled, all four async functions exist but throw `OxPHP\Async\AsyncException` when called. Set `ASYNC_WORKERS` to a value greater than `0` to enable background execution.

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

### First Settled Promise (Race)

`oxphp_async_await_race()` returns as soon as one promise settles, whether it fulfilled or rejected:

```php
<?php
// fetch_from_primary_db() and fetch_from_replica_db() are loaded by opcache.preload:
// see Your Own Functions and Classes in Tasks.
$p1 = oxphp_async(fn() => fetch_from_primary_db());
$p2 = oxphp_async(fn() => fetch_from_replica_db());

$winner = oxphp_async_await_race([$p1, $p2], 5.0);
// $winner = ['id' => int, 'value' => mixed]

echo "Promise {$winner['id']} won: {$winner['value']}";
```

Non-winning promises remain awaitable individually after `oxphp_async_await_race()` returns. If the winning promise rejected, `OxPHP\Async\AsyncException` is thrown — losers are still awaitable. This is the JavaScript `Promise.race` analog.

### First Fulfilled Promise

`oxphp_async_await_any()` returns as soon as one promise FULFILLS. Rejections are accumulated and only become observable if every promise rejects. This is the JavaScript `Promise.any` analog — useful for fallback / redundancy patterns ("any mirror that responds").

```php
<?php
$mirror_a = oxphp_async(fn() => file_get_contents('https://mirror-a.example.com/data'));
$mirror_b = oxphp_async(fn() => file_get_contents('https://mirror-b.example.com/data'));
$mirror_c = oxphp_async(fn() => file_get_contents('https://mirror-c.example.com/data'));

try {
    $winner = oxphp_async_await_any([$mirror_a, $mirror_b, $mirror_c], 5.0);
    // ['id' => one of the input ids, 'value' => its result]
} catch (\OxPHP\Async\AggregateAsyncException $e) {
    foreach ($e->getErrors() as $i => $err) {
        // $err keyed by input position 0..N-1
    }
    foreach ($e->getErrorMap() as $promise_id => $err) {
        // $err keyed by promise id
    }
} catch (\OxPHP\Async\TimeoutException $e) {
    foreach ($e->getPartialErrors() as $promise_id => $err) {
        // promises that already rejected before the deadline
    }
    $cancelled = $e->getCancelledPromiseIds();
    // Promise ids that had not settled at the deadline. The cancel flag
    // is set on each AND their receivers were dropped — passing any of
    // these ids to oxphp_async_await*() afterwards throws "unknown or
    // already-awaited promise id". Treat the list as an audit trail, not
    // a resumable queue.
}
```

Non-winning promises that were still pending at the moment of victory remain awaitable individually. Promises that already rejected before the winner do not — their results were consumed when accumulated as candidate errors.

### Exception Types

| Class | Thrown by | Notes |
|-------|-----------|-------|
| `OxPHP\Async\AsyncException`           | `oxphp_async_await()`, `oxphp_async_await_all()`, `oxphp_async_await_race()` | Single error with message and optional original-exception details. |
| `OxPHP\Async\TimeoutException`         | All four await-* on deadline | Extends `AsyncException`. For `oxphp_async_await_any()` timeouts, `getPartialErrors()` and `getCancelledPromiseIds()` are populated; for the other call sites both return `[]`. |
| `OxPHP\Async\AggregateAsyncException`  | `oxphp_async_await_any()` when every promise rejects | Extends `AsyncException`. Provides `getErrors()` (positional, keyed 0..N-1), `getErrorMap()` (id-keyed), `getPromiseIds()`. |

## Error Handling

Exceptions thrown inside an async closure are captured and re-thrown at await time as `OxPHP\Async\AsyncException`:

```php
<?php
$promise = oxphp_async(function () {
    throw new \RuntimeException('Something failed');
});

try {
    $result = oxphp_async_await($promise);
} catch (\OxPHP\Async\AsyncException $e) {
    // "Async task failed: [RuntimeException] Something failed"
    echo $e->getMessage();
}
```

`exit()` and `die()` inside an async closure are also caught and converted to `OxPHP\Async\AsyncException`. The async worker survives and continues processing new tasks.

### Exception Hierarchy

```text
\Exception
  └── OxPHP\Async\AsyncException                   # All async errors
        ├── OxPHP\Async\TimeoutException           # Timeout-specific
        └── OxPHP\Async\AggregateAsyncException    # Multiple failures (await_all / await_any)
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

## Composition (Nested Async)

An async task may itself call `oxphp_async()` and await the result. Because each task runs inside a scheduler fiber, awaiting a nested promise suspends the *task's* fiber and frees its worker to run the nested task — so a task can fan out to children without holding a worker while it waits.

```php
<?php
$p = oxphp_async(function (): int {
    // Dispatched and awaited from inside an async task
    $inner = oxphp_async(fn () => 21);
    return oxphp_async_await($inner) * 2;
});

$result = oxphp_async_await($p); // 42
```

`oxphp_async_await_all()`, `oxphp_async_await_race()`, and `oxphp_async_await_any()` may likewise be called from inside a task fiber. The number of concurrent task fibers (queued + running) is bounded by `ASYNC_MAX_FIBERS × ASYNC_WORKERS`; a dispatch that would exceed the cap is rejected immediately with `OxPHP\Async\AsyncException` instead of blocking, so a fan-out cannot deadlock by waiting on capacity it is itself holding.

## Limitations

Async closures run on separate threads. This imposes restrictions on what data can cross the thread boundary:

| Allowed | Not allowed |
|---------|-------------|
| `null`, `bool`, `int`, `float`, `string` | Any other object — `OxPHP\Shared\Shareable` is reserved for the `Shared\*` types, and a class written in PHP can neither implement it nor extend a class that does |
| Arrays of scalar types | Resources (file handles, DB connections, streams) |
| Nested scalar arrays | Closures whose `use` captures non-Shareable objects |
| `Shared\*` instances (`Counter`, `Map`, `Channel`, `Atomic`, `Flag`, `Mutex`, `Once`, `Pool`, `Registry`) | |

Additional constraints:

- **User functions only** — the closure must be user-defined, not a wrapper around a built-in function
- **Your script's functions and classes do not exist in a task** — an async worker thread never runs your script or the files it includes, so nothing they declare exists there; built-in functions and classes and [those loaded by `opcache.preload`](#your-own-functions-and-classes-in-tasks) exist on every thread. A task that calls a function declared at the top level of the closure's own file, or declares a class — anonymous or named — inside the closure, fails before its first statement runs: `oxphp_async_await()` throws `OxPHP\Async\AsyncException` with a message such as `Async task failed: [Error] Call to undefined function helper() at /var/www/html/worker.php:12: it belongs to a script this PHP thread has not loaded; …`. The whole closure is checked, the closures it creates and the branches it never takes included, so a task that only names such a function in a branch it does not take fails as well. A function the file declares inside an `if` or another function is missing too, but the task runs until it reaches the call and then fails with PHP's own `Call to undefined function`. All of this is with OPcache, which is on by default, and only for a closure in a file OPcache caches. Without OPcache, and for a file it compiles without caching — a blacklisted one, say, or one changed too recently for OPcache to cache it, under `opcache.file_update_protection` — a call to a function of a file included before the closure's file is refused before the task starts as well. OPcache may replace a call to a function that only returns a constant with that constant, and such a call then works in a task; that is not a way around this. A named class the task instantiates fails with PHP's own `Class "…" not found` when the task reaches it. The same goes for a `Shared\Pool` factory or destroy callback that a task's acquire or `evict()` runs — see [Shared\Pool](../shared-state/shared-pool.md#factory-and-destroy-semantics), which also says what such a callback may capture there and what becomes of its `static` variables.
- **Serialization overhead** — arguments and return values are serialized across the thread boundary. Large arrays or strings add latency
- **No shared state for plain PHP values** — each async worker has its own PHP environment. Plain variables, arrays, and class instances are copied (or rejected) across the boundary. Use the [shared-state primitives](../shared-state/shared-state.md) (`Shared\Counter`, `Shared\Map`, `Shared\Channel`, …) to pass references that are visible to both threads
- **No pool from a request outside worker mode** — a `Shared\Pool` created outside worker mode, by an HTTP request or by a script under `oxphp run`, crosses into a task, but every acquire on it there throws `ClosedException`, and `Registry::pool()` or `Registry::global()` there throws `SharedException` rather than bind it to a name: such a pool can only be used by the request that created it. See [Shared\Pool](../shared-state/shared-pool.md#outside-worker-mode)
- **No end of script inside a task** — a task is not a request, so `register_shutdown_function()` called from an async closure registers a callback that never runs: there is no response for it to write into and no request for it to be the end of, and it is discarded when the task finishes. Do that work at the end of the closure itself
- **No execution deadline inside a task** — `max_execution_time` does not bound what a task runs, and neither does `set_time_limit()` called inside one. Both still succeed — `set_time_limit()` returns `true`, and `ini_set()` returns the previous value, which is what it returns anywhere — and the value still reads back through `ini_get('max_execution_time')`; nothing counts it down. The engine keeps one execution timer per thread, and a pool thread runs every task of its life inside the single request it opened at startup — so a deadline armed there belongs to no task in particular, and would end whichever one happened to be running PHP code when it ran out, including a task that asked for no limit at all. Bound a task from the side that waits for it instead, with the timeout argument of `oxphp_async_await()` and its variants, which [cancels the task it gives up on](#timeouts-cancel-the-abandoned-task). One thing does still read the directive: with [`RUNTIME_HOOKS=streams`](../operations/configuration.md#runtime-hooks) on, a wait for a connection another fiber is using is bounded by the smaller of `max_execution_time` and `default_socket_timeout`, so a task that lowers it shortens that wait — for itself and for the tasks that run on that thread after it, since an ini value a task sets is not rolled back until the request ends and a pool thread's request does not end

## Your Own Functions and Classes in Tasks

Functions and classes loaded by [`opcache.preload`](../php/opcache.md#preloading) exist on every PHP thread, the async worker threads included. Put what tasks need into files the preload script loads:

```ini
; /usr/local/etc/php/conf.d/zz-preload.ini
opcache.preload=/var/www/html/preload.php
```

```php
<?php
// /var/www/html/preload.php
require __DIR__ . '/src/tasks.php';
```

```php
<?php
// /var/www/html/src/tasks.php
function thumbnail_path(string $path): string
{
    return dirname($path) . '/thumb-' . basename($path);
}

final class Thumbnail
{
    public function __construct(public readonly string $path) {}
}
```

A task can then call `thumbnail_path()` and use `Thumbnail`, whether its closure is written in your script or in a preloaded file. A closure written in a preloaded file can also declare anonymous classes and call that file's functions. Preloaded code is fixed for the life of the server: restart it after changing those files. Constants are not preloaded: one that a preloaded file defines with `const` or `define()` does not exist in a task.

A class is preloaded only when its parent class, interfaces and traits are built in or preloaded as well, so the preload script has to load the files that declare them too. If it does not, a class loaded with `require` stops the server from starting with a fatal error, and one compiled with `opcache_compile_file()` is left out with a `Can't preload unlinked class` warning at startup, so a task that uses it gets `Class "…" not found`. Your script may still `require` a preloaded file: its functions and classes are not declared a second time, but its other top-level statements run in your script.

When PHP runs as root, it refuses to preload unless `opcache.preload_user` is set. Started as root, OxPHP switches to `www-data` before PHP starts, so the published image needs no `opcache.preload_user` unless the server keeps root (`--user=root`).

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.13.0
    ports:
      - "80:80"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - WORKER_MODE_ENABLED=true
      - ENTRY_FILE=worker.php
      - ASYNC_WORKERS=4
      - ASYNC_QUEUE_CAPACITY=256
```

## Troubleshooting

### "Async pool is disabled. Set ASYNC_WORKERS > 0 to enable."

The async pool is not configured. When `ASYNC_WORKERS=0` (the default), the async functions are registered but throw `OxPHP\Async\AsyncException` on every call.

**Fix:** Set `ASYNC_WORKERS` to a positive value:

```bash
ASYNC_WORKERS=4
```

### "Failed to dispatch async task (pool full)"

The async pool is running but all queue slots are occupied, or the process-global in-flight cap (`ASYNC_MAX_FIBERS × ASYNC_WORKERS`, covering queued + running tasks) has been reached. Both raise `OxPHP\Async\AsyncException` on dispatch and increment `oxphp_async_tasks_rejected_total`.

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

In traditional mode, `oxphp_async_await()` blocks the worker thread. If all PHP workers are blocked waiting for async results, the server stops processing requests.

**Fix:** Enable worker mode (`WORKER_MODE_ENABLED=true`) so that `oxphp_async_await()` suspends the fiber instead of blocking the thread.

### Timeouts cancel the abandoned task

`OxPHP\Async\TimeoutException` is raised on the await side the moment the deadline passes. The background task is no longer left running unobserved: a task parked in `oxphp_sleep()` or suspended awaiting a child promise is resumed and unwinds (its `finally` blocks run), and a CPU-bound task that never yields is interrupted at an opcode boundary — so cancellation is best-effort with a short latency bound, not instantaneous. The same cancellation applies to the promises `oxphp_async_await_all()` abandons and to the losers of `oxphp_async_await_race()` / `oxphp_async_await_any()`. A task that still cannot be interrupted in time is *stranded* — cancelled but drained at request end, which can extend `RSHUTDOWN` by up to a few seconds; watch `oxphp_async_tasks_stranded_total`.

That bound holds only while the task's code is in OPcache's shared memory. Without OPcache, and for a closure in code OPcache does not keep there — code passed to `eval()`, a file it compiled without caching (a blacklisted file, say, or one changed too recently for OPcache to cache it, under `opcache.file_update_protection`), or a file it holds for `opcache.file_cache` in the request's own memory, read from that cache or compiled for it (with `opcache.file_cache_only=1`, or while the shared memory is full or restarting) — the closure's code lives in the memory of the request that dispatched the task, which can free it — when it lets go of the closure, or when it ends — while the task is still running it. The end of the request then waits for the task to stop, however long that takes, and logs a warning once the few-second budget has run out. With traditional, Framework or SPA routing the response waits with it, unless the script already sent it with `oxphp_finish_request()`; a streamed response stays open until the task stops. In Worker mode the response is not held, and what the task captured is released once the task stops, but a worker that is recycled or shut down waits for the task before it exits. A task that never stops — one that catches the cancellation and keeps going, or one inside a call into C code that does not return — then keeps that PHP worker from finishing, the request's end with traditional, Framework or SPA routing and its exit in Worker mode, until the server is restarted. This covers only the closure the request itself passes to `oxphp_async()`. A closure the task creates while it runs still has the reference counts of its code and name changed from the async worker thread, and the request does not wait for a task that another task dispatched without awaiting it, so without OPcache that task can still run code that has been freed.

## Best Practices

- **Always set timeouts** on `oxphp_async_await()` calls in production to prevent indefinite waits
- **Use worker mode** to get non-blocking fiber-based awaiting instead of blocking the worker thread
- **Keep closures small** — dispatch focused units of work, not entire request handlers
- **Extract scalars before dispatch** — pull IDs, strings, and config values out of objects before passing to the closure
- **Monitor the async pool** — check `oxphp_async_tasks_rejected_total` in Prometheus metrics. If rejections are rising, increase `ASYNC_WORKERS` or `ASYNC_QUEUE_CAPACITY`

## See Also

- [Worker Mode](worker-mode.md) -- persistent PHP workers with fiber-based concurrency
- [PHP Functions](../php/functions.md) -- `oxphp_async()`, `oxphp_async_await()`, and related function reference
- [Metrics](../operations/metrics.md) -- async pool Prometheus metrics
- [Configuration Reference](../operations/configuration.md) -- `ASYNC_WORKERS` and `ASYNC_QUEUE_CAPACITY`
