---
title: Worker Class
description: Reference for the OxPHP\Server\Worker class — a unified runtime handle for worker introspection, the worker entry point, and per-thread metrics.
---

# Worker Class

`OxPHP\Server\Worker` is the unified runtime handle for everything that relates to a single OxPHP OS worker thread. It is a final, stateless wrapper over the bridge thread-local state and is registered by the SAPI extension itself, so it is always available — both in traditional mode and in [worker mode](../features/worker-mode.md). Each call reads live state directly from the runtime; the object itself caches nothing.

`Worker::current()` returns a singleton per OS thread: two calls on the same thread always return the same instance.

## Quick reference

| Method | Description |
|--------|-------------|
| `Worker::current(): self` | Returns the singleton handle for the current OS thread. |
| `Worker::isWorkerMode(): bool` | Returns `true` if the server is running in worker mode (i.e. `WORKER_FILE` is set). |
| `getId(): int` | Numeric worker identifier in the range `0..N-1` for the current OS thread. |
| `getStartTime(): float` | Unix timestamp (seconds) when this OS worker thread was spawned. |
| `getRequestCount(): int` | 1-based count of requests handled by this OS thread. Increases in both modes. |
| `getMemoryUsage(): int` | Live PHP memory usage in bytes (`zend_memory_usage(0)`). |
| `getRss(): int` | Process resident set size in bytes. Uncached — call at most once per request. |
| `getMaxMemoryBytes(): int` | Configured memory cap in bytes. `0` means unlimited. |
| `scheduleExit(): void` | Marks the worker for graceful exit after the current request completes. No-op in traditional mode. |
| `isExitScheduled(): bool` | Returns `true` if `scheduleExit()` has been called for the current worker. Always `false` in traditional mode. |
| `getExitReason(): ?string` | Pending exit reason: `'scheduled'`, `'max_memory'`, `'error'`, or `null` when no exit is pending. Always `null` in traditional mode. |
| `serve(callable $h): void` | Enters the request loop. Throws `InvalidServeContextException` outside worker mode. |

## Mode matrix

| Method | Traditional mode | Worker mode |
|--------|------------------|-------------|
| `current()` | Singleton per OS thread. | Singleton per OS thread. |
| `isWorkerMode()` | `false` | `true` |
| `getId()` | OS-thread index in the worker pool. | OS-thread index in the worker pool. |
| `getStartTime()` | Time the OS thread was spawned (typically server start). | Time the OS thread was spawned. |
| `getRequestCount()` | 1-based, increments across requests reusing the same OS thread (`1, 2, 3, …`). | 1-based, increments per request handled by the worker. |
| `getMemoryUsage()` | Live PHP memory at the moment of the call. | Live PHP memory at the moment of the call. |
| `getRss()` | Live process RSS. | Live process RSS. |
| `getMaxMemoryBytes()` | `0` (no recycle cap applies). | Value of `WORKER_MAX_MEMORY_MIB` × 1 MiB, or `0` if unset. |
| `scheduleExit()` | No-op (the script is exiting anyway). | Sets the exit flag; the request loop stops after the current handler returns. |
| `isExitScheduled()` | Always `false`. | `true` after `scheduleExit()` has been called on this thread. |
| `getExitReason()` | Always `null`. | `null` until an exit is pending; then one of `'scheduled'`, `'max_memory'`, `'error'`. |
| `serve(callable)` | Throws `OxPHP\Server\Exception\InvalidServeContextException`. | Enters the request loop. |

## Examples

### Per-worker logging context

Tag every log line with the worker id and per-thread request counter so you can correlate request traffic with a specific worker.

```php
<?php
$worker = OxPHP\Server\Worker::current();

$logger->info('handling request', [
    'worker_id'      => $worker->getId(),
    'request_number' => $worker->getRequestCount(),
]);
```

### Bootstrap once per OS thread

`getRequestCount()` is 1-based, so the first request handled by any thread sees the value `1`. This is a portable place to run lazy, per-thread initialization that should happen exactly once.

```php
<?php
$worker = OxPHP\Server\Worker::current();

if ($worker->getRequestCount() === 1) {
    bootstrap();
}
```

### scheduleExit

Application-driven worker recycling. The current request completes normally; the loop checks `isExitScheduled()` afterwards and breaks out. The supervisor respawns a fresh worker, re-running the outer scope of the worker file.

```php
<?php
$worker = OxPHP\Server\Worker::current();

handleRequest();

// Reload bootstrap on every request when developing locally.
if (getenv('OXPHP_DEV') === '1') {
    $worker->scheduleExit();
}
```

`scheduleExit()` is idempotent and a no-op outside worker mode. Use cases:

- **Hot reload during development** — exit after every request so the outer-scope bootstrap re-runs.
- **RSS-based recycling** — `WORKER_MAX_MEMORY_MIB` measures only the Zend allocator. With extension-heavy stacks (curl, mysqli) you can additionally recycle when the process RSS crosses your own threshold:

  ```php
  if ($worker->getRss() > 256 * 1024 * 1024) {
      $worker->scheduleExit();
  }
  ```

- **Coordinated rolling restarts** — gate the call on a sentinel file or signal so an external orchestrator can drain workers cleanly.

### Worker entry point

In a `WORKER_FILE` script, call `serve()` to enter the request loop.

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

OxPHP\Server\Worker::current()->serve(function () {
    handleRequest();
});
```

### RSS observability

`getRss()` returns the live process resident set size in bytes. The call is a real syscall — cheap, but not free. Read it at most once per request.

```php
<?php
$worker = OxPHP\Server\Worker::current();
$rss = $worker->getRss();

$metrics->gauge('php_worker_rss_bytes', $rss, [
    'worker_id' => (string) $worker->getId(),
]);
```

## Migration from `oxphp_*` functions

The legacy free functions remain available and route through the same internal state — they are not deprecated. New code should prefer the class API for discoverability and consistency.

| Legacy function | Class API |
|-----------------|-----------|
| `oxphp_is_worker()` | `OxPHP\Server\Worker::isWorkerMode()` |
| `oxphp_worker_id()` | `OxPHP\Server\Worker::current()->getId()` |
| `oxphp_worker(callable)` | `OxPHP\Server\Worker::current()->serve(callable)` |

## Caveats

- **`getRss()` is uncached.** Each call performs a syscall (`/proc/self/statm` read on Linux, `getrusage(RUSAGE_SELF)` on macOS). Cheap but not free — call it at most once per request, typically inside a metrics handler, not on every log line.
- **Cloning is forbidden.** `clone $worker` throws `\Error("Cloning OxPHP\\Server\\Worker is not allowed")`. The worker handle represents OS-thread identity; cloning it would create the misleading impression of a second handle for the same thread.
- **Outside an OxPHP host** (e.g. when an extension that links the SAPI is loaded into PHP CLI), `Worker::current()` still returns an instance, but every accessor returns its zero-state value: `getId()` is `0`, `getStartTime()` is the process start time, `getRequestCount()` is `0`, `getRss()` is the live RSS, and `serve()` throws `InvalidServeContextException`.

## See Also

- [Worker Mode](../features/worker-mode.md) — overview of persistent PHP processes and the bootstrap-once pattern
- [PHP Functions](functions.md) — reference for the legacy `oxphp_*` free functions
- [Request API](request-api.md) — `OxPHP\Http\RequestInterface::startTime()` for per-request timing
