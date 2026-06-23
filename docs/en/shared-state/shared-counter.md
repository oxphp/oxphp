---
title: Shared\Counter
description: Atomic int64 accumulator shared across PHP workers — lock-free signed add, atomic exchange, compare-and-set, windowed reset via set(0).
---

# Shared\Counter

`OxPHP\Shared\Counter` is a process-wide atomic 64-bit signed integer specialised for **accumulation**: counting events, summing deltas, rolling window totals. Every operation is lock-free; two workers adding concurrently never lose a tick.

For arbitrary atomic state that must *synchronise other memory* — state machines, version stamps, seqlocks, bitflag masks — use [`Shared\Atomic`](shared-atomic.md) instead.

## Overview

- **Atomic int64.** Range `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`. Overflow wraps.
- **Lock-free.** `add` compiles to a single `fetch_add`.
- **Always Relaxed.** Operations are atomic (no lost ticks, no torn reads) but establish *no happens-before* with other memory. A Counter is statistics, not a synchronisation point — if you need ordering, use `Shared\Atomic`.
- **Shareable.** Instances can be stored inside `Shared\Map` / `Shared\Channel` and handed to fibers via `use` captures.

## API Reference

```php
namespace OxPHP\Shared;

final class Counter implements Shareable
{
    public function __construct(int $initial = 0);

    public function get(): int;                            // current
    public function set(int $value): int;                  // returns previous; set(0) = window reset
    public function add(int $delta = 1): int;              // returns new; add()=+1, add(-1)=decrement
    public function compareAndSet(int $expect, int $new): bool;

    public function id(): int;
}
```

| Method          | Returns     | Use case                                                              |
|-----------------|-------------|----------------------------------------------------------------------|
| `get`           | current     | Read without mutation.                                                |
| `set`           | previous    | Atomic exchange; `set(0)` is the end-of-window read-and-zero.         |
| `add`           | new         | `add()` increments by 1, `add(-1)` decrements, any delta otherwise.  |
| `compareAndSet` | bool        | Bounded / saturating counters (cap, floor) via a CAS loop.           |
| `id`            | registry id | Logging, tracing, `/__ox_shared/entry?id=…` correlation.             |

## Examples

### Per-worker request counter

```php
<?php
$requests = new OxPHP\Shared\Counter();

oxphp_worker(function () use ($requests) {
    $count = $requests->add();          // +1, returns the new total
    header("X-Request-Count: {$count}");
    echo "ok";
});
```

### Windowed rollover

```php
<?php
$hits = new OxPHP\Shared\Counter();

// Every N minutes in your cron/worker loop:
$prev = $hits->set(0);                   // atomically reads and zeroes
logWindowMetric($prev);
```

### Bounded counter (CAS loop)

```php
<?php
$slots = new OxPHP\Shared\Counter();
$cap   = 100;

// Claim a slot only while under the cap.
do {
    $cur = $slots->get();
    if ($cur >= $cap) {
        // full — reject
        break;
    }
} while (!$slots->compareAndSet($cur, $cur + 1));
```

### Bulk accumulation

```php
<?php
$bytes = new OxPHP\Shared\Counter();

// Sum a batch in PHP, then one atomic add (one FFI call).
$deltas   = array_map(fn ($req) => strlen($req['body']), $batch);
$newTotal = $bytes->add(array_sum($deltas));
```

## Semantics & gotchas

- **`set()` returns the previous value, then stores — atomically.** `set(0)` is the snapshot-and-zero (`LongAdder::sumThenReset`) pattern; `set($n)` seeds any new starting point.
- **Relaxed ordering.** Each operation is atomic, but a Counter does not publish other memory. If a reader must observe data a writer wrote *before* bumping the integer, that is synchronisation — use [`Shared\Atomic`](shared-atomic.md) with `Ordering::Release`/`Acquire`.
- **`compareAndSet` is Relaxed/Relaxed and takes no ordering arguments.** It is correct for decisions made on the counter's own value (cap, floor, claim-by-value). CAS that publishes other state belongs to `Shared\Atomic`.
- **Overflow wraps.** Adding past `INT_MAX` loops to `INT_MIN`. For counters running for months at thousands-per-second, keep the value in tens-of-trillions range or reset periodically.
- **No fractional values.** Counting bytes for float-precision averages? Track numerator (Counter) and denominator (Counter) separately and divide at read time.

## Exceptions

| Exception               | Raised by                                                    |
|-------------------------|--------------------------------------------------------------|
| `StaleHandleException`  | Any method on a handle whose registry entry was evicted.     |
| `UninitializedException`| `id()` on a wrapper that has not finished `__construct`.     |

Counters never throw on overflow or extreme values — they wrap.

## Observability

See [Shared Observability](shared-observability.md) for the full tour. Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ value, type: "Counter" }`.
- Prometheus `oxphp_shared_counter_value{counter_id="…"}` gauge tracks the current value.
- Registry-wide counters (`oxphp_shared_operations_total`, `oxphp_shared_objects_total`) cover Counter via the `type="Counter"` label.

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
