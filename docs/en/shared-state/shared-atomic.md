---
title: Shared\Atomic
description: Generic atomic int64 shared across PHP workers — load/store, swap, CAS, fetch-arithmetic and fetch-bitwise with explicit memory-ordering control.
---

# Shared\Atomic

`OxPHP\Shared\Atomic` is a process-wide atomic 64-bit signed integer with the full primitive surface — `load`, `store`, `swap`, `compareAndSet`, plus `fetchAdd`/`Sub`/`And`/`Or`/`Xor`. Every operation is lock-free; memory ordering is explicit and defaults to `SeqCst`.

## Overview

- **Atomic int64 primitive.** Range `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`. Overflow wraps.
- **Lock-free.** Each operation compiles to a single CPU atomic instruction (`load`, `store`, `xchg`, `cmpxchg`, `xadd`, etc.).
- **Memory ordering is yours to pick.** Pass an `OxPHP\Shared\Ordering` enum value when you need `Relaxed` / `Acquire` / `Release` / `AcqRel` / `SeqCst`. Defaults are `SeqCst`, so callers who don't care get the strongest guarantee.

When to use Atomic instead of `Shared\Counter`:

- **State machines** — `compareAndSet` for `idle → busy → done`.
- **Version stamps / generation counters** — `fetchAdd(1)` returns the previous version; readers can use it to detect races.
- **CAS loops** — read with `load`, compute new, retry `compareAndSet` until it succeeds.
- **Bitflag masks** — `fetchOr` to set, `fetchAnd` to clear.

`Counter` is the right tool for accumulation (`add`); `Atomic` is the right tool for arbitrary atomic state.

## API Reference

```php
namespace OxPHP\Shared;

final class Atomic implements Shareable
{
    public function __construct(int $initial = 0);

    public function load(Ordering $order = Ordering::SeqCst): int;
    public function store(int $value, Ordering $order = Ordering::SeqCst): void;
    public function swap(int $value, Ordering $order = Ordering::SeqCst): int;            // returns prev

    public function compareAndSet(
        int $expect,
        int $new,
        Ordering $success = Ordering::SeqCst,
        Ordering $failure = Ordering::SeqCst,
    ): bool;

    public function fetchAdd(int $delta, Ordering $order = Ordering::SeqCst): int;        // returns prev
    public function fetchSub(int $delta, Ordering $order = Ordering::SeqCst): int;        // returns prev
    public function fetchAnd(int $mask,  Ordering $order = Ordering::SeqCst): int;        // returns prev
    public function fetchOr (int $mask,  Ordering $order = Ordering::SeqCst): int;        // returns prev
    public function fetchXor(int $mask,  Ordering $order = Ordering::SeqCst): int;        // returns prev

    public function id(): int;
}
```

| Method            | Returns       | Use case                                                      |
|-------------------|---------------|---------------------------------------------------------------|
| `load`            | current       | Read the value with chosen ordering.                          |
| `store`           | void          | Write a new value, discarding the old.                        |
| `swap`            | previous      | Atomic replace; `swap(0)` is the snapshot-and-zero pattern.   |
| `compareAndSet`   | swapped?      | Optimistic transitions and CAS loops.                         |
| `fetchAdd`/`Sub`  | previous      | Generation counters, bounded counters via CAS, deltas.        |
| `fetchAnd`/`Or`/`Xor` | previous  | Bitflag masks: set, clear, toggle.                            |
| `id`              | registry id   | Logging, tracing, `/__ox_shared/entry?id=…` correlation.      |

## Memory ordering

Short primer:

- **Relaxed** — atomicity only, no ordering relative to other memory accesses.
- **Acquire** (loads) — pairs with a `Release` store; reads after this op observe writes the releaser had completed.
- **Release** (stores) — pairs with an `Acquire` load; writes before this op are visible to acquirers.
- **AcqRel** (read-modify-write) — both halves of an Acquire load and a Release store.
- **SeqCst** — single global total order across all `SeqCst` operations.

Each operation accepts only orderings that are meaningful for it:

| Operation | Allowed |
|---|---|
| `load` | `Relaxed`, `Acquire`, `SeqCst` |
| `store` | `Relaxed`, `Release`, `SeqCst` |
| `swap`, `fetchAdd`, `fetchSub`, `fetchAnd`, `fetchOr`, `fetchXor` | any |
| `compareAndSet` `success` | any |
| `compareAndSet` `failure` | `Relaxed`, `Acquire`, `SeqCst` |

Defaults are `Ordering::SeqCst` everywhere, so callers who don't think about ordering still get safe behaviour. An invalid combination throws `OxPHP\Shared\InvalidOrderingException` before the FFI call.

For the deep dive on the C++/Rust memory model, see the [Rust `std::sync::atomic::Ordering` docs](https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html).

## Examples

### State machine via compareAndSet

```php
<?php
use OxPHP\Shared\Atomic;

$state = new Atomic(initial: 0); // 0=idle, 1=busy, 2=done

if (!$state->compareAndSet(expect: 0, new: 1)) {
    throw new RuntimeException('another worker is already processing');
}

try {
    doWork();
    $state->store(2);
} catch (Throwable $e) {
    $state->store(0); // release back to idle on error
    throw $e;
}
```

### Generation counter / version stamp

```php
<?php
$version = new OxPHP\Shared\Atomic();

// Each writer bumps the version and gets the value it just superseded.
$prev = $version->fetchAdd(1);
publishUpdate($prev + 1, $payload);
```

### Optimistic update via CAS loop

```php
<?php
use OxPHP\Shared\Atomic;
use OxPHP\Shared\Ordering;

$cell = new Atomic(initial: 100);

// Saturate-add: never go above 1000.
do {
    $cur = $cell->load(Ordering::Acquire);
    $next = min($cur + 7, 1000);
    if ($cur === $next) {
        break; // already at cap
    }
} while (!$cell->compareAndSet($cur, $next, Ordering::AcqRel, Ordering::Acquire));
```

### Bitflag mask

```php
<?php
const FLAG_READY    = 1 << 0;
const FLAG_DRAINING = 1 << 1;
const FLAG_FAILED   = 1 << 2;

$flags = new OxPHP\Shared\Atomic();

$flags->fetchOr(FLAG_READY);                  // set bit
$flags->fetchAnd(~FLAG_DRAINING);             // clear bit
$snapshot = $flags->load();
if ($snapshot & FLAG_FAILED) {
    raiseAlert();
}
```

## Semantics & gotchas

- **`fetchAdd` returns the previous value, not the new one.** This deliberately contrasts with `Counter::add`, which returns the new total. Different abstraction, different return convention — pick the class that matches the semantics you mean.
- **Overflow wraps.** `i64::MIN.fetchSub(1)` yields `i64::MAX`. No exception is raised.
- **Default ordering is `SeqCst`.** It is the safest choice and the slowest. Drop to `Acquire`/`Release`/`Relaxed` only when you can articulate why.
- **Single int64 only.** For compound state (multiple coupled fields), use `Shared\Mutex`.

## Exceptions

| Exception                    | Raised by                                                              |
|------------------------------|------------------------------------------------------------------------|
| `StaleHandleException`       | Any method on a handle whose registry entry was evicted.               |
| `UninitializedException`     | `id()` on a wrapper that has not finished `__construct`.               |
| `InvalidOrderingException`   | An operation receives a memory ordering invalid for it.                |

## Observability

See [Shared Observability](shared-observability.md) for the full tour. Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ value, type: "Atomic" }`.
- Registry-wide counters (`oxphp_shared_operations_total`, `oxphp_shared_objects_total`) cover Atomic via the `type="Atomic"` label.

## When not to use

- **Compound state.** Multiple fields that must update together → `Shared\Mutex`.
- **Counting / accumulation.** Use `Shared\Counter` — its `add` returning the new total matches the domain.
- **Floats or decimals.** Not supported; wrap a struct in `Shared\Mutex`, or pair two Counters (numerator / denominator).
- **Cross-host coordination.** Atomic is in-process only. For multi-host state use Redis, a database, or a metric pipeline.
- **Durability.** Atomic state evaporates at server stop. Persist snapshots elsewhere if the value must survive restarts.

## Related

- [Shared State](shared-state.md) — overview and migration patterns.
- [Shared\Counter](shared-counter.md) — when the value is a domain accumulator.
- [Shared\Mutex](shared-mutex.md) — when state spans more than one int64.
- [Shared\Flag](shared-flag.md) — when the value is just on/off.
