---
title: Shared State
description: Process-wide primitives — counters, flags, maps, channels, pools — that let PHP workers coordinate without an external store like Redis or APCu.
---

# Shared State

`OxPHP\Shared\*` is a collection of concurrent primitives that live inside the server process and are visible to every PHP worker. They let workers coordinate mutable state — counters, feature flags, caches, work queues, connection pools — without reaching for Redis, Memcached, or APCu.

Everything described here is fully in-process. Shared state is lost when the server stops. If you need durability or multi-host coordination, see [Migrating to an external store](migrating-to-external-store.md).

## Why shared state exists

PHP workers under a traditional SAPI do not share memory. Each worker has its own opcodes, its own static class properties, its own globals. Coordinating state across workers has historically meant an out-of-process dependency: APCu for same-host caches, Redis for counters, a dedicated queue broker for fan-out.

OxPHP runs the PHP engine as a multi-threaded in-process SAPI. That means a carefully designed set of primitives *can* hand workers a safe view of the same piece of state. That is what `OxPHP\Shared\*` gives you:

- **Zero-latency access.** No network round-trip; no serialisation to a socket. Per-op costs are in microseconds.
- **No external dependency.** One fewer service to deploy, monitor, and keep alive.
- **Typed primitives.** A `Shared\Counter` is an atomic integer. A `Shared\Map` is a concurrent hash map. You do not re-implement correctness on top of `INCR` semantics.
- **Cycle- and lifecycle-safe.** The runtime tracks references so a handle cannot outlive its registry entry, and stores reject graphs that would leak memory.

Shared state is not a silver bullet. It does not replace Redis when you need durability across restarts, and it does not replace a real message broker when you need multiple hosts. The shape of its sweet spot is covered in [When not to use](#when-not-to-use) below.

## Mental model

Every `Shared\*` instance is backed by an entry in a process-wide **registry**. A PHP object (the handle you hold) carries a registry ID; the registry owns the real state.

```
 ┌──────────────────── PHP worker 1 ────────────────────┐
 │  $counter (Shared\Counter, id=7) ──┐                  │
 └────────────────────────────────────┼─────────────────┘
                                      │
                                      ▼
                            ┌─────────────────────┐
                            │   Shared registry   │
                            │                     │
                            │   id=7: Counter = 42│
                            │   id=8: Map { … }   │
                            │   id=9: Channel(16) │
                            └─────────▲───────────┘
                                      │
 ┌────────────────────────────────────┼─────────────────┐
 │  $counter (Shared\Counter, id=7) ──┘                  │
 └──────────────────── PHP worker 2 ────────────────────┘
```

Consequences:

- **Handles share state by reference.** Two workers holding the "same" counter see each other's writes immediately.
- **Lifetime follows references.** The registry entry is freed after the last handle drops and no other Shared entry points to it.
- **`clone` is forbidden.** Cloning a handle would create two PHP objects that appear distinct but mutate the same registry entry — confusing and bug-prone. All types throw on `clone`.
- **Cross-thread handoff is explicit.** To hand a Shared value to a background fiber use `oxphp_async(fn () use ($thing) { ... })`. The `use` import transfers the handle; the registry entry itself is thread-safe.

## The v1 primitives

OxPHP 0.3 ships seven types. Pick by semantics, not by what feels familiar:

| Type                                     | Shape                    | Good for                                                              |
|------------------------------------------|--------------------------|-----------------------------------------------------------------------|
| [`Shared\Counter`](shared-counter.md)    | int64 accumulator        | request counts, per-tenant usage, feature-gate hit tracking           |
| [`Shared\Atomic`](shared-atomic.md)      | atomic int64 primitive   | state machines, version stamps, CAS loops, bitflag masks              |
| [`Shared\Flag`](shared-flag.md)          | atomic bool              | kill-switches, one-off initialisation markers, circuit-breaker state  |
| [`Shared\Once`](shared-once.md)          | init-once container      | expensive singleton initialisation across workers (one run wins)      |
| [`Shared\Mutex`](shared-mutex.md)        | poisoning mutual excl.   | critical sections over a non-atomic value                             |
| [`Shared\Channel`](shared-channel.md)    | bounded MPMC queue       | producer/consumer pipelines, work fan-out                             |
| [`Shared\Map`](shared-map.md)            | concurrent string→mixed  | keyed caches, per-tenant state, registry-style lookups                |
| [`Shared\Pool`](shared-pool.md)          | bounded object pool      | expensive per-thread resources (DB handles, parsers)                  |

> **Reach for the simplest type that fits.** `Counter` beats `Map<string, int>` with one key; `Flag` beats `Counter` for true/false; `Mutex<T>` beats ad-hoc compare-and-set chains.

## Quick start — atomic counter under concurrency

```php
<?php
// worker.php — entry script in worker mode, runs once per PHP worker
require __DIR__ . '/vendor/autoload.php';

$requests = new OxPHP\Shared\Counter();

oxphp_worker(function () use ($requests) {
    $requests->inc();                 // atomic under concurrency
    header('X-Request-Count: ' . $requests->get());
    echo 'hello';
});
```

[Worker mode](worker-mode.md) runs the outer scope once per worker thread; the `use ($requests)` capture keeps the same `Shared\Counter` handle live across every request that worker handles. Across worker threads the registry entry itself is the same — all worker threads that hold handles with the same `id()` mutate the same atomic int64.

When a fiber needs to see the counter too, hand it through `use`:

```php
<?php
oxphp_async(function () use ($requests) {
    $requests->inc();                 // runs on whatever worker picks the fiber up
});
```

> Creating `new OxPHP\Shared\Counter()` in two different places produces two independent counters with different `id()`s. Shared state is shared *by handle*, not by name. Plumb the handle through DI, closure captures, or a bootstrap file — do not re-construct in each worker and expect them to merge.

## Canonical example — migrating a hand-rolled counter

Teams commonly hand-roll coordination on top of APCu or static arrays. Here is the pattern for migrating to `Shared\*`, using a per-IP rate limiter as the running example.

### Before: static + external locking

```php
<?php
// Fragile: not atomic under concurrency, does not survive reloads
// on some APCu builds, shares counters across unrelated hosts in a pool.
final class NaiveRateLimiter
{
    public function __construct(
        private int $maxRequests,
        private int $windowSeconds,
    ) {}

    public function allow(string $ip): bool
    {
        $key     = "rl:{$ip}";
        $now     = time();
        $current = apcu_fetch($key);

        if ($current === false || $now - $current['start'] >= $this->windowSeconds) {
            apcu_store($key, ['count' => 1, 'start' => $now], $this->windowSeconds * 2);
            return true;
        }

        apcu_store(
            $key,
            ['count' => $current['count'] + 1, 'start' => $current['start']],
            $this->windowSeconds * 2,
        );

        return $current['count'] + 1 <= $this->maxRequests;
    }
}
```

Three bugs hide in that snippet: the `apcu_fetch` + `apcu_store` pair is not atomic, the window start is lost on a close race, and cleanup is effectively whatever APCu decides to TTL-evict.

### After: Shared\Map + update()

```php
<?php
final class RateLimiter
{
    public function __construct(
        private OxPHP\Shared\Map $buckets,
        private int $maxRequests,
        private int $windowSeconds,
    ) {}

    public function allow(string $ip): bool
    {
        $now = time();

        // Atomic read-modify-write. The closure sees the current value
        // (or null on first hit) and returns the new shape to store.
        $state = $this->buckets->update($ip, function ($current) use ($now) {
            if ($current === null || $now - $current['start'] >= $this->windowSeconds) {
                return ['count' => 1, 'start' => $now];
            }
            return ['count' => $current['count'] + 1, 'start' => $current['start']];
        });

        return $state['count'] <= $this->maxRequests;
    }

    /** Background cleanup — call from a scheduled worker or oxphp_async loop. */
    public function sweep(): void
    {
        $now    = time();
        $cutoff = $this->windowSeconds * 2;
        foreach ($this->buckets->keys() as $ip) {
            $state = $this->buckets->get($ip);
            if ($state !== null && $now - $state['start'] >= $cutoff) {
                $this->buckets->remove($ip);
            }
        }
    }
}

// Bootstrap
$limiter = new RateLimiter(
    buckets:       new OxPHP\Shared\Map(maxEntries: 50_000),
    maxRequests:   100,
    windowSeconds: 60,
);

// Per-request
if (!$limiter->allow($_SERVER['REMOTE_ADDR'])) {
    http_response_code(429);
    header('Retry-After: 60');
    echo '429 Too Many Requests';
    return;
}
```

What the migration bought you:

- **Atomicity.** `update($key, fn)` is a single RMW; no read-then-store race.
- **Deterministic cleanup.** `sweep()` is predictable and runs on a schedule you control.
- **One less dependency.** APCu is no longer in the deployment story.
- **Accurate count under load.** Two concurrent hits on the same IP never clobber each other.

> The built-in [Rate Limiting](rate-limiting.md) feature (`RATE_LIMIT=...`) continues to run at the connection layer and is faster than a PHP-level limiter. Reach for a custom limiter only when you need PHP-level policy — per-tenant, per-route, per-user-id rather than per-IP.

### When to pick Map vs Counter

The example uses a `Map<string, array{count,start}>` because each IP needs two fields kept in sync. When you just need a running total with no associated window state, a `Counter` is cheaper:

```php
<?php
$hits = new OxPHP\Shared\Counter();

// Every request increments, no read-modify-write cycle.
$current = $hits->inc();          // atomic increment-then-get
```

Reach for `Map<string, Counter>` (a map of counters) when you need per-key totals with no window logic:

```php
<?php
$perTenant = new OxPHP\Shared\Map();

$counter = $perTenant->getOrSet($tenantId, fn () => new OxPHP\Shared\Counter());
$counter->inc();
```

## Handle semantics

Every `Shared\*` object is a thin PHP wrapper around a registry ID. A few rules follow from that:

1. **Identity is the registry ID, not the PHP object.** Two handles with the same ID point at the same state. `$a->id() === $b->id()` is the equality test.
2. **Serialisation is blocked.** `serialize($counter)` throws. The registry lives only in this process — there is nothing useful to put on the wire. Use the [migration guide](migrating-to-external-store.md) when you genuinely need to cross a process boundary.
3. **`clone` is blocked.** For the same reason — a cloned wrapper would look distinct but share state. Construct a fresh instance if you want an independent value.
4. **`id()` is stable until the last reference drops.** Persist it in logs, include it in trace spans, use it as the lookup key when the internal server returns an entry in [Observability](../operations/shared-observability.md).

## Lifecycle

Entries are reference-counted. A registry entry stays alive while **any** of the following hold:

- A PHP wrapper in any worker references it.
- It is a value stored inside another live `Shared\Map` / `Shared\Channel`.
- A pending async operation captured it by closure.

Once the refcount reaches zero the registry calls a type-specific `on_drop` (which releases nested refs, closes channels, drops pool slots, etc.), then frees the slot.

There is no explicit `close()` on most types. `Shared\Channel` has one because senders and receivers need a way to signal "no more items"; it does **not** free the registry entry early — that still requires dropping all references. `Shared\Pool::evict()` drops idle slots but leaves the pool itself in place.

## Cycle safety

Storing one Shareable inside another is allowed; storing A inside B while B (directly or transitively) already reaches A would create a cycle and leak. Every mutation that adds a reference runs a bounded BFS first and rejects the write with `OxPHP\Shared\CycleException` before touching state:

```php
<?php
$a = new OxPHP\Shared\Map();
$b = new OxPHP\Shared\Map();
$a->set('b', $b);                // fine

try {
    $b->set('a', $a);            // closes the loop — rejected
} catch (OxPHP\Shared\CycleException $e) {
    // $b is untouched, no partial state, no leaked retain on $a
}
```

The walker budget is tunable: `SHARED_CYCLE_DETECT_DEPTH` (default 16) and `SHARED_CYCLE_DETECT_EDGES` (default 10 000). Very large legitimate graphs that exceed the budget surface `CycleException` with a `bounds exceeded` message.

## Cross-thread handoff

Shared handles are safe to use from any worker and from async contexts. The only rule: **pass them through `use`, not `global`.** Closures captured by async operations need explicit imports so the runtime can hold refcounts correctly.

```php
<?php
$queue = new OxPHP\Shared\Channel(256);

oxphp_async(function () use ($queue) {              // ← explicit use
    while (($job = $queue->recv(timeout: 30.0)) !== null) {
        process($job);
    }
});

$queue->send(['url' => $_POST['url']]);
```

`Shared\*` instances are **not** serialisable, so do not try to ship them through shell commands, HTTP bodies, or session storage.

## Observability

Every registry entry is visible through the [internal server](../operations/internal-server.md). See [Shared Observability](../operations/shared-observability.md) for the full tour. At a glance:

- `GET /__ox_shared/summary` — aggregate counts, memory, and ops by type.
- `GET /__ox_shared/entries` — every live entry with id, type, refcount, and size.
- `GET /__ox_shared/entry?id=N` — type-specific detail for one entry.
- `GET /__ox_shared/graph?id=N` — BFS walk of outgoing references (useful after a `CycleException`).
- `/metrics` — Prometheus counters and gauges prefixed `oxphp_shared_*`.

Disable introspection in production untrusted-tenant setups with `SHARED_INTROSPECTION_ENABLED=false`; metrics stay on.

## Configuration

All env vars are read at startup. Defaults are sized for hundreds of entries on a single host; bump them for registry-heavy deployments.

| Env var                         | Default | Effect                                                                |
|---------------------------------|---------|-----------------------------------------------------------------------|
| `SHARED_MAX_ENTRIES`            | 100 000 | Global cap on all Shared entries combined. Insert past this fails.    |
| `SHARED_MAX_BYTES`              | 1 GiB   | Global cap on estimated memory across all Shared entries.             |
| `SHARED_SOFT_LIMIT_RATIO`       | 0.7     | Start shedding lowest-priority work when usage crosses this fraction. |
| `SHARED_CYCLE_DETECT_DEPTH`     | 16      | BFS depth during cycle check. Raise for deep legitimate graphs.       |
| `SHARED_CYCLE_DETECT_EDGES`     | 10 000  | Edges walked during cycle check. Raise for dense legitimate graphs.   |
| `SHARED_PREVIEW_ARRAY_LIMIT`    | 20      | Entries sampled in `/entry?id=…` previews.                            |
| `SHARED_PREVIEW_STRING_LIMIT`   | 256     | Per-string truncation in previews.                                    |
| `SHARED_INTROSPECTION_ENABLED`  | true    | Toggles the `/__ox_shared/*` API.                                     |
| `SHARED_METRICS_ENABLED`        | true    | Toggles the `oxphp_shared_*` Prometheus exposition.                   |
| `SHARED_SHUTDOWN_TIMEOUT_SECONDS`| 5.0    | Max wait for blocked senders/receivers/pool callers at server stop.   |

## When not to use

Shared state is in-process. That constrains when it fits.

- **Multiple hosts.** If you run more than one OxPHP process (the normal case for anything beyond a single box), workers in process A cannot see `Shared\*` entries in process B. Use Redis, NATS, or whatever you already deploy. [Migrating to an external store](migrating-to-external-store.md) walks through the common patterns.
- **Durability.** Shared state evaporates on process restart. If your counter needs to survive a deploy, persist it elsewhere.
- **Unbounded values.** A `Shared\Map` with no `maxEntries` can be driven OOM by an attacker. Always set a cap on anything keyed by user input.
- **Large payloads.** Values are copied across the FFI boundary on read. Stuffing 10 MB arrays in `Shared\Map` is the wrong shape — put the blob in object storage and share the URL.
- **Replacing OPcache / APCu caching.** OPcache already caches compiled scripts; APCu caches request-scoped data per worker (which is cheaper when you do not actually need cross-worker visibility).

## Common gotchas

- **Forgetting to cap `Shared\Map`.** Unbounded maps keyed by user IPs / user IDs / session tokens are the top self-inflicted OOM. Always pass `maxEntries` and catch `CapacityException`.
- **Reading a whole array on every request.** `Map::get` copies. If you touch a large array dozens of times per request, cache the copy in a request-scoped variable.
- **Treating `recv` / `get` as non-null.** Every read can legitimately return `null` (closed channel, missing key). Always null-check.
- **Using `global` with async.** Fibers started by `oxphp_async` need their captures in a `use (…)` clause. `global` references are not tracked.
- **Clone surprise.** `clone $counter` throws. Early users often try it; learn the alternative (`new Shared\Counter($counter->get())`) once.

## Related

- [Shared\Counter](shared-counter.md) — domain accumulator.
- [Shared\Atomic](shared-atomic.md) — generic atomic int64 with full memory-ordering control.
- [Shared\Flag](shared-flag.md) — atomic bool / kill-switch.
- [Shared\Once](shared-once.md) — run-once across workers.
- [Shared\Mutex](shared-mutex.md) — poisoning mutex over a value.
- [Shared\Channel](shared-channel.md) — bounded MPMC queue.
- [Shared\Map](shared-map.md) — concurrent keyed store.
- [Shared\Pool](shared-pool.md) — bounded object pool.
- [Shared Observability](../operations/shared-observability.md) — introspection, metrics, diagnostics.
- [Migrating to an external store](migrating-to-external-store.md) — when shared state outgrows a single host.
