---
title: Shared\Counter
description: Atomic int64 accumulator shared across PHP workers — lock-free increment/decrement, signed addition, batched accumulation, windowed reset.
---

# Shared\Counter

`OxPHP\Shared\Counter` is a process-wide atomic 64-bit signed integer specialised for **accumulation**: counting events, summing deltas, rolling window totals. Every operation is lock-free and linearisable; two workers incrementing concurrently never lose a tick.

For arbitrary atomic state — state machines, version stamps, CAS loops, bitflag masks — use [`Shared\Atomic`](shared-atomic.md) instead.

## Overview

- **Atomic int64.** Range `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`. Overflow wraps.
- **Lock-free.** `inc` / `dec` / `add` compile to a single `fetch_add`.
- **Shareable.** Instances can be stored inside `Shared\Map` / `Shared\Channel` and handed to fibers via `use` captures.

## API Reference

```php
namespace OxPHP\Shared;

final class Counter implements Shareable
{
    public function __construct(int $initial = 0);

    public function get(): int;
    public function inc(int $by = 1): int;            // returns new
    public function dec(int $by = 1): int;            // returns new
    public function add(int $delta): int;             // returns new
    public function addBatch(array $deltas): int;     // returns new
    public function reset(): int;                     // returns previous, atomically zeroes

    public function id(): int;
}
```

| Method           | Returns     | Use case                                                        |
|------------------|-------------|-----------------------------------------------------------------|
| `get`            | current     | Read without mutation.                                          |
| `inc` / `dec`    | new         | Per-event tallies; `$by` lets you skip by N in one atomic op.   |
| `add`            | new         | Any delta, positive or negative.                                |
| `addBatch`       | new         | Bulk accumulation with one FFI round trip.                      |
| `reset`          | previous    | End-of-window roll-over: atomically read the total and zero it. |
| `id`             | registry id | Logging, tracing, `/__ox_shared/entry?id=…` correlation.        |

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

### Windowed rollover

```php
<?php
$hits = new OxPHP\Shared\Counter();

// Every N minutes in your cron/worker loop:
$prev = $hits->reset();                // atomically reads and zeroes
logWindowMetric($prev);
```

> Need `compareAndSet`, `swap`, or other low-level atomic operations for state machines or version stamps? Counter is a domain accumulator — reach for [`Shared\Atomic`](shared-atomic.md).

### Bulk accumulation

```php
<?php
$bytes = new OxPHP\Shared\Counter();

// Count bytes from a batch in one FFI call instead of N.
$deltas = array_map(fn ($req) => strlen($req['body']), $batch);
$newTotal = $bytes->addBatch($deltas);
```

## Semantics & gotchas

- **`reset()` returns the previous value, then zeroes — atomically.** It's the snapshot-and-zero pattern from `LongAdder::sumThenReset`. There is no `reset(int $newValue)`; if you need to seed a non-zero starting point, construct a fresh `Counter(initial: …)` or use `Shared\Atomic::store`.
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
- [Shared\Atomic](shared-atomic.md) — generic atomic int64 with CAS, swap, and full memory-ordering control.
- [Shared\Map](shared-map.md) — when counts are keyed (`Map<string, Counter>`).
- [Shared\Flag](shared-flag.md) — when the value is just on/off.
- [Shared\Mutex](shared-mutex.md) — when a counter must update in lockstep with other fields.
