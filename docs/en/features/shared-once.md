---
title: Shared\Once
description: Run-once container — exactly one worker's factory produces the value, everyone else sees the memoised result, across the whole OxPHP process.
---

# Shared\Once

`OxPHP\Shared\Once` runs an initialisation closure exactly once across the whole process and makes its result visible to every subsequent caller. It is the primitive for "expensive thing that should happen at most once, no matter how many workers start up in parallel."

## Overview

- **Run-once across workers.** Two workers racing into `init($factory)` run the factory only on one of them; the loser waits and sees the winner's value.
- **Memoised forever.** Once initialised, `get()` returns the value without running anything.
- **Reentrancy-safe.** Calling `init()` on the same Once from inside its own factory throws `DeadlockException` instead of hanging.
- **Shareable.** Instances live in the registry and travel through `use` captures and `Shared\Map` entries.

## API Reference

```php
namespace OxPHP\Shared;

final class Once implements Shareable
{
    public function __construct();

    public function get(): mixed;                   // null if not yet set
    public function isInitialized(): bool;
    public function trySet(mixed $value): bool;     // true if this call won
    public function init(callable $factory): mixed; // runs factory exactly once

    public function id(): int;
}
```

| Method          | Returns      | Use case                                                         |
|-----------------|--------------|------------------------------------------------------------------|
| `get`           | value or null| Pure read; `null` if nobody has initialised yet.                 |
| `isInitialized` | bool         | Probe without fetching the value.                                |
| `trySet`        | winner?      | Direct value-first init when you already have the value in hand. |
| `init`          | stored value | Factory-based init; returns the memoised value on every call.    |
| `id`            | registry id  | Logging / observability correlation.                             |

## Examples

### Expensive config loaded once per process

```php
<?php
$config = new OxPHP\Shared\Once();

oxphp_worker(function () use ($config) {
    $cfg = $config->init(function () {
        // Runs in exactly one worker; everyone else sees the result.
        return json_decode(file_get_contents('/etc/myapp.json'), true);
    });

    echo $cfg['greeting'];
});
```

### Value-first initialisation when the value is already known

```php
<?php
$buildSha = new OxPHP\Shared\Once();

// Usually loaded from a build-time constant, not computed at runtime.
if ($buildSha->trySet(getenv('GIT_SHA') ?: 'unknown')) {
    // We stored it.
}

// Everyone reads the memoised value.
$sha = $buildSha->get();   // never null after the first trySet above
```

### Database connection bootstrap

```php
<?php
$pool = new OxPHP\Shared\Once();

$conn = $pool->init(function () {
    return new PDO(getenv('DB_DSN'), getenv('DB_USER'), getenv('DB_PASS'), [
        PDO::ATTR_PERSISTENT => true,
    ]);
});
```

For a connection pool with multiple slots, see [Shared\Pool](shared-pool.md) — `Once` gives you *one* value; `Pool` gives you N.

## Semantics & gotchas

- **`get()` returns `null` before `init()` / `trySet()` succeeds.** Distinguish "not set yet" from "set to null" using `isInitialized()`.
- **The factory runs at most once per process.** Even if it throws, it counts as the attempt — `Once` does not retry. Wrap retryable logic inside the factory yourself.
- **Reentrance throws.** `init()` from inside its own factory raises `DeadlockException` with the id in the message. Restructure the graph so the inner call uses a different `Once` or fetches the not-yet-stored value differently.
- **The factory's return is serialised into shared-safe form.** Scalars and nested arrays of scalars pass through; closures, resources, and non-`Shareable` PHP objects raise `TypeException`.

## Exceptions

| Exception                | Raised by                                                       |
|--------------------------|-----------------------------------------------------------------|
| `DeadlockException`      | `init()` called recursively on the same Once from its factory.  |
| `TypeException`          | Factory returns a non-serialisable value (closure, resource).   |
| `StaleHandleException`   | Any method on a handle whose registry entry was evicted.        |
| `UninitializedException` | `id()` on a wrapper that has not finished `__construct`.        |

If the factory itself throws, that exception propagates unchanged; the Once remains un-initialised and the next `init` call will retry.

## Observability

See [Shared Observability](../operations/shared-observability.md). Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ initialized: bool, type: "Once" }` plus a preview of the stored value when available.
- Prometheus `oxphp_shared_once_initialized{once_id="…"}` gauge (0 or 1).

## When not to use

- **Values that change after creation.** `Once` is write-once. Use `Shared\Mutex` or `Shared\Map` when the stored state mutates.
- **Per-worker local state.** Static class properties or module globals are cheaper when the value does not need to be shared.
- **Expensive *per-request* computations.** Cache inside the request, not in shared state — you will leak memory otherwise.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Mutex](shared-mutex.md) — when the one-shot value later mutates.
- [Shared\Pool](shared-pool.md) — one-shot init of *N* equivalent resources.
- [Shared\Map](shared-map.md) — keyed init using `getOrSet($key, $factory)`.
