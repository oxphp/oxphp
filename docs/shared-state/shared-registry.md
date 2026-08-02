---
title: Shared\Registry
description: Name-keyed process-global handles for Shared\* primitives — one Map / Counter / Channel that every worker and every request converges on, without external stores.
---

# Shared\Registry

`OxPHP\Shared\Registry` is the **name-keyed** companion to the rest of `OxPHP\Shared\*`. Where `new Shared\Map()` produces an anonymous entry shared only by handle propagation (`use` capture, async fibers, nesting), `Registry::map('cache', fn() => new Shared\Map(...))` binds an entry under a string key — and every caller of `Registry::map('cache', …)`, on any worker thread, in any request, gets the same entry.

It is the answer to the question *"how do I share one `Shared\Map` across all workers, or across all requests in traditional mode?"*. The other `Shared\*` types are still themselves the right unit of mutable state; `Registry` is just how you put a name on one of them.

## Mental model

```
 worker #1     worker #2     worker #3
   │              │              │
   └───── Registry::map('cache', $factory) ─────┐
                                                 ▼
                       ┌─────────────────────────────────────┐
                       │ SharedRegistry (process-global)     │
                       │                                     │
                       │   names: { "cache" → Bound(Arc<E>) }│
                       │   entries: { id=7: Map { … } }      │
                       └─────────────────────────────────────┘
```

- The **first** caller of `Registry::map($key, $factory)` for an unbound key runs the factory and pins the resulting entry under the name.
- Every subsequent caller — same thread, other workers, later requests — receives the same entry. The factory is **not** re-run; it is ignored on a hit.
- Concurrent first-touches block on a per-key gate: exactly **one** thread runs the factory, the others wait and get the winner's entry. No double resource acquisition for connection pools.

Identity-by-name complements identity-by-handle. Anonymous (`new Shared\*()`) and named entries coexist in the same process-global registry. The names index simply adds a string lookup on top.

## Quick start — one counter across every worker

```php
<?php
// worker.php — entry script in worker mode, executed once per worker thread
require __DIR__ . '/vendor/autoload.php';

$requests = OxPHP\Shared\Registry::counter(
    'request-counter',
    fn() => new OxPHP\Shared\Counter(),
);

oxphp_worker(function () use ($requests) {
    $n = $requests->add();          // atomic across ALL workers — one shared int64
    header('X-Request-Count: ' . $n);
    echo "hello\n";
});
```

Compare to the captured-handle pattern (`$x = new Shared\Counter()` in the bootstrap). That pattern produces one counter **per worker thread** — each worker runs its own bootstrap and gets its own anonymous entry. Aggregate counts diverge by a factor of the worker pool size. `Registry::counter('request-counter', …)` instead converges every worker on a single entry, so the count is the actual total.

The same shape works in traditional mode (no `WORKER_MODE_ENABLED`). The first request that touches `'request-counter'` creates the entry; every subsequent request — on any worker thread — sees it. This is the same-host APCu replacement story, with typed primitives and atomic operations instead of `apcu_fetch` / `apcu_store`.

## API reference

```php
namespace OxPHP\Shared;

final class Registry
{
    // Typed get-or-create. On hit, the factory is ignored; on miss it
    // runs at most once across all workers (block-losers) and must
    // return a fresh instance of the matching type.
    public static function map(string $key, callable $factory): Map;
    public static function counter(string $key, callable $factory): Counter;
    public static function atomic(string $key, callable $factory): Atomic;
    public static function flag(string $key, callable $factory): Flag;
    public static function once(string $key, callable $factory): Once;
    public static function mutex(string $key, callable $factory): Mutex;
    public static function channel(string $key, callable $factory): Channel;
    public static function pool(string $key, callable $factory): Pool;

    // Untyped escape hatch — returns whatever is bound (no type guard).
    public static function global(string $key, callable $factory): Shareable;

    // Namespace management — operates on the name index, NOT the objects.
    public static function remove(string $key): bool;
    public static function keys(): array;            // list<string>

    // Layer-wide introspection.
    public static function memoryUsage(): int;       // estimated bytes, all Shared\* entries
    public static function count(): int;             // live Shared\* entries (named + anonymous)
}
```

| Method | Returns | Use |
|---|---|---|
| `map` / `counter` / `atomic` / `flag` / `once` / `mutex` / `channel` / `pool` | the requested `Shared\*` type | Primary surface. Type-guarded on hit; type-validated on factory return. |
| `global` | `Shareable` | Untyped get-or-create. Reach for it only when you genuinely don't know the bound type up front. |
| `remove` | `bool` | Drop the name binding + pin. **Does not destroy the object.** |
| `keys` | `list<string>` | Currently-bound keys (Bound only; in-flight Creating slots are not listed). |
| `memoryUsage` | `int` | Process-wide estimated bytes — see [Memory and introspection](#memory-and-introspection). |
| `count` | `int` | Process-wide live entries (named **and** anonymous). |

`Registry` is a static facade: `new Registry()` throws `Shared\SharedException`.

## Lifecycle — pinned by default

A bound key holds a **strong** reference to its entry; the entry is alive for the lifetime of the process unless you explicitly call `remove(key)` or the process tears down. This is deliberate: in traditional mode, where each request creates its own PHP-side handles and they die at request end, the name index's pin is the *only* reason the entry survives between requests.

Invalidate the *contents* of a named entry by mutating it in place — `$cache->clear()`, `$counter->set(0)`, `$bucket->remove($k)` — not by removing the name. Mutation is shared by reference: every holder of the same key sees the change immediately.

### `remove` is namespace management, not object destruction

`remove($key)` drops the binding and the pin. The entry itself survives while any other handle references it (a captured bootstrap variable, a value nested in another `Shared\Map`, an in-flight `oxphp_async`). When the last handle drops, the entry self-deregisters as usual.

After `remove`, the key is free. The next `Registry::map($key, …)` creates a **new** entry — distinct id. **Captured handles to the previous binding continue to operate on the old (now-anonymous) entry; they do not auto-converge on the new one.**

```php
$cache = Registry::map('cache', fn() => new Shared\Map());
$id_a = $cache->id();

Registry::remove('cache');
$cache->set('x', 1);                        // still mutates the OLD entry — fine, but it's no longer "cache"

$fresh = Registry::map('cache', fn() => new Shared\Map());
$id_b = $fresh->id();                       // different id — this is a new entry

assert($id_a !== $id_b);
assert($cache->get('x') === 1);              // OLD entry retained value
assert($fresh->get('x') === null);           // NEW entry is empty
```

If you rotate keys (per-tenant entries that come and go, key-versioning), address them by name per call (`Registry::map($key, …)` per request) instead of capturing the handle once in bootstrap. Captured handles + key rotation diverges silently; address-by-name converges on whatever the current binding is.

`remove` returns `true` if a bound key was dropped, `false` if the key was absent.

## Errors

| Exception | When |
|---|---|
| `Shared\TypeException` | Typed method on a key bound to a different type; factory returned the wrong `Shared\*` type or a non-`Shareable`. |
| `Shared\CapacityException` | Creation would exceed `SHARED_MAX_ENTRIES` / `SHARED_MAX_BYTES` caps. |
| `Shared\DeadlockException` (reentrant) | `Registry::map($key, …)` for the *same* `$key` from inside its own factory on the same thread. |
| `Shared\DeadlockException` (cross-key cycle) | Waited longer than 30 s on another thread's `Creating` slot — most plausibly factory A holds key K1 while waiting on K2 whose factory is held by thread B waiting on K1. Distinct message from the reentrant case. |
| `Shared\SharedException` (draining) | The server is shutting down — the registry refuses new acquires and binds. Expected during graceful shutdown; not a code bug. |
| `Shared\SharedException` (bind race) | A peer creator was already settled into the slot while this thread's factory was running (the factory's entry was NOT pinned under the key). Retry the call. |
| `\InvalidArgumentException` (SPL) | Empty `$key`. Argument validation, distinct from domain type errors. |
| *(factory's exception)* | If the factory throws, the slot is aborted (Creating → absent, waiters wake to retry) and the original exception propagates to the creator. |

`Shared\DeadlockException` extends `OxPHP\Async\AsyncException` — `catch (AsyncException)` sweeps it together with bounded-wait timeouts elsewhere in `Shared\*`. The two distinct `DeadlockException` cases share the class; tell them apart by the message (`"reentrant get-or-create"` vs `"waited too long … cross-key cycle"`).

## Memory and introspection

`Registry::memoryUsage()` and `Registry::count()` report the **whole Shared\* layer**, not just named entries. Anonymous entries created via `new Shared\*()` (the bulk of current `Shared\*` usage — bootstrap captures, in-flight values inside `Map` and `Channel`, async-fiber captures) are included.

This is deliberate. The two numbers exist for capacity / OOM monitoring; that monitoring must see per-worker and in-flight anonymous state, not just the named namespace. As a result:

- Both numbers are **transient** — they rise and fall with in-flight requests and per-worker handles.
- `Registry::count()` is **not** equal to `count(Registry::keys())`. `keys()` is the named namespace only.
- `memoryUsage()` is a **static accounting estimate**, not actual RSS. It is the same number that `SHARED_MAX_BYTES` caps. For the real heap footprint, use a heap profiler (`heaptrack`, `jemalloc_stats_print`, `mi_stats_print`) or container memory metrics.

Per-entry detail — id, type, refcount, byte cost — lives on the [internal introspection endpoint](../features/internal-server.md) at `/__ox_shared/entries`. There is intentionally no per-entry PHP API to avoid duplicating that surface.

## When not to use

- **Cross-process, cross-host.** The registry lives inside one OxPHP process. Multiple OxPHP instances don't share it. Use Redis / NATS / your existing broker; see [Migrating to an external store](migrating-to-external-store.md).
- **Durability across restarts.** The registry evaporates on process exit. Persist via the same external store.
- **High-churn ephemeral keys.** Pinned-by-default semantics mean dynamic keys you generate per-request leak entries until you call `remove`. Bound by `SHARED_MAX_*` caps, but still poor form. For short-lived per-request state, use a normal PHP variable.
- **A cache invalidation primitive.** `remove($key)` is for retiring a name, not for "clear the cache." Invalidate contents in place (`$map->clear()`, `$map->remove($member_key)`); the name binding survives.

## See also

- [Shared State](shared-state.md) — overview of the layer, identity-by-handle, when `new Shared\*()` is the right tool.
- [Shared\Map](shared-map.md), [Shared\Counter](shared-counter.md), [Shared\Pool](shared-pool.md), … — the typed primitives `Registry` returns.
- [Shared Observability](shared-observability.md) — `/__ox_shared/*` JSON API and Prometheus metrics.
- [Migrating to an external store](migrating-to-external-store.md) — when you outgrow one process.
