---
title: Shared\Pool
description: Process-wide bounded object pool for expensive per-thread resources — database connections, JSON parsers, HTTP clients — with strict budget, per-thread affinity, and idle-timeout eviction.
---

# Shared\Pool

`OxPHP\Shared\Pool` is a bounded pool of per-thread resources. It is the primitive for managing objects that are expensive to create, cannot be recreated cheaply, and should not exist in unlimited numbers — typically database connections, prepared-statement caches, reusable JSON decoders, or HTTP client sessions.

A Pool gives each PHP worker thread its own lane of ready-to-use resources, enforces a strict maximum across the whole pool, and recycles idle slots automatically so you do not pay for capacity you do not use.

## Overview

- **Strict budget.** `maxSize` is a hard cap across the whole pool. When the pool is saturated, an acquire either waits, returns `null`, or throws — depending on which acquire method you call.
- **Per-thread affinity.** Each worker thread has its own idle queue. An acquire pulls from the local queue first and never hands a slot minted on thread A to thread B (v1).
- **Factory runs in the acquiring worker.** Resources are minted lazily on first demand per thread, not at pool construction. An acquire inside an `oxphp_async()` task runs it on the task's thread, where the functions and classes of your script do not exist — see [Factory and destroy semantics](#factory-and-destroy-semantics).
- **Destroy callback on eviction.** An optional `destroy($resource)` closure runs when the pool drops a slot on the thread holding it (idle timeout, manual eviction, the pool going away there). It does not run at server stop unless you ask for it — see [When the destroy callback is skipped](#when-the-destroy-callback-is-skipped).
- **Idle-timeout eviction.** In worker mode, slots sitting idle longer than `idleTimeoutMs` are destroyed by the worker thread holding them, which a background scheduler wakes for it. Set `idleTimeoutMs: 0` to disable idle eviction entirely. Slots held elsewhere are not evicted this way — see [Idle-timeout eviction](#idle-timeout-eviction).
- **RAII handles.** `acquire()` returns a `Handle`; the slot returns to the pool automatically when the handle goes out of scope (including on exception), or earlier via `$handle->release()`.
- **Shareable.** In worker mode a pool survives request boundaries and is shared by handle (`use ($pool)` in closures). Its factory and destroy callback still end with the worker that created the pool — see [When the worker that created a pool exits](#when-the-worker-that-created-a-pool-exits). Outside worker mode a pool lasts only as long as the request that creates it — see [Outside worker mode](#outside-worker-mode).

## API Reference

```php
namespace OxPHP\Shared;

final class Pool implements Shareable
{
    public function __construct(
        callable  $factory,                  // fn(): object — create a resource
        ?callable $destroy       = null,     // fn(object): void — tear down a resource
        int       $maxSize       = 32,       // hard cap on live slots; > 0
        int       $idleTimeoutMs = 300_000,  // idle ms before eviction; 0 disables it
    );

    // acquire family — millisecond timeout trichotomy
    public function acquire(): Pool\Handle;                 // wait forever
    public function tryAcquire(): ?Pool\Handle;             // non-blocking; null if saturated
    public function acquireTimeout(int $ms): Pool\Handle;   // bounded; $ms > 0

    // with family — scope-guard around the raw resource
    public function with(callable $body): mixed;                 // wait forever
    public function withTimeout(callable $body, int $ms): mixed; // bounded; $ms > 0

    public function stats(): Pool\Stats;   // point-in-time snapshot of counters
    public function evict(): int;          // force-evict all idle slots now; returns count
    public function id(): int;
}

namespace OxPHP\Shared\Pool;

class Handle
{
    public function get(): mixed;     // the underlying resource (throws after release)
    public function release(): void;  // return the slot now; idempotent; also runs on destruct
}

final class Stats
{
    public function inUse(): int;    // slots currently checked out
    public function idle(): int;     // free slots ready to hand out
    public function waiting(): int;  // callers waiting in acquire
    public function size(): int;     // inUse() + idle() (live slots)
    public function maxSize(): int;  // configured cap

    public function utilization(): float;  // inUse() / maxSize(), 0.0 if maxSize() == 0
}
```

| Method            | Returns       | Use case                                                               |
|-------------------|---------------|------------------------------------------------------------------------|
| `acquire`         | `Handle`      | Check out a resource, waiting forever for a free slot.                  |
| `tryAcquire`      | `?Handle`     | Non-blocking check-out. Returns `null` immediately if the pool is saturated. |
| `acquireTimeout`  | `Handle`      | Check out within a bounded budget of `$ms` (`> 0`); throws `OperationTimeoutException` on expiry. |
| `with`            | mixed         | Scope-guard: acquire (forever), run `$body($resource)` with the raw resource, release even on exception. The closure's return value passes through. |
| `withTimeout`     | mixed         | Like `with`, but the acquire is bounded by `$ms`.                       |
| `stats`           | `Pool\Stats`  | Point-in-time snapshot of pool counters.                                |
| `evict`           | int           | Force-evict all idle slots now (regardless of `idleTimeoutMs`); returns the count dropped. |
| `id`              | int           | Registry identifier; useful for logging / observability.               |
| `Handle::get`     | mixed         | The underlying resource. Throws `StaleHandleException` after release.   |
| `Handle::release` | void          | Return the slot to the pool now. Idempotent; also runs automatically on destruct (RAII). |

Timeouts follow the same trichotomy as `Shared\Mutex` and `Shared\Channel`: a bare method waits forever, a `try*` method is non-blocking, and a `*Timeout(int $ms)` method waits a bounded number of milliseconds. There is no float-seconds timeout.

## Examples

### Database connection pool

```php
<?php
$db = new OxPHP\Shared\Pool(
    factory: function () {
        return new PDO(
            getenv('DB_DSN'),
            getenv('DB_USER'),
            getenv('DB_PASS'),
            [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION],
        );
    },
    destroy: function (PDO $conn) {
        // Nothing to do — PDO closes on destruct. The callback exists
        // for resources that need explicit teardown (sockets, handles).
    },
    maxSize: 16,
    idleTimeoutMs: 60_000,     // free idle connections after 1 min
);

// In a request handler
$users = $db->with(function (PDO $conn) use ($userId) {
    $stmt = $conn->prepare('SELECT * FROM users WHERE id = ?');
    $stmt->execute([$userId]);
    return $stmt->fetch();
});
```

`with()` is the shortest-lifetime pattern: acquire happens on entry, release happens on return *or* on exception — you cannot leak a handle. It also hands your closure the **raw resource** directly, so you skip the `Handle::get()` step.

### Manual acquire / release

```php
<?php
$h = $pool->acquire();        // waits forever for a free slot

$conn = $h->get();
$conn->beginTransaction();
doWork($conn);
$conn->commit();

$h->release();                // or just let $h fall out of scope (RAII)
```

The handle returns its slot automatically when it is destroyed — including if an exception unwinds the stack — so an explicit `release()` is optional. Reach for a manual handle (over `with()`) only when the resource must survive multiple calls in a handler sequence.

### Reusable parser pool

```php
<?php
$parsers = new OxPHP\Shared\Pool(
    factory: fn () => new JsonMachine\Parser(),
    maxSize: 8,
);

$doc = $parsers->with(fn ($p) => $p->parse($body));
```

### Non-blocking acquire with fallback

```php
<?php
$h = $pool->tryAcquire();
if ($h === null) {
    // Pool saturated — degrade gracefully without waiting.
    http_response_code(503);
    header('Retry-After: 1');
    return;
}
// ... use $h->get(); slot returns on scope exit.
```

### Bounded acquire with a timeout

```php
<?php
try {
    $h = $pool->acquireTimeout(100);   // wait up to 100 ms
} catch (OxPHP\Shared\OperationTimeoutException $e) {
    http_response_code(503);
    header('Retry-After: 1');
    return;
}
// ... use $h->get();
```

## Factory and destroy semantics

The factory runs **lazily** on the acquiring worker thread. A pool with `maxSize: 32` does not pre-allocate 32 resources; it mints them as demand arrives, bounded by `maxSize` across all threads combined.

- The factory must return a PHP object. Returning a non-object surfaces as `TypeException` from the acquire call, and the slot is not counted against the budget.
- A factory that throws propagates its own exception to the acquire caller unchanged, and the slot is not counted against the budget.
- A fatal error inside the factory ends the request, and the slot is not counted against the budget either.
- The destroy callback (if supplied) runs when the pool drops a slot on the thread holding it, while PHP code is running there: idle timeout expiry, explicit `evict()`, or the pool's last reference going away on that thread. It runs on a worker thread (not on the Tokio thread driving the eviction scheduler), so PHP is safe to call. It does not run at server stop, nor for a slot another thread holds — see [When the destroy callback is skipped](#when-the-destroy-callback-is-skipped).
- A destroy callback that throws is logged but does not poison the pool — the slot is already being destroyed, so there is nothing useful to roll back.
- In worker mode an acquire inside an `oxphp_async()` task runs the factory on the task's thread of the `ASYNC_WORKERS` pool, and the destroy callback for the slots that thread holds runs on it as well — when `evict()` is called inside a task, for one. That thread never ran your script, so nothing your script declares exists on it; built-in functions and classes and [those loaded by `opcache.preload`](../features/async-promises.md#your-own-functions-and-classes-in-tasks) do. A factory or destroy callback that calls a function declared in its own file, or declares a class — anonymous or named — inside its body, is refused there before its first statement runs, even if it already ran on a worker thread. A refused factory throws `Error` from the acquire call, and the slot is not counted against the budget. A refused destroy callback is skipped the way one that throws is: the slot is dropped all the same, and `evict()` counts it. Besides the callback's own body and the closures it declares, the check covers what the callback carries with it: the objects and closures it captured, arrays of them included; the class of `$this`, if the callback uses `$this`; the class `self::`, `static::` or `parent::` stands for, if the callback's body calls a method or creates an object through it (`self::make()`, `new static()`); the class whose `__call()` or `__callStatic()` method a callback ends up in when it names a method its class does not have or one the code creating the pool cannot call, directly or through a closure made from it such as `$object->make(...)`; and the closure behind a `[$closure, '__invoke']` callback. A class is checked whole, every method and property hook it and its parent classes declare, `__destruct()` included, whichever of them the callback calls. So a callback that captured an object, or uses `$this`, is refused on such a thread when any method or property hook of that class calls a function declared in the file it is written in, or declares a class. Anything the callback reaches another way is not checked and must be built in or preloaded as well. For example: an object held in a property, a static property, a class constant or a `static` variable of a function or method, or inside a built-in object such as `ArrayObject`, `SplObjectStorage` or a `Generator`; an object a parameter's default value creates (`new self()`); a method a string callable names through `self`, `static` or `parent`, such as `call_user_func('static::make')`; or a class a closure names, such as `Maker::make()` or `new Maker()`, once the closure has run: until then a missing class throws, but every thread shares a closure's run-time cache, so from the first run on, PHP reuses the class it found then, on every thread. A call from there to a function of your script still crashes the server. A destroy callback that is not refused and does run on a task's thread — even one with an empty body — can take the server down at a later request, so do not call `evict()` inside a task on a pool that has a destroy callback. A factory that passes the check and calls a closure it captured can do the same when an acquire inside a task runs it.
- In worker mode the same check runs on every worker thread. A pool shared between them — bound to a name with `Registry::pool()`, say — runs its factory on each worker thread that acquires from it, and its destroy callback on the thread that holds the slot. If either — or a method or closure the check above follows it to — is written in a file that only some requests include, and calls a function declared in that file or declares a class, the callback is refused the same way on a worker thread that never included that file, even if it already ran on another one.
- A fatal error in the destroy callback that `evict()` runs ends the request or the task, as it would anywhere else. The slot being destroyed counts as dropped, and the idle slots `evict()` had not reached yet stay in the pool. The object in the dropped slot is not freed, though: in worker mode, and in a pool created inside an `oxphp_async()` task, it stays alive until its thread exits, so a connection in it stays open all that time without counting against `maxSize`. `evict()` holds no lock of the pool while the callback runs, so the callback can call `tryAcquire()` and `release()` on that same pool.
- A fatal error in the destroy callback that the idle-timeout eviction runs has no request to end: it retires the worker, which logs a warning saying so, and the pool starts a replacement. The slot being destroyed counts as dropped, and the eviction stops there: it calls no destroy callback after the one that failed, and the idle slots it had not reached on that worker, in this pool and in any other, are released when the worker exits.
- A fatal error in the destroy callback that runs when the pool's last reference goes away — an `unset()` of the last variable holding it, or a `Shared\Map` letting go of it — ends the request or the task, as it would anywhere else. The slot being destroyed counts as dropped. The pool's other idle slots on that thread go without `$destroy`, and so do those of any other pool the same call lets go of — two pools a map's `clear()` removes, say. None of these objects is freed, and neither are the pool's factory and destroy callbacks: outside worker mode PHP frees them along with the rest of the request the error ended, but in worker mode, and in an `oxphp_async()` task, they stay alive until the thread exits, so a connection in one stays open all that time without counting against `maxSize`.

## Per-thread affinity

v1 pools are strict per-thread: a slot minted on worker thread A cannot be acquired on worker thread B. Practically this means `stats()->idle()` can be non-zero on worker A while worker B is waiting in `acquire()`. This keeps slots hot in the thread that uses them (DB connections, OPcache-primed objects) and avoids shuffling resources across cores.

Cross-thread work stealing is a v1.x candidate. Until then, size `maxSize` against the number of worker threads × expected per-thread concurrency, not just aggregate demand.

## Outside worker mode

With traditional, Framework or SPA routing every HTTP request is a PHP request of its own, and when it ends PHP frees every object it created — including the pool's factory and destroy closures and every object the factory returned. A pool a request creates there is tied to that request:

- Only that request can acquire from it. `acquire()`, `tryAcquire()`, `acquireTimeout()`, `with()` and `withTimeout()` called from any other request — one that reached the pool through a `Shared\Map`, say — or from a task started with `oxphp_async()`, which runs on a thread of the `ASYNC_WORKERS` pool, throw `ClosedException`.
- When the request ends, the pool closes. The objects still idle in it are freed by PHP along with the rest of the request, like any other object the request created; `$destroy` is not called for them. Any later acquire throws `ClosedException`.
- If the pool's last reference goes away in another request or in a task while the creating request still runs, the pool closes then, and `$destroy` is not called for its idle objects either: PHP frees them when the creating request ends.
- `Registry::pool()` called from a request throws `SharedException` without calling its factory, and `Registry::global()` throws it when its factory returns a pool: a registry name outlives every request that could use the pool. In a task the request hands its pool to, both throw it when their factory returns that pool.
- `idleTimeoutMs` has no effect: no slot outlives the request.

A pool created inside a task started with `oxphp_async()` is not tied to a request — the task's thread runs a single PHP request for as long as it lives — and a task can bind it to a name with `Registry::pool()`. Its idle slots are not evicted by `idleTimeoutMs`, though — see [Idle-timeout eviction](#idle-timeout-eviction). A request that calls `Registry::pool()` with that name still gets `SharedException`. Handing such a pool to HTTP requests served without worker mode gains them nothing, though: each object such a request gets from it is one that request's own call to the factory created, it is reused only within that request, and when the request ends PHP frees it along with the rest of the request, without `$destroy`. Without worker mode, use such a pool from tasks only.

`oxphp run` executes the whole script as one request, so there a pool the script creates — `Registry::pool()` included — serves it from start to finish and closes when it ends, without `$destroy` for the objects still idle at that point, and `idleTimeoutMs` has no effect on it. A task started with `oxphp_async()` cannot acquire from it or bind it to a name there either.

To keep database connections open across requests without worker mode, use the driver's persistent connections instead of a pool — for PDO, `PDO::ATTR_PERSISTENT => true`, which keeps a connection open in each PHP worker thread for the next request on that thread that connects with the same DSN and credentials. To share one pool across requests, run the application in [worker mode](../features/worker-mode.md).

## When the worker that created a pool exits

A pool's factory and destroy callback belong to the PHP request that created the pool, like every closure that request makes, and PHP frees them when that request ends. In worker mode that request is the worker's script, which runs for as long as the worker does, so it ends when the worker is [recycled](../features/worker-mode.md#recycling), when a dynamic pool retires it, and when the server stops. In a task started with `oxphp_async()` it is the request of the task's thread, which lasts as long as that thread. The pool itself lives on wherever another worker still holds it — under a name, in a `Shared\Map`, or in a variable of its own — so the callbacks end before the pool does:

- Once the script of the request that created the pool has run to its end — `oxphp_worker()` has returned and the code after it has run — that request waits for the calls other threads are making into the factory or the destroy callback to return. Only then do its shutdown functions and destructors run and PHP free its memory. The wait has no limit: a callback that waits in turn for something the exiting worker gives up only as it shuts down — a lock it releases in a destructor, say — keeps that worker from exiting at all. After 5 seconds of waiting the server logs a warning.
- From then on no other thread calls them. The slots that already exist are still handed out and given back as before. An acquire that has to make a new slot throws `ClosedException`, saying that the pool's factory belonged to a request that has ended. A slot that another thread destroys — with `evict()` or by idle eviction — goes without `$destroy`, and is counted in `oxphp_shared_pool_destroy_skipped_total{reason="request_ended"}`.
- The request that created the pool can still call them itself while it shuts down, in its shutdown functions and its destructors.

A pool bound to a name gets new callbacks from the next call to `Registry::pool()` or `Registry::global()` for that name that is made in a request that is not shutting down. That call runs its factory, and the bound pool takes the factory and the destroy callback of the pool the factory returns — not its `maxSize` or its `idleTimeoutMs` — when that pool was created by the request making the call; a pool the factory got from elsewhere, such as another name, gives it none. The call returns the bound pool, as on any other hit. From then on new slots come from the new factory, and the new destroy callback is called for every slot of the pool, including the slots the old factory made. If the factory returns anything but a pool, the call throws `TypeException`, and if it throws, the exception propagates; either way the bound pool keeps the callbacks it had. When the workers call `Registry::pool()` for the name as they boot, the replacement of the worker that created the pool gives it new callbacks as soon as it boots. Until then, the other workers can only use the slots that exist; one that calls `Registry::pool()` again — inside the request handler, say — gives the pool its own callbacks first:

```php
oxphp_worker(function () {
    $db = Registry::pool('db', fn () => new Pool(
        factory: fn () => new PDO(getenv('DB_DSN')),
        maxSize: 16,
    ));
    // ...
});
```

A pool not bound to a name never gets new callbacks: once the request that created it has ended, it hands out the slots it has, and an acquire that needs a new one throws `ClosedException`.

## Idle-timeout eviction

Idle-timeout eviction works in worker mode. A background scheduler looks at every pool twice a second, and when a worker holds a slot that has been idle longer than `idleTimeoutMs`, it wakes that worker. As soon as the worker has no request in flight it destroys every slot of its own that has been idle that long, in every pool, calling `$destroy` for each, and the budget is released at the same point. A worker waiting for its next request does this at once and serves no request for it; one that always has a request in flight — one carrying long-lived streams, say — does it the next time it has none. Once the server has begun its graceful shutdown the eviction no longer runs, so that a fatal error in `$destroy` cannot retire a worker and have the pool boot a replacement in the middle of the drain: a slot that goes stale during the drain stays in the pool like every other idle slot, and the server's stop does not call `$destroy` for it — see [When the destroy callback is skipped](#when-the-destroy-callback-is-skipped).

`$destroy` runs there between requests, on the worker's own PHP engine. What it outputs there, buffered or not, belongs to no response and is discarded, and a shutdown function it registers there is dropped without running. `max_execution_time` does not apply to it, so a callback that never returns stops that worker from serving, and a fatal error in it retires the worker — see [Factory and destroy semantics](#factory-and-destroy-semantics).

Slots held anywhere else are not evicted by `idleTimeoutMs`: they stay until `evict()` is called on the thread holding them, or until that thread or the pool goes away. That covers the slots a task started with `oxphp_async()` acquires and releases, which park on the task's thread, and every pool under `oxphp run`. Without worker mode a pool a request creates closes with that request anyway — see [Outside worker mode](#outside-worker-mode).

Tune `idleTimeoutMs` to the cost of recreation:

- **Cheap to re-mint** (JSON decoder, string pool): set 10_000–60_000 ms; free memory fast when traffic dies down.
- **Expensive to re-mint** (DB connection, TLS session): set 300_000 ms (default) to 900_000 ms; pay recreation cost less often.
- **Never evict**: pass `0`. Idle slots then live until the pool is dropped.

`$pool->evict()` force-evicts **all** idle slots reachable from the calling worker right now — regardless of `idleTimeoutMs` — and returns how many were dropped. It is the operational "flush idle now" escape hatch (e.g. a downstream service restarted and you want the next acquire to mint fresh resources). In-use slots are untouched.

## When the destroy callback is skipped

`$destroy` is called only on the thread that holds the slot, and only while PHP code is running there: every PHP thread has a memory heap of its own, which no other thread may free into, and a callback with no PHP code to return to would turn an exception it throws into a fatal error, or hand it to your exception handler from inside the pool's teardown. Anywhere else the slot is let go without it, and PHP frees the object in it when the request of the thread holding it ends, as it frees every object still alive at that point:

- **Server stop.** The shutdown closes every pool but destroys no slot: it runs on a thread that holds none. In worker mode, to have `$destroy` called for the slots a worker holds, call `evict()` after `oxphp_worker()` returns — the code after it runs on that worker once its loop has ended, the server's stop among the reasons:

  ```php
  $pool = new Pool(factory: ..., destroy: ...);

  oxphp_worker(function () use ($pool) {
      // ... handle the request
  });

  // The loop has ended: hand the slots this worker holds to $destroy.
  $pool->evict();
  ```

  On a pool bound to a name with `Registry::pool()`, the destroy callback belongs to the worker that created the pool, or to the one that last gave it new callbacks, and a worker whose loop ends after that worker's script has ended — at a stop, one that was still finishing a request — lets its slots go without it: see the next case.

- **The request that created the pool has ended.** In worker mode and in tasks started with `oxphp_async()`, once the request that created the pool has begun to shut down, no other thread calls its destroy callback, and the slots other threads destroy go without it — see [When the worker that created a pool exits](#when-the-worker-that-created-a-pool-exits).

- **The end of a request.** A pool whose last reference goes away while its request is ending — once a worker's script has run to its end, say — goes away with no PHP code running, so `$destroy` is not called for the slots of its own thread either. Outside worker mode this is how a pool a request creates ends, unless the request lets go of it earlier — see [Outside worker mode](#outside-worker-mode).
- **Slots another thread holds.** When the pool's last reference goes away on one thread, the idle slots another thread holds — another worker's, or a task's started with `oxphp_async()` — are let go there and then. Their objects stay alive until that thread's request ends, which for a worker or a task's thread is when the thread exits, and until then a connection in one stays open without counting against `maxSize`.
- **A fatal error while a pool goes away.** Once `$destroy` has failed with a fatal error in the teardown of a pool whose last reference went away, no PHP code runs until the error has ended the request, so the slots that teardown had not reached yet — and those of any other pool the same call lets go of — go without it. See [Factory and destroy semantics](#factory-and-destroy-semantics).

Each slot let go in one of these ways, when its pool was given a destroy callback, is counted in `oxphp_shared_pool_destroy_skipped_total` — see [Observability](#observability).

## Budget & acquire semantics

Every acquire variant first tries to satisfy the request immediately — reuse an idle slot, or (if the pool is below `maxSize`) mint a new one via the factory. Only when the pool is **saturated** — no idle slot **and** at `maxSize` — does the behaviour differ:

| State at call time                          | `acquire()`           | `tryAcquire()`     | `acquireTimeout($ms)`                     |
|---------------------------------------------|-----------------------|--------------------|-------------------------------------------|
| Idle slot in the local thread's queue       | reused immediately    | reused immediately | reused immediately                        |
| No idle slot, but below `maxSize`           | factory mints a slot  | factory mints a slot | factory mints a slot                    |
| Saturated (at `maxSize`, all in use)        | waits **forever**     | returns `null`     | waits up to `$ms`, then `OperationTimeoutException` |

Once the request that created the pool has ended, the factory is no longer called, and the second row throws `ClosedException` instead — see [When the worker that created a pool exits](#when-the-worker-that-created-a-pool-exits).

`$ms` must be `> 0`; `0` or negative raises `TypeException`. There is deliberately no float-seconds form and no "infinite" sentinel argument — use the bare `acquire()` for an unbounded wait.

### What a wait holds

In worker mode, and in a task started with `oxphp_async()`, a wait on a saturated pool suspends the fiber running the request or the task, and the thread goes on running the others — the one holding the slot being waited for among them, when it is on the same thread. A suspended wait looks for a free slot again after 1 ms, then after 2 ms, then every 4 ms, so a slot given back on its thread reaches it within a few milliseconds of the thread being free to resume it. Waiters are not served in the order they arrived; the next free slot goes to whichever looks first.

Where a fiber switch is not possible, the wait holds the thread instead, as it does in a request served without worker mode: inside a fiber your code starts itself, with `new Fiber` or through a scheduler such as Revolt or AMPHP, and in the other places where `oxphp_sleep()` waits on the thread — see the note under [Suspension Points](../features/fiber-multiplexing.md#suspension-points). There a slot held by anything else suspended on the same thread — another request, or another fiber of that scheduler — cannot come back while the wait lasts. Unless a slot is destroyed meanwhile, leaving room for a new one — by `evict()` or idle eviction on another thread, for instance — `acquireTimeout()` throws `OperationTimeoutException` once its `$ms` are up, and `acquire()` can wait until the server shuts down. Use the bounded form in such code.

A task cancelled while its wait is suspended — because the side awaiting it gave up or the request that started it ended, for instance — is resumed, and its acquire throws `OxPHP\Async\AsyncException`.

## Exceptions

| Exception                  | Raised by                                                            |
|----------------------------|----------------------------------------------------------------------|
| `OperationTimeoutException` | `acquireTimeout` / `withTimeout` exceeded `$ms` without a free slot. **Extends `Async\AsyncException`, not `SharedException`.** |
| `TypeException`            | Non-positive `maxSize`, negative `idleTimeoutMs`, `$ms <= 0`, or a factory that returned a non-object. |
| `StaleHandleException`     | `Handle::get()` after the handle was released.                       |
| `ClosedException`          | Any acquire on a closed pool (server shutdown); an acquire that has to make a new slot once the request that created the pool has ended — see [When the worker that created a pool exits](#when-the-worker-that-created-a-pool-exits); and outside worker mode any acquire on a pool a request created that is not made by that request while it runs — see [Outside worker mode](#outside-worker-mode). |
| `SharedException`          | `Registry::pool()` called from an HTTP request served without worker mode, and `Registry::global()` there when its factory returns a pool; either of them in a task when the factory returns a pool such a request created — see [Outside worker mode](#outside-worker-mode). |
| `UninitializedException`   | A method call on a pool wrapper that has not finished `__construct`. |
| `Async\AsyncException`     | An acquire in an `oxphp_async()` task that was cancelled while its wait was suspended — see [What a wait holds](#what-a-wait-holds). |

`tryAcquire()` does **not** throw on saturation — it returns `null`. Because `OperationTimeoutException` extends `OxPHP\Async\AsyncException` (not `SharedException`), a `catch (SharedException)` will **not** catch an acquire timeout; use `catch (OxPHP\Async\AsyncException)` or catch `OperationTimeoutException` directly.

**Why this differs from `Mutex::tryWithLock()`.** Both are non-blocking `try*` calls, but `Pool` returns `null` on contention while `Mutex` throws `ContentionException`. The split is structural, not stylistic. `Pool` is **handle-first**: every acquire hands back a `Handle`, so the "saturated" outcome has a natural carrier — `?Handle`, where `null` means "no slot" and never collides with a real value (a `Handle` is never itself a user value). `Mutex` is **closure-only** by design — it deliberately never hands a lock guard back to PHP, so a held lock cannot leak past the closure. That leaves `tryWithLock` with no object to return as nullable, and the closure's own `mixed` result may legitimately be `null` — so `null` cannot double as "not acquired". With neither a handle nor a free sentinel, the only unambiguous contention signal left is an exception. Catch accordingly: `tryAcquire` → test for `null`; `tryWithLock` → `catch (ContentionException)`.

Exceptions thrown inside the factory propagate to the acquire caller unchanged and do not consume budget. Exceptions inside the `with()` / `withTimeout()` body propagate to the caller after the slot is released. A fatal error inside the body ends the request, and the slot goes back to the pool all the same, in whatever state the body left it: as after a throw, except that no `finally` block in the body has run. If a body can be ended halfway through an exchange — a reply not yet read, a transaction still open — start each body by putting the resource back into a known state.

## Observability

See [Shared Observability](shared-observability.md) for the full tour. Quick references:

- `GET /__ox_shared/entry?id=N` exposes `type: "Pool"` and, under `type_specific`, `{ count, in_use, idle, waiting, max_size, idle_by_thread, rebalance_strategy }`.
- `GET /__ox_shared/summary` includes a `Pool` bucket with `count`, `bytes`, and `ops`. Per-pool gauges like `waiting` and the `evicted_total` counter are exposed on `/metrics` (below), not aggregated in the summary.
- Prometheus metrics per pool:
  - `oxphp_shared_pool_count{pool_id="…"}`           — gauge, total slots (in-use + idle).
  - `oxphp_shared_pool_in_use{pool_id="…"}`          — gauge.
  - `oxphp_shared_pool_idle{pool_id="…"}`            — gauge.
  - `oxphp_shared_pool_waiting{pool_id="…"}`         — gauge, queued acquires.
  - `oxphp_shared_pool_acquire_total{pool_id="…",result="ok|timeout|closed|saturated"}` — counter. `saturated` counts non-blocking `tryAcquire` calls that found the pool full (distinct from `timeout`, which means a wait elapsed).
  - `oxphp_shared_pool_evicted_total{pool_id="…",reason="idle_timeout|evict|shutdown"}` — counter.
  - `oxphp_shared_pool_wait_seconds_*{pool_id="…"}`  — acquire-wait histogram (bucket / sum / count).
- Prometheus metric for all pools together: `oxphp_shared_pool_destroy_skipped_total{reason="no_frame|other_thread|request_ended"}` — counter of the slots let go without the destroy callback their pool was given, in the cases listed under [When the destroy callback is skipped](#when-the-destroy-callback-is-skipped).

Alert-worthy combinations: rising `waiting` with flat `count` means the pool is saturated and should be resized; rising `acquire_total{result="timeout"}` with normal `in_use` means the factory is slow (or blocking); rising `acquire_total{result="saturated"}` means callers keep hitting `tryAcquire` on a full pool (backpressure firing).

## When not to use

- **Cheap or immutable resources.** A pool's overhead is larger than re-creating a simple object. Use it for resources where creation costs milliseconds or kilobytes.
- **Objects that cannot be reused safely.** If the resource accumulates per-request state (open transactions, pending reads) and you cannot reliably reset it, pooling leaks state between requests. Return slots to a known state in your request-finishing code, or do not pool.
- **Cross-host resources.** A pool is in-process. For multi-host connection pooling, prefer a connection-bucket service or a sidecar (pgbouncer, proxy-sql).
- **Unbounded fan-out.** If you need one connection per in-flight HTTP call, that is not a pool — that is an N-per-request problem. Use a `Shared\Channel` to serialise work behind a bounded pool instead.
- **Resources with their own pool semantics.** Many client libraries already pool internally (e.g. Guzzle's connection pool). Stacking a `Shared\Pool` on top is double bookkeeping; prefer the library's own pooling.

## Related

- [Shared State](shared-state.md) — overview and mental model.
- [Shared\Once](shared-once.md) — when you need exactly one resource (not a pool of N).
- [Shared\Channel](shared-channel.md) — pair with a pool for producer/consumer pipelines.
- [Shared\Map](shared-map.md) — one `Pool` per tenant keyed by name.
- [Worker Mode](../features/worker-mode.md) — pool handles across requests within one worker thread.
