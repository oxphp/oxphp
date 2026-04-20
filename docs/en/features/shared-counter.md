---
title: Shared\Counter
description: Atomic int64 shared across PHP workers — lock-free increment/decrement, compare-and-set, and bulk-add for high-throughput counting.
---

# Shared\Counter

`OxPHP\Shared\Counter` is a process-wide atomic 64-bit signed integer. Every operation is lock-free and linearisable; two workers incrementing concurrently never lose a tick.

## Overview

- **Atomic int64.** Range `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`. Overflow wraps.
- **Lock-free.** `inc` / `dec` / `add` compile to a single `fetch_add`; `compareAndSet` is one CAS.
- **Shareable.** Instances can be stored inside `Shared\Map` / `Shared\Channel` and handed to fibers via `use` captures.

## API Reference

```php
namespace OxPHP\Shared;

final class Counter implements Shareable
{
    public function __construct(int $initial = 0);

    public function get(): int;
    public function set(int $value): int;             // returns previous
    public function inc(int $by = 1): int;            // returns new
    public function dec(int $by = 1): int;            // returns new
    public function add(int $delta): int;             // returns new
    public function compareAndSet(int $expect, int $new): bool;
    public function addBatch(array $deltas): int;     // returns new
    public function reset(int $newValue = 0): int;    // returns previous

    public function id(): int;
}
```

| Method           | Returns   | Use case                                                        |
|------------------|-----------|-----------------------------------------------------------------|
| `get`            | current   | Read without mutation.                                          |
| `set`            | previous  | Replace unconditionally; useful for initialisation with seed.   |
| `inc` / `dec`    | new       | Per-event tallies; `$by` lets you skip by N in one atomic op.   |
| `add`            | new       | Any delta, positive or negative.                                |
| `compareAndSet`  | swapped?  | Optimistic state machines (idle → busy → done).                 |
| `addBatch`       | new       | Bulk accumulation with one FFI round trip.                      |
| `reset`          | previous  | End-of-window roll-over; `reset()` zeroes, `reset(n)` seeds.    |
| `id`             | registry id | Logging, tracing, `/__ox_shared/entry?id=…` correlation.      |

## Examples

### Per-worker request counter

```php
<?php
$requests = new OxPHP\Shared\Counter();

oxphp_worker(function () use ($requests) {
    $count = $requests->inc();
    header("X-Request-Count: {$count}");
    echo "ok";
});
```

### Optimistic state machine

```php
<?php
$state = new OxPHP\Shared\Counter(initial: 0); // 0=idle, 1=busy, 2=done

if (!$state->compareAndSet(expect: 0, new: 1)) {
    throw new RuntimeException('another worker is already processing');
}

try {
    doWork();
    $state->set(2);
} catch (Throwable $e) {
    $state->set(0); // release back to idle on error
    throw $e;
}
```

### Windowed rollover

```php
<?php
$hits = new OxPHP\Shared\Counter();

// Every N minutes in your cron/worker loop:
$prev = $hits->reset();                // atomically reads-and-zeroes
logWindowMetric($prev);
```

### Bulk accumulation

```php
<?php
$bytes = new OxPHP\Shared\Counter();

// Count bytes from a batch in one FFI call instead of N.
$deltas = array_map(fn ($req) => strlen($req['body']), $batch);
$newTotal = $bytes->addBatch($deltas);
```

## Semantics & gotchas

- **`set` returns the previous value, not the new one.** This matches `std::atomic<T>::exchange` semantics and is consistent with `reset`. Use `get()` after `set()` if you want what you just wrote.
- **`addBatch` is not atomic across items.** It is a loop of `fetch_add` under the hood — the final value is correct, but other workers see intermediate totals during the batch. Use `Shared\Mutex` wrapping a Counter if you need whole-batch visibility.
- **Overflow wraps.** Adding `INT_MAX + 1` loops back to `INT_MIN`. For monotonic counters that may run for months at thousands-per-second, keep the value in tens-of-trillions range or reset periodically.
- **No fractional values.** If you are counting bytes and need float-precision averages, track numerator (Counter) and denominator (Counter) separately and divide at read time.

## Exceptions

| Exception               | Raised by                                                    |
|-------------------------|--------------------------------------------------------------|
| `StaleHandleException`  | Any method on a handle whose registry entry was evicted.     |
| `UninitializedException`| `id()` on a wrapper that has not finished `__construct`.     |

Counters never throw on overflow or extreme values — they wrap.

## Observability

See [Shared Observability](../operations/shared-observability.md) for the full tour. Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ value, type: "Counter" }`.
- Prometheus `oxphp_shared_counter_value{counter_id="…"}` gauge tracks the current value.
- Registry-wide counters (`oxphp_shared_ops_total`, `oxphp_shared_objects_total`) cover Counter via the `type="Counter"` label.

## When not to use

- **Floats or decimals.** Use a pair of Counters (numerator / denominator) or a `Shared\Mutex<array{total_cents: int, count: int}>`.
- **Non-numeric events that need rich context.** If you need `{count, last_actor, last_reason}` coupled to one key, reach for `Shared\Map` or `Shared\Mutex`.
- **Cross-host totals.** A Counter is in-process only. For multi-host aggregation use a metric pipeline (Prometheus + `rate()`, or a central Redis `INCR`).
- **Durability.** Counter state evaporates at server stop. Persist snapshots elsewhere if the total must survive restarts.

## Related

- [Shared State](shared-state.md) — overview and migration patterns.
- [Shared\Map](shared-map.md) — when counts are keyed (`Map<string, Counter>`).
- [Shared\Flag](shared-flag.md) — when the value is just on/off.
- [Shared\Mutex](shared-mutex.md) — when a counter must update in lockstep with other fields.
