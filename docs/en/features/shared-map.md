---
title: Shared\Map
description: Process-wide concurrent hash-map for coordinating state across PHP workers — atomic reads, batched writes, cycle-safe nested Shareable values.
---

# Shared\Map

`OxPHP\Shared\Map` is a concurrent string-keyed map that lives in the shared registry and is visible to every PHP worker in the process. It is the go-to primitive when two workers — or a request handler and a background task — need to share mutable state that survives the request lifecycle.

## Overview

- **String → mixed.** Keys are PHP strings. Values can be any scalar, array (including nested arrays), or another `Shareable` instance.
- **Concurrent.** Writes from different workers don't require external locking. Per-key operations are atomic at the shard level.
- **Cycle-safe.** Storing a `Shareable` that would eventually reach back into this Map is rejected with `CycleException` before any mutation happens — no leaks on the rejected path.
- **Optional per-instance cap.** Pass `maxEntries` at construction for a strict ceiling; overwrites of existing keys are always allowed, new keys are rejected with `CapacityException` once the cap is hit.
- **Registry-backed.** Every Map has a stable numeric `id()`; it survives request boundaries and is shared by handle.

## API Reference

```php
namespace OxPHP\Shared;

final class Map implements Shareable
{
    public function __construct(?int $maxEntries = null);

    public function get(string $key, mixed $default = null): mixed;
    public function set(string $key, mixed $value): void;
    public function has(string $key): bool;
    public function remove(string $key): mixed;
    public function clear(): void;
    public function count(): int;
    public function keys(): array;
    public function maxEntries(): ?int;

    public function setIfAbsent(string $key, mixed $value): bool;

    public function setMany(array $kv): int;
    public function getMany(array $keys): array;
    public function removeMany(array $keys): int;

    public function id(): int;
}
```

| Method         | Use case                                                                             |
|----------------|--------------------------------------------------------------------------------------|
| `__construct`  | Create with an optional `maxEntries` cap (null = unbounded).                         |
| `get`          | Fetch by key; returns `$default` when missing (default `null`).                      |
| `set`          | Insert or replace; overwrites existing values.                                       |
| `has`          | Presence check without fetching the value.                                           |
| `remove`       | Remove a key and return its previous value (`null` when missing).                    |
| `clear`        | Drop every entry and release the Map's hold on any nested `Shareable`.               |
| `count`        | Current number of entries.                                                           |
| `keys`         | Snapshot of all keys at call time. Iteration order is undefined (shard order).       |
| `maxEntries`   | Reports the configured cap (or `null` when unbounded).                               |
| `setIfAbsent`  | Atomic insert-if-missing. Returns `true` when stored, `false` when the key existed.  |
| `setMany`      | Bulk insert; returns the number of pairs stored before any error.                    |
| `getMany`      | Bulk read; missing keys come back as `null` in a keyed result array.                 |
| `removeMany`   | Bulk remove; returns the number of keys that were actually deleted.                  |
| `id`           | Numeric registry identifier; useful for logging + `/__ox_shared/entry?id=…`.         |

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

// Any request handler reads without contention.
$rpm = $config->get('rate_limit.default_rpm', 60);
```

### Per-tenant rate limiter

```php
<?php
$buckets = new OxPHP\Shared\Map(maxEntries: 50_000);

$key = "tenant:{$tenantId}";
$created = $buckets->setIfAbsent($key, ['tokens' => 100, 'refill_at' => time() + 60]);
// If another request beat us to it, $created is false — the existing bucket wins.

$state = $buckets->get($key);
if ($state['tokens'] === 0) {
    throw new RateLimitException();
}
```

### Coordinating counters across workers

```php
<?php
$counters = new OxPHP\Shared\Map();

// Store a Shareable counter under a key; handlers across workers mutate it.
$counters->set('requests_handled', new OxPHP\Shared\Counter());

// Any worker can increment via the stored Shareable.
$counters->get('requests_handled')->inc();
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

To atomically update an array value, remove + set the new shape, or use a nested `Shared\Counter` / `Shared\Map` for fields that change independently. Closure-based `update($key, fn)` is landing in a follow-up commit.

### Nested Shareable retains are automatic

When you `set($key, $shareable)` the Map retains the Shareable for as long as the entry lives. `remove`, `clear`, or eviction releases that retain. The PHP wrapper you passed in stays valid independently:

```php
<?php
$map     = new OxPHP\Shared\Map();
$counter = new OxPHP\Shared\Counter(10);
$map->set('c', $counter);

$retrieved = $map->get('c');           // same Shareable identity
$retrieved->inc();                      // mutation visible via $counter too
echo $counter->get();                   // 11

$map->remove('c');                      // Map releases its hold
$counter->inc();                        // $counter still alive via PHP var
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

// $b is untouched — no partial state, no leaked retains.
$b->has('a');                           // false
```

Nested references inside arrays are checked too:

```php
try {
    $b->set('shape', ['self' => $a]);
} catch (OxPHP\Shared\CycleException $e) { /* rejected */ }
```

The walker is bounded by `SHARED_CYCLE_DETECT_DEPTH` (default 16) and `SHARED_CYCLE_DETECT_EDGES` (default 10 000). Very large graphs can surface `CycleException` with `bounds exceeded` in the message; raise the env knobs or break the graph.

### Per-instance cap vs overwrites

```php
<?php
$m = new OxPHP\Shared\Map(maxEntries: 3);
$m->set('a', 1);
$m->set('b', 2);
$m->set('c', 3);

try {
    $m->set('d', 4);                    // 4th *new* key
} catch (OxPHP\Shared\CapacityException $e) { /* … */ }

$m->set('a', 99);                       // overwriting is always OK at cap
```

Cap violations throw `CapacityException`. The message names the limit so operators can raise it via the constructor.

### Batched operations are per-key atomic, not batch-atomic

`setMany`, `getMany`, and `removeMany` apply operations one key at a time. If `setMany` hits a `CapacityException` or `CycleException` partway through, earlier keys remain stored — the partial success is intentional, matching the spec. Wrap a whole batch in `Mutex<Map>` (shipping in a later release) if you need all-or-nothing semantics.

## Exceptions

All methods that can fail throw subclasses of `OxPHP\Shared\SharedException`:

| Exception              | Raised by                               |
|------------------------|-----------------------------------------|
| `CapacityException`    | `set` / `setIfAbsent` / `setMany` past `maxEntries`. |
| `CycleException`       | Any write that would close a reachability cycle (`extends TypeException`). |
| `TypeException`        | Constructor receiving non-positive `maxEntries`; non-serialisable values (closures, resources); non-string batched keys. |
| `StaleHandleException` | Method call on a handle whose registry entry has been evicted. |
| `UninitializedException` | `id()` on a wrapper that hasn't finished `__construct`. |

## Observability

Every Map is visible through the internal API:

- `GET /__ox_shared/summary` — aggregate counts by type, including `Map`.
- `GET /__ox_shared/entries` — list all entries with id / type / refcount / mem_bytes.
- `GET /__ox_shared/entry?id=N` — per-instance details for Map include `key_count`, `max_entries`, `saturation`, and `sample_keys` (truncated by the preview limit).
- `GET /__ox_shared/graph?id=N[&depth=D][&edges=E]` — BFS walk of outgoing Shareable references. Handy when a `CycleException` fires and you want to see the path the walker took.

Prometheus exposes per-Map gauges at `/metrics`:

| Metric                                 | Meaning                                   |
|----------------------------------------|-------------------------------------------|
| `oxphp_shared_map_entries{map_id="…"}` | Current key count.                        |
| `oxphp_shared_map_max_entries{map_id="…"}` | Configured cap (0 when unbounded).        |
| `oxphp_shared_map_saturation{map_id="…"}` | `entries / max_entries`, 0 when unbounded. |

The registry-wide gauges (`oxphp_shared_objects_total`, `oxphp_shared_bytes`, `oxphp_shared_capacity_saturation`) cover Map automatically via the `type="Map"` label.

## Configuration

| Env var                         | Default | Effect                                                                |
|---------------------------------|---------|-----------------------------------------------------------------------|
| `SHARED_MAX_ENTRIES`            | 100 000 | Global cap on all Shared entries combined.                            |
| `SHARED_MAX_BYTES`              | 1 GiB   | Global cap on estimated memory across all Shared entries.             |
| `SHARED_CYCLE_DETECT_DEPTH`     | 16      | Max BFS depth during cycle check. Raise for deep legit graphs.        |
| `SHARED_CYCLE_DETECT_EDGES`     | 10 000  | Max edges walked during cycle check. Raise for dense legit graphs.    |
| `SHARED_PREVIEW_ARRAY_LIMIT`    | 20      | Number of entries sampled in `/entry?id=…` `sample_keys`.             |
| `SHARED_INTROSPECTION_ENABLED`  | true    | Toggles the `/__ox_shared/*` API.                                     |

## Related

- [`Shared\Counter`](shared-counter.md) — atomic integer; store inside a Map for per-key hit counts.
- [`Shared\Channel`](shared-channel.md) — MPMC queue; complementary when you need FIFO pipelines rather than keyed lookup.
- [`Shared\Mutex`](shared-mutex.md) — when you need strict mutual exclusion around a stored value.
