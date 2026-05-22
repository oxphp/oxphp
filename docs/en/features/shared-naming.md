---
title: Shared\* Naming Conventions
description: Naming conventions for the OxPHP\Shared\* concurrency API — the canonical method vocabulary (get/set, try*/timeout wait policies, is*, fetch*, compareAndSet) that every shared primitive follows.
---

# `OxPHP\Shared\*` Naming Conventions

The `OxPHP\Shared\*` namespace is the application-level concurrency API:
`Atomic`, `Counter`, `Flag`, `Map`, `Channel`, `Mutex`, `Once`, `Pool`.
Method names follow a single set of rules so users can predict the API
without consulting the docs for every type.

This document is the canonical reference. New primitives — and changes
to existing ones — MUST follow it.

## Rules

### 1. Read a value — `get()`

PHP convention. Used by `Map::get()`, `Counter::get()`, `Once::get()`.

`Atomic::load(?Ordering $order = null)` is the deliberate exception:
its existence carries the ordering argument, signalling that the read
is part of a memory-model contract distinct from a plain getter.

### 2. Write a value — `set()`, `store()` for atomics

`Map::set()`, `Mutex` value reset (via `with`), `Once::getOrInit()`.
`Atomic::store($value, ?Ordering)` mirrors `load` for the same reason.

### 3. Number of elements — `count(): int` + `\Countable`

Every container exposes `count(): int` and implements `\Countable`.
This lets `count($obj)` work natively:

```php
$ch  = new OxPHP\Shared\Channel(1024);
$map = new OxPHP\Shared\Map();
$pool = new OxPHP\Shared\Pool($factory);

count($ch);    // queued items
count($map);   // entries
count($pool);  // total live slots (in-use + idle)
```

No `size()`, `len()`, or `pending()` — these are forbidden on the
public surface, regardless of which language the implementer's
muscle memory comes from.

### 4. Boolean-getter — `is*()` prefix

`Channel::isClosed()`, `Flag::isSet()`.

No bare verbs (`test`, `check`) and no domain-specific names
(`closed`). The `is` prefix marks a pure read of a boolean property.

A type whose state is richer than a single boolean exposes it as a
`status()` method returning an enum instead of an `is*()` getter —
`Channel`'s `RecvResult::status()` and `Once::status(): Once\Status`
(Uninitialized/Pending/Ready/Poisoned) follow this. Reach for `status()`
when the answer has more than two cases.

`Mutex` does **not** expose `isCorrupted()` — corruption is sticky,
non-recoverable, and surfaced via `CorruptedMutexException` on the
next acquire. There's nothing useful to do with the probe other than
re-acquire and catch.

### 5. Wait-policy trichotomy — `try*` / bare / `*Timeout`

Blocking primitives (Channel, Mutex) express the **wait policy**
through the method name, not through an overloaded `?float $timeout`
argument:

| Suffix       | Behaviour                                                     | Examples                                                  |
|--------------|---------------------------------------------------------------|-----------------------------------------------------------|
| `try*`       | Non-blocking; reports the failure variant immediately.        | `Channel::trySend`, `Channel::tryRecv`, `Mutex::tryWithLock` |
| (bare name)  | Block forever (or until the request fiber is cancelled).      | `Channel::send`, `Channel::recv`, `Mutex::withLock`       |
| `*Timeout`   | Bounded wait. Takes a mandatory `int $ms > 0`.                | `Channel::sendTimeout`, `Channel::recvTimeout`, `Mutex::withLockTimeout` |

The trichotomy moves three ambiguous policies (`null` = forever, `0`
= try, positive = bounded) out of one parameter and into three
methods with self-documenting names. The `$ms` argument on `*Timeout`
methods is **strictly positive** — zero, negative, non-int, and
absent values raise `OxPHP\Shared\TypeException` at the bridge.

`try*` shares one further sub-meaning that predates the trichotomy:

- **Conditional-success op.** `Map::trySet` succeeds only when the
  key was absent; collision → `false`, no exception. Parallel to
  `HashMap::try_insert`.

The unifying invariant for `try*`: it either returns a value-typed
Result (Channel) or throws a `ContentionException` (Mutex). It never
returns `null` to encode "did not succeed" — that was the old
API and produced the null-coalescing ambiguity the trichotomy
eliminates.

### 6. Compare-and-swap — `compareAndSet()`

`Atomic::compareAndSet()`, `Flag::compareAndSet()`. Always returns
`bool` (the swap happened, or it didn't).

### 7. Replace and return previous — `swap()`, `exchange()`

`Atomic::swap()` for ints, `Flag::exchange()` for bools.

Naming asymmetry is historical and intentional: `swap` reads as
"replace contents of two locations" in low-level contexts; `exchange`
is more common in PHP for "swap with new". Both return the previous
value.

### 8. Atomic RMW returning previous — `fetch*()` prefix

`Atomic::fetchAdd()`, `fetchSub()`, `fetchAnd()`, `fetchOr()`,
`fetchXor()`.

The `fetch` prefix encodes the return contract: **the value before
the operation**. This contrasts with `Counter::add()` /
`Counter::inc()` / `Counter::dec()`, which return the **new** value
(LongAdder-style aggregate counter).

When adding new RMW methods, pick the contract first, then the name:

- prev-value return → `fetchVerb(args)`
- new-value return → bare `verb(args)`

Do not mix.

### 9. Reset to default — `clear()`

`Map::clear()`, `Flag::clear()` (in the sense of "set to false").
Returns `void` for plain reset; returns the previous value when the
caller can reasonably want it (`Flag::clear()`, `Counter::reset()`).

`Counter::reset()` is the documented exception that keeps `reset`:
the LongAdder convention is `sumThenReset`, and renaming would
mislead users coming from Java's `LongAdder` / Go's
`atomic.Int64.Swap(0)`.

### 10. Registry identity — `id(): int`

Every `Shared\*` instance exposes `id(): int` for logs and the
`/__ox_shared/entries/:id` observability endpoint.

## Cheat sheet

| Concept                     | Canonical name           | Examples                                |
| --------------------------- | ------------------------ | --------------------------------------- |
| Read a value                | `get()`                  | `Map::get`, `Counter::get`              |
| Read an atomic              | `load($order)`           | `Atomic::load`                          |
| Write a value               | `set()`                  | `Map::set`                              |
| Write an atomic             | `store($v, $order)`      | `Atomic::store`                         |
| Number of elements          | `count(): int`           | `Map::count`, `Channel::count`, `Pool::count` |
| Has key / has element       | `has($key): bool`        | `Map::has`                              |
| Boolean property            | `is*(): bool`            | `Flag::isSet`, `Channel::isClosed`      |
| Non-blocking wait           | `try*()`                 | `Channel::trySend`, `Mutex::tryWithLock`, `Map::trySet` |
| Forever wait                | bare verb                | `Channel::send`, `Channel::recv`, `Mutex::withLock`     |
| Bounded wait                | `*Timeout(int $ms)`      | `Channel::sendTimeout`, `Mutex::withLockTimeout`        |
| Compare-and-swap            | `compareAndSet()`        | `Atomic::compareAndSet`                 |
| Swap, return prev           | `swap()` / `exchange()`  | `Atomic::swap`, `Flag::exchange`        |
| Atomic RMW, return prev     | `fetch*()`               | `Atomic::fetchAdd`                      |
| Atomic RMW, return new      | bare verb                | `Counter::inc`, `Counter::add`          |
| Reset to default            | `clear()`                | `Map::clear`, `Flag::clear`             |
| Registry id                 | `id(): int`              | every `Shared\*` type                   |

## Adding a new `Shared\*` type

When proposing a new primitive, fill out this checklist before merging:

- [ ] Every method maps to a row in the cheat sheet, or has an ADR
  explaining the exception (see `Atomic::load/store` and
  `Counter::reset` above).
- [ ] If the type holds a collection of values, it implements
  `\Countable` and exposes `count(): int`.
- [ ] Read methods are `get` or `load` (atomic only).
- [ ] Boolean getters use the `is*` prefix.
- [ ] Wait-policy variants follow the `try*` / bare / `*Timeout(int $ms)`
  trichotomy. The `*Timeout` variant takes `int $ms > 0` and rejects
  zero / negative / non-int input with `TypeException`. Conditional-
  success ops (`Map::trySet`) keep the `try*` prefix and may still
  return `bool`; new wait-policy `try*` methods return either a value-
  typed Result or throw a domain exception — never `null`-to-encode.
- [ ] No `len`, `size`, `pending`, `test`, `setIfAbsent`, or other
  ad-hoc names.
- [ ] Domain-specific verbs (`evict`, `drain`, `flush`, etc.) appear
  only when no canonical entry in the cheat sheet covers the concept.

## Observability names lag the PHP API

The operator-facing surface — Prometheus metric names and the JSON at
`/__ox_shared/entries/:id` — is a separate contract from the PHP API.
Renaming it breaks dashboards and alert rules. To avoid silent
inconsistency, the affected names are emitted **twice** for one
release cycle:

| Surface     | Deprecated (still emitted) | Canonical            |
| ----------- | -------------------------- | -------------------- |
| Prometheus  | `oxphp_shared_channel_pending` | `oxphp_shared_channel_count` |
| Prometheus  | `oxphp_shared_pool_size`       | `oxphp_shared_pool_count`    |
| JSON entry  | `Channel.pending`              | `Channel.count`             |
| JSON entry  | `Pool.size`                    | `Pool.count`                |

The deprecated metric `# HELP` lines carry a `(deprecated, removed in
a future release; use *_count)` prefix, and the `ox_shared` plugin
emits a startup `WARN` whenever introspection or metrics are enabled.

Migrate dashboards and alert rules to the `_count` names before the
deprecation cycle closes. After removal, only the canonical names
will be emitted, and Prometheus/Grafana panels referencing the old
ones will start returning empty series.

## Stability

These rules are part of the `OxPHP\Shared\*` 1.0 contract. After the
1.0 release, renames are breaking changes and require a deprecation
cycle. Before 1.0, the rules are still binding — new methods that
violate them will be rejected in review.
