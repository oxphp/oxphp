---
title: Shared\Mutex
description: Poisoning mutex over a stored value — atomic multi-step updates across PHP workers with RAII-style critical sections and closure-based access.
---

# Shared\Mutex

`OxPHP\Shared\Mutex` is a process-wide mutual-exclusion lock that wraps a stored scalar value. You never touch the lock directly — you hand a closure to `with()` (blocking) or `tryWith()` (non-blocking), and the runtime holds the lock for the closure's duration, releasing even if the closure throws.

## Overview

- **Guards a value, not just a section.** The wrapped value is passed into your closure by reference, and the new value it returns is committed back.
- **Poisons on closure failure.** If the closure throws, the mutex is marked poisoned; subsequent `with()` calls fail with `PoisonedException` until you explicitly call `clearPoison()`. This prevents other workers from operating on state that was mid-update when an error occurred.
- **Deadlock-avoidant.** Re-entering the same mutex on the same thread (including via nested async calls captured on this thread) raises `DeadlockException` instead of hanging.
- **Timed acquire.** `with($fn, $timeout)` waits up to `$timeout` seconds; zero means wait indefinitely. `tryWith` is instantaneous.

## API Reference

```php
namespace OxPHP\Shared;

final class Mutex implements Shareable
{
    public function __construct(mixed $initial = null, ?float $defaultTimeout = null);

    public function isPoisoned(): bool;
    public function clearPoison(): void;

    public function with(callable $fn, float $timeout = 0.0): mixed;
    public function tryWith(callable $fn): mixed;

    public function id(): int;
}
```

The closure signature is `function (mixed $value): mixed` — it receives the current stored value by value and returns the new value to store. If the closure returns `null`, the stored value becomes `null`.

| Method        | Return                   | Use case                                                      |
|---------------|--------------------------|---------------------------------------------------------------|
| `with`        | closure return           | Atomic RMW on the stored value; blocks up to `$timeout`.      |
| `tryWith`     | closure return or `null` | Same, but returns `null` immediately if the lock is held.     |
| `isPoisoned`  | bool                     | Probe whether the mutex is in the poisoned state.             |
| `clearPoison` | void                     | Reset to a usable state after handling the prior failure.     |

## Examples

### Atomic multi-field update

A Counter is enough when the value is a single integer. A Mutex wins when several fields must update in lockstep:

```php
<?php
$stats = new OxPHP\Shared\Mutex(['hits' => 0, 'bytes' => 0]);

$stats->with(function (array $s) use ($responseBytes) {
    $s['hits']  += 1;
    $s['bytes'] += $responseBytes;
    return $s;                     // committed back atomically
});
```

Another worker reading `$stats->with(fn ($s) => $s)` either sees both fields updated or neither — never the bumped `hits` without the matching `bytes`.

### Non-blocking probe + degrade

```php
<?php
$budget = new OxPHP\Shared\Mutex(['tokens' => 100, 'refill_at' => time()]);

$allowed = $budget->tryWith(function (array $b) {
    if ($b['tokens'] <= 0) return $b;     // no op, just inspect
    $b['tokens'] -= 1;
    return $b;
});

if ($allowed === null) {
    // Lock held by another worker — shed the request instead of queuing.
    http_response_code(503);
    return;
}
```

### Timed acquire

```php
<?php
$cache = new OxPHP\Shared\Mutex(null, defaultTimeout: 2.0);

try {
    $result = $cache->with(function ($c) { /* ... */ return $c; }, timeout: 5.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    // Someone else held the lock longer than 5s.
}
```

### Recovering a poisoned mutex

```php
<?php
try {
    $state->with(function ($s) {
        doRiskyThing($s);          // throws
        return $s;
    });
} catch (Throwable $e) {
    // Other workers now get PoisonedException from $state->with(...).
    if ($state->isPoisoned()) {
        reinitialiseFromPersistentStore($state);
        $state->clearPoison();
    }
    throw $e;
}
```

## Semantics & gotchas

- **The closure runs with the lock held.** Keep it short. Do not call `sleep`, do not block on network I/O, do not re-enter other Shared\* types that could call back into this mutex.
- **Poison is strict by default.** Any exception inside the closure poisons the mutex, even if the stored value was not touched. If you need a non-poisoning try-compute pattern, do it outside the mutex and call `with` only to commit.
- **`$defaultTimeout` applies when you pass `0.0` to `with`.** Pass an explicit `timeout:` named argument to override per call.
- **Stored value is scalar-only in v1.** Strings, ints, floats, booleans, and nested arrays of those work; objects, closures, and resources raise `TypeException`.
- **Reentry on the same thread throws.** Use a different mutex or restructure the code — re-entering is a bug, not a feature.

## Exceptions

| Exception                | Raised by                                                           |
|--------------------------|---------------------------------------------------------------------|
| `PoisonedException`      | `with` / `tryWith` on a poisoned mutex (after a prior throw).       |
| `TimeoutException`       | `with` exceeded `$timeout` (or `defaultTimeout`) without acquiring. |
| `DeadlockException`      | Reentrant `with`/`tryWith` on the same mutex on the same thread.    |
| `TypeException`          | Constructor or closure-returned value is not serialisable.          |
| `StaleHandleException`   | Method call on a handle whose registry entry was evicted.           |
| `UninitializedException` | `id()` on a wrapper that has not finished `__construct`.            |

`tryWith` deliberately returns `null` on contention rather than throwing — contention is not an error.

## Observability

See [Shared Observability](../operations/shared-observability.md). Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ type: "Mutex", poisoned, waiters, last_acquire_ms, held_by_thread }`.
- Prometheus metrics per instance:
  - `oxphp_shared_mutex_waiters{mutex_id="…"}` — current waiter count.
  - `oxphp_shared_mutex_acquires_total{mutex_id="…"}` — lifetime acquires.
  - `oxphp_shared_mutex_contended_total{mutex_id="…"}` — acquires that had to wait.
  - `oxphp_shared_mutex_poisoned{mutex_id="…"}` — 0 / 1.

## When not to use

- **Single atomic value.** If the guarded value is one int or one bool, use `Shared\Counter` or `Shared\Flag` — both are lock-free and cheaper.
- **Long-running work.** Do not hold a mutex across I/O, `sleep`, or fiber awaits. Use a `Shared\Channel` producer/consumer pattern instead.
- **High-contention hot path.** If every request must take the same mutex, you have serialised your throughput. Partition the state (e.g. `Shared\Map<tenant_id, Mutex>`) or pre-aggregate in per-worker locals and flush periodically.
- **Cross-host mutual exclusion.** In-process only. Use a distributed lock (Redis `SET NX`, etcd) for multi-host coordination.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Counter](shared-counter.md) — when the guarded state is one integer.
- [Shared\Flag](shared-flag.md) — when the guarded state is one bool.
- [Shared\Channel](shared-channel.md) — when you need waiting + handoff rather than mutual exclusion.
- [Shared\Map](shared-map.md) — partition a Mutex per key to avoid global contention.
