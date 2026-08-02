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

### 3. Number of elements — `count(): int`

Every container that exposes its current size does so under the name
`count(): int`. `Channel` additionally implements `\Countable`, so
`count($ch)` works as a native idiom for queued items. `Map` and
`Pool` expose `count(): int` as a method but do not implement
`\Countable` — call it directly:

```php
$ch  = new OxPHP\Shared\Channel(1024);
$map = new OxPHP\Shared\Map();
$pool = new OxPHP\Shared\Pool($factory);

count($ch);       // queued items (Channel implements \Countable)
$map->count();    // entries
$pool->count();   // total live slots (in-use + idle)
```

No `size()`, `len()`, or `pending()` — these are forbidden on the
public surface, regardless of which language the implementer's
muscle memory comes from.

### 4. Boolean-getter — `is*()` prefix

`Channel::isClosed()`.

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

Conditional-success ops live on `Map` under the `setIfAbsent`
spelling rather than `try*`: `Map::setIfAbsent` commits only when the
key was absent and returns `bool` (parallel to `HashMap::try_insert`).
The `setIfAbsent` name is reserved for that single semantics; do not
reuse it elsewhere.

The unifying invariant for `try*`: it either returns a value-typed
Result (Channel) or throws a `ContentionException` (Mutex). It never
returns `null` to encode "did not succeed" — that was the old
API and produced the null-coalescing ambiguity the trichotomy
eliminates.

### 6. Compare-and-swap — `compareAndSet()`

`Atomic::compareAndSet()`, `Flag::compareAndSet()`. Always returns
`bool` (the swap happened, or it didn't).

### 7. Replace and return previous — `swap()`

`Atomic::swap()` for ints, `Flag::swap()` for bools. Returns the
previous value.

### 8. Atomic RMW returning previous — `fetch*()` prefix

`Atomic::fetchAdd()`, `fetchSub()`, `fetchAnd()`, `fetchOr()`,
`fetchXor()`.

The `fetch` prefix encodes the return contract: **the value before
the operation**. This contrasts with `Counter::add()`, which returns the **new**
value (LongAdder-style aggregate counter).

When adding new RMW methods, pick the contract first, then the name:

- prev-value return → `fetchVerb(args)`
- new-value return → bare `verb(args)`

Do not mix.

### 9. Reset to default — `clear()`

`Map::clear()` — empty the container; returns `void`.

`Counter` has no `clear()` — `set(0)` is its windowed reset.
`Counter::set()` is the documented exception that returns the
**previous** value (not `void`): it is the atomic exchange, and
`set(0)` reading the prior total is the LongAdder `sumThenReset`
idiom. (`Atomic` spells the same operation `swap()`; Counter keeps
`set` because `set($n)` reads naturally for seeding and windowing.)

### 10. Registry identity — `id(): int`

Every `Shared\*` instance exposes `id(): int` for logs and the
`/__ox_shared/entry?id=<id>` observability endpoint.

## Cheat sheet

| Concept                     | Canonical name           | Examples                                |
| --------------------------- | ------------------------ | --------------------------------------- |
| Read a value                | `get()`                  | `Map::get`, `Counter::get`              |
| Read an atomic              | `load($order)`           | `Atomic::load`                          |
| Write a value               | `set()`                  | `Map::set`                              |
| Write an atomic             | `store($v, $order)`      | `Atomic::store`                         |
| Number of elements          | `count(): int`           | `Map::count`, `Channel::count`, `Pool::count` |
| Boolean property            | `is*(): bool`            | `Channel::isClosed`                     |
| Conditional insert          | `setIfAbsent($k, $v)`    | `Map::setIfAbsent`                      |
| Non-blocking wait           | `try*()`                 | `Channel::trySend`, `Mutex::tryWithLock` |
| Forever wait                | bare verb                | `Channel::send`, `Channel::recv`, `Mutex::withLock`     |
| Bounded wait                | `*Timeout(int $ms)`      | `Channel::sendTimeout`, `Mutex::withLockTimeout`        |
| Compare-and-swap            | `compareAndSet()`        | `Atomic::compareAndSet`                 |
| Swap, return prev           | `swap()`                 | `Atomic::swap`, `Flag::swap`            |
| Atomic RMW, return prev     | `fetch*()`               | `Atomic::fetchAdd`                      |
| Atomic RMW, return new      | bare verb                | `Counter::add`                          |
| Reset to default            | `clear()`                | `Map::clear`                            |
| Registry id                 | `id(): int`              | every `Shared\*` type                   |

## Adding a new `Shared\*` type

When proposing a new primitive, fill out this checklist before merging:

- [ ] Every method maps to a row in the cheat sheet, or has an ADR
  explaining the exception (see `Atomic::load/store` and
  `Counter::set` above).
- [ ] If the type holds a collection of values, it implements
  `\Countable` and exposes `count(): int`.
- [ ] Read methods are `get` or `load` (atomic only).
- [ ] Boolean getters use the `is*` prefix.
- [ ] Wait-policy variants follow the `try*` / bare / `*Timeout(int $ms)`
  trichotomy. The `*Timeout` variant takes `int $ms > 0` and rejects
  zero / negative / non-int input with `TypeException`. Wait-policy
  `try*` methods return either a value-typed Result or throw a domain
  exception — never `null`-to-encode. Conditional-success ops follow
  the dedicated `setIfAbsent` naming instead of `try*`.
- [ ] No `len`, `size`, `pending`, `test`, or other ad-hoc names.
- [ ] Domain-specific verbs (`evict`, `drain`, `flush`, etc.) appear
  only when no canonical entry in the cheat sheet covers the concept.

## Observability names lag the PHP API

The operator-facing surface — Prometheus metric names and the JSON at
`/__ox_shared/entry?id=<id>` — is a separate contract from the PHP API.
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
