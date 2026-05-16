---
title: Shared\Flag
description: Atomic boolean shared across PHP workers — kill-switches, circuit breakers, and one-shot initialisation markers with lock-free set/clear/isSet.
---

# Shared\Flag

`OxPHP\Shared\Flag` is a process-wide atomic boolean. Every operation is lock-free; two workers flipping the flag concurrently cannot observe an intermediate state.

## Overview

- **Atomic bool.** Single bit of state with `isSet` / `set` / `clear` / `exchange` / `compareAndSet`.
- **Lock-free.** All mutations are a single CPU atomic. Safe under contention.
- **Shareable.** Instances live in the registry and can be stored inside `Shared\Map`, passed through `use` captures, etc.

## API Reference

```php
namespace OxPHP\Shared;

final class Flag implements Shareable
{
    public function __construct(bool $initial = false);

    public function isSet(): bool;
    public function set(): bool;                                 // returns previous
    public function clear(): bool;                               // returns previous
    public function exchange(bool $new): bool;                   // returns previous
    public function compareAndSet(bool $expect, bool $new): bool;

    public function id(): int;
}
```

| Method          | Returns  | Use case                                                         |
|-----------------|----------|------------------------------------------------------------------|
| `isSet`         | current  | Pure read.                                                       |
| `set`           | previous | Turn on unconditionally. Previous value tells you if you won.    |
| `clear`         | previous | Turn off unconditionally.                                        |
| `exchange`      | previous | Swap to an explicit value; useful when toggling is conditional.  |
| `compareAndSet` | swapped? | One-shot initialisation: succeed only if the flag was the expected value. |

## Examples

### Kill-switch

```php
<?php
$maintenance = new OxPHP\Shared\Flag();

// In a request handler
if ($maintenance->isSet()) {
    http_response_code(503);
    header('Retry-After: 60');
    echo 'under maintenance';
    return;
}

// In an admin endpoint
$maintenance->set();     // enable
$maintenance->clear();   // disable
```

### One-shot initialisation winner

```php
<?php
$migrated = new OxPHP\Shared\Flag();

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
$tripped = new OxPHP\Shared\Flag();

try {
    callDownstream();
} catch (DownstreamFailedException $e) {
    $wasAlreadyTripped = $tripped->set();
    if (!$wasAlreadyTripped) {
        alertOncall($e);        // fire alert only on first trip
    }
    throw $e;
}
```

For a complete circuit breaker you will usually want a `Shared\Counter` for the failure window and a `Shared\Flag` for the tripped state — reset the flag via `clear()` once the window cools down.

## Semantics & gotchas

- **`set` / `clear` / `exchange` return the *previous* value.** That is deliberately the most useful return: "did I change anything?" is `$prev !== $new`.
- **`compareAndSet` is how you express "first one wins".** Plain `set()` always succeeds, so it cannot express "don't overwrite if already set".
- **No waiting.** A Flag does not block. If you need to wait for a transition, pair it with a `Shared\Channel` or use `Shared\Once`.

## Exceptions

| Exception                | Raised by                                                    |
|--------------------------|--------------------------------------------------------------|
| `StaleHandleException`   | Any method on a handle whose registry entry was evicted.     |
| `UninitializedException` | `id()` on a wrapper that has not finished `__construct`.     |

## Observability

See [Shared Observability](../operations/shared-observability.md). Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ value: true|false, type: "Flag" }`.
- Prometheus `oxphp_shared_flag_value{flag_id="…"}` gauge (0 or 1).
- Registry-wide metrics cover Flag via the `type="Flag"` label.

## When not to use

- **Multi-state logic.** A Flag is two-valued. If you need idle/busy/done or any three-state machine, reach for `Shared\Counter` (use integer enum values) or `Shared\Mutex` over an enum-like array.
- **Waiting for a transition.** Flags do not block. Pair with a `Shared\Channel` (or a `Shared\Counter` you `compareAndSet`-poll) when a worker should wait until the flag flips.
- **Counting events.** A Flag is not a counter. Use `Shared\Counter` for tallies.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Counter](shared-counter.md) — when you need more than on/off.
- [Shared\Once](shared-once.md) — when the value computed once is richer than a bool.
- [Shared\Mutex](shared-mutex.md) — when a flag flip must co-commit with other state.
