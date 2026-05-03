---
title: Worker Mode
description: Persistent PHP processes that bootstrap once and handle multiple requests, eliminating per-request startup overhead in OxPHP.
---

# Worker Mode

Worker mode runs persistent PHP processes that bootstrap once and handle multiple requests, eliminating per-request startup overhead. Instead of tearing down and rebuilding PHP state on every request, your application loads its autoloader, configuration, and database connections a single time and reuses them across the lifetime of the worker.

## How It Works

1. **Set `WORKER_FILE`** to the path of your bootstrap script. This enables worker mode for all PHP workers in the pool.
2. **PHP starts and runs the outer scope once** — autoloader registration, configuration loading, database connections, and any other initialization code execute a single time.
3. **Call `oxphp_worker(callback)`** to enter the request loop. OxPHP begins dispatching incoming HTTP requests to your callback.
4. **Between requests**, superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`, `php://input`), output buffers, and response headers are reset automatically. A soft reset cleans per-request state while preserving bootstrapped resources in the outer scope.
5. **Outer scope persists** — variables defined before `oxphp_worker()`, static properties, database connections, and autoloaders remain available across all requests handled by that worker.

> **Note:** Setting `WORKER_FILE` also changes routing behavior. All requests that do not match a static file on disk are dispatched to the worker instead of returning 404. See [Routing](routing.md) for details.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_FILE` | *(unset)* | Path to the worker PHP script. Setting this enables worker mode |
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

// Shutdown: runs when the worker exits
$app->terminate();
```

## What Gets Reset Between Requests

OxPHP performs a soft reset between requests. The following state is cleaned automatically:

- **Superglobals** — `$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`, and `php://input` are repopulated with the new request data
- **Output buffers** — all output buffers are flushed and cleaned
- **Response headers** — HTTP status code and headers are reset to defaults
- **Error state** — last error information (message, file, line, type) and connection status are cleared. User-registered error handlers (`set_error_handler()`), exception handlers (`set_exception_handler()`), and the `error_reporting()` level persist across requests

## What Persists

The following state survives across requests within the same worker:

- **Variables in outer scope** — anything defined before `oxphp_worker()` and captured via `use`
- **Static properties** — class static properties retain their values
- **Database connections** — PDO, MySQLi, and other persistent connections remain open
- **Autoloaders** — registered autoloaders (Composer, custom) remain active
- **Loaded classes and functions** — all previously loaded classes, interfaces, traits, and functions

## Recycling

Workers are automatically recycled (restarted with a fresh PHP process) when any of the following conditions are met:

- **Max memory exceeded** — the worker's PHP memory usage exceeds `WORKER_MAX_MEMORY_MIB` MiB
- **Application requested exit** — the handler called [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit). Useful for app-controlled hot reload, file-mtime-based reload, or per-request bootstrap re-execution
- **Consecutive errors** — the worker encounters 3 consecutive handler failures (fatal errors, timeouts, or unhandled exceptions). Note that `exit()`/`die()` calls are not counted as failures

When a worker is recycled, the PHP process terminates and a new one starts, re-executing the outer scope of the worker script. For memory-based and scheduled exit, the current request completes normally before the worker exits. For error-based recycling, the worker exits after the failed request.

## Development Reloading

Worker Mode persists bootstrap state (autoloader, DI container, DB connections) in memory, so `opcache.validate_timestamps=1` alone is not enough to pick up changes to code that ran during the outer scope. For development loops there are two options:

- **Recycle every request.** Call `OxPHP\Server\Worker::current()->scheduleExit()` at the end of every handler invocation (gated on a `OXPHP_DEV` env flag, for example). The current request completes normally, then the worker exits and is respawned, re-executing the outer scope. This trades the worker-mode performance win for FPM-style reload semantics — simplest and most reliable for active development.
- **Keep the worker warm, reload request handlers.** Skip `scheduleExit()` entirely, enable `opcache.validate_timestamps=1`, and keep your bootstrap minimal. Code loaded inside the request callback will be refreshed by OPcache on the next request; code loaded once in the outer scope will not. See [OPcache and JIT → Development Settings](../php/opcache.md#development-settings) for the full list of caveats.

## Troubleshooting

### Requests hang and never complete

If `oxphp_worker()` is never called in the bootstrap script, no requests are dispatched and every request waits indefinitely. Verify that your script calls `oxphp_worker()` unconditionally in the normal code path.

### State leaks between requests

Variables defined inside the `oxphp_worker()` callback are cleaned up by PHP's garbage collector, but static properties and globals defined in the outer scope persist. If you see data from one request appearing in another, check for static properties or global variables that accumulate state across calls.

**Fix:** Reset static state explicitly at the start of each request callback, or avoid storing per-request state in statics.

### Worker recycles immediately (memory limit)

The worker memory limit is checked after each request using PHP's reported memory usage. If your bootstrap phase allocates a large amount of memory (e.g. loading a large cache), the initial memory footprint may already be close to the limit.

**Fix:** Increase `WORKER_MAX_MEMORY_MIB` or defer large allocations to the first request.

### Worker recycles immediately (error limit)

Three consecutive handler failures trigger a recycle. Check your application logs for exceptions or fatal errors occurring in the request callback.

**Check:** Look for errors in the access log or structured log output:

```bash
docker logs <container> 2>&1 | grep '"level":"error"'
```

### Database connection drops after idle

If your database server closes idle connections, reconnect attempts in the next request may fail. Use a connection pool that handles reconnection, or catch the exception and reconnect manually.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INDEX_FILE=index.php
      - WORKER_FILE=/var/www/html/worker.php
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

- **Set `WORKER_MAX_MEMORY_MIB`** (e.g. `128`) so a leaking worker recycles automatically instead of consuming the host. Combine with `Worker::scheduleExit()` for application-driven recycling on top.
- **Avoid storing per-request state in static properties or globals.** Since these persist across requests, leftover state from one request can leak into another.
- **Validate the soft reset early.** Add `Worker::current()->scheduleExit()` to your handler under a development flag and exercise the application end-to-end — this catches state-leak bugs before you commit to long-lived workers.
- **Handle database idle timeouts.** If your database driver disconnects after an idle period, catch the exception and reconnect, or use a connection pool that handles reconnection automatically.
- **Keep the outer scope minimal.** Only bootstrap what truly needs to persist — autoloaders, configuration, and shared services. Defer request-specific setup to the callback.

## See Also

- [Routing](routing.md) — how `WORKER_FILE` affects URL routing
- [Early Response](early-response.md) — send the response immediately and continue background processing
- [PHP Functions](../php/functions.md) — full reference for `oxphp_worker()`, `oxphp_is_worker()`, and other built-in functions
- [Configuration Reference](../operations/configuration.md) — complete list of environment variables
