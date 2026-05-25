---
title: Shared\Mutex
description: Cross-thread mutual exclusion over a stored value — atomic multi-step updates from PHP workers via withLock / tryWithLock / withLockTimeout with explicit wait policies.
---

# Shared\Mutex

`OxPHP\Shared\Mutex` is a process-wide mutual-exclusion lock that wraps a stored value. You never touch the lock directly — you hand a closure to one of three method variants, and the runtime holds the lock for the closure's duration, releasing it even if the closure throws.

## Overview

- **Guards a value, not just a section.** The wrapped value is passed into your closure **by reference**, so direct mutation inside the closure commits back when the closure returns normally.
- **Three explicit wait policies** instead of one overloaded `?float $timeout`:
  - `withLock($fn)` — block forever (or until the request fiber is cancelled).
  - `tryWithLock($fn)` — non-blocking; throws `ContentionException` if the lock is held.
  - `withLockTimeout($fn, int $ms)` — bounded wait; throws `OperationTimeoutException` on deadline expiry.
- **PHP exceptions propagate freely.** If your closure throws an ordinary PHP exception, the lock releases and the exception bubbles up. The mutex is **not** corrupted — partial mutation is acceptable; the caller is responsible for restoring invariants.
- **Rust panics corrupt the mutex.** If a Rust panic crosses the FFI boundary (a server bug), the mutex enters a sticky corrupted state and every subsequent acquisition throws `CorruptedMutexException`. There is no recovery API — discard the instance and create a new one.
- **Deadlock-avoidant.** Re-entering the same mutex on the same thread (including via nested async calls captured on this thread) raises `DeadlockException` instead of hanging.

## API Reference

```php
namespace OxPHP\Shared;

final class Mutex implements Shareable
{
    public function __construct(mixed $initial = null);

    public function withLock(callable $fn): mixed;
    public function tryWithLock(callable $fn): mixed;
    public function withLockTimeout(callable $fn, int $ms): mixed;

    public function id(): int;
}
```

The closure signature is `function (mixed &$value): mixed` — `$value` is passed by reference, so you mutate it in place. The closure's normal return value is forwarded to the caller of `withLock` / `tryWithLock` / `withLockTimeout`, but **the return is restricted to scalars and `null`** (string, int, float, bool, byte-string, null). Returning an array or a `Shared\*` instance raises `OxPHP\Shared\TypeException`. To bubble structured state up to the caller, either mutate `&$value` in place and re-read it after the call, or stage what you need in a `use (&$captured)` variable. Lifting this restriction is tracked separately.

| Method                | Behaviour                                                       |
|-----------------------|-----------------------------------------------------------------|
| `withLock($fn)`       | Block until acquired, then run the closure. Forever / cancel.   |
| `tryWithLock($fn)`    | Non-blocking. Throws `ContentionException` if held.             |
| `withLockTimeout($fn, $ms)` | Bounded wait. `$ms > 0` required. Throws `OperationTimeoutException` on deadline. |
| `id()`                | Registry identifier; useful for logging / observability.        |

`$ms` is a strict positive integer of milliseconds. Zero, negative, non-int, and absent values raise `OxPHP\Shared\TypeException` at the bridge — call `withLock` (forever) or `tryWithLock` (non-blocking) instead of trying to express those policies through `$ms`.

## Why Mutex throws but Channel returns Results

Contention and timeout are **rare events** for a well-designed mutex (locks should be held for short critical sections; sustained contention is a smell). They are **routine events** for a channel (a fan-out dispatcher sees Full/Closed/Timeout on every busy cycle). So:

- `Mutex` uses **exception-style** — the rare path is the exceptional one.
- `Channel` uses **Result-style** — the common path stays out of the throw/catch machinery.

If you find yourself wrapping every `withLock` in `try { … } catch (ContentionException) { … }`, you are using the wrong primitive. Reach for `Shared\Channel` for queue-shaped workloads or `Shared\Counter` / `Shared\Flag` for single-value atomicity.

The same structural reason explains why `Pool::tryAcquire()` can return `null` where `Mutex::tryWithLock()` throws. `Pool` is **handle-first**: `tryAcquire(): ?Handle` carries "saturated" as `null`, and a `Handle` is never itself a user value, so there is no ambiguity. `Mutex` is **closure-only** — it deliberately never hands a lock guard back to PHP (so a held lock can't leak past the closure), which leaves no object to return as nullable, and the closure's own `mixed` result may already be `null`. With no free sentinel, contention surfaces as `ContentionException`. The two `try*` surfaces diverge because of what each type can hand back, not a style preference.

## Examples

### Atomic multi-field update

A Counter is enough when the value is a single integer. A Mutex wins when several fields must update in lockstep:

```php
<?php
$stats = new OxPHP\Shared\Mutex(['hits' => 0, 'bytes' => 0]);

$stats->withLock(function (array &$s) use ($responseBytes) {
    $s['hits']  += 1;
    $s['bytes'] += $responseBytes;
});
```

Another worker observing the value reads both fields in a single critical section:

```php
$snapshot = ['hits' => 0, 'bytes' => 0];
$stats->withLock(function (array &$s) use (&$snapshot) {
    $snapshot = $s;
});
// $snapshot sees both fields from the same update or neither — never the
// bumped 'hits' without the matching 'bytes'. (We capture through use(&$x)
// because the closure's own return is currently scalar-only — see the
// closure-signature note above.)
```

### Non-blocking probe + degrade

```php
<?php
use OxPHP\Shared\{Mutex, ContentionException};

$budget = new Mutex(['tokens' => 100, 'refill_at' => time()]);

try {
    $budget->tryWithLock(function (array &$b) {
        if ($b['tokens'] <= 0) {
            // No tokens — leave state untouched.
            return;
        }
        $b['tokens'] -= 1;
    });
} catch (ContentionException) {
    // Lock held by another worker — shed the request instead of queuing.
    http_response_code(503);
    return;
}
```

### Timed acquire

```php
<?php
use OxPHP\Shared\{Mutex, OperationTimeoutException};

$counter = new Mutex(0);

try {
    // Return value is scalar — int $next — so the closure return is forwarded.
    $next = $counter->withLockTimeout(function (int &$c) {
        $c += 1;
        return $c;
    }, ms: 5000);
} catch (OperationTimeoutException) {
    // Someone else held the lock longer than 5s.
}
```

Named arguments are encouraged — `ms: 5000` reads as "5000 milliseconds" without needing the reader to remember the parameter order.

### Catch all concurrency conditions in one place

`OperationTimeoutException`, `ContentionException`, and `DeadlockException` all extend `OxPHP\Async\AsyncException`. One catch sweeps every concurrency outcome across the Shared\* and Async\* surfaces:

```php
<?php
use OxPHP\Async\AsyncException;

try {
    $state->withLockTimeout($fn, 100);
} catch (AsyncException) {
    // timeout, contention, deadlock, or any await-related concurrency error
}
```

### Catastrophic recovery from a corrupted mutex

A Rust panic during closure invocation (a server bug, not anything the PHP code did) leaves the lock in a sticky corrupted state. There is no `clearPoison()` equivalent — discard the instance:

```php
<?php
use OxPHP\Shared\{Mutex, CorruptedMutexException};

try {
    $state->withLock($fn);
} catch (CorruptedMutexException) {
    // Old instance is dead. Recreate from the persistent source of truth.
    $state = new Mutex($initialState);
}
```

## Semantics & gotchas

- **The closure runs with the lock held.** Keep it short. Do not call `sleep`, do not block on network I/O, do not re-enter other Shared\* types that could call back into this mutex.
- **PHP throws no longer corrupt the lock.** This is a deliberate change from the previous Poisoned-on-any-throw policy: the partial mutation policy is now "caller is responsible for restoring invariants". If you need a non-mutating try-compute pattern, do it outside the mutex and call `withLock` only to commit the final value.
- **Stored value is scalar-ish.** Strings, ints, floats, booleans, `null`, and nested arrays of those work. Objects, closures, and resources raise `TypeException`.
- **Closure return is also scalar-only — *for now*.** The stored value can be an array (mutate it via `&$value`), but the closure's own *return* path supports only string/int/float/bool/null/byte-string. Returning an array or `Shared\*` instance raises `OxPHP\Shared\TypeException`. Workaround: capture into a `use (&$x)` variable, or read the state through a follow-up `withLock` that returns a scalar projection.
- **Reentry on the same thread throws `DeadlockException`.** Use a different mutex or restructure the code — same-thread re-entry is a bug, not a feature.
- **Fiber cancellation propagates as `Async\AsyncException`.** A `withLock` interrupted by request cancellation raises that exception; the lock is released cleanly.

## Exceptions

| Exception                    | Parent                          | Raised by                                                            |
|------------------------------|---------------------------------|----------------------------------------------------------------------|
| `ContentionException`        | `Async\AsyncException`          | `tryWithLock` on a held lock.                                        |
| `OperationTimeoutException`  | `Async\AsyncException`          | `withLockTimeout` deadline expired.                                  |
| `DeadlockException`          | `Async\AsyncException`          | Same-thread re-entry or detected wait-for cycle.                     |
| `CorruptedMutexException`    | `Shared\SharedException`        | A prior closure invocation crashed via Rust panic; mutex is unusable.|
| `TypeException`              | `Shared\SharedException`        | Constructor or `$ms` argument violated its type contract.            |
| `StaleHandleException`       | `Shared\SharedException`        | Method call on a handle whose registry entry was evicted.            |
| `UninitializedException`     | `Shared\SharedException`        | `id()` on a wrapper that has not finished `__construct`.             |

## Observability

See [Shared Observability](shared-observability.md). Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ type: "Mutex", corrupted, waiters, last_acquire_ms, held_by_thread }`.
- Prometheus metrics per instance:
  - `oxphp_shared_mutex_waiters{mutex_id="…"}` — current waiter count.
  - `oxphp_shared_mutex_acquires_total{mutex_id="…"}` — lifetime acquires.
  - `oxphp_shared_mutex_contended_total{mutex_id="…"}` — acquires that had to wait.
  - `oxphp_shared_mutex_corrupted{mutex_id="…"}` — 0 / 1 (renamed from `_poisoned`).

## When not to use

- **Single atomic value.** If the guarded value is one int or one bool, use `Shared\Counter` or `Shared\Flag` — both are lock-free and cheaper.
- **Long-running work.** Do not hold a mutex across I/O, `sleep`, or fiber awaits. Use a `Shared\Channel` producer/consumer pattern instead.
- **High-contention hot path.** If every request must take the same mutex, you have serialised your throughput. Partition the state (e.g. `Shared\Map<tenant_id, Mutex>`) or pre-aggregate in per-worker locals and flush periodically.
- **Cross-host mutual exclusion.** In-process only. Use a distributed lock (Redis `SET NX`, etcd) for multi-host coordination.

## Migrating from the previous API

| Was                                                 | Is                                                                            |
|-----------------------------------------------------|-------------------------------------------------------------------------------|
| `$m->with($fn)` (forever)                           | `$m->withLock($fn)`                                                            |
| `$m->with($fn, $secs)`                              | `$m->withLockTimeout($fn, $ms)` with `$ms` in milliseconds                     |
| `$m->tryWith($fn)` → `null` on contention           | `$m->tryWithLock($fn)` → throws `ContentionException`                          |
| `$m->isPoisoned()` / `$m->clearPoison()`            | removed; PHP throws no longer corrupt the mutex                                |
| `PoisonedException` (Rust panic path)               | `CorruptedMutexException` (no public clear API)                                |
| `Shared\TimeoutException`                           | `Shared\OperationTimeoutException` (now extends `Async\AsyncException`)        |
| `DeadlockException extends Shared\TimeoutException` | `DeadlockException extends Async\AsyncException`                               |

The closure signature also changed from `function (mixed $value): mixed` (return-to-commit) to `function (mixed &$value): mixed` (by-ref mutation, normal return is the closure's value, not the new state). If the closure returns nothing, the stored value keeps whatever the by-ref mutation left in it. **One pre-existing limitation carries forward**: the closure's *return* value must be scalar (string / int / float / bool / null / byte-string) — returning an array or `Shared\*` instance throws `OxPHP\Shared\TypeException`. The stored value can still be an array; mutate it through `&$value` and use `use (&$x)` to bubble structured data up.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Counter](shared-counter.md) — when the guarded state is one integer.
- [Shared\Flag](shared-flag.md) — when the guarded state is one bool.
- [Shared\Channel](shared-channel.md) — when you need waiting + handoff rather than mutual exclusion (and want Result-style returns instead of exception-style).
- [Shared\Map](shared-map.md) — partition a Mutex per key to avoid global contention.
