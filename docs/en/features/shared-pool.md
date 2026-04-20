---
title: Shared\Pool
description: Process-wide bounded object pool for expensive per-thread resources — database connections, JSON parsers, HTTP clients — with strict budget, per-thread affinity, and idle-timeout eviction.
---

# Shared\Pool

`OxPHP\Shared\Pool` is a bounded pool of per-thread resources. It is the primitive for managing objects that are expensive to create, cannot be recreated cheaply, and should not exist in unlimited numbers — typically database connections, prepared-statement caches, reusable JSON decoders, or HTTP client sessions.

A Pool gives each PHP worker thread its own lane of ready-to-use resources, enforces a strict maximum across the whole pool, and recycles idle slots automatically so you do not pay for capacity you do not use.

## Overview

- **Strict budget.** `maxSize` is a hard cap across the whole pool. Acquires past the cap block until a slot is released or timeout hits.
- **Per-thread affinity.** Each worker thread has its own idle queue. An acquire pulls from the local queue first and never hands a slot minted on thread A to thread B (v1).
- **Factory runs in the acquiring worker.** Resources are minted lazily on first demand per thread, not at pool construction.
- **Destroy callback on eviction.** An optional `destroy($resource)` closure runs when the pool drops a slot (idle timeout, pool eviction, server stop).
- **Idle-timeout eviction.** Slots sitting idle longer than `idleTimeout` are destroyed by a background task that ticks every 500 ms.
- **Shareable.** Pools survive request boundaries and are shared by handle (`use ($pool)` in closures).

## API Reference

```php
namespace OxPHP\Shared;

final class Pool implements Shareable
{
    public function __construct(
        callable  $factory,
        ?callable $destroy = null,
        int       $maxSize = 32,
        float     $idleTimeout = 300.0,
        ?float    $defaultAcquireTimeout = 5.0,
    );

    public function acquire(float $timeout = 0.0): Pool\Handle;
    public function release(Pool\Handle $handle): void;
    public function with(callable $body, float $timeout = 0.0): mixed;

    public function size(): int;
    public function inUse(): int;
    public function idle(): int;
    public function waiting(): int;
    public function maxSize(): int;

    public function evict(): int;
    public function id(): int;
}

namespace OxPHP\Shared\Pool;

final class Handle
{
    public function get(): mixed;     // the underlying resource
}
```

| Method         | Returns  | Use case                                                          |
|----------------|----------|-------------------------------------------------------------------|
| `acquire`      | `Handle` | Check out a resource; blocks up to `$timeout`. `0.0` uses default. |
| `release`      | void     | Return a handle to the pool. Idempotent within the handle's lifetime; double-release is rejected. |
| `with`         | mixed    | Scope-guarded acquire + release around a closure. The closure's return value is passed through. Preferred over manual `acquire`/`release`. |
| `size`         | int      | Current slot count (in-use + idle) across all threads.            |
| `inUse`        | int      | Slots currently handed out.                                       |
| `idle`         | int      | Slots sitting in per-thread queues.                               |
| `waiting`      | int      | Acquires parked waiting for a free slot.                          |
| `maxSize`      | int      | Configured hard cap.                                              |
| `evict`        | int      | Force the eviction scheduler to sweep now; returns count dropped. |
| `id`           | int      | Registry identifier; useful for logging / observability.          |

## Examples

### Database connection pool

```php
<?php
$db = new OxPHP\Shared\Pool(
    factory: function () {
        return new PDO(
            getenv('DB_DSN'),
            getenv('DB_USER'),
            getenv('DB_PASS'),
            [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION],
        );
    },
    destroy: function (PDO $conn) {
        // Nothing to do — PDO closes on destruct. The callback exists
        // for resources that need explicit teardown (sockets, handles).
    },
    maxSize: 16,
    idleTimeout: 60.0,        // free idle connections after 1 min
);

// In a request handler
$users = $db->with(function (PDO $conn) use ($userId) {
    $stmt = $conn->prepare('SELECT * FROM users WHERE id = ?');
    $stmt->execute([$userId]);
    return $stmt->fetch();
});
```

`with()` is the shortest-lifetime pattern: acquire happens on entry, release happens on return *or* on exception — you cannot leak a handle.

### Manual acquire / release

```php
<?php
$h = $pool->acquire(timeout: 2.0);

try {
    $conn = $h->get();
    $conn->beginTransaction();
    doWork($conn);
    $conn->commit();
} finally {
    $pool->release($h);
}
```

Reach for manual acquire only when the scope crosses a boundary `with()` cannot express (e.g. the resource needs to survive multiple calls in a handler sequence).

### Reusable parser pool

```php
<?php
$parsers = new OxPHP\Shared\Pool(
    factory: fn () => new JsonMachine\Parser(),
    maxSize: 8,
);

$doc = $parsers->with(fn ($p) => $p->parse($body));
```

### Short acquire timeout + fallback

```php
<?php
try {
    $h = $pool->acquire(timeout: 0.1);    // 100ms
} catch (OxPHP\Shared\TimeoutException $e) {
    // Pool saturated — degrade gracefully
    http_response_code(503);
    header('Retry-After: 1');
    return;
}

try {
    // ...
} finally {
    $pool->release($h);
}
```

## Factory and destroy semantics

The factory runs **lazily** on the acquiring worker thread. A pool with `maxSize: 32` does not pre-allocate 32 resources; it mints them as demand arrives, bounded by `maxSize` across all threads combined.

- The factory is expected to return a non-null value. Returning `null` or throwing surfaces as an exception from `acquire()` and the slot is not counted against the budget.
- The destroy callback (if supplied) runs when the pool drops a slot: idle timeout expiry, explicit `evict()`, server shutdown, or pool-handle eviction. It runs on a worker thread (not on the Tokio thread driving the eviction scheduler), so PHP is safe to call.
- A destroy callback that throws is logged but does not poison the pool — the slot is already being destroyed, so there is nothing useful to roll back.

## Per-thread affinity

v1 pools are strict per-thread: a slot minted on worker thread A cannot be acquired on worker thread B. Practically this means `idle()` can be non-zero on worker A while worker B is blocking in `acquire()`. This keeps slots hot in the thread that uses them (DB connections, OPcache-primed objects) and avoids shuffling resources across cores.

Cross-thread work stealing is a v1.x candidate. Until then, size `maxSize` against the number of worker threads × expected per-thread concurrency, not just aggregate demand.

## Idle-timeout eviction

Idle slots are evicted by a background scheduler that ticks every 500 ms. When a slot has been idle longer than `idleTimeout`, the scheduler flags it; the owning worker destroys it on its next entry to `execute_request` (with the PHP engine alive, so `$destroy` runs in a normal request context). Budget is released at the same point.

Tune `idleTimeout` to the cost of recreation:

- **Cheap to re-mint** (JSON decoder, string pool): set 10–60 s; free memory fast when traffic dies down.
- **Expensive to re-mint** (DB connection, TLS session): set 300 s (default) to 900 s; pay recreation cost less often.
- **Never evict**: pass a very large number. Not recommended; a single hot slot that later goes unused indefinitely is memory you never reclaim.

`$pool->evict()` forces a sweep now and returns how many slots were dropped. Useful in tests and in admin endpoints that shed load on demand.

## Budget & acquire timeout

`acquire(timeout)` behaves as follows:

| State at call time                          | Result                                       |
|---------------------------------------------|----------------------------------------------|
| Idle slot in the local thread's queue       | Reused immediately, factory is not called.   |
| No idle slot, but `size() < maxSize`        | Factory runs, a new slot is minted.          |
| `size() == maxSize` and all slots `inUse`   | Block up to `timeout`, then `TimeoutException`. |

Passing `0.0` for `timeout` uses `$defaultAcquireTimeout` (default 5 s). To wait indefinitely, pass a very large number — there is deliberately no "infinite" sentinel, since a pool that deadlocks waiting forever is harder to diagnose than one that times out and throws.

## Exceptions

| Exception                | Raised by                                                            |
|--------------------------|----------------------------------------------------------------------|
| `TimeoutException`       | `acquire` exceeded `$timeout` without a free slot.                   |
| `CapacityException`      | Creation would breach `maxSize` despite budget reconciliation (rare). |
| `TypeException`          | Non-positive `maxSize`, factory return is not a PHP object, etc.     |
| `StaleHandleException`   | Method call on a handle whose registry entry was evicted, or on a `Pool\Handle` that was already released. |
| `UninitializedException` | `id()` on a pool wrapper that has not finished `__construct`.        |

Exceptions thrown inside the factory propagate to the `acquire()` caller unchanged and do not consume budget. Exceptions inside the `with()` body propagate to the caller after the handle is released.

## Observability

See [Shared Observability](../operations/shared-observability.md) for the full tour. Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ type: "Pool", size, in_use, idle, waiting, max_size, idle_timeout_ms }`.
- `GET /__ox_shared/summary` counts Pool instances and aggregate `waiting_total`, `evicted_total`.
- Prometheus metrics per pool:
  - `oxphp_shared_pool_size{pool_id="…"}`            — gauge, total slots.
  - `oxphp_shared_pool_in_use{pool_id="…"}`          — gauge.
  - `oxphp_shared_pool_idle{pool_id="…"}`            — gauge.
  - `oxphp_shared_pool_waiting{pool_id="…"}`         — gauge, queued acquires.
  - `oxphp_shared_pool_acquires_total{pool_id="…"}`  — counter.
  - `oxphp_shared_pool_evicted_total{pool_id="…",reason="idle_timeout|manual|shutdown"}` — counter.
  - `oxphp_shared_pool_factory_errors_total{pool_id="…"}` — counter of factory exceptions.
  - `oxphp_shared_pool_acquire_timeouts_total{pool_id="…"}` — counter of acquire timeouts.

Alert-worthy combinations: rising `waiting` with flat `size` means the pool is saturated and should be resized; rising `acquire_timeouts_total` with normal `in_use` means factory is slow (or blocking).

## When not to use

- **Cheap or immutable resources.** A pool's overhead is larger than re-creating a simple object. Use it for resources where creation costs milliseconds or kilobytes.
- **Objects that cannot be reused safely.** If the resource accumulates per-request state (open transactions, pending reads) and you cannot reliably reset it, pooling leaks state between requests. Return slots to a known state in your request-finishing code, or do not pool.
- **Cross-host resources.** A pool is in-process. For multi-host connection pooling, prefer a connection-bucket service or a sidecar (pgbouncer, proxy-sql).
- **Unbounded fan-out.** If you need one connection per in-flight HTTP call, that is not a pool — that is an N-per-request problem. Use a `Shared\Channel` to serialise work behind a bounded pool instead.
- **Resources with their own pool semantics.** Many client libraries already pool internally (e.g. Guzzle's connection pool). Stacking a `Shared\Pool` on top is double bookkeeping; prefer the library's own pooling.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Once](shared-once.md) — when you need exactly one resource (not a pool of N).
- [Shared\Channel](shared-channel.md) — pair with a pool for producer/consumer pipelines.
- [Shared\Map](shared-map.md) — one `Pool` per tenant keyed by name.
- [Worker Mode](worker-mode.md) — pool handles across requests within one worker thread.
