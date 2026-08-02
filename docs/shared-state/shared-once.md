---
title: Shared\Once
description: Run-once container — exactly one worker's factory produces the value, everyone else sees the memoised result, across the whole OxPHP process.
---

# Shared\Once

`OxPHP\Shared\Once` runs an initialisation closure exactly once for **a single `Once` cell** and makes its result visible to every subsequent caller of that same cell. It is the primitive for "expensive thing that should happen at most once."

To get true cross-worker / cross-request "exactly once across the whole process" semantics, bind the `Once` under a name via [`Shared\Registry::once(...)`](shared-registry.md) so every worker converges on the same cell. The bare `new Shared\Once()` constructor pattern produces a separate cell per worker thread in worker mode (each worker's bootstrap runs the constructor) and per request in traditional mode — that gives once-per-cell, not once-per-process.

## Overview

- **Run-once across workers _for one cell_.** Two workers racing into `getOrInit($factory)` on the same `Once` cell run the factory on only one of them; the loser blocks and receives the winner's value. Combine with `Shared\Registry::once` to make "the same cell" mean "the same name across every worker."
- **A four-state machine.** A cell is `Uninitialized`, `Pending` (a factory is running right now), `Ready`, or `Poisoned`. Read it with `status()`.
- **No ambiguous null.** `get()` throws on an unset cell instead of returning `null`, so a stored `null` is a real value, not "missing".
- **Reentrancy-safe.** Calling `getOrInit()` on the same Once from inside its own factory throws `DeadlockException` instead of hanging.
- **Configurable failure policy.** By default a failed factory resets the cell so a later call can retry. Opt into `Poison` to make a failed factory disable the cell permanently.
- **Shareable.** Instances live in the registry and travel through `use` captures and `Shared\Map` entries.

## API Reference

```php
namespace OxPHP\Shared;

final class Once implements Shareable
{
    public function __construct(Once\FailureMode $onFactoryError = Once\FailureMode::Reset);

    public function get(): mixed;                    // throws if not Ready
    public function status(): Once\Status;           // never throws
    public function trySet(mixed $value): bool;      // true if this call won
    public function getOrInit(callable $factory): mixed; // runs factory at most once

    public function id(): int;
}

namespace OxPHP\Shared\Once;

enum Status { case Uninitialized; case Pending; case Ready; case Poisoned; }
enum FailureMode: int { case Reset = 0; case Poison = 1; }
```

| Method      | Returns       | Use case                                                              |
|-------------|---------------|----------------------------------------------------------------------|
| `get`       | stored value  | Read a value you know is `Ready`. Throws on uninit / pending / poison.|
| `status`    | `Once\Status` | Introspection / diagnostics. Never throws (the safe poison observer). |
| `trySet`    | winner?       | Push-model init for a value already in hand (no side-effect resource).|
| `getOrInit` | stored value  | Pull-model init; the canonical race-free primitive.                   |
| `id`        | registry id   | Logging / observability correlation.                                  |

## Examples

### Expensive config loaded once per process

```php
<?php
// Registry::once binds the cell under a name so every worker's bootstrap
// converges on it. Without Registry the bare `new Once()` here would
// create one cell PER worker thread, and the factory would run once
// per worker, not once per process.
$config = OxPHP\Shared\Registry::once(
    'app-config',
    fn() => new OxPHP\Shared\Once(),
);

oxphp_worker(function () use ($config) {
    $cfg = $config->getOrInit(function () {
        // Runs in exactly one worker process-wide; every other worker
        // (and every later request, in traditional mode) blocks here
        // and sees the result.
        return json_decode(file_get_contents('/etc/myapp.json'), true);
    });

    echo $cfg['greeting'];
});
```

`getOrInit()` is the cache-stampede-safe pattern: under a burst of concurrent first-touches the factory runs exactly once *when it succeeds*, and every caller — including those that lost the race — receives the winner's value. If the winning factory throws in `Reset` mode the next blocked caller becomes the initialiser and retries, so a *persistently* failing factory under load retries serially rather than fanning out in parallel. Use `Poison` mode (below) when a failure should be terminal instead.

### Branch on state without triggering init

```php
<?php
use OxPHP\Shared\Once\Status;

$cfg = new OxPHP\Shared\Once();

$report = match ($cfg->status()) {
    Status::Ready        => $cfg->get(),
    Status::Pending      => 'initialising…',
    Status::Uninitialized => 'not started',
    Status::Poisoned     => 'config load failed',
};
```

`status()` is for introspection — it never triggers the factory and never throws, even on a poisoned cell. To actually obtain the value race-free, call `getOrInit()`.

### Value-first initialisation when the value is already known

```php
<?php
$buildSha = new OxPHP\Shared\Once();

// A plain value with no acquisition side effects — trySet is fine here.
$buildSha->trySet(getenv('GIT_SHA') ?: 'unknown');

$sha = $buildSha->get();   // Ready after the trySet above
```

Use `trySet()` only for values without side-effecting acquisition. For resources (connections, file handles, sockets) use `getOrInit()` instead: a `trySet()` that loses the race merely drops a plain value to the garbage collector, but a resource acquired *before* a lost race would leak.

A `false` return means the cell was already `Ready` **or** `Pending` — it does *not* guarantee a later `get()` will succeed, because a `Pending` factory on another thread can still fail and reset the cell (in `Reset` mode). Don't write `if (!$o->trySet($v)) { $x = $o->get(); }`; if you need the value, call `getOrInit()`.

### Database connection bootstrap

```php
<?php
// Name the cell so only one PDO connection is opened across the
// entire OxPHP process. The factory acquires a resource — exactly
// what `getOrInit`'s block-losers semantics protect.
$pool = OxPHP\Shared\Registry::once('db-conn', fn() => new OxPHP\Shared\Once());

$conn = $pool->getOrInit(function () {
    return new PDO(getenv('DB_DSN'), getenv('DB_USER'), getenv('DB_PASS'), [
        PDO::ATTR_PERSISTENT => true,
    ]);
});
```

For a connection pool with multiple slots, see [Shared\Pool](shared-pool.md) — `Once` gives you *one* value; `Pool` gives you N.

### Fail-fast on a broken prerequisite

```php
<?php
use OxPHP\Shared\Once\FailureMode;

// If this initialisation fails, the app cannot recover — poison the cell so
// every later access fails loudly instead of retrying a doomed factory.
$secrets = new OxPHP\Shared\Once(onFactoryError: FailureMode::Poison);

$secrets->getOrInit(fn () => loadSecretsOrThrow());
```

## Semantics & gotchas

- **`get()` throws when the cell is not `Ready`.** `UninitializedException` for an empty or `Pending` cell, `PoisonedException` for a poisoned one. Use `status()` to branch without an exception, or `getOrInit()` to obtain the value safely.
- **The factory runs at most once per successful initialisation.** Concurrent callers block on the winner; they do not run their own copy.
- **Failure policy is set at construction, not per call.** `Reset` (default) returns the cell to `Uninitialized` on factory failure so a later call retries; `Poison` makes the cell terminally `Poisoned`. In both modes the factory's exception is re-thrown to the *current* caller.
- **Poison is cross-thread-honest, not object-identical.** A PHP exception object cannot cross worker threads, so a poisoned cell captures the failure's class, message, and code. Later callers on any thread receive a fresh `PoisonedException` carrying that information — the same details, not the same object.
- **Reentrance throws.** `getOrInit()` from inside its own factory raises `DeadlockException`. Restructure so the inner call uses a different `Once`.
- **Full value range.** Scalars, arrays, and nested `Shareable` values are stored and read back. Closures, resources, and non-`Shareable` PHP objects raise `TypeException`.

## Exceptions

| Exception                | Raised by                                                          |
|--------------------------|-------------------------------------------------------------------|
| `UninitializedException` | `get()` on an `Uninitialized` or `Pending` cell.                  |
| `PoisonedException`      | `get()` / `getOrInit()` / `trySet()` on a `Poisoned` cell.        |
| `DeadlockException`      | `getOrInit()` called recursively on the same Once from its factory.|
| `TypeException`          | A stored value is not serialisable (closure, resource).           |
| `StaleHandleException`   | Any method on a handle whose registry entry was evicted.          |

If the factory itself throws, that exception propagates unchanged to the current caller. In `Reset` mode the cell stays uninitialised and the next `getOrInit` retries; in `Poison` mode the cell becomes poisoned.

## Observability

See [Shared Observability](shared-observability.md). Quick references:

- `GET /__ox_shared/entry?id=N` exposes `{ status: "uninitialized" | "pending" | "ready" | "poisoned", type: "Once" }` plus a preview of the stored value when `ready`.

## When not to use

- **Values that change after creation.** `Once` is write-once. Use `Shared\Mutex` or `Shared\Map` when the stored state mutates.
- **Per-worker local state.** Static class properties or module globals are cheaper when the value does not need to be shared.
- **Expensive *per-request* computations.** Cache inside the request, not in shared state — you will leak memory otherwise.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Mutex](shared-mutex.md) — when the one-shot value later mutates.
- [Shared\Pool](shared-pool.md) — one-shot init of *N* equivalent resources.
- [Shared\Map](shared-map.md) — keyed init using `getOrSet($key, $factory)`.
