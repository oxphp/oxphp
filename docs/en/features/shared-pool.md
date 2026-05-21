---
title: Shared\Pool
description: Process-wide bounded object pool for expensive per-thread resources — database connections, JSON parsers, HTTP clients — with strict budget, per-thread affinity, and idle-timeout eviction.
---

# Shared\Pool

`OxPHP\Shared\Pool` is a bounded pool of per-thread resources. It is the primitive for managing objects that are expensive to create, cannot be recreated cheaply, and should not exist in unlimited numbers — typically database connections, prepared-statement caches, reusable JSON decoders, or HTTP client sessions.

A Pool gives each PHP worker thread its own lane of ready-to-use resources, enforces a strict maximum across the whole pool, and recycles idle slots automatically so you do not pay for capacity you do not use.

## Overview

- **Strict budget.** `maxSize` is a hard cap across the whole pool. When the pool is saturated, an acquire either waits, returns `null`, or throws — depending on which acquire method you call.
- **Per-thread affinity.** Each worker thread has its own idle queue. An acquire pulls from the local queue first and never hands a slot minted on thread A to thread B (v1).
- **Factory runs in the acquiring worker.** Resources are minted lazily on first demand per thread, not at pool construction.
- **Destroy callback on eviction.** An optional `destroy($resource)` closure runs when the pool drops a slot (idle timeout, manual eviction, server stop).
- **Idle-timeout eviction.** Slots sitting idle longer than `idleTimeoutMs` are destroyed by a background task. Set `idleTimeoutMs: 0` to disable idle eviction entirely.
- **RAII handles.** `acquire()` returns a `Handle`; the slot returns to the pool automatically when the handle goes out of scope (including on exception), or earlier via `$handle->release()`.
- **Shareable.** Pools survive request boundaries and are shared by handle (`use ($pool)` in closures).

## API Reference

```php
namespace OxPHP\Shared;

final class Pool implements Shareable
{
    public function __construct(
        callable  $factory,                  // fn(): object — create a resource
        ?callable $destroy       = null,     // fn(object): void — tear down a resource
        int       $maxSize       = 32,       // hard cap on live slots; > 0
        int       $idleTimeoutMs = 300_000,  // idle ms before eviction; 0 disables it
    );

    // acquire family — millisecond timeout trichotomy
    public function acquire(): Pool\Handle;                 // wait forever
    public function tryAcquire(): ?Pool\Handle;             // non-blocking; null if saturated
    public function acquireTimeout(int $ms): Pool\Handle;   // bounded; $ms > 0

    // with family — scope-guard around the raw resource
    public function with(callable $body): mixed;                 // wait forever
    public function withTimeout(callable $body, int $ms): mixed; // bounded; $ms > 0

    public function stats(): Pool\Stats;   // point-in-time snapshot of counters
    public function evict(): int;          // force-evict all idle slots now; returns count
    public function id(): int;
}

namespace OxPHP\Shared\Pool;

final class Handle
{
    public function get(): mixed;     // the underlying resource (throws after release)
    public function release(): void;  // return the slot now; idempotent; also runs on destruct
}

final class Stats
{
    public function inUse(): int;    // slots currently checked out
    public function idle(): int;     // free slots ready to hand out
    public function waiting(): int;  // callers blocked in acquire
    public function size(): int;     // inUse() + idle() (live slots)
    public function maxSize(): int;  // configured cap

    public function utilization(): float;  // inUse() / maxSize(), 0.0 if maxSize() == 0
}
```

| Method            | Returns       | Use case                                                               |
|-------------------|---------------|------------------------------------------------------------------------|
| `acquire`         | `Handle`      | Check out a resource, waiting forever for a free slot.                  |
| `tryAcquire`      | `?Handle`     | Non-blocking check-out. Returns `null` immediately if the pool is saturated. |
| `acquireTimeout`  | `Handle`      | Check out within a bounded budget of `$ms` (`> 0`); throws `OperationTimeoutException` on expiry. |
| `with`            | mixed         | Scope-guard: acquire (forever), run `$body($resource)` with the raw resource, release even on exception. The closure's return value passes through. |
| `withTimeout`     | mixed         | Like `with`, but the acquire is bounded by `$ms`.                       |
| `stats`           | `Pool\Stats`  | Point-in-time snapshot of pool counters.                                |
| `evict`           | int           | Force-evict all idle slots now (regardless of `idleTimeoutMs`); returns the count dropped. |
| `id`              | int           | Registry identifier; useful for logging / observability.               |
| `Handle::get`     | mixed         | The underlying resource. Throws `StaleHandleException` after release.   |
| `Handle::release` | void          | Return the slot to the pool now. Idempotent; also runs automatically on destruct (RAII). |

Timeouts follow the same trichotomy as `Shared\Mutex` and `Shared\Channel`: a bare method waits forever, a `try*` method is non-blocking, and a `*Timeout(int $ms)` method waits a bounded number of milliseconds. There is no float-seconds timeout.

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
    idleTimeoutMs: 60_000,     // free idle connections after 1 min
);

// In a request handler
$users = $db->with(function (PDO $conn) use ($userId) {
    $stmt = $conn->prepare('SELECT * FROM users WHERE id = ?');
    $stmt->execute([$userId]);
    return $stmt->fetch();
});
```

`with()` is the shortest-lifetime pattern: acquire happens on entry, release happens on return *or* on exception — you cannot leak a handle. It also hands your closure the **raw resource** directly, so you skip the `Handle::get()` step.

### Manual acquire / release

```php
<?php
$h = $pool->acquire();        // waits forever for a free slot

$conn = $h->get();
$conn->beginTransaction();
doWork($conn);
$conn->commit();

$h->release();                // or just let $h fall out of scope (RAII)
```

The handle returns its slot automatically when it is destroyed — including if an exception unwinds the stack — so an explicit `release()` is optional. Reach for a manual handle (over `with()`) only when the resource must survive multiple calls in a handler sequence.

### Reusable parser pool

```php
<?php
$parsers = new OxPHP\Shared\Pool(
    factory: fn () => new JsonMachine\Parser(),
    maxSize: 8,
);

$doc = $parsers->with(fn ($p) => $p->parse($body));
```

### Non-blocking acquire with fallback

```php
<?php
$h = $pool->tryAcquire();
if ($h === null) {
    // Pool saturated — degrade gracefully without waiting.
    http_response_code(503);
    header('Retry-After: 1');
    return;
}
// ... use $h->get(); slot returns on scope exit.
```

### Bounded acquire with a timeout

```php
<?php
try {
    $h = $pool->acquireTimeout(100);   // wait up to 100 ms
} catch (OxPHP\Shared\OperationTimeoutException $e) {
    http_response_code(503);
    header('Retry-After: 1');
    return;
}
// ... use $h->get();
```

## Factory and destroy semantics

The factory runs **lazily** on the acquiring worker thread. A pool with `maxSize: 32` does not pre-allocate 32 resources; it mints them as demand arrives, bounded by `maxSize` across all threads combined.

- The factory must return a PHP object. Returning a non-object surfaces as `TypeException` from the acquire call, and the slot is not counted against the budget.
- A factory that throws propagates its own exception to the acquire caller unchanged, and the slot is not counted against the budget.
- The destroy callback (if supplied) runs when the pool drops a slot: idle timeout expiry, explicit `evict()`, or server shutdown. It runs on a worker thread (not on the Tokio thread driving the eviction scheduler), so PHP is safe to call.
- A destroy callback that throws is logged but does not poison the pool — the slot is already being destroyed, so there is nothing useful to roll back.

## Per-thread affinity

v1 pools are strict per-thread: a slot minted on worker thread A cannot be acquired on worker thread B. Practically this means `stats()->idle()` can be non-zero on worker A while worker B is blocking in `acquire()`. This keeps slots hot in the thread that uses them (DB connections, OPcache-primed objects) and avoids shuffling resources across cores.

Cross-thread work stealing is a v1.x candidate. Until then, size `maxSize` against the number of worker threads × expected per-thread concurrency, not just aggregate demand.

## Idle-timeout eviction

Idle slots are evicted by a background scheduler. When a slot has been idle longer than `idleTimeoutMs`, the scheduler flags it; the owning worker destroys it on its next request (with the PHP engine alive, so `$destroy` runs in a normal request context). Budget is released at the same point.

Tune `idleTimeoutMs` to the cost of recreation:

- **Cheap to re-mint** (JSON decoder, string pool): set 10_000–60_000 ms; free memory fast when traffic dies down.
- **Expensive to re-mint** (DB connection, TLS session): set 300_000 ms (default) to 900_000 ms; pay recreation cost less often.
- **Never evict**: pass `0`. Idle slots then live until the pool is dropped.

`$pool->evict()` force-evicts **all** idle slots reachable from the calling worker right now — regardless of `idleTimeoutMs` — and returns how many were dropped. It is the operational "flush idle now" escape hatch (e.g. a downstream service restarted and you want the next acquire to mint fresh resources). In-use slots are untouched.

## Budget & acquire semantics

Every acquire variant first tries to satisfy the request immediately — reuse an idle slot, or (if the pool is below `maxSize`) mint a new one via the factory. Only when the pool is **saturated** — no idle slot **and** at `maxSize` — does the behaviour differ:

| State at call time                          | `acquire()`           | `tryAcquire()`     | `acquireTimeout($ms)`                     |
|---------------------------------------------|-----------------------|--------------------|-------------------------------------------|
| Idle slot in the local thread's queue       | reused immediately    | reused immediately | reused immediately                        |
| No idle slot, but below `maxSize`           | factory mints a slot  | factory mints a slot | factory mints a slot                    |
| Saturated (at `maxSize`, all in use)        | waits **forever**     | returns `null`     | waits up to `$ms`, then `OperationTimeoutException` |

`$ms` must be `> 0`; `0` or negative raises `TypeException`. There is deliberately no float-seconds form and no "infinite" sentinel argument — use the bare `acquire()` for an unbounded wait.

## Exceptions

| Exception                  | Raised by                                                            |
|----------------------------|----------------------------------------------------------------------|
| `OperationTimeoutException` | `acquireTimeout` / `withTimeout` exceeded `$ms` without a free slot. **Extends `Async\AsyncException`, not `SharedException`.** |
| `TypeException`            | Non-positive `maxSize`, negative `idleTimeoutMs`, `$ms <= 0`, or a factory that returned a non-object. |
| `StaleHandleException`     | `Handle::get()` after the handle was released.                       |
| `UninitializedException`   | A method call on a pool wrapper that has not finished `__construct`. |

`tryAcquire()` does **not** throw on saturation — it returns `null`. Because `OperationTimeoutException` extends `OxPHP\Async\AsyncException` (not `SharedException`), a `catch (SharedException)` will **not** catch an acquire timeout; use `catch (OxPHP\Async\AsyncException)` or catch `OperationTimeoutException` directly.

Exceptions thrown inside the factory propagate to the acquire caller unchanged and do not consume budget. Exceptions inside the `with()` / `withTimeout()` body propagate to the caller after the slot is released.

## Observability

See [Shared Observability](../operations/shared-observability.md) for the full tour. Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ type: "Pool", size, in_use, idle, waiting, max_size, idle_by_thread, rebalance_strategy }`.
- `GET /__ox_shared/summary` counts Pool instances and aggregate `waiting_total`, `evicted_total`.
- Prometheus metrics per pool:
  - `oxphp_shared_pool_size{pool_id="…"}`            — gauge, total slots (in-use + idle).
  - `oxphp_shared_pool_in_use{pool_id="…"}`          — gauge.
  - `oxphp_shared_pool_idle{pool_id="…"}`            — gauge.
  - `oxphp_shared_pool_waiting{pool_id="…"}`         — gauge, queued acquires.
  - `oxphp_shared_pool_acquire_total{pool_id="…",result="ok|timeout|closed|saturated"}` — counter. `saturated` counts non-blocking `tryAcquire` calls that found the pool full (distinct from `timeout`, which means a wait elapsed).
  - `oxphp_shared_pool_evicted_total{pool_id="…",reason="idle_timeout|evict|shutdown"}` — counter.
  - `oxphp_shared_pool_wait_seconds_*{pool_id="…"}`  — acquire-wait histogram (bucket / sum / count).

Alert-worthy combinations: rising `waiting` with flat `size` means the pool is saturated and should be resized; rising `acquire_total{result="timeout"}` with normal `in_use` means the factory is slow (or blocking); rising `acquire_total{result="saturated"}` means callers keep hitting `tryAcquire` on a full pool (backpressure firing).

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
