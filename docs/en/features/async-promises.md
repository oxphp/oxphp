---
title: Async Promises
description: Execute PHP closures asynchronously on a dedicated worker pool
---

OxPHP provides a promise-based async API that lets PHP code dispatch closures for execution on a dedicated thread pool, separate from the HTTP worker pool. This enables true parallel computation without blocking the request thread.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0` (disabled) | Number of dedicated async worker threads. `0` disables the async pool |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS * 64` | Bounded channel size for pending async tasks. Tasks are rejected when full |

To enable the async pool:

```bash
ASYNC_WORKERS=4
```

Set to `0` (or leave unset) to disable async support entirely. When disabled, calling `oxphp_async()` emits an `E_WARNING` and returns `false`.

## PHP API

### `oxphp_async`

Dispatches a closure for asynchronous execution on the async worker pool.

```php
oxphp_async(Closure $closure, mixed ...$args): int|false
```

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `$closure` | `Closure` | The closure to execute asynchronously |
| `...$args` | `mixed` | Arguments passed to the closure via deep copy |

**Return value:** A promise ID (positive integer) on success, or `false` if the async pool is not configured or the queue is full.

**Example:**

```php
<?php
$promise = oxphp_async(function(int $n): int {
    return $n * $n;
}, 42);

$result = oxphp_async_await($promise);
// 1764
```

### `oxphp_async_await`

Blocks the current thread until the async task completes and returns the result.

```php
oxphp_async_await(int $promise_id, ?float $timeout = null): mixed
```

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `$promise_id` | `int` | *(required)* | The promise ID returned by `oxphp_async()` |
| `$timeout` | `?float` | `null` | Timeout in seconds. `null` waits indefinitely |

**Return value:** The return value of the closure.

**Throws:**

| Exception | When |
|-----------|------|
| `OxPHP\AsyncException` | The closure threw an exception or called `die()`/`exit()` |
| `OxPHP\AsyncTimeoutException` | The timeout expired before the task completed |

### `oxphp_async_await_all`

Awaits multiple promises and returns all results as an array.

```php
oxphp_async_await_all(array $promise_ids, ?float $timeout = null): array
```

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `$promise_ids` | `array` | *(required)* | Array of promise IDs |
| `$timeout` | `?float` | `null` | Per-promise timeout in seconds |

**Return value:** An associative array mapping promise IDs to their results.

**Throws:** `OxPHP\AsyncException` or `OxPHP\AsyncTimeoutException` if any promise fails or times out.

### `oxphp_async_await_any`

Races multiple promises and returns the first to complete, regardless of array order. Uses `futures::select_all` internally for true concurrent race semantics. Non-winning promises remain individually awaitable via `oxphp_async_await()`.

```php
oxphp_async_await_any(array $promise_ids, ?float $timeout = null): array
```

**Parameters:**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `$promise_ids` | `array` | *(required)* | Array of promise IDs |
| `$timeout` | `?float` | `null` | Overall timeout in seconds for any promise to complete |

**Return value:** An associative array with `id` (the winning promise ID) and `value` (its result).

**Throws:** `OxPHP\AsyncException` if the first completed promise threw an exception. `OxPHP\AsyncTimeoutException` if no promise completes within the timeout.

**Note:** On timeout, all specified promises are cancelled and cannot be awaited individually afterwards.

## Data Transfer Semantics

Closures execute on a different OS thread with its own PHP ZTS engine state and `zend_mm_heap`. Data cannot be shared by pointer between threads — any pointer to a string or array allocated on one thread's heap becomes invalid on another thread. OxPHP uses **portable binary serialization** to safely transfer all data across thread boundaries.

### How it works

All values crossing thread boundaries (arguments, `use`-variables, return values) are serialized into a flat byte buffer using system `malloc`. The buffer is transferred to the destination thread, which deserializes it using `emalloc` on its own heap. This guarantees that every PHP allocation belongs to the correct per-thread `zend_mm_heap`.

### Captured variables (via `use`)

Variables captured by the closure's `use` clause are **serialized** on the source thread and **deserialized** as independent copies on the async worker thread. The source variables remain unchanged and fully usable during async execution.

```php
<?php
$config = ['db' => 'mysql', 'timeout' => 30];

$p = oxphp_async(function() use ($config): string {
    // $config is an independent copy — reads and writes work normally
    return $config['db'];
});

// $config remains writable here — it was copied, not frozen
$config['timeout'] = 60; // safe
$result = oxphp_async_await($p);
```

### Arguments

Arguments passed via `...$args` are serialized and deserialized the same way. Supported types:

| Type | Transfer |
|------|----------|
| `null`, `bool`, `int`, `float` | Serialized as value (1–9 bytes) |
| `string` | Serialized with length prefix + data |
| `array` | Recursive serialization (keys + values) |
| `resource` | **Rejected** — throws `OxPHP\AsyncException` |
| `object` | **Rejected** — throws `OxPHP\AsyncException` (objects cannot be serialized across threads) |

### Return values

The closure's return value is serialized on the async worker thread and deserialized on the source thread using the same mechanism. All scalar types, strings, and arrays (including nested) are supported.

## Exception Handling

Exceptions thrown inside the async closure are caught and re-thrown as `OxPHP\AsyncException` when `oxphp_async_await()` is called. The original exception class and message are preserved in the `AsyncException` message:

```php
<?php
$p = oxphp_async(function(): never {
    throw new \DomainException('invalid value', 422);
});

try {
    oxphp_async_await($p);
} catch (\OxPHP\AsyncException $e) {
    echo $e->getMessage();
    // "Async task failed: [DomainException] invalid value"
}
```

### die() and exit()

Calls to `die()` or `exit()` inside async closures are caught by `zend_try`/`zend_catch` and converted to `OxPHP\AsyncException`. The async worker pool survives — subsequent tasks execute normally:

```php
<?php
// Round 1: die() — caught as AsyncException
$p1 = oxphp_async(function(): never { die('fatal'); });
try { oxphp_async_await($p1); } catch (\OxPHP\AsyncException $e) { /* handled */ }

// Round 2: pool is alive, normal task works
$p2 = oxphp_async(function(): int { return 42; });
$result = oxphp_async_await($p2); // 42
```

### Exception classes

| Class | Parent | When |
|-------|--------|------|
| `OxPHP\AsyncException` | `Exception` | Closure threw an exception, or `die()`/`exit()` was called |
| `OxPHP\AsyncTimeoutException` | `OxPHP\AsyncException` | Timeout expired before task completed |

## Promise Scope and Lifetime

Promises are stored in **thread-local storage** on the PHP worker thread that created them. This has three implications:

1. **Thread-bound.** A promise can only be awaited by the same worker thread that called `oxphp_async()`. Another worker thread has its own promise map and cannot see foreign promise IDs.

2. **Request-scoped.** At the end of each request (RSHUTDOWN), all outstanding promises are automatically cleaned up. In worker mode the same cleanup runs between requests. Promises cannot survive across request boundaries.

3. **IDs are per-thread, not globally unique.** The promise counter increments monotonically within a thread and does not reset between requests. Two different worker threads may both have a promise with ID `0` — these are independent promises in separate maps.

```
Worker Thread #1                    Worker Thread #2
┌─ Request A ─────────────┐        ┌─ Request C ──────────┐
│ PROMISE_MAP: {0, 1, 2}  │        │ PROMISE_MAP: {0, 1}  │
│ await_any([0, 1]) → OK  │        │ await(0) → OK        │
│ await(2) → OK           │        │ await(1) → OK        │
│ RSHUTDOWN: cleanup      │        │ RSHUTDOWN: cleanup    │
├─ Request B ─────────────┤        └──────────────────────┘
│ PROMISE_MAP: {3, 4}     │  ← counter continues from 3
│ (not awaited)           │
│ RSHUTDOWN: cancel + free│  ← promises 3, 4 cleaned up
└─────────────────────────┘
```

To share async results between requests, use an external mechanism (Redis, shared memory, database).

## RSHUTDOWN Cleanup

If a PHP request ends without awaiting all dispatched promises, the RSHUTDOWN hook automatically cleans up outstanding promises. Each non-awaited promise is given a 5-second timeout to complete, after which it is cancelled. This prevents resource leaks from forgotten promises.

```php
<?php
$p1 = oxphp_async(function(): int { return 1; });
$p2 = oxphp_async(function(): int { return 2; });

$result = oxphp_async_await($p1);
// $p2 is never awaited — cleaned up automatically at request end
```

## Architecture

The async pool is a **separate** set of OS threads from the HTTP PHP worker pool. This separation prevents deadlocks: if all HTTP workers dispatched async tasks and then blocked on `oxphp_async_await()`, a shared pool would deadlock.

> **Fiber-Aware Behavior:** In worker mode with the fiber scheduler active, `oxphp_async_await()` suspends the current request fiber instead of blocking the worker thread. This allows other requests to be processed while waiting for async results. See [Fiber-Based Request Multiplexing](fiber-multiplexing.md) for details.

```
HTTP Worker Thread              Async Worker Thread
─────────────────              ────────────────────
oxphp_async($fn)
  ├─ serialize use-vars
  ├─ serialize args
  ├─ create AsyncTask           recv(AsyncTask)
  ├─ send via channel ──────►     ├─ oxphp_async_reset()
  │                               ├─ deserialize use-vars
  │                               ├─ deserialize args
  │                               ├─ oxphp_execute_async_task()
  │                               ├─ serialize retval
oxphp_async_await($id)                  ├─ send AsyncResult
  ├─ block on oneshot ◄──────     └─ free local data
  ├─ deserialize result
  └─ return value
```

All data crossing thread boundaries goes through portable binary serialization: serialize with system `malloc` on the source thread, transfer the buffer, deserialize with `emalloc` on the destination thread. This guarantees every PHP allocation belongs to the correct per-thread `zend_mm_heap`.

Each async worker thread:
1. Initializes TSRM (Zend Thread Safety) thread-local storage
2. Calls `php_request_startup()` once at thread start
3. Loops: receive task → reset state → deserialize data → execute closure → serialize result → send
4. Calls `php_request_shutdown()` on exit

## Sizing and Tuning

Async workers are dedicated OS threads with full PHP ZTS initialization. They compete for CPU cores with HTTP workers and the Tokio runtime. Correct sizing prevents CPU contention and ensures async tasks don't starve HTTP request processing.

### Thread budget

The total thread count across all pools should stay within the CPU core count:

```
Total threads = TOKIO_WORKERS + PHP_WORKERS + ASYNC_WORKERS ≤ CPU cores
```

Exceeding this budget causes context switching overhead that degrades throughput for all pools.

| 8-core server | TOKIO | PHP | ASYNC | Total | Assessment |
|---------------|-------|-----|-------|-------|------------|
| Conservative | 4 | 4 | 2 | 10 | good — slight oversubscription is fine |
| Aggressive | 4 | 4 | 4 | 12 | acceptable if async tasks are I/O-bound |
| Oversubscribed | 4 | 8 | 8 | 20 | bad — context switch overhead dominates |

### ASYNC_WORKERS by workload type

The optimal count depends on whether async tasks are CPU-bound or I/O-bound:

| Workload | Formula | Rationale |
|----------|---------|-----------|
| **CPU-bound** (computation, data processing) | `CPU_cores / 4` | More threads won't help — they compete for the same cores as HTTP workers |
| **I/O-bound** (sleep, network calls, file I/O) | `PHP_WORKERS` | Threads spend most time blocked on I/O, not consuming CPU |
| **Mixed** (typical) | `PHP_WORKERS / 2` | Start here, adjust based on metrics |

### ASYNC_QUEUE_CAPACITY by latency requirements

Each task in the queue holds serialized arguments in memory. Queue depth trades memory and latency for burst tolerance:

| Scenario | Recommended capacity | Rationale |
|----------|---------------------|-----------|
| **Web requests** (latency-sensitive) | `ASYNC_WORKERS * 4..8` | Reject early and fall back rather than queue and wait |
| **Background/batch** (throughput-sensitive) | `ASYNC_WORKERS * 64` (default) | Acceptable to buffer tasks for later processing |

### Avoiding stalls

When an HTTP worker calls `oxphp_async_await()`, it blocks until the async task completes. If all HTTP workers block simultaneously and async workers can't keep up, request throughput drops to zero.

The constraint:

```
ASYNC_WORKERS ≥ max concurrent oxphp_async_await() callers
```

In practice, estimate what fraction of HTTP requests use async:

| Async usage | Sizing rule |
|-------------|-------------|
| Every request dispatches async | `ASYNC_WORKERS ≥ PHP_WORKERS` |
| ~30% of requests | `ASYNC_WORKERS ≥ PHP_WORKERS / 3` |
| Rare (1 in 10) | `ASYNC_WORKERS ≥ PHP_WORKERS / 4` |

### Memory overhead

Each async worker thread consumes:
- OS thread stack: 2–8 MB (platform-dependent)
- PHP ZTS heap: ~2–10 MB (depends on extensions and INI settings)
- Approximate total: **4–18 MB per worker**

Queued tasks add memory for serialized arguments (varies by payload size, typically small).

### Example configurations

**8-core server, Laravel, ~30% of requests use async:**

```bash
TOKIO_WORKERS=0           # auto: 4 (CPU/2)
PHP_WORKERS=4
ASYNC_WORKERS=2           # CPU/4, sufficient for 30% async load
ASYNC_QUEUE_CAPACITY=16   # low-latency: 2 * 8
```

**16-core server, batch processing, every request fans out:**

```bash
TOKIO_WORKERS=0           # auto: 8 (CPU/2)
PHP_WORKERS=6
ASYNC_WORKERS=6           # match PHP workers — all requests use async
ASYNC_QUEUE_CAPACITY=384  # 6 * 64, high throughput
```

**4-core container, occasional background tasks:**

```bash
TOKIO_WORKERS=1           # single-threaded, save cores for PHP
PHP_WORKERS=2
ASYNC_WORKERS=1           # minimal — async is rare
ASYNC_QUEUE_CAPACITY=8    # 1 * 8
```

### Monitoring

After deployment, use these Prometheus queries to validate sizing:

```promql
# Tasks rejected? → increase ASYNC_QUEUE_CAPACITY or ASYNC_WORKERS
rate(oxphp_async_tasks_rejected_total[5m]) > 0

# Task backlog growing? → increase ASYNC_WORKERS
oxphp_async_tasks_dispatched_total
  - oxphp_async_tasks_completed_total
  - oxphp_async_tasks_failed_total
  - oxphp_async_tasks_cancelled_total

# CPU saturated? → reduce ASYNC_WORKERS
# Check system metrics: load average, CPU utilization, steal time
```

## Metrics

When the async pool is active (at least one task dispatched), five Prometheus counters are emitted:

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_async_tasks_dispatched_total` | counter | Total tasks submitted via `oxphp_async()` |
| `oxphp_async_tasks_completed_total` | counter | Tasks that returned a value successfully |
| `oxphp_async_tasks_failed_total` | counter | Tasks that threw an exception or called `die()`/`exit()` |
| `oxphp_async_tasks_cancelled_total` | counter | Tasks cancelled (timeout or RSHUTDOWN cleanup) |
| `oxphp_async_tasks_rejected_total` | counter | Tasks rejected because the async queue was full |

These counters are omitted from the Prometheus output when no async tasks have been dispatched and none have been rejected.

## Async Worker Environment

Async workers are **isolated PHP engines**. Each async worker thread runs `php_request_startup()` once at thread start and then loops executing closures. Unlike HTTP workers, async workers do **not** execute your application's bootstrap code — no `vendor/autoload.php`, no framework initialization, no service container.

### What works

| Category | Examples |
|----------|---------|
| Built-in PHP functions | `array_map`, `json_encode`, `preg_match`, `hash`, `mb_*`, `date` |
| Built-in extensions | `PDO` (create new connection inside closure), `curl_*`, `file_get_contents` |
| Pure computation | Math, string processing, array manipulation, data transformation |
| Scalar data via `use` | `string`, `int`, `float`, `bool`, `array` (deep-copied) |

```php
<?php
// ✅ Works: create a new PDO connection inside the closure
$dsn = 'mysql:host=127.0.0.1;dbname=app';
$user = 'root';
$pass = 'secret';

$p = oxphp_async(function() use ($dsn, $user, $pass): array {
    $pdo = new PDO($dsn, $user, $pass);
    return $pdo->query('SELECT count(*) FROM users')->fetch();
});
```

### What does NOT work

| Category | Why |
|----------|-----|
| `DB::connection()`, `app('cache')`, facades | Service container not initialized — these rely on Laravel's bootstrap |
| Composer autoloader | `vendor/autoload.php` not executed — classes outside built-in extensions are undefined |
| Eloquent models, Doctrine entities | Require autoloader + framework bootstrap |
| `$_SERVER`, `$_GET`, `$_POST` | Superglobals are not populated on async workers — they have no HTTP request context |
| Static state from HTTP worker | `static` class properties, global variables — each thread has its own ZTS copy |
| Objects via `use` | Objects cannot be serialized across threads — throws `AsyncException` at dispatch time |

```php
<?php
// ❌ Fails: autoloader not available, class not found
$p = oxphp_async(function(): void {
    $user = User::find(1);  // Fatal: Class "User" not found
});

// ❌ Fails: service container not initialized
$p = oxphp_async(function(): void {
    $db = app('db');  // Fatal: Function "app" not found
});

// ❌ Fails: objects cannot cross thread boundary
$pdo = new PDO($dsn, $user, $pass);
$p = oxphp_async(function() use ($pdo): array {
    // AsyncException: Cannot pass object values in use-vars
    return $pdo->query('SELECT 1')->fetch();
});
```

### Workaround: pass connection parameters, not connections

Since objects can't cross thread boundaries, pass the raw connection parameters and create connections inside the closure:

```php
<?php
$config = [
    'dsn'  => 'mysql:host=127.0.0.1;dbname=app',
    'user' => 'root',
    'pass' => getenv('DB_PASSWORD'),
];

$promises = [];
foreach ($chunks as $chunk) {
    $promises[] = oxphp_async(function() use ($config, $chunk): int {
        // Each async worker creates its own connection
        $pdo = new PDO($config['dsn'], $config['user'], $config['pass']);
        $pdo->beginTransaction();
        foreach ($chunk as $row) {
            $pdo->prepare('UPDATE t SET v=? WHERE id=?')->execute([$row['v'], $row['id']]);
        }
        $pdo->commit();
        return count($chunk);
    }, $chunk);
}

$results = oxphp_async_await_all($promises);
$total = array_sum($results);
```

## Usage Patterns

### Parallel computation

```php
<?php
// Dispatch multiple CPU-bound tasks
$promises = [];
foreach ($chunks as $i => $chunk) {
    $promises[$i] = oxphp_async(function() use ($chunk): array {
        return array_map('process_record', $chunk);
    });
}

// Collect all results
$results = oxphp_async_await_all($promises);
```

### Background work after response

```php
<?php
echo json_encode(['status' => 'accepted']);
oxphp_finish_request();

// Response is sent — now do background work in parallel
$p1 = oxphp_async(function() use ($data): void { send_email($data); });
$p2 = oxphp_async(function() use ($data): void { update_analytics($data); });
oxphp_async_await_all([$p1, $p2]);
```

### Process results in completion order

Use `oxphp_async_await_any()` in a loop to process promises as they finish — fastest first:

```php
<?php
$promises = [
    oxphp_async(fn() => call_api_a()),  // 500ms
    oxphp_async(fn() => call_api_b()),  // 100ms
    oxphp_async(fn() => call_api_c()),  // 300ms
];

while (!empty($promises)) {
    $winner = oxphp_async_await_any($promises);
    process_result($winner['id'], $winner['value']);

    // Remove the winner, keep racing the rest
    $promises = array_values(
        array_filter($promises, fn($id) => $id !== $winner['id'])
    );
}
// Processing order: api_b (100ms), api_c (300ms), api_a (500ms)
```

Each iteration takes receivers from the internal promise map, races them with `select_all`, returns the winner, and puts the remaining receivers back. No promises are lost or leaked — each is cleaned up exactly once when it wins.

### Timeout protection

```php
<?php
$p = oxphp_async(function(): array {
    return fetch_from_slow_api();
});

try {
    $result = oxphp_async_await($p, 2.0); // 2-second timeout
} catch (\OxPHP\AsyncTimeoutException $e) {
    $result = cached_fallback();
}
```

## See Also

- [PHP Extension Functions](../php/functions.md) --- full function reference including `oxphp_async`, `oxphp_async_await`, `oxphp_async_await_all`, `oxphp_async_await_any`
- [Fiber-Based Request Multiplexing](fiber-multiplexing.md) --- fiber-aware `oxphp_async_await()` suspends instead of blocking the worker
- [Worker Pool](../architecture/worker-pool.md) --- HTTP worker pool architecture (separate from async pool)
- [Metrics](../operations/metrics.md#async-tasks) --- Prometheus metrics including async task counters
- [Configuration](../operations/configuration.md#async-pool) --- `ASYNC_WORKERS` and `ASYNC_QUEUE_CAPACITY`
