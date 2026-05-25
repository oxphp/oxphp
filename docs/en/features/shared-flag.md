---
title: Shared\Flag
description: Atomic boolean shared across PHP workers — kill-switches, circuit breakers, and one-shot markers with lock-free load/store/swap/compareAndSet and explicit memory ordering.
---

# Shared\Flag

`OxPHP\Shared\Flag` is a process-wide atomic boolean — the bool twin of [`Shared\Atomic`](shared-atomic.md). Every operation is lock-free; two workers flipping the flag concurrently cannot observe an intermediate state.

## Overview

- **Atomic bool.** Single bit of state with `load` / `store` / `swap` / `compareAndSet`.
- **Explicit memory ordering.** Every operation takes an optional [`Ordering`](shared-atomic.md), defaulting to `SeqCst`, exactly like `Shared\Atomic`.
- **Lock-free.** All mutations are a single CPU atomic. Safe under contention.
- **Shareable.** Instances live in the registry and can be stored inside `Shared\Map`, passed through `use` captures, etc.

## API Reference

```php
namespace OxPHP\Shared;

final class Flag implements Shareable
{
    public function __construct(bool $initial = false);

    public function load(Ordering $order = Ordering::SeqCst): bool;             // Relaxed | Acquire | SeqCst
    public function store(bool $value, Ordering $order = Ordering::SeqCst): void; // Relaxed | Release | SeqCst
    public function swap(bool $value, Ordering $order = Ordering::SeqCst): bool;   // any ordering; returns previous
    public function compareAndSet(
        bool $expect,
        bool $new,
        Ordering $success = Ordering::SeqCst,
        Ordering $failure = Ordering::SeqCst,                                  // Relaxed | Acquire | SeqCst
    ): bool;

    public function id(): int;
}
```

| Method          | Returns  | Use case                                                         |
|-----------------|----------|------------------------------------------------------------------|
| `load`          | current  | Pure read.                                                       |
| `store`         | void     | Set to an explicit value unconditionally.                        |
| `swap`          | previous | Set to an explicit value; the return tells you whether you changed it. `swap(true)` is test-and-set ("did I win?"). |
| `compareAndSet` | swapped? | One-shot initialisation: succeed only if the flag was the expected value. |

## Examples

### Kill-switch

```php
<?php
use OxPHP\Shared\Flag;

$maintenance = new Flag();

// In a request handler
if ($maintenance->load()) {
    http_response_code(503);
    header('Retry-After: 60');
    echo 'under maintenance';
    return;
}

// In an admin endpoint
$maintenance->store(true);   // enable
$maintenance->store(false);  // disable
```

### One-shot initialisation winner

```php
<?php
use OxPHP\Shared\Flag;

$migrated = new Flag();

if ($migrated->compareAndSet(expect: false, new: true)) {
    // First worker to arrive wins — run the migration once.
    runSchemaMigration();
} else {
    // Someone else already ran it.
}
```

### Circuit breaker trip

```php
<?php
use OxPHP\Shared\Flag;

$tripped = new Flag();

try {
    callDownstream();
} catch (DownstreamFailedException $e) {
    $wasAlreadyTripped = $tripped->swap(true);   // set true, learn the prior state
    if (!$wasAlreadyTripped) {
        alertOncall($e);        // fire alert only on first trip
    }
    throw $e;
}
```

For a complete circuit breaker you will usually want a `Shared\Counter` for the failure window and a `Shared\Flag` for the tripped state — reset the flag via `store(false)` once the window cools down.

### Publish a payload, then signal with cheaper ordering

```php
<?php
use OxPHP\Shared\Flag;
use OxPHP\Shared\Map;
use OxPHP\Shared\Ordering;

$ready = new Flag();
$config = new Map();

// Producer: write the payload, then publish with Release.
$config->set('dsn', $dsn);
$ready->store(true, Ordering::Release);

// Consumer: an Acquire load that observes `true` also observes the payload.
if ($ready->load(Ordering::Acquire)) {
    $dsn = $config->get('dsn');
}
```

## Semantics & gotchas

- **`swap` returns the *previous* value.** That is the most useful return: "did I change anything?" is `$prev !== $new`. `swap(true)` is the canonical test-and-set.
- **`store` returns `void`.** If you need the prior value, use `swap`.
- **`compareAndSet` is how you express "first one wins".** Plain `store(true)` always succeeds, so it cannot express "don't overwrite if already set".
- **Memory ordering matches `Shared\Atomic`.** `load` rejects `Release`/`AcqRel`, `store` rejects `Acquire`/`AcqRel`, and `compareAndSet`'s `$failure` rejects `Release`/`AcqRel` — each raises `InvalidOrderingException`. The default `SeqCst` is always safe.
- **No waiting.** A Flag does not block. If you need to wait for a transition, pair it with a `Shared\Channel` or use `Shared\Once`.

## Exceptions

| Exception                  | Raised by                                                    |
|----------------------------|--------------------------------------------------------------|
| `StaleHandleException`     | Any method on a handle whose registry entry was evicted.     |
| `UninitializedException`   | `id()` on a wrapper that has not finished `__construct`.     |
| `InvalidOrderingException` | An `Ordering` not allowed for the operation (see above).     |

## Observability

See [Shared Observability](../operations/shared-observability.md). Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ value: true|false, type: "Flag" }`.
- Prometheus `oxphp_shared_flag_value{flag_id="…"}` gauge (0 or 1).
- Registry-wide metrics cover Flag via the `type="Flag"` label.

## When not to use

- **Multi-state logic.** A Flag is two-valued. If you need idle/busy/done or any three-state machine, reach for `Shared\Counter` (use integer enum values) or `Shared\Mutex` over an enum-like array.
- **Waiting for a transition.** Flags do not block. Pair with a `Shared\Channel` (or a `Shared\Counter` you `compareAndSet`-poll) when a worker should wait until the flag flips.
- **Counting events.** A Flag is not a counter. Use `Shared\Counter` for tallies.
- **Integer state.** If the switch is really a small integer, use [`Shared\Atomic`](shared-atomic.md) directly.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Atomic](shared-atomic.md) — the int64 twin, same ordering model.
- [Shared\Counter](shared-counter.md) — when you need more than on/off.
- [Shared\Once](shared-once.md) — when the value computed once is richer than a bool.
- [Shared\Mutex](shared-mutex.md) — when a flag flip must co-commit with other state.
