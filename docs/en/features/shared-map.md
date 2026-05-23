---
title: Shared\Map
description: Process-wide concurrent hash-map for coordinating state across PHP workers — null-as-absence model, a single linearisable compareAndSet, lazy iteration, and cycle-safe nested Shareable values.
---

# Shared\Map

`OxPHP\Shared\Map` is a concurrent map that lives in the shared registry and is visible to every PHP worker in the process. It is the go-to primitive when two workers — or a request handler and a background task — need to share mutable state that survives the request lifecycle.

## Overview

- **`int|string → mixed`.** Keys are PHP integers or strings, kept **distinct** (`123` and `"123"` are different keys; there is no PHP array-style key coercion). String keys are **binary-safe** — stored as opaque bytes (like PHP arrays / Go / Redis), so non-UTF-8 keys (incl. embedded NUL) round-trip faithfully. Values may be any scalar, array of scalars/arrays, or another `Shareable` instance.
- **`null` means absence — never a stored value.** Writing a `null` value throws `TypeException`; a `null` return always means "no such key". This removes the classic `get()`-returned-null ambiguity at the root (the same choice `java.util.concurrent.ConcurrentHashMap` and Go's `sync.Map` make).
- **One linearisable conditional primitive.** `compareAndSet` covers atomic insert / replace / remove via the `null` absence sentinel; build any read-modify-write on top of it.
- **Concurrent.** Writes from different workers don't require external locking; per-key operations are atomic at the shard level.
- **Cycle-safe.** Storing a `Shareable` that would reach back into this Map is rejected with `CycleException` before any mutation — no leaks on the rejected path.
- **Bounded by an approximate soft cap.** `maxEntries` is an OOM-safety ceiling, not exact accounting.

## API Reference

```php
namespace OxPHP\Shared;

final class Map implements Shareable
{
    public function __construct(?int $maxEntries = null);   // null = unbounded; <= 0 throws TypeException

    // reads
    public function get(int|string $key): mixed;            // null ⟺ absent
    public function getMany(iterable $keys): \Iterator;     // lazy; skips absent keys
    public function count(): int;                           // striped, weakly consistent
    public function maxEntries(): ?int;

    // writes
    public function set(int|string $key, mixed $value): void;
    public function setIfAbsent(int|string $key, mixed $value): mixed;  // prev; null ⟺ inserted
    public function setMany(iterable $entries): int;
    public function remove(int|string $key): bool;          // existed?
    public function removeMany(iterable $keys): int;
    public function clear(): int;                           // entries removed

    // value-returning
    public function swap(int|string $key, mixed $value): mixed;  // prev; null ⟺ was absent
    public function pop(int|string $key): mixed;                 // prev; null ⟺ was absent

    // conditional (single linearisable primitive)
    public function compareAndSet(int|string $key, mixed $expected, mixed $new): bool;

    // iteration
    public function forEach(callable $fn): void;            // weakly consistent; callback runs lock-free

    public function id(): int;
}
```

| Method         | Use case                                                                             |
|----------------|--------------------------------------------------------------------------------------|
| `__construct`  | Create with an optional `maxEntries` cap (null = unbounded; `<= 0` throws).           |
| `get`          | Fetch by key; `null` ⟺ absent.                                                        |
| `getMany`      | Lazily stream `key => value` for known keys; absent keys are skipped (see below).     |
| `count`        | Approximate entry count (weakly consistent under concurrent writes).                  |
| `maxEntries`   | Reports the configured cap (or `null` when unbounded).                                |
| `set`          | Insert or replace; no previous value materialised.                                    |
| `setIfAbsent`  | Atomic insert-if-missing; returns the existing value, or `null` if it inserted.       |
| `setMany`      | Bulk insert from any iterable; returns the number written.                            |
| `remove`       | Remove a key; returns whether it existed (no value materialised).                     |
| `removeMany`   | Bulk remove; returns the number actually deleted.                                     |
| `clear`        | Drop every entry (releasing nested `Shareable` holds); returns the count removed.     |
| `swap`         | Overwrite and return the previous value (`null` ⟺ was absent).                        |
| `pop`          | Remove and return the previous value (`null` ⟺ was absent).                           |
| `compareAndSet`| Atomic insert / replace / remove keyed on the current content (see below).            |
| `forEach`      | Weakly-consistent traversal; the callback runs with no lock held.                     |
| `id`           | Numeric registry identifier; useful for logging + `/__ox_shared/entry?id=…`.          |

There is no `has()`, `update()`, `getOrSet()`, `keys()`, `trySet()`, `updateMany()`, and the class does **not** implement `Countable` — see [Migrating from the old surface](#migrating-from-the-old-surface).

## The `null`-is-absence model

`null` is reserved as the absence sentinel everywhere:

- `set` / `swap` / `setIfAbsent` with a `null` value → `TypeException`.
- `get` / `swap` / `pop` / `setIfAbsent` returning `null` ⟺ the key was absent.
- In `compareAndSet`, `null` on either side means "absent" (not "store null").

If you need to record "no value", remove the key (or use a key absence) rather than storing `null`. Because there is no `has()` and no concurrent `has()`+`get()` race, presence is checked atomically with a single `get($k) !== null`.

## compareAndSet — the conditional primitive

```php
$map->compareAndSet($key, expected: null, new: $v);    // insert iff absent   (= setIfAbsent, returns bool)
$map->compareAndSet($key, expected: $a,   new: $b);    // replace iff current === $a
$map->compareAndSet($key, expected: $a,   new: null);  // remove  iff current === $a
```

It returns `true` iff the swap was applied. Equality is **by content**: scalars by value, strings and arrays by their serialised bytes, and nested `Shareable` values by registry identity. Array equality matches PHP `===` for the common cases (lists, all-int- or all-string-keyed arrays); arrays that *interleave* int and string keys are compared over the Map's normalised storage form, so a specific int/string ordering is not distinguished (the Map reorders such arrays on read-back too). Build a read-modify-write as an explicit retry loop — and keep the closure pure, since under contention it runs more than once:

```php
do {
    $cur = $map->get('counter');          // null if absent
    $next = ($cur ?? 0) + 1;
} while (!$map->compareAndSet('counter', $cur, $next));
```

There is no ABA hazard here: the store is content-addressed (a value equal in content *is* the same value for a value store), and nested-`Shareable` identity uses monotonic, never-reused registry ids. For stampede-safe lazy initialisation use [`Shared\Once`](shared-once.md); for pooled resources use [`Shared\Pool`](shared-pool.md).

## Memory model — where copies happen

Values are stored as a serialised representation, **not** as zvals — so "zero-copy" does not apply to values:

| Operation                         | Serialise into shared heap | Materialise previous value into a zval |
|-----------------------------------|:--------------------------:|:--------------------------------------:|
| `set` / `setMany`                 | yes                        | no                                     |
| `remove` / `removeMany`           | —                          | no                                     |
| `setIfAbsent`                     | yes                        | only if a previous value exists        |
| `swap` / `pop`                    | yes / —                    | yes                                    |
| `get` / `getMany`                 | (the key only)             | yes                                    |
| `compareAndSet`                   | yes (`$new`)               | no                                     |

The write-path serialisation is unavoidable for any value entering shared memory. The read-back into a fresh zval is paid **only** by the methods that return a previous/looked-up value — so `set`/`remove` are "no return materialisation", not "free". The exception is a **nested `Shareable`** value: it is stored by reference (an id + a refcount bump), not deep-copied.

## Concurrency

- **`count()` is weakly consistent.** Entry counts are striped per shard and summed on read; the result is exact when the map is quiescent and a close approximation under concurrent writes (the same contract as `ConcurrentHashMap::size`). Striping keeps writes off a single hot counter.
- **`maxEntries` is a soft cap.** It is checked against the striped sum, so under concurrent inserts the map may overshoot by up to the shard count before rejecting a new key with `CapacityException`. Treat it as an OOM-safety budget, not exact accounting. Overwriting an existing key always succeeds at the cap. There is **no eviction** — a cache with LRU/TTL eviction is a different primitive.
- **`forEach` runs the callback with no lock held.** It snapshots one shard's keys at a time, releases the shard, then re-fetches each value and invokes `$fn(key, value)`. Keys deleted between snapshot and call are skipped; keys added after a shard's snapshot may be missed; values may be fresher than snapshot time. Return `false` from the callback to stop early. Because only keys are snapshotted, a slow callback never pins deleted values.

## Examples

### Shared configuration cache

```php
<?php
$config = new OxPHP\Shared\Map(maxEntries: 1024);

// Warm once at app bootstrap.
$config->setMany([
    'rate_limit.default_rpm' => 600,
    'feature.new_checkout'   => true,
    'timeout.downstream_ms'  => 250,
]);

// Any request handler reads without contention; null ⟺ not configured.
$rpm = $config->get('rate_limit.default_rpm') ?? 60;
```

### Per-tenant rate limiter

```php
<?php
$buckets = new OxPHP\Shared\Map(maxEntries: 50_000);

$key  = "tenant:{$tenantId}";
$prev = $buckets->setIfAbsent($key, ['tokens' => 100, 'refill_at' => time() + 60]);
// $prev === null ⟺ we created the bucket; otherwise it holds the existing one.

$state = $buckets->get($key);
if ($state['tokens'] === 0) {
    throw new RateLimitException();
}
```

### Coordinating counters across workers

```php
<?php
$counters = new OxPHP\Shared\Map();
$counters->set('requests_handled', new OxPHP\Shared\Counter());

// Any worker increments via the stored Shareable (stored by reference).
$counters->get('requests_handled')->inc();
```

### Iterating a large map

```php
<?php
$sessions->forEach(function (int|string $key, mixed $value): bool|null {
    if ($value['expires_at'] < time()) {
        // safe: forEach holds no lock during the callback
        return null; // keep going
    }
    return null;
});

// Or read a known subset lazily, stopping early:
foreach ($cache->getMany($hotKeys) as $key => $value) {
    if (enoughCollected()) break;     // remaining keys are never materialised
    handle($key, $value);
}
```

## Semantics & gotchas

### Arrays are copied on read

```php
<?php
$m = new OxPHP\Shared\Map();
$m->set('cfg', ['timeout' => 5, 'retries' => 3]);

$cfg = $m->get('cfg');
$cfg['timeout'] = 10;     // mutates the returned copy only
// $m->get('cfg')['timeout'] is still 5
```

To update an array value atomically, read it, modify the copy, and commit with `compareAndSet` (retrying on conflict), or store independently-changing fields as nested `Shared\Counter` / `Shared\Map`.

### Nested Shareable retains are automatic

```php
<?php
$map     = new OxPHP\Shared\Map();
$counter = new OxPHP\Shared\Counter(10);
$map->set('c', $counter);

$retrieved = $map->get('c');           // same Shareable identity
$retrieved->inc();                      // mutation visible via $counter too
echo $counter->get();                   // 11

$counter2 = $map->pop('c');             // Map releases its hold, returns the value
$counter2->inc();                       // still alive via the returned wrapper
```

### Cycle detection rejects before it mutates

```php
<?php
$a = new OxPHP\Shared\Map();
$b = new OxPHP\Shared\Map();
$a->set('b', $b);                       // fine

try {
    $b->set('a', $a);                   // closes the loop
} catch (OxPHP\Shared\CycleException $e) {
    // message: "cycle would form: #… → #… (inserting into #…)"
}

$b->get('a');                           // null — $b untouched, no leaked retains
```

Nested references inside arrays are checked too. The walker is bounded by `SHARED_CYCLE_DETECT_DEPTH` (default 16) and `SHARED_CYCLE_DETECT_EDGES` (default 10 000); very large graphs surface `CycleException` with `bounds exceeded` — raise the env knobs or break the graph.

### Per-value size cap

A single value whose serialised size exceeds `SHARED_MAX_VALUE_SIZE` (default 1 MiB) is rejected with `ValueTooLargeException`. This guards against an allocation bomb from PHP-side input. It applies to every write path (`set`, `setIfAbsent`, `swap`, `compareAndSet`, `setMany`).

### Batched operations are per-key atomic, not batch-atomic

`setMany`, `getMany`, and `removeMany` apply one key at a time. If `setMany` hits a `CapacityException`, `CycleException`, or `ValueTooLargeException` partway through, earlier keys remain stored — the partial success is intentional. Use a `Shared\Mutex` around the map if you need all-or-nothing semantics.

## Migrating from the old surface

This is a breaking redesign with no compatibility shims:

| Old                                   | New                                                              |
|---------------------------------------|------------------------------------------------------------------|
| `has($k)`                             | `get($k) !== null` (atomic — no `has`/`get` race)                |
| `get($k, $default)`                   | `get($k) ?? $default`                                            |
| `trySet($k, $v): bool`                | `setIfAbsent($k, $v): mixed` (returns prev; `null` ⟺ inserted)   |
| `remove($k)` (returned prev)          | `remove($k): bool`, or `pop($k)` to get the value                |
| `update($k, $fn)`                     | `compareAndSet` retry loop, or `Shared\Once` for once-init       |
| `getOrSet($k, $fn)`                   | `setIfAbsent`, or `Shared\Once` / `Shared\Pool` by case          |
| `updateMany(...)`                     | a loop of `compareAndSet`                                        |
| `keys(): array`                       | `forEach(...)`, or `getMany($knownKeys)`                         |
| `count($map)` (Countable)             | `$map->count()`                                                 |
| storing a `null` value                | use key absence / `remove`                                       |

## Exceptions

All methods that can fail throw subclasses of `OxPHP\Shared\SharedException`:

| Exception                | Raised by                                                                 |
|--------------------------|---------------------------------------------------------------------------|
| `CapacityException`      | A new key past `maxEntries` (`set` / `setIfAbsent` / `compareAndSet` / `setMany`). |
| `ValueTooLargeException` | A value over the per-value cap (`SHARED_MAX_VALUE_SIZE`).                  |
| `CycleException`         | A write that would close a reachability cycle (`extends TypeException`).   |
| `TypeException`          | A `null` value; a non-storable value (object/closure/resource); a non-int/string key; `maxEntries <= 0`. |
| `StaleHandleException`   | A method call on a handle whose registry entry was evicted.               |

## Observability

Every Map is visible through the internal API:

- `GET /__ox_shared/summary` — aggregate counts by type, including `Map`.
- `GET /__ox_shared/entries` — list all entries with id / type / refcount / mem_bytes.
- `GET /__ox_shared/entry?id=N` — per-instance details for Map include `key_count`, `max_entries`, `saturation`, and `sample_keys` (truncated by the preview limit).
- `GET /__ox_shared/graph?id=N[&depth=D][&edges=E]` — BFS walk of outgoing Shareable references; handy after a `CycleException`.

Prometheus exposes per-Map gauges at `/metrics`:

| Metric                                     | Meaning                                    |
|--------------------------------------------|--------------------------------------------|
| `oxphp_shared_map_entries{map_id="…"}`     | Current (approximate) key count.           |
| `oxphp_shared_map_max_entries{map_id="…"}` | Configured cap (0 when unbounded).         |
| `oxphp_shared_map_saturation{map_id="…"}`  | `entries / max_entries`, 0 when unbounded. |

## Configuration

| Env var                         | Default | Effect                                                                |
|---------------------------------|---------|-----------------------------------------------------------------------|
| `SHARED_MAX_ENTRIES`            | 100 000 | Global cap on all Shared entries combined.                            |
| `SHARED_MAX_BYTES`              | 1 GiB   | Global cap on estimated memory across all Shared entries.             |
| `SHARED_MAX_VALUE_SIZE`         | 1 MiB   | Per-value serialised size cap; larger values throw `ValueTooLargeException`. |
| `SHARED_CYCLE_DETECT_DEPTH`     | 16      | Max BFS depth during cycle check. Raise for deep legit graphs.        |
| `SHARED_CYCLE_DETECT_EDGES`     | 10 000  | Max edges walked during cycle check. Raise for dense legit graphs.    |
| `SHARED_PREVIEW_ARRAY_LIMIT`    | 20      | Number of entries sampled in `/entry?id=…` `sample_keys`.             |
| `SHARED_INTROSPECTION_ENABLED`  | true    | Toggles the `/__ox_shared/*` API.                                     |

## Related

- [`Shared\Counter`](shared-counter.md) — atomic integer; store inside a Map for per-key hit counts.
- [`Shared\Once`](shared-once.md) — stampede-safe lazy initialisation when `setIfAbsent` would re-run an expensive factory.
- [`Shared\Channel`](shared-channel.md) — MPMC queue; complementary when you need FIFO pipelines rather than keyed lookup.
- [`Shared\Mutex`](shared-mutex.md) — when you need strict mutual exclusion around a stored value.
