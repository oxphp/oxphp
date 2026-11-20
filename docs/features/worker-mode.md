---
title: Worker Mode
description: Persistent PHP workers that bootstrap once and handle multiple requests, eliminating per-request startup overhead in OxPHP.
---

# Worker Mode

Worker mode runs persistent PHP workers that bootstrap once and handle multiple requests, eliminating per-request startup overhead. Instead of tearing down and rebuilding PHP state on every request, your application loads its autoloader, configuration, and database connections a single time and reuses them across the lifetime of the worker.

## How It Works

1. **Set `WORKER_MODE_ENABLED=true`** and point **`ENTRY_FILE`** at your bootstrap script. This enables worker mode for all PHP workers in the pool.
2. **PHP starts and runs the outer scope once** — autoloader registration, configuration loading, database connections, and any other initialization code execute a single time.
3. **Call `oxphp_worker(callback)`** to enter the request loop. OxPHP begins dispatching incoming HTTP requests to your callback.
4. **Between requests**, superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`, `$_REQUEST`, `php://input`), output buffers, response headers, the session a request opened, the ini directives a request changed, and the environment variables a request set with `putenv()` are reset automatically — see [What Gets Reset Between Requests](worker-state.md#what-gets-reset-between-requests) for what that covers and where it stops. A soft reset cleans per-request state while preserving bootstrapped resources in the outer scope. `$_ENV` is a deliberate exception: with PHP's default `auto_globals_jit=1` it is **not** reset — see [Superglobals](../php/superglobals.md#_env).
5. **Outer scope persists** — variables defined before `oxphp_worker()`, static properties, database connections, and autoloaders remain available across all requests handled by that worker.

> **Note:** Worker mode changes routing behavior. All requests that do not match a static file on disk are dispatched to the worker instead of returning 404. See [Routing](routing.md) for details.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_MODE_ENABLED` | `false` | Enable persistent worker mode. Accepts `true`, `1`, `yes`. Requires `ENTRY_FILE` to point at a `.php` script |
| `ENTRY_FILE` | *(unset)* | Path to the worker bootstrap script. Resolved against `DOCUMENT_ROOT` when relative; `..` segments and absolute paths are allowed (worker bootstraps living outside the public document root are a supported layout). A worker that serves no static files can run with `DOCUMENT_ROOT=false` and an absolute `ENTRY_FILE` — see [No Document Root](routing.md#no-document-root) |
| `WORKER_MAX_MEMORY_MIB` | `0` | Maximum PHP memory per worker in MiB before recycling. `0` = unlimited |

For application-driven recycling, call [`OxPHP\Server\Worker::scheduleExit()`](../php/worker-class.md#scheduleexit) from inside a request handler. The worker exits cleanly after the current request completes.

## Writing a Worker Script

A worker script has two parts: the outer scope that runs once at startup, and the callback passed to `oxphp_worker()` that runs on every request.

```php
<?php
// Outer scope: runs once at startup
require __DIR__ . '/../vendor/autoload.php';

$config = parse_ini_file(__DIR__ . '/../config/app.ini');
$db = new PDO($config['dsn'], $config['user'], $config['pass'], [
    PDO::ATTR_PERSISTENT => true,
]);

$app = new MyApp\Application($config, $db);

// Request loop: runs for every request
oxphp_worker(function () use ($app) {
    $app->handle();
});

// Teardown: runs when this worker's loop ends, which is not
// only at server shutdown — see Recycling below
$app->terminate();
```

## What Gets Reset Between Requests

Between requests OxPHP resets the state a request owns: superglobals and `php://input`, output buffers, response headers, error state, the session the request opened, the ini directives it changed and the environment variables it set with `putenv()`. The reset has limits — some of that state stays with the worker while other requests on it are in flight, and a few settings are not put back — and they are listed with each item in [Worker State Between Requests](worker-state.md#what-gets-reset-between-requests).

## What Persists

State the worker holds rather than a request survives across requests: variables the handler captures from the outer scope, static properties, open database connections, registered autoloaders, loaded classes and functions, and the ini directives and environment variables set during bootstrap. `$_ENV` is deliberately not reset. See [What Persists](worker-state.md#what-persists).

## Recycling

A worker is recycled — its loop ends and its PHP state is torn down — when its memory passes `WORKER_MAX_MEMORY_MIB`, when the handler calls [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit), on an OPcache restart, after three consecutive requests that came apart, and in a few cases where a request that ended abnormally leaves the worker unfit to keep serving. Whether a fresh worker takes its place is decided by the pool. [Worker Recycling](worker-recycling.md#recycling) lists the conditions, which failures count, and what happens to the other requests the worker was serving.

## Request Deadlines

Every request a worker serves is bounded by `max_execution_time` on its own, whether the worker took it with nothing else running or while it was carrying others — with [fiber multiplexing](fiber-multiplexing.md) a worker runs several requests on one thread, and each still gets the whole of its limit and no more:

- **A request's limit counts from the moment the worker takes it.** It starts at the value of `max_execution_time` the worker's bootstrap left — the `php.ini` value, or whatever the outer scope of the worker script set — and, as everywhere in OxPHP's thread-safe PHP build, it counts wall-clock time, time spent suspended included (see [Timeouts](timeouts.md#wall-clock-not-cpu-time)).
- **`set_time_limit()` applies to the request that calls it.** It restarts that request's limit from the call, as it does under PHP-FPM, and leaves every other request on the worker as it was. Only the request's own deadline moves with it; the directive the call writes stays with the worker (see the ini boundaries under [What Gets Reset Between Requests](worker-state.md#what-gets-reset-between-requests)).
- **When a limit runs out, it ends the request it belongs to.** That request is read as having run out of time: a `504`, the timeout bit set in its own `connection_status()` for its shutdown functions, neutral for the recycle count. The requests running beside it carry on.
- **A suspended request is ended when it resumes, not at the moment its deadline passes.** A request waiting in `oxphp_async_await()`, `oxphp_sleep()`/`oxphp_usleep()`, or a `sleep()` or a socket wait (a read, and with `net` a connect, a TLS handshake or a write that waits for room) taken over by `RUNTIME_HOOKS` runs no PHP code, and a time limit can only end code that runs. A request whose limit passed while it waited is ended as soon as it is back in PHP code: once the call it was waiting in returns, the request ends there as at any other timeout, its shutdown functions included.

  A suspended request resumes when what it waits for arrives — the promise settles, the sleep elapses, the socket becomes readable (or writable, for a write waiting for room, or connected, for a connect) or the wait reaches its timeout — the stream's own (`default_socket_timeout` unless `stream_set_timeout()` set another) for a read or a write, the timeout passed to `stream_socket_client()` or `fsockopen()` for a connect — or when the server cancels it.

  A call that waits once — a sleep, a single read — returns when it is resumed; one that waits again and again inside, such as `stream_get_contents()` reading a stream to its end, returns only when it is done as a whole. Two waits are the exception:

  - `session_start()` waiting for another request's session to close (see [Worker State Between Requests](worker-state.md#a-worker-has-one-session-open-at-a-time)) is resumed every quarter of a second, and returns, without opening the session, the first time it is resumed out of time.
  - a streaming response's flush that waits for its client to make room (see [Server-Sent Events](sse.md#how-it-works)) is resumed whenever the client takes a chunk and otherwise at least every 16 milliseconds, and returns the first time it is resumed out of time, having sent what it was waiting to send only if the client made room for it by then.

  A server shutdown that passes its drain deadline, and a recycle of the worker, cancel every request suspended at that moment, and one whose limit had already passed is ended by that cancellation like the others: a `503`, with its shutdown functions run and not cut short by the spent limit.

- **What a worker runs between requests is under no limit.** The destructors of objects the worker's periodic garbage collection frees run outside any request, and a time limit bounds none of that.

Nothing else ends a suspended request for you. A client disconnecting does not end a non-streaming request, which runs to its end by design (see [Recycling](worker-recycling.md#recycling)), and a request running longer than the supervisor's threshold is reported through `oxphp_worker_stuck_total`, not cancelled. Put the limit on the wait itself: pass a timeout to `oxphp_async_await()` and its variants, and set timeouts on sockets and database clients.

## Development Reloading

Worker Mode persists bootstrap state (autoloader, DI container, DB connections) in memory, so `opcache.validate_timestamps=1` alone is not enough to pick up changes to code that ran during the outer scope. For development loops there are two options:

- **Recycle every request.** Call `OxPHP\Server\Worker::current()->scheduleExit()` at the end of every handler invocation (gated on a `OXPHP_DEV` env flag, for example). The current request completes normally, then the worker exits and a replacement re-executes the outer scope — immediately on a static pool, which is what a development setup normally runs. This trades the worker-mode performance win for FPM-style reload semantics — simplest and most reliable for active development.
- **Keep the worker warm, reload request handlers.** Skip `scheduleExit()` entirely, enable `opcache.validate_timestamps=1`, and keep your bootstrap minimal. Code loaded inside the request callback will be refreshed by OPcache on the next request; code loaded once in the outer scope will not. See [OPcache and JIT → Development Settings](../php/opcache.md#development-settings) for the full list of caveats.

## Troubleshooting

### Requests hang and never complete

If `oxphp_worker()` is never called in the bootstrap script, no requests are dispatched and every request waits indefinitely. Verify that your script calls `oxphp_worker()` unconditionally in the normal code path.

### State leaks between requests

Variables defined inside the `oxphp_worker()` callback are cleaned up by PHP's garbage collector, but static properties and globals defined in the outer scope persist. If you see data from one request appearing in another, check for static properties or global variables that accumulate state across calls.

**Fix:** Reset static state explicitly at the start of each request callback, or avoid storing per-request state in statics.

### 500s on a fresh worker with `RUNTIME_HOOKS`

**Symptom:** Requests fail with an application error soon after a worker starts or recycles, and only with `RUNTIME_HOOKS` on. Yii3 reports `CircularReferenceException: Circular reference to "Yiisoft\Router\Middleware\Router" detected while building`.

**Cause:** One request is building a shared service, the build waits on a hooked socket, and a second request reaches the same half-built service. A worker [runs its first request alone](runtime-hooks-shared-state.md#hooked-waits-and-shared-application-state) until it ends or 250 ms after it first waits, so this takes a build still running after that, or one that happens in a later request.

**Fix:** Build the service in the worker script before `oxphp_worker()`. On Yii3 this line cures this exception: `$container->get(\Yiisoft\Router\Middleware\Router::class);`. If the errors instead say that per-request state is already set, or it disappears halfway through a request, the state lives in a shared service and building ahead will not help: run without `RUNTIME_HOOKS`. Yii3's `CurrentRoute` is such a service, so this applies to any Yii3 handler that waits on a hooked socket.

### Worker recycles immediately (memory limit)

The worker memory limit is checked after each request using PHP's reported memory usage. If your bootstrap phase allocates a large amount of memory (e.g. loading a large cache), the initial memory footprint may already be close to the limit.

**Fix:** Increase `WORKER_MAX_MEMORY_MIB` or defer large allocations to the first request. With `RUNTIME_HOOKS` on, read [500s on a fresh worker](#500s-on-a-fresh-worker-with-runtime_hooks) before moving anything that waits on I/O there.

**Check:** At `LOG_LEVEL=info`, each such recycle logs the usage that passed the limit as `memory_bytes`, next to the limit as `max_memory_bytes`. Every earlier check found the usage within the limit, so the difference between the two is at most what the worker grew by since the last check before it: a small one fits gradual growth, a large one a single jump. On a worker that recycles after its first request, that jump includes everything the bootstrap allocated.

```bash
docker logs <container> 2>&1 | grep '"reason":"max_memory"'
```

### Worker recycles immediately (error limit)

Three consecutive counted failures trigger a recycle. A fatal error in the request callback is the usual one, so check your application logs for those first — not for uncaught exceptions, and not for cancelled requests, unless a fatal was reported on the cancelled request as well, before or after the cancellation. A recycle with no fatal behind it is one of the two fiber failures instead, and the `WARN` line below names which: `fiber would not start` points at `fiber.stack_size` or at the host — that C stack is mapped from the operating system rather than allocated from PHP's heap, so neither `memory_limit` nor the `WORKER_MAX_MEMORY_MIB` above is the knob for it — and `fiber refused to park` points at a `Fiber::suspend()` your own code keeps retrying.

The server says so itself at `WARN`: one line per counted failure, naming the worker, where the failure came from and how many have run consecutively, and one line for the recycle that follows the third. The worker is named because the count is per worker — three lines from one worker are a recycle, three lines from three workers are not.

**Check:** Look for errors in the access log or structured log output:

```bash
docker logs <container> 2>&1 | grep -E '"level":"(ERROR|WARN)"'
```

### Leaks or skipped destructors under Xdebug

Xdebug's `develop` mode — the default when `xdebug.mode` is not set, and in force whenever `develop` is one of the modes listed — takes over PHP's error reporting from the first request on and does not pass errors on, so the worker's own handling of an error never runs. On a worker serving such requests:

- a request ended by a fatal error leaves what its abandoned frames were holding allocated for the life of the worker, instead of the worker giving it back;
- after a request exceeds `memory_limit`, the worker neither hands that memory back and puts the limit back in force, nor recycles for it, as [Recycling](worker-recycling.md#recycling) describes;
- after a fatal error, the objects the worker keeps and those of the other requests in flight on it are released without their destructors running, as PHP does after a fatal, rather than as [Objects after a fatal error](worker-state.md#what-gets-reset-between-requests) describes.

`debug` passes errors on to what stood before it, so none of this happens under it, but like `develop` it keeps the server's own error callback out of the chain: under either, errors are not logged with the server's structured fields, running out of `max_execution_time` is answered like any other fatal error instead of with `504`, and in worker mode the status of a fatal error is left to PHP, which answers `500` only with `display_errors` off. `profile`, `trace` and `off` leave error handling alone. Keep `develop` for development.

### Database connection drops after idle

If your database server closes idle connections, reconnect attempts in the next request may fail. Use a connection pool that handles reconnection, or catch the exception and reconnect manually.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.15.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - WORKER_MODE_ENABLED=true
      - ENTRY_FILE=/var/www/html/worker.php
      - WORKER_MAX_MEMORY_MIB=128
```

## PHP API

Worker introspection and the worker entry point are exposed through the
[`OxPHP\Server\Worker`](../php/worker-class.md) class.

```php
<?php
$worker = OxPHP\Server\Worker::current();
$worker->serve(function () {
    handleRequest();
});
```

**Legacy free functions** (`oxphp_is_worker`, `oxphp_worker_id`, `oxphp_worker`)
remain available and route through the same internal state. New code should
prefer the class API.

The class also exposes runtime introspection useful for graceful self-recycling, observability, and health checks:

| Method | Returns |
|--------|---------|
| `Worker::isWorkerMode(): bool` | Whether the server is running in worker mode |
| `$worker->id(): int` | Stable per-thread worker ID |
| `$worker->startTime(): float` | Unix timestamp of when this worker started |
| `$worker->requestCount(): int` | Number of requests this worker has handled |
| `$worker->memoryUsage(): int` | Current `memory_get_usage(true)` for this worker |
| `$worker->rss(): int` | Current resident set size in bytes (Linux/macOS) |
| `$worker->maxMemoryBytes(): int` | Recycle threshold — `WORKER_MAX_MEMORY_MIB` × 1 MiB, or `0` when unlimited |
| `$worker->isExitScheduled(): bool` | Whether `scheduleExit()` has been called |
| `$worker->exitReason(): ?string` | `null` while running; `"scheduled"`, `"max_memory"`, or `"error"` once the worker is going down |

See [`OxPHP\Server\Worker`](../php/worker-class.md) for full signatures and worked examples.

## PHP Examples

### Detecting Worker Mode

Use `OxPHP\Server\Worker::isWorkerMode()` to check whether the current process is running in worker mode. This is useful for writing code that works in both traditional and worker mode.

```php
<?php
if (OxPHP\Server\Worker::isWorkerMode()) {
    // Reuse a persistent connection
    $redis = new Redis();
    $redis->pconnect('redis', 6379);
} else {
    // Traditional mode: connect per request
    $redis = new Redis();
    $redis->connect('redis', 6379);
}
```

### Symfony Worker Script

```php
<?php
use App\Kernel;

require __DIR__ . '/../vendor/autoload.php';

$kernel = new Kernel('prod', false);
$kernel->boot();

oxphp_worker(function () use ($kernel) {
    $request = Symfony\Component\HttpFoundation\Request::createFromGlobals();
    $response = $kernel->handle($request);
    $response->send();
    $kernel->terminate($request, $response);
});

$kernel->shutdown();
```

## Best Practices

- **Set `WORKER_MAX_MEMORY_MIB`** (e.g. `128`) so a leaking worker recycles automatically instead of consuming the host. Combine with `Worker::scheduleExit()` for application-driven recycling on top. A worker can grow even when the application leaks nothing: when a fatal error or a cancellation abandons a request inside an internal function, what that function had allocated for itself is not given back. A request cancelled in the middle of `usort()` keeps the whole array being sorted alive, and a worker that grew by more than 4 MiB that way is recycled, as described under [Recycling](worker-recycling.md#recycling); smaller leaks stay until `WORKER_MAX_MEMORY_MIB` is passed or, without it, until requests start failing against `memory_limit`. A fatal error or a cancellation that ends a request inside a `Fiber` it runs — one it started, or the one PHP runs garbage-collection destructors in — leaves what the fiber's own frames were holding allocated for the life of the worker in the same way.
- **Avoid storing per-request state in static properties or globals.** Since these persist across requests, leftover state from one request can leak into another.
- **Validate the soft reset early.** Add `Worker::current()->scheduleExit()` to your handler under a development flag and exercise the application end-to-end — this catches state-leak bugs before you commit to long-lived workers.
- **Handle database idle timeouts.** If your database driver disconnects after an idle period, catch the exception and reconnect, or use a connection pool that handles reconnection automatically.
- **Keep the outer scope minimal.** Only bootstrap what truly needs to persist — autoloaders, configuration, and shared services. Defer request-specific setup to the callback. With `RUNTIME_HOOKS` on, build shared services here rather than lazily in a request: [a request can overlap the build](#500s-on-a-fresh-worker-with-runtime_hooks).

## See Also

- [Routing](routing.md) — how worker mode plugs into URL routing
- [Early Response](early-response.md) — send the response immediately and continue background processing
- [PHP Functions](../php/functions.md) — full reference for `oxphp_worker()`, `oxphp_is_worker()`, and other built-in functions
- [Configuration Reference](../operations/configuration.md) — complete list of environment variables
