# Changelog

All notable changes to OxPHP are documented in this file.

## [0.6.0] - 2026-05-28

### Migration from 0.5.0

Mechanical replacements unless noted otherwise. Apply with grep/sed before upgrading.

**1. `oxphp_async_await_any()` — name kept, semantics replaced**

The function under that name now follows JS `Promise.any`: returns the first FULFILLED promise, throws `AggregateAsyncException` only when every promise rejects. The previous "first settled, success or failure" behavior moved to `oxphp_async_await_race()`.

```php
// If you wanted "first response, regardless of success":
$winner = oxphp_async_await_race([$p1, $p2], 5.0);

// If you wanted "first SUCCESS, ignore failures": same call, new failure shape:
try {
    $winner = oxphp_async_await_any([$p1, $p2], 5.0);
} catch (\OxPHP\Async\AggregateAsyncException $e) {
    // every promise rejected
} catch (\OxPHP\Async\TimeoutException $e) {
    // deadline elapsed before any fulfilled
}
```

**2. Cancelled-request HTTP status no longer collapses to `500`**

The wire status now reflects the cancel reason: `max_execution_time` exhaustion → `504`, graceful-drain shutdown → `503` with `Retry-After: 5`, mid-request client disconnect → `499` (log-only, not on the wire). Supervisor kills (`Stuck`) and userland cancels (`UserCancel`) keep `500`.

- Replace any monitoring/log pattern that maps `500` to "timeout" with `504` (or, more robustly, the `oxphp_request_cancelled_total{reason}` metric).
- If you ship a custom `ERROR_PAGES_DIR`, add `504.html`, `503.html`, and optionally `499.html` next to `500.html`.
- `5xx` rate SLOs will drop after rollout because `499` is no longer 5xx — this is honest improvement, not a regression.

**3. `Shared\Counter` — `inc`/`dec`/`addBatch`/`reset` removed**

`Counter::inc()`, `Counter::dec()`, `Counter::addBatch()`, and `Counter::reset()` were removed. `inc()`/`dec()` collapse into `add(int $delta = 1)` (`add()` adds 1, `add(-1)` decrements); `addBatch($deltas)` becomes `add(array_sum($deltas))`; `reset()` becomes `set(0)`, which — like the old `reset()` — returns the previous value. `get()`, `set()` (atomic exchange, returns the previous value), `compareAndSet()`, and `id()` keep their 0.5.0 signatures. The behavioural changes: `add()` gains a default delta of `1`, and every operation is now `Relaxed` rather than `SeqCst` — a Counter is a statistics accumulator, not a synchronisation point — use `Shared\Atomic` (with an explicit `Ordering`) when a counter must synchronise other memory or run a CAS that publishes other state.

```php
// Before (0.5.0)
$c->inc();
$c->inc(5);
$c->dec();
$c->addBatch([1, 2, 3]);
$prev = $c->reset();

// After
$c->add();
$c->add(5);
$c->add(-1);
$c->add(array_sum([1, 2, 3]));
$prev = $c->set(0);
```

**4. `OxPHP\Shared\*` — unified method naming**

| Before                          | After                       |
| ---                             | ---                         |
| `$ch->pending()`                | `$ch->count()`              |
| `$pool->size()`                 | `$pool->count()`            |
| `$flag->test()`                 | `$flag->isSet()`            |
| `$map->setIfAbsent($k, $v)`     | `$map->trySet($k, $v)`      |

`Map`, `Channel`, and `Pool` now also implement `\Countable`, so
`count($map)`, `count($ch)`, `count($pool)` work without calling the
method directly. The rationale and the rules for new methods live in
[`docs/en/shared-state/shared-naming.md`](docs/en/shared-state/shared-naming.md).

**5. `OxPHP\Server\Worker` — drop the `get` prefix**

| Before                       | After                     |
| ---                          | ---                       |
| `$w->getId()`                | `$w->id()`                |
| `$w->getStartTime()`         | `$w->startTime()`         |
| `$w->getRequestCount()`      | `$w->requestCount()`      |
| `$w->getMemoryUsage()`       | `$w->memoryUsage()`       |
| `$w->getRss()`               | `$w->rss()`               |
| `$w->getMaxMemoryBytes()`    | `$w->maxMemoryBytes()`    |
| `$w->getExitReason()`        | `$w->exitReason()`        |

`Worker::current()`, `Worker::isWorkerMode()`, `scheduleExit()`, `isExitScheduled()`, and `serve()` are unchanged.

**6. Base exception class renames**

```php
// Before
catch (\OxPHP\Async\Exception $e) { ... }
catch (\OxPHP\Shared\Exception $e) { ... }

// After
catch (\OxPHP\Async\AsyncException $e) { ... }
catch (\OxPHP\Shared\SharedException $e) { ... }
```

Subclasses (`TimeoutException`, `BorrowException`, `ClosedException`, …) keep their names — only the parent FQN changes.

**7. `oxphp_request_heartbeat()` → `set_time_limit()`**

```php
// Before
oxphp_request_heartbeat(30);

// After
set_time_limit(30);
```

Both reset the per-request timer to N seconds from now.

**8. `REQUEST_TIMEOUT_SECONDS` → `max_execution_time`**

```ini
; php.ini (or oxphp.ini)
max_execution_time = 30
```

Or per-script: `set_time_limit(30);`. Drop `REQUEST_TIMEOUT_SECONDS` from your deployment manifest.

**9. `sapi` key removed from `oxphp_server_info()`**

```php
// Before
$sapi = oxphp_server_info()['sapi'];  // hardcoded "oxphp", lied about the real SAPI

// After
$sapi = php_sapi_name();              // "cli-server"
```

**10. `Shared\Once` — `getOrInit()`, `status()`, and a failure policy**

`init()` is renamed to `getOrInit()`. `isInitialized(): bool` is removed in favour of `status(): Once\Status` (`Uninitialized`/`Pending`/`Ready`/`Poisoned`). `get()` now throws `UninitializedException` on an unset or in-flight cell instead of returning `null`, so a stored `null` is a real value distinguishable via `status()`. A failed `getOrInit()` factory is retryable by default; opt into permanent failure with `new Once(onFactoryError: Once\FailureMode::Poison)`, after which value methods throw `PoisonedException`. `trySet()` now accepts arrays and nested `Shareable` values, not only scalars.

```php
// Before
$o = new OxPHP\Shared\Once();
$v = $o->init(fn () => compute());
if ($o->isInitialized()) { $cached = $o->get(); }

// After
$o = new OxPHP\Shared\Once();
$v = $o->getOrInit(fn () => compute());
if ($o->status() === OxPHP\Shared\Once\Status::Ready) { $cached = $o->get(); }
```

**11. `Shared\Flag` — redesigned as the bool twin of `Shared\Atomic`**

`isSet()` / `set()` / `clear()` / `exchange()` are removed. Flag now mirrors `Shared\Atomic`: `load` / `store` / `swap` / `compareAndSet`, each taking an optional `Ordering` (default `SeqCst`). `swap` returns the previous value; `store` returns `void`.

```php
$f->isSet();               // → $f->load()
$f->set();                 // → $f->store(true)   (or $f->swap(true) for the prior value)
$f->clear();               // → $f->store(false)  (or $f->swap(false) for the prior value)
$f->exchange($new);        // → $f->swap($new)
$f->compareAndSet($e, $n); // unchanged (now also accepts optional $success / $failure Ordering)
```

### Breaking changes

- **`Shared\Channel` and `Shared\Mutex` adopt a trichotomous wait-policy API.** The single overloaded `?float $timeout` argument was replaced with three explicit methods per direction — `try*` for non-blocking, the bare verb for forever, and `*Timeout(int $ms)` for bounded — and the return shape moved from mixed/null/bool to value-typed Result classes (Channel) or exception-style (Mutex). No alias shims. Mechanical migration:

  | Was | Is |
  |---|---|
  | `$ch->trySend($v): bool` | `$ch->trySend($v): SendResult` (`isOk` / `isFull` / `isClosed`) |
  | `$ch->send($v, ?float $timeout = null)` (throws `TimeoutException` / `ClosedException`) | `$ch->send($v): SendResult` (forever) / `$ch->sendTimeout($v, int $ms): SendResult` |
  | `$ch->tryRecv(): mixed` (`null` on empty, throws on closed) | `$ch->tryRecv(): RecvResult` (`isOk` / `isEmpty` / `isClosed`; `value()` / `valueOr($d)`) |
  | `$ch->recv(?float $timeout = null): mixed` (`null` on timeout or closed) | `$ch->recv(): RecvResult` (forever) / `$ch->recvTimeout(int $ms): RecvResult` |
  | `$ch->sendMany($vs, ?float $timeout = null): int` (throws `TimeoutException` on partial) | `$ch->sendMany($vs, int $ms): int` (partial count, no throw on timeout/close) |
  | `$ch->recvMany($max, ?float $timeout = null): array` | `$ch->recvMany($max, int $ms): array` |
  | `$m->with($fn, ?float $timeout = null): mixed` | `$m->withLock($fn): mixed` / `$m->withLockTimeout($fn, int $ms): mixed` |
  | `$m->tryWith($fn): mixed` (`null` on contention) | `$m->tryWithLock($fn): mixed` (throws `ContentionException`) |
  | `$m->isPoisoned()`, `$m->clearPoison()` | removed; PHP throws no longer corrupt the mutex |

  Timeout parameters on the `*Timeout` methods are `int $ms (> 0)` in milliseconds, not `?float $seconds` — zero, negative, non-int, and absent values raise `OxPHP\Shared\TypeException` (use `try*` or the bare verb for those policies). The Channel `RecvResult` `value()` accessor throws `OxPHP\Shared\SharedException` if called on a non-Ok variant; use `isOk()` / `valueOr()` / `status()` to dispatch. The Mutex closure signature changed from `function ($value): mixed` (return-to-commit) to `function (&$value): mixed` (by-ref mutation; the return value becomes the caller's return value). `Shared\TimeoutException` is removed — `OperationTimeoutException` (now under `Async\AsyncException`) replaces it for `withLockTimeout` and the Pool-saturated path; `Shared\ClosedException` remains registered but is deprecated and only thrown by the still-unmigrated `Shared\Pool`; `Shared\PoisonedException` is now a first-class part of the redesigned `Shared\Once` (its `Poison` failure mode) and is no longer deprecated. `Shared\DeadlockException` is reparented from `Shared\TimeoutException` to `Async\AsyncException`, so a single `catch (Async\AsyncException)` now sweeps every concurrency outcome across Shared\* and Async\*.
- `Shared\Counter` reshaped to a minimal accumulator: `inc()`, `dec()`, `addBatch()`, and `reset()` were removed in favour of `add(int $delta = 1)` (covers increment and decrement) and `set(0)` (windowed reset, returns the previous value). `get()`, `set()`, `compareAndSet()`, and `id()` are retained with their 0.5.0 signatures; `add()` gains a default delta of `1`. All operations switched from `SeqCst` to `Relaxed` — a Counter is statistics, not a synchronisation point; use `Shared\Atomic` (with an explicit `Ordering`) to synchronise other memory, run an ordered CAS, or store arbitrary atomic int state.
- `oxphp_async_await_any(array, ?float): array` was renamed to `oxphp_async_await_race(array, ?float): array`. The implementation is unchanged — first settled (success or failure) wins, as before. If your code relied on this behavior, replace the function name in-place.
- `OxPHP\Shared\*` method naming unified across types. The renames below are mechanical (semantics and signatures unchanged), and ship without alias shims — update call sites with sed before upgrading. The rules are documented at [`docs/en/shared-state/shared-naming.md`](docs/en/shared-state/shared-naming.md).
  - `Channel::pending()` → `Channel::count()`
  - `Pool::size()` → `Pool::count()`
  - `Flag::test()` → `Flag::isSet()`
  - `Map::setIfAbsent($key, $value)` → `Map::trySet($key, $value)`

### Added

- `OxPHP\Shared\Channel\RecvResult` and `OxPHP\Shared\Channel\SendResult` — value-typed returns for the new Channel API. `RecvResult` accessors: `isOk`, `isEmpty`, `isTimeout`, `isClosed`, `value` (throws `SharedException` on non-Ok), `valueOr($default)`, `status(): RecvStatus`. `SendResult` is payload-free: `isOk`, `isFull`, `isTimeout`, `isClosed`, `status(): SendStatus`. Closed / full / timeout are normal outcomes for fan-out dispatchers, so they appear as result variants instead of exceptions on the hot path.
- `OxPHP\Shared\Channel\RecvStatus` and `OxPHP\Shared\Channel\SendStatus` — unbacked enums for exhaustive `match` dispatch on the Result discriminant.
- `OxPHP\Shared\OperationTimeoutException` (extends `OxPHP\Async\AsyncException`) — thrown by `Mutex::withLockTimeout` and `Pool::acquire` on deadline expiry. Cross-plugin parent makes a single `catch (Async\AsyncException)` sweep both Shared\* timeouts and Async\* await timeouts.
- `OxPHP\Shared\ContentionException` (extends `OxPHP\Async\AsyncException`) — thrown by `Mutex::tryWithLock` when the lock is held.
- `OxPHP\Shared\CorruptedMutexException` (extends `OxPHP\Shared\SharedException`) — thrown on every subsequent `Mutex::withLock*` call after a prior Rust panic crossed the FFI boundary inside the closure. Sticky, non-recoverable — discard the instance and create a new one.
- `Shared\Atomic` — generic int64 atomic primitive. Methods: `load`, `store`, `swap`, `compareAndSet`, `fetchAdd`, `fetchSub`, `fetchAnd`, `fetchOr`, `fetchXor`. Each accepts an optional `Shared\Ordering` parameter (default `Ordering::SeqCst`). `fetch*` returns the previous value (Rust convention), in deliberate contrast to `Counter::add` which returns the new value.
- `Shared\Ordering` — backed-int enum with `Relaxed`, `Acquire`, `Release`, `AcqRel`, `SeqCst`. Maps one-to-one to `std::sync::atomic::Ordering`.
- `Shared\InvalidOrderingException` (extends `Shared\SharedException`) — thrown when an `Atomic` operation receives a memory ordering invalid for that operation (e.g. `store(Ordering::Acquire)`, `compareAndSet(_, _, _, Ordering::Release)`).
- `oxphp_async_await_any(array, ?float): array` now exists with proper JavaScript `Promise.any`-style semantics: the first FULFILLED promise wins. Rejections are accumulated. If every promise rejects, throws the new `OxPHP\Async\AggregateAsyncException` carrying all errors (`getErrors()`, `getErrorMap()`, `getPromiseIds()`). On timeout, throws `OxPHP\Async\TimeoutException` with `getPartialErrors()` and `getCancelledPromiseIds()` populated.
- `OxPHP\Async\AggregateAsyncException` (extends `AsyncException`) — new exception class. Methods: `getErrors(): list<\Throwable>` (positional, keyed 0..N-1 by input position), `getErrorMap(): array<int, \Throwable>` (keyed by promise id), `getPromiseIds(): list<int>`.
- `OxPHP\Async\TimeoutException::getPartialErrors(): array<int, \Throwable>` and `getCancelledPromiseIds(): list<int>` — new methods. Existing throw sites (`oxphp_async_await()`, `oxphp_async_await_all()`, `oxphp_async_await_race()`) populate them with empty arrays; only `oxphp_async_await_any()` timeouts fill them. The cancelled-id list is an audit trail — those promises have already been signalled to cancel and their receivers stranded, so they cannot be re-awaited.
- `OxPHP\Shared\Map`, `OxPHP\Shared\Channel`, and `OxPHP\Shared\Pool` now `implements \Countable`. `count($map)`, `count($channel)`, and `count($pool)` work directly without calling the `->count()` method. For `Pool` the count covers total live slots (in-use + idle).
- Naming guide for `OxPHP\Shared\*` published at `docs/en/shared-state/shared-naming.md`. New `Shared\*` primitives must follow the rules listed there (`get`/`load` for reads, `set`/`store` for writes, `count()` via `\Countable`, `is*` for boolean getters, `try*` for non-blocking attempts, `fetch*` for atomic RMW returning prev value).
- `OxPHP\Shared\Once\Status` (unbacked enum: `Uninitialized`, `Pending`, `Ready`, `Poisoned`) and `OxPHP\Shared\Once\FailureMode` (backed-int enum: `Reset = 0`, `Poison = 1`). `Once::status(): Once\Status` reports the cell's state and never throws; `Once::getOrInit(callable): mixed` is the canonical race-free get-or-init (it replaces `init()`). `Once::__construct` takes `Once\FailureMode $onFactoryError = Reset` to choose retryable-vs-terminal factory-failure behaviour.
- `OxPHP\Shared\Registry` — name-keyed process-global handles for every `Shared\*` type. `Registry::map($key, $factory)`, `Registry::counter($key, $factory)`, etc. (one method per type, plus an untyped `Registry::global($key, $factory)` escape hatch) bind a `Shared\*` entry under a string key so every worker thread and every request that reaches the same key converges on the same entry. The factory runs at most once per successful bind (block-losers across worker threads; reentrancy from inside its own factory throws `Shared\DeadlockException`). Named entries are pinned for process lifetime; `Registry::remove($key): bool` drops the binding (the underlying object survives while any handle holds it, and the next typed call under the same key creates a NEW entry — documented namespace-management semantics). `Registry::keys(): array` lists currently-bound keys. `Registry::memoryUsage(): int` and `Registry::count(): int` report the whole Shared\* layer (estimate, not RSS; transient — `count() != count(keys())`). Closes the gap where the `new Shared\*()` bootstrap pattern produced per-worker instances rather than one shared entry; in traditional mode it also gives same-host APCu-style cross-request persistence.

### Removed

- `OxPHP\Shared\TimeoutException` class. The exception was a `SharedException` sibling thrown by `Channel::send`/`sendMany`, `Mutex::with`/`withLock` timed variants, and `Pool::acquire`. The new replacement is `OxPHP\Shared\OperationTimeoutException` (now under `OxPHP\Async\AsyncException`); Channel's `*Timeout` methods return `RecvResult::Timeout` / `SendResult::Timeout` instead of throwing. `catch (OxPHP\Shared\TimeoutException)` clauses must be updated — there is no `class_alias` shim.
- `Shared\Mutex::isPoisoned()`, `Shared\Mutex::clearPoison()`. Public poison observability and recovery were removed: the underlying behaviour they exposed never gave the caller useful work to do (corruption is now sticky and always a server bug; PHP throws no longer corrupt the lock at all). Catch `OxPHP\Shared\CorruptedMutexException` and discard the instance instead.
- `Shared\Mutex::with($fn, ?float $timeout)` and `Shared\Mutex::tryWith($fn)`. Use `withLock` / `withLockTimeout($fn, int $ms)` / `tryWithLock` instead.
- `Shared\Channel::send($v, ?float $timeout)`, `Channel::recv(?float $timeout)`, `Channel::sendMany(..., ?float $timeout)`, `Channel::recvMany(..., ?float $timeout)`. The float-seconds timeout parameter is gone everywhere on Channel. Use `sendTimeout($v, int $ms)` / `recvTimeout(int $ms)` for bounded waits, and pass the bare verbs (`send($v)` / `recv()`) for forever. The batch methods now take a mandatory `int $ms (> 0)` and return partial results without throwing on timeout or mid-batch close.
- `REQUEST_TIMEOUT_SECONDS` env var. Use `max_execution_time` in `php.ini` (or `set_time_limit($seconds)` per script) instead.
- `oxphp_request_heartbeat($time)` PHP function. Use `set_time_limit($seconds)` instead — both reset the per-request timer to N seconds from now.
- `oxphp_bridge_set_deadline` / `_get_deadline` / `_is_deadline_expired` C exports from the bridge.
- `tokio::time::timeout` wrapping of the dispatch future. SIGALRM-driven `max_execution_time` is now the single execution-timeout source.
- `Shared\Once::init()` (renamed to `getOrInit()`) and `Shared\Once::isInitialized()` (replaced by `status(): Once\Status`). No alias shims — update call sites before upgrading.
- **BREAKING:** `sapi` key from the array returned by `oxphp_server_info()`. The key used to hardcode `"oxphp"`, contradicting `php_sapi_name()` which reports `"cli-server"` (the real SAPI module name, kept that way for OPcache compatibility). Callers reading `$info['sapi']` will now get `null`. Use `php_sapi_name()` to get the SAPI identifier directly.

### Changed

- Execution-timeout cancellation now bails through the unified `Request cancelled (timeout)` error instead of `Maximum execution time of N second(s) exceeded`. Userland-visible state (`connection_status() & PHP_CONNECTION_TIMEOUT`, registered shutdown handlers) is preserved.
- **BREAKING:** Cancelled requests no longer collapse to a single `500`. The wire status now reflects the cause: `max_execution_time` / `set_time_limit()` exhaustion → **`504 Gateway Timeout`**; graceful-drain cancellation → **`503 Service Unavailable`** with a `Retry-After: 5` header (userland-set `Retry-After` wins); client closed the connection mid-request → **`499`** (nginx-style "Client Closed Request", visible only in access logs and metrics — never written to the wire); supervisor-initiated kills (`Stuck`) and userland-initiated cancels (`UserCancel`) keep returning `500`. Anything that pattern-matched `500` to detect timeouts must switch to `504` (or, more robustly, the `oxphp_request_cancelled_total{reason}` metric). Operators with a custom `ERROR_PAGES_DIR` should add `504.html`, `503.html`, and optionally `499.html` next to their existing `500.html`. `ClientAbort` moving out of `5xx` will improve generic `5xx`-rate SLOs after rollout — this is honest improvement (these were never server errors), but called out so the SLO drop isn't mistaken for a regression.
- **BREAKING:** `OxPHP\Server\Worker` instance methods dropped the `get` prefix to match the rest of the public PHP API (which uses noun-style accessors like `Request::method()`, `Request::headers()`). Renames: `getId()` → `id()`, `getStartTime()` → `startTime()`, `getRequestCount()` → `requestCount()`, `getMemoryUsage()` → `memoryUsage()`, `getRss()` → `rss()`, `getMaxMemoryBytes()` → `maxMemoryBytes()`, `getExitReason()` → `exitReason()`. No `__call` shim — call sites must be updated. `Worker::current()`, `Worker::isWorkerMode()`, `scheduleExit()`, `isExitScheduled()`, and `serve()` are unchanged.
- **BREAKING:** Renamed base exception classes to remove shadowing of PHP's global `\Exception` inside the `OxPHP\Async\` and `OxPHP\Shared\` namespaces: `OxPHP\Async\Exception` → `OxPHP\Async\AsyncException`, `OxPHP\Shared\Exception` → `OxPHP\Shared\SharedException`. Subclasses (`TimeoutException`, `BorrowException`, `ClosedException`, etc.) keep their names; only their parent FQN changes. No `class_alias` shim — any `catch (\OxPHP\Async\Exception $e)` or `catch (\OxPHP\Shared\Exception $e)` clauses must be updated to the new names.
- `OxPHP\Shared\*` timeout convention unified. Every wait method now takes `?float $timeout = null`: `null` waits forever, `0.0` is a non-blocking try, positive values are seconds, `INF` is forever, `NaN` and negative values raise `OxPHP\Shared\TypeException`. `Mutex::__construct` no longer accepts `$defaultTimeout`. `Pool::__construct` no longer accepts `$defaultAcquireTimeout`; pass the timeout at the call site (`acquire()` / `with()`). `Channel::tryRecv()` no longer accepts an argument and is non-blocking, matching `trySend()` (this only fixes the stub — the implementation never accepted the argument). Blocking methods on Mutex, Pool, and `Channel::send` / `sendMany` raise `TimeoutException` on deadline expiry; `Channel::recv` and `recvMany` instead return `null` / a partial array on timeout, intentionally asymmetric with send. Pool's `idleTimeout` lifecycle parameter is unchanged.
- Repository Dockerfile layout reorganized to separate "how the official image is built" from "how to use the image in your project":
  - `Dockerfile` → `docker/dev/Dockerfile` (used by `compose.yml`).
  - `Dockerfile.alpine-release` → `docker/release/alpine/Dockerfile` (used by CI to publish `ghcr.io/oxphp/oxphp`). The `alpine/` subdirectory leaves room for future `docker/release/debian/`, `docker/release/distroless/` variants.
  - `Dockerfile.best.example` → `examples/dockerfile/Dockerfile` (copy-and-adapt template for downstream users; also adds a sibling `README.md`).
  No `Dockerfile*` remains in the repo root, so a stray `docker build .` no longer accidentally kicks off the dev build. Update any tooling that referenced the old paths.
- `ox_shared.metrics_enabled` is now an actual runtime opt-out for per-entry operation counters on `Shared\*` primitives. Previously the flag was inert on the `record_op` path — per-entry counters incremented regardless of the setting, and only the registry's coarse aggregate metrics responded to it. Now, when `metrics_enabled = false`, `Entry::ops` stays at `0` and introspection snapshots (`OxPHP\Shared\introspect()`, `oxphp_shared_*` debug exports) report `0` for per-entry op counts. Operators who were reading per-entry `ops` values while running with `metrics_enabled = false` will see `0` instead of the previously-incrementing approximate count; switch the flag back to `true` (the default) to restore the prior behaviour.
- `Shared\*` memory accounting now books per-entry storage-chain overhead (`Arc<Entry>`, DashMap shard bucket, allocator prologues — ~200 B per entry) and propagates container growth (`Map::set`, `Channel::send`, `Pool::try_reserve_budget`) into the registry's `total_bytes` gauge. Previously `mem_bytes()` for scalar types (`Atomic`, `Counter`, `Flag`) reported only the inner content (~8–16 B) and container types froze the value at insert time — operators who relied on `OX_SHARED_MAX_BYTES` as a hard cap saw real RSS exceed the configured limit by ~12× for scalar-heavy workloads and arbitrarily for Map/Channel growth. **Operator action**: a worker that previously sustained ~6M `Shared\Atomic` entries under `OX_SHARED_MAX_BYTES=128MiB` will now top out around ~600K. Either raise the cap to match the previous structural budget (e.g. `≈1.6GiB`) or rely on the orchestrator-level memory limit (cgroups / k8s `resources.limits.memory`) and treat `OX_SHARED_MAX_BYTES` as a grace cap. The accounted bytes still drift ±10–30% vs `mallinfo` — the constant is a structural estimate of the storage chain, not a heap-profiler measurement.
- `OxPHP\Shared\*::id()` is now seeded from `getrandom` at registry start, so the value returned by `$shared->id()` is a large opaque number instead of the previous `1, 2, 3, …` monotonic sequence. The id remains stable for the lifetime of the entry within the process, the documented `$a->id() === $b->id()` identity test is unchanged, and the value continues to address the `/__ox_shared/preview?id=…` and `/__ox_shared/entries/:id` observability endpoints. **Operator action**: code that *parses* an id (regex-matched it, range-checked `< N`, treated it as an insertion-order proxy, or persisted it outside this process expecting it to resolve elsewhere) will need to stop — the id is a per-process opaque token, not a stable handle. The wire format on tag-7 cross-thread transfer is unchanged (`u64`). On the rare path where `getrandom` is blocked (seccomp/sandbox), the registry falls back to the legacy monotonic counter and logs a `WARN`.
- **BREAKING:** `Shared\Once::get()` now throws on a cell that is not `Ready` — `UninitializedException` when empty or while a factory is in flight (`Pending`), `PoisonedException` when a `Poison`-mode factory previously failed — instead of returning `null`. A stored `null` is therefore a real value, distinguishable from "not set" via `status()`. `trySet()` now accepts the full value range (arrays and nested `Shareable`), not just scalars, and throws `PoisonedException` on a poisoned cell. Factory-failure behaviour is selected at construction: `Reset` (default) returns the cell to `Uninitialized` so a later `getOrInit()` retries, `Poison` makes it terminally `Poisoned`; in both modes the factory's exception is re-thrown to the current caller. The per-instance observability JSON at `/__ox_shared/entry?id=N` now reports `status` (`uninitialized`/`pending`/`ready`/`poisoned`) instead of inferring a boolean `initialized` from a non-null snapshot, fixing a mislabel of cells storing `null`.

### Deprecated

- `PHP_DENY_DIRS` env var renamed to `PHP_DENY_PATHS` to reflect that values are glob patterns and may match individual `.php` files, not only directories. The legacy name remains accepted as an alias and emits a startup `WARN`; when both are set, `PHP_DENY_PATHS` wins and `PHP_DENY_DIRS` is reported as ignored. The alias will be removed in a future release — switch to `PHP_DENY_PATHS` in your environment and orchestration configs.
- `SHARED_SHUTDOWN_TIMEOUT_SECONDS` env var (and its `OX_SHARED_SHUTDOWN_TIMEOUT_SECONDS` alias) is deprecated and ignored. The setting never gated anything: `SharedRegistry::drain()` is synchronous — `Shared\Channel` and `Shared\Pool` wake blocked waiters via `close()` and return immediately; `Map`, `Mutex`, `Counter`, `Flag`, `Atomic`, and `Once` never block. The overall graceful-shutdown deadline is owned at server level by `DRAIN_TIMEOUT_SECONDS` (default `30s`), which waits on the connection-drain loop in `main.rs` long enough for woken PHP requests to unwind and flush. The `SharedConfig::shutdown_timeout_seconds` field is removed in this release; the env-var aliases are still accepted (with a startup `WARN`) for one release cycle and will be removed afterwards. Tune `DRAIN_TIMEOUT_SECONDS` instead.
- `OxPHP\Shared\*` observability names trailing the renamed PHP API are emitted as deprecated aliases alongside the new names: Prometheus `oxphp_shared_channel_pending` (use `oxphp_shared_channel_count`) and `oxphp_shared_pool_size` (use `oxphp_shared_pool_count`); JSON keys `Channel.pending` and `Pool.size` at `/__ox_shared/entries/:id` (use `.count`). The deprecated `# HELP` lines are tagged so dashboards picking the metric up via help-text discovery surface the migration hint. A startup `WARN` from the `ox_shared` plugin announces the dual emission whenever introspection or metrics are enabled. The deprecated aliases will be removed in a future release — update Grafana panels, Prometheus alert rules, and any JSON consumers before upgrading. **Scrape sizing note:** during the deprecation window each `Shared\Channel` and `Shared\Pool` emits one extra gauge line (`_pending` plus `_count`, `_size` plus `_count`) carrying the same value as its canonical counterpart, so the contribution of these series to `/metrics` doubles for the duration. The extra cardinality is `1 × N_channels + 1 × N_pools` and disappears when the aliases are removed.

### Performance

- Reduced per-call overhead of `Shared\*` primitive operations: the PHP wrapper now holds the registry entry directly, so the global shared-map lookup is gone from every call. Earlier in this cycle the per-call lookup count was halved (two → one) when the per-entry op counter stopped re-resolving the entry; this change removes the remaining one. The optimisation is unconditional and applies whether `ox_shared.metrics_enabled` is on or off. On a 14-core development host the per-op hot path is now within criterion noise of a raw atomic load — at 8 threads the geomean ratio between the previous and the new shape is approximately 4.7×, with the largest wins on contended read-only ops (`Atomic::load`, `Flag::isSet` — renamed from `test` later in this cycle, `Once::status` — replacing `isInitialized` later in this cycle); the improvement is expected to be larger on 32–64-core hosts where the DashMap shard lock dominated.

### Fixed

- `Request::startTime(true)` and `oxphp_server_info()['request_time']` now agree across all SAPI modes and lifecycle phases. Both return `0.0` when no HTTP request is being processed — during worker boot (top-level code in the entry script before `oxphp_worker()` enters its receive loop) and between requests in worker mode — and the request start timestamp during request handling. Previously the worker-mode field leaked the worker thread's spawn time during boot and the previous request's timestamp between requests, while traditional mode left it set to the last finished request after `php_request_shutdown`. Code that reads either API outside an active request (boot-phase initialization, async callbacks running between requests) will now observe `0.0` instead of a misleading non-zero value. OPcache and other RSHUTDOWN consumers of `sapi_get_request_time()` still see a valid timestamp because the field is reseated to the current wall-clock time immediately before the worker's final `php_request_shutdown`.
- SSE / streaming: `connection_aborted()` now correctly returns `true` after the client disconnects mid-stream, matching standard PHP / php-fpm semantics. Previously the flag stayed `false` for streaming responses, so portable loops like `while (!connection_aborted()) { echo ...; flush(); }` could only terminate via implicit bailout instead of breaking out cleanly through their `finally` blocks. Mid-stream disconnects are now also detected on the next flush via the streaming channel — previously only the early-response oneshot was probed, which had already been consumed when streaming started, so disconnect detection was effectively disabled for the lifetime of the stream.
- `OTEL_TRACES_SAMPLER_ARG` invalid or out-of-range values are now clamped to `[0.0, 1.0]` and logged at warn level, per the OpenTelemetry specification. Previously, parse errors silently fell back to `1.0` and out-of-range values (e.g. `2.5`, `-1`) were passed through to the SDK unchecked. A typo such as `OTEL_TRACES_SAMPLER_ARG=o.1` (letter `o` for `0`) now surfaces a warning instead of silently turning 10 % sampling into 100 %.
- Unknown `OTEL_TRACES_SAMPLER` values now emit a warn log identifying the offending value, instead of silently defaulting to `parentbased_traceidratio`. The fallback sampler is unchanged.

## [0.5.0] - 2026-05-05

Headline work since `v0.4.0`: a **canonical entry-script + worker-mode model** (`ENTRY_FILE` / `WORKER_MODE_ENABLED` retiring `INDEX_FILE` / `WORKER_FILE`), a new `OxPHP\Server\Worker` class for runtime introspection and application-driven recycling via `Worker::scheduleExit()`, strict parsing of boolean and `STATIC_MAX_AGE` env vars, and a clearer `STATIC_MAX_AGE` / `STATIC_REVALIDATE` rename for the static-file cache.

### Added

- New PHP class `OxPHP\Server\Worker` — unified runtime handle for worker introspection. Methods: `current`, `isWorkerMode`, `getId`, `getStartTime`, `getRequestCount` (1-based count of requests handled by this OS thread; grows in both modes since traditional reuses persistent threads), `getMemoryUsage`, `getRss`, `getMaxMemoryBytes`, `scheduleExit`, `isExitScheduled`, `getExitReason`, `serve`. Available in both traditional and worker modes. See `docs/en/php/worker-class.md`.
- New PHP exception `OxPHP\Server\Exception\InvalidServeContextException`, thrown by `Worker::serve()` when called outside worker mode.
- `Worker::scheduleExit()` — application-driven worker recycling. Marks the current worker for graceful exit after the active request completes; the supervisor respawns a fresh worker, re-running the outer scope. Companion methods `Worker::isExitScheduled()` and `Worker::getExitReason()` expose the pending exit state. No-op in traditional mode.
- Environment variables `ENTRY_FILE` and `WORKER_MODE_ENABLED` — single canonical entry script plus an explicit worker-mode toggle. `ENTRY_FILE` selects the routing mode by extension (unset = direct mapping, `*.php` = front controller, non-`.php` = SPA fallback). When `WORKER_MODE_ENABLED=true`, `ENTRY_FILE` must point at a `.php` script and the server runs persistent workers. Resolution accepts relative paths (against `DOCUMENT_ROOT`, including `..`) and absolute paths. The startup `mode_decided` log line records which combination was selected. See `docs/en/operations/configuration.md`.

### Changed

- `oxphp_worker_recycles_by_reason_total{reason="max_requests"}` Prometheus label is renamed to `reason="scheduled"` to reflect that the recycle reason is now driven by `Worker::scheduleExit()` instead of an automatic request counter.
- `/config` endpoint now reports `entry_file` and `worker_mode_enabled` in place of `index_file`, `worker_file`, and the synthetic `worker_mode` boolean.
- Static file cache environment variables renamed for clarity: `STATIC_CACHE_TTL` → `STATIC_MAX_AGE` (the value is the `Cache-Control: max-age` it sets), and `STATIC_CACHE` → `STATIC_REVALIDATE` with the polarity flipped (`STATIC_REVALIDATE=on` enables mtime revalidation; previously `STATIC_CACHE=off` did the same thing). Defaults are unchanged: 30 days `max-age`, no revalidation. `/config` reports `static_max_age` and `static_revalidate` in place of `static_cache_ttl` and `static_cache_enabled`.
- **BREAKING:** `STATIC_MAX_AGE` (and the deprecated `STATIC_CACHE_TTL`) are now strictly parsed: garbage values like `STATIC_MAX_AGE=garbage` fail at startup with an error naming the variable, where they previously silently fell back to a missing `Cache-Control` header. Empty assignments (`STATIC_MAX_AGE=`) and unset variables still fall back to the default (30 days), matching the bool-parser policy.
- **BREAKING:** boolean environment variables are now strictly parsed against a fixed canonical set (`on`/`true`/`1`/`yes` for truthy, `off`/`false`/`0`/`no` for falsy, case-insensitive and trimmed). Any non-empty value outside that set — including typos like `ture` — fails fast at startup with an error naming the variable, rather than silently defaulting. An unset variable or empty assignment (`FOO=`) falls back to the default; this matches Docker Compose / Kubernetes substitution like `FOO=${FOO}` when the host variable is missing. Affected variables: `WORKER_MODE_ENABLED`, `STATIC_REVALIDATE`, `TRACE_CONTEXT`, `SUPERGLOBALS_ENABLED`, `SHARED_ENABLED`, `SHARED_METRICS_ENABLED`, `SHARED_INTROSPECTION_ENABLED`, `SHARED_INTROSPECTION_PREVIEW_ENABLED`, `SHARED_POISON_STRICT`, `PROFILER_ENABLED`, `PROFILER_INTERNAL`, `PROFILER_EXPORT_XHGUI`. The legacy `STATIC_CACHE` compatibility shim remains intentionally lenient (only `off` enables revalidation, anything else disables). Audit any deployment that relied on non-canonical bool values like `enabled` — these now refuse to start.

### Deprecated

- Environment variable `WORKER_MAX_REQUESTS` — parsed and ignored; emits a `WARN` log line at startup if set. Migrate to `WORKER_MAX_MEMORY_MIB` for safety-net recycling, or to `Worker::scheduleExit()` for application-driven recycling. Will be removed entirely in a subsequent release.
- Environment variables `INDEX_FILE` and `WORKER_FILE` — still parsed for backwards compatibility; emit a `WARN` log line at startup and map onto the new model: `INDEX_FILE=...` ≡ `ENTRY_FILE=...`, and `WORKER_FILE=...` ≡ `WORKER_MODE_ENABLED=true ENTRY_FILE=...`. When both legacy and new variables are set, the new ones win and the warning still fires. The legacy forms will be removed in a subsequent release.
- Environment variables `STATIC_CACHE_TTL` and `STATIC_CACHE` — still parsed for backwards compatibility; emit a `WARN` log line at startup and map onto the new model: `STATIC_CACHE_TTL=...` ≡ `STATIC_MAX_AGE=...`, and `STATIC_CACHE=off` ≡ `STATIC_REVALIDATE=on`. When both legacy and new variables are set, the new ones win and the warning still fires. The legacy forms will be removed in a subsequent release.

### Internal

- New benchmark tooling under `scripts/`: `bench-wrk.sh` (one-shot wrk runner against a configurable target) and `sweep-config.sh` (matrix sweep over `TOKIO_WORKERS` × `PHP_WORKERS` for tuning). Not wired into CI; local-only.
- Bump dependencies for the post-`0.4.0` cycle: `opentelemetry` / `opentelemetry_sdk` / `opentelemetry-otlp` / `opentelemetry-semantic-conventions` `0.27 → 0.31`, `tonic` `0.12 → 0.14` (now requires the explicit `grpc-tonic` feature on `opentelemetry-otlp`), `rand` `0.8 → 0.10`, `getrandom` `0.2 → 0.4`, `reqwest` `0.12 → 0.13`, `brotli` `7 → 8`, `lru` `0.12 → 0.18`. No user-visible behavior change; OTel migration switches to `SdkTracerProvider` with `with_batch_exporter`, `Resource::builder()`, and the new `force_flush()` `Result` shape, and `rand`/`getrandom` call sites move to the `Rng::random` / `getrandom::fill` APIs.

## [0.4.0] - 2026-05-02

Headline work since `v0.3.0`: **PHP 8.5 support** (opt-in via `:php8.5*` tags, `latest` still resolves to 8.4 pending soak), and a chain of fiber / `Shared\*` / streaming bugfixes that surfaced once worker-mode `Channel`/`Map`/`Pool` traffic and `oxphp_async()` workers got real exercise.

### Added

- PHP 8.5 support. Build pipeline now produces `:php8.5`, `:php8.5-alpine{X.Y}`, and patch-pinned `:0.X.Y-php8.5.Z-alpine{X.Y}` tags alongside the existing 8.4 ones. `latest` and unsuffixed `{ver}` continue to resolve to PHP 8.4 in this release; the default flip to 8.5 lands in a follow-up release after a soak window. To opt in early, pull `:php8.5` (or any `*-php8.5*` variant).
- `SAPI_HEADER_DELETE_PREFIX` support (PHP 8.5.6+) — `header_remove('X-Foo-')` now strips every previously-set header whose `"{name}: {value}"` line case-insensitively starts with the given prefix, matching upstream SAPI behavior. PHP < 8.5.6 keeps the existing exact-match semantics.

### Fixed

- Streaming responses on the traditional executor losing chunked `Transfer-Encoding` after the first `oxphp_stream_flush` on a worker. The bridge per-request context (`stream_mode` / `headers_sent` / `finished`) is `__thread`-local and was leaking across requests; the traditional path now resets it before each request to match worker mode.
- `oxphp_bridge_in_fiber()` misreporting fiber context — both on the main thread (where PHP seeds `EG(current_fiber_context)` to `EG(main_fiber_context)` at request startup) and inside a user-level `Fiber::start()` body (which installs its own `zend_fiber_context` distinct from main). The latter caused `Shared\Channel::recv()` / `send()` inside a user Fiber to take the fiber-suspend path, hit rc=1 from `oxphp_bridge_fiber_await` ("not in oxphp fiber"), and surface as `RuntimeException: recv: fiber_await rc=1`. The predicate now keys off the SAPI's private `oxphp_current_fiber` __thread pointer via a registered callback (`oxphp_bridge_set_in_fiber_check`), which is the only authoritative source for "is this thread inside an oxphp scheduler fiber". User fibers correctly fall through to the thread-blocking branch.
- `DELETE /__profiler/runs/{id}` panicking the worker with "cannot block from within a runtime". The internal HTTP server dispatches sync handlers from inside hyper's async service, so the index-lock acquire switched from `blocking_lock()` to a `try_lock` retry loop with a 5 s deadline that degrades to `503` on real contention.
- Plugin-defined classes, interfaces, enums, and free functions advertised no parameter names, so PHP named-argument syntax (e.g. `$ch->send('x', timeout: 0.1)`) failed with "Unknown named parameter" and `ReflectionParameter::getName()` returned an empty list. The bridge method/function registration now carries per-parameter name/type/optional arrays, and the SAPI extension synthesizes a full `zend_internal_arg_info` array with real names instead of an unnamed return-only stub.
- `OxPHP\Shared\Channel`, `Shared\Map`, and `Shared\Pool` handles arriving as `null` on the receiving worker thread when captured by an `oxphp_async()` closure (via `use ($var)` or as a variadic arg). The cross-thread wrapper rebuild in the C bridge listed only `Counter`/`Flag`/`Once`/`Mutex` in its tag→class switch, so the other three `SharedType` variants fell through to `default` and produced `IS_NULL`. The mapping now lives only in Rust (`SharedType::php_class_cstr`) and the C bridge calls a weak-linked `oxphp_shared_class_name(type_tag)` export, so adding a `SharedType` variant cannot drift again. Re-enables the `shared/test_channel_fiber_*` worker-mode suites.

### Internal

- `src/php/bindings.rs` split into `src/php/bindings/{common.rs, v8_4.rs, v8_5.rs}` with cfg-selected per-version modules. `build.rs` detects the linked PHP via `php-config --vernum` (or `PHP_VERSION_ID` env override).
- `release.yml` gains a `php-suite` pre-publish gate running a focused subset of `tests/run_all.sh` (`headers, cookies, get_post_request, input, pathinfo, errors`) against both 8.4 and 8.5 images before manifest creation. Failure aborts the publish entirely.
- `weekly-rebuild.yml` gains skip-if-unchanged via upstream digest annotation comparison plus the same focused subset suite job, gated on whether any matrix cell actually rebuilt.

## [0.3.0] - 2026-04-22

Headline work since `v0.2.0`: **shared state for PHP workers without Redis** (seven `OxPHP\Shared\*` primitives), a per-request **PHP profiler**, **APM auto-instrumentation**, trusted-proxy and Kubernetes integrations for production deployments, security-header hardening, and a cosign-signed parametrized Docker image matrix.

### Added

#### `OxPHP\Shared\*` primitives

See the [Shared State overview](docs/en/shared-state/shared-state.md) for the concept and mental model, and the per-type docs for API reference, runnable examples, and gotchas.

- [`Shared\Counter`](docs/en/shared-state/shared-counter.md) — atomic int64 with `inc` / `dec` / `add` / `compareAndSet` / `addBatch` / `reset`.
- [`Shared\Flag`](docs/en/shared-state/shared-flag.md) — atomic bool with `test` / `set` / `clear` / `exchange` / `compareAndSet`.
- [`Shared\Once`](docs/en/shared-state/shared-once.md) — run-once container with `init(factory)` / `trySet` / `get`. Reentrant `init` throws `DeadlockException`.
- [`Shared\Mutex`](docs/en/shared-state/shared-mutex.md) — poisoning mutex guarding a stored value. `with(callable, timeout)` and `tryWith(callable)` scope-guard the critical section; poisoning isolates failed-mid-update state.
- [`Shared\Channel`](docs/en/shared-state/shared-channel.md) — bounded MPMC queue with fiber-aware `send` / `recv`. `sendMany` / `recvMany` for batching.
- [`Shared\Map`](docs/en/shared-state/shared-map.md) — concurrent `string → mixed` store with `get` / `set` / `update` / `getOrSet` / `setIfAbsent` / batched `setMany` / `getMany` / `removeMany`. Per-instance cap via `maxEntries`.
- [`Shared\Pool`](docs/en/shared-state/shared-pool.md) — bounded object pool with lazy factory, optional destroy callback, strict `maxSize` budget, per-thread affinity, and idle-timeout eviction. `with($body)` scope-guards acquire/release.

#### Shared-registry observability

See [Shared Observability](docs/en/shared-state/shared-observability.md) for the operator's reference.

- Internal-server endpoints: `/__ox_shared/summary`, `/entries`, `/entry?id=…`, `/preview?id=…`, `/types`, `/graph?id=…` for live registry introspection.
- Prometheus metrics under `oxphp_shared_*` — aggregate-per-type (`objects_total`, `operations_total`, `bytes`, `capacity_saturation`) plus per-instance for Channel / Map / Pool.
- Cross-thread deadlock detector — `oxphp_shared_deadlock_detected_total` ticks when the wait-for scanner finds a mutex cycle.
- Shared `preview` previews are gated behind `SHARED_INTROSPECTION_PREVIEW_ENABLED` so production deployments can disable value exposure without losing shape counts.

#### Profiling

- Per-request PHP profiler (`plugin-profiler` feature — now part of the default Cargo feature set; `-DOXPHP_WITH_PROFILER=1` propagated to both C build stages) with four output formats: xhprof, speedscope, pprof, collapsed.
- PHP SDK: `OxPHP\Profile\{start, stop, pause, resume, mark, metric, is_active}` functions.
- Seven PHP attributes: `#[Profile]`, `#[Exclude]`, `#[Sample]`, `#[Tag]`, `#[Mark]`, `#[SlowThreshold]`, `#[MemoryThreshold]`.
- Trigger modes: cookie (`OXPROF=<token>`), header (`X-OxPHP-Profile: <token>`), query (`?__oxprof=<token>`), and statistical (`PROFILER_SAMPLE_RATE`).
- In-memory LRU cache (`PROFILER_RETENTION_COUNT`) + disk retention with background trimmer (5-second cadence, atomic rename).
- Token-bucket disk write rate limiting (`PROFILER_DISK_MAX_PER_SEC`).
- HTTP push (`PROFILER_EXPORT_URL`) with 3× exponential backoff retry, 5 s wallclock cap, bearer-token auth, xhgui envelope auto-detect.
- Internal HTTP routes at `/__profiler/` — list, metadata, raw format download, speedscope redirect, DELETE, config, stats — with optional bearer-token auth and path-traversal revalidation.
- Prometheus metrics: `oxphp_profiler_runs_total{source}`, `spans_collected_total`, `bytes_written_total{format}`, `disk_drops_total`, `http_push_failures_total`, `truncated_total`, `in_memory_runs`.
- `xhgui` Docker test profile demonstrating the full push → mongo → xhgui UI flow.
- Per-locale documentation at `docs/{en,ru,zh}/features/profiling.md`.

#### APM & tracing

- APM plugin (`plugin-apm`) with auto-instrumentation, PHP tracing SDK, and error capture.
- Plugin PHP builder API for registering Rust-backed PHP functions and classes from plugins. Async and APM subsystems migrated from the C extension into Rust plugins.
- Return-type support in the C builder API for plugin methods.

#### HTTP & routing

- **Trusted proxy support** via `TRUSTED_PROXIES` — accepts trusted-proxy CIDR list (or `private`), processes RFC 7239 `Forwarded` and `X-Forwarded-*` headers, and overrides `REMOTE_ADDR`, `HTTPS`, `REQUEST_SCHEME`, `SERVER_NAME`, `SERVER_PORT` for PHP using the rightmost-non-trusted algorithm.
- **Kubernetes health probes** at `/readyz` and `/livez` with graceful-shutdown awareness.
- `PATH_INFO` splitting via `SPLIT_PATH_INFO_ENABLED` — nginx/PHP-FPM-style front-controller routing.
- `PHP_DENY_DIRS` env var to block `.php` execution in specified paths.
- Dot-path access blocked by default, with an RFC 8615 `.well-known` exception.

#### Security headers

- `X-Content-Type-Options: nosniff` on all responses.
- Configurable `X-Frame-Options` (`FRAME_OPTIONS`, default `SAMEORIGIN`) for clickjacking protection.

#### Operations

- CLI argument parsing: `--help`, `--version`, `--config --check`.
- Startup errors emitted as structured JSON logs (previously plain text).
- Docker `HEALTHCHECK` wired into `compose.yml`.

#### Supply chain & packaging

- Parametrized Docker image matrix: two Dockerfiles (dev + `Dockerfile.alpine-release`) sharing `ARG PHP_VERSION` / `ARG ALPINE_VERSION` / `ARG BASE_IMAGE`.
- Canonical minor-floating (`{ver}-php{minor}-alpine{alpine}`) and patch-pinned tags published to `ghcr.io/oxphp/oxphp`, plus aliases (`php{minor}`, `latest`, etc.).
- cosign-signed release images via GitHub OIDC.
- Weekly rebuild workflow re-publishes canonical tags with fresh upstream PHP patches and re-signs.
- Prod image now ships `php` CLI, `docker-php-ext-install`, `phpize`, and `www-data` out of the box (was bare alpine in 0.2.0).

#### Testing

- PHP integration test suite — 186 tests across 21 groups and 12 Docker profiles, covering apm, async, errors, framework, pathinfo, ratelimit, TLS, timeout, worker and more.

#### Configuration

All Shared-state tunables are read at startup via the `SHARED_*` env-var prefix (fallbacks to `OX_SHARED_*` and bare keys). See [Shared State → Configuration](docs/en/shared-state/shared-state.md#configuration) for the full table. Highlights:

- `SHARED_MAX_ENTRIES` (default 100 000) / `SHARED_MAX_BYTES` (default 1 GiB) — global caps.
- `SHARED_CYCLE_DETECT_DEPTH` (16) / `SHARED_CYCLE_DETECT_EDGES` (10 000) — cycle-check walker bounds.
- `SHARED_INTROSPECTION_ENABLED` / `SHARED_METRICS_ENABLED` — per-feature kill switches.
- `SHARED_LOCK_DIAGNOSTICS` (`off` / `warn` / `strict`) — escalates reentry / deadlock signals.

#### Rust plugin-author API

- `MapInner::retain<F>` — exposes `DashMap::retain` with proper refcount release for nested `SharedValue::Shared` targets. Lets plugin authors prune a map in a single shard-walk instead of the N-lock `keys()`+`remove()` pattern.

#### Documentation

- [`docs/en/shared-state/shared-state.md`](docs/en/shared-state/shared-state.md) — overview, mental model, type-selection matrix, canonical hand-rolled-counter → `Shared\*` migration example.
- Per-type docs for all seven Shared\* v1 types (see list above).
- [`docs/en/shared-state/shared-observability.md`](docs/en/shared-state/shared-observability.md) — introspection endpoints, Prometheus catalogue, diagnostic playbooks.
- [`docs/en/shared-state/migrating-to-external-store.md`](docs/en/shared-state/migrating-to-external-store.md) — when and how to promote `Shared\*` state to Redis / NATS / Kafka.

#### Tooling

- `tests/soak/pool_soak.sh` + `tests/soak/workload.php` — manual (non-CI) 24h soak harness for pre-release Shared\Pool stability sign-off. Not wired into `tests/run_all.sh`; [invocation notes in the observability doc](docs/en/shared-state/shared-observability.md#long-running-soak-harness).

### Changed

- **Routing refactored** into per-mode modules with a performance and behavior overhaul.
- **Request latency reduced across all stack layers** — hot-path allocations, routing, and response assembly.
- `oxphp_request_heartbeat($time)` now also resets PHP's own `max_execution_time` timer to `$time` seconds alongside the server-side deadline. Previously only the server deadline was extended, so long-running scripts could still be killed by Zend's "Maximum execution time exceeded" fatal even after a heartbeat. Scripts that opted out of the PHP timer via `set_time_limit(0)` or `max_execution_time=0` are left alone — the heartbeat does not re-enable a disabled timer.
- Welcome page redesigned as a minimal "is running" status page.
- **Prod image `USER` policy**: `Dockerfile.alpine-release` no longer sets a final `USER` — matches `nginx:alpine` / `php-fpm:alpine` / `frankenphp:alpine` conventions. Deployments drop privileges at the orchestrator level (`docker run --user www-data`, Compose `user:`, Kubernetes `runAsUser`). `chown www-data:www-data /var/www/html` still runs at build time.
- SAPI executor split into per-file modules; worker pool hot path tightened.
- Decorator registry migrated from `unsafe static mut` to `OnceLock`.
- Legacy plugin modules removed in favor of `ox_*` rewrites.
- PHP worker config parsing centralized in `Config`.
- Hyper updated to 1.9; unused `serde` dependency dropped.

### Breaking Changes

- **Async namespace migration**: all async-related PHP classes moved under `OxPHP\Async\`:
  - `OxPHP\AsyncException` → `OxPHP\Async\Exception`
  - `OxPHP\AsyncTimeoutException` → `OxPHP\Async\TimeoutException`
  - `OxPHP\AsyncBorrowException` → `OxPHP\Async\BorrowException`
  - `OxPHP\BorrowedProxy` → `OxPHP\Async\BorrowedProxy`
- Async functions (`oxphp_async`, `oxphp_async_await`, etc.) are now provided by the `plugin-async` feature flag. Without it, the functions are not available. Function names are unchanged.
- **Plugin API**: `Plugin::shutdown` now takes `&mut self` (was `&self`). Plugin authors must update implementations.
- **Plugin config**: `env::set_var` side-effects from plugin init no longer propagate to the core server — plugins must publish core-relevant flags through the explicit core-flags API.
- **`RequestComplete` event**: string-serialized metadata replaced with typed fields.

### Performance

- `Shared\Pool` acquire/release uncontested hot path: **≤ 5 µs gate, ~0.9 µs observed in Docker**. Per-thread affinity keeps slots hot in the acquiring thread without cross-thread handoff.
- Map `set` / `get` path avoids serialisation for nested `Shareable` refs — the refcount-bump retain path is cycle-checked before any mutation, so rejected inserts leak nothing.
- Request path: fewer allocations and clones across routing, response assembly and hot-path dispatch.

### Fixed

- Pool chaos reclaim: in-flight slot counts are refunded when a SAPI worker thread panics mid-acquire, so a crashing worker no longer silently burns budget in the surviving workers' view.
- Cross-thread `Shared\*` access no longer depends on the `worker_liveness` hook for Map / Counter / Flag / Once / Mutex — only Pool uses thread-registration (for its affinity + reclaim paths).
- Async worker SIGBUS from cross-thread `MAP_PTR` access.
- `headers_list()` returning empty — the header handler now returns `SAPI_HEADER_ADD`.
- `payload()` returning null for JSON body after a PDO query reused the request buffer.
- `SecurityHeadersHandler` env-variable race: `FRAME_OPTIONS` is now resolved at startup rather than read per request.
- Decorator `RejectedException` dispatch and instance-cache collisions across requests.
- `-Wint-to-pointer-cast` warning in bridge `server_context` assignment.
- Default-feature (`php`) compile/clippy errors that were previously masked by the `--no-default-features` CI profile.
- TLS test profile now generates v3 certificates dynamically and the runner supports HTTPS.
- E2E runner `curl_args` parsing no longer strips shell quotes incorrectly.
- Cancelled-task exception class corrected to `OxPHP\Async\Exception`.

## [0.2.0] - 2026-03-27

### Added

#### Async & Concurrency

- Fiber-based request multiplexing in worker mode — concurrent I/O within a single worker thread
- Async promises (`oxphp_async()`) — parallel PHP execution via dedicated thread pool
- Distributed tracing with W3C Trace Context propagation and OpenTelemetry export

#### PHP API

- HTTP Object API (`OxPHP\Http\Request`) with lazy bridge accessors
- HTTP interfaces (`OxPHP\Http\RequestInterface`, `OxPHP\Http\SessionInterface`, `OxPHP\Http\AttributesInterface`) with clone/serialize blocking on request-scoped classes
- Attribute-based decorator system (`oxphp_register_decorator()`, `OxPHP\Decorator\AttributeInterface`) with PHP observer integration

#### Server Variables

- `HTTPS` — set to `"on"` when TLS is active
- `REQUEST_SCHEME` — `"https"` or `"http"` per PHP-FPM/nginx convention
- `DOCUMENT_URI` — alias for `SCRIPT_NAME` for nginx/PHP-FPM compatibility
- `REQUEST_TIME_FLOAT` — request start time with microsecond precision

#### HTTP Compliance

- `Date` header on all HTTP responses per RFC 9110 §6.6.1
- `Content-Type` header on all error responses per RFC 9110

#### Observability

- Request duration histograms, byte counters, and subsystem metrics
- `trace_context` field exposed in `/config` endpoint

#### Static Files

- `STATIC_CACHE=off` mode with mtime-based content cache revalidation via `stat()` checks

### Changed

- Default listen port changed from 8080 to 80 (TLS-aware: defaults to 443 when `TLS_CERT` is set)
- Backpressure response changed from 503 to 529 (Site is overloaded)
- `workers_idle` metric now calculated dynamically during scrape (was always 0 in static pool mode)
- `workers_spawned_total` counter now includes initial worker spawn
- `/config` endpoint now exposes `log_level` and other missing runtime settings

### Fixed

- `SERVER_PROTOCOL` now reflects actual HTTP version (was hardcoded to `HTTP/1.1`) per RFC 3875
- `REQUEST_TIME` now returns request start time (was returning current time)
- IPv6 Host header parsing for `SERVER_NAME` and `SERVER_PORT`
- Request timeout now returns 408 instead of 504 per RFC 9110
- Duplicate `oxphp_response_time_us_total` Prometheus metric removed
- Session state cleanup added to worker soft reset to prevent state leaks between requests
- Missing fiber source files added to alpine-release Dockerfile

## [0.1.0] - 2026-03-08

First public release. OxPHP replaces nginx + PHP-FPM with a single async binary
written in Rust, providing HTTP serving, native PHP execution via custom SAPI,
and built-in observability.

### Core

- Async HTTP/1.1 server built on Hyper + Tokio with graceful shutdown
- Custom PHP SAPI (`oxphp`) with full superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`)
- C bridge library (`liboxphp_bridge.so`) for zero-copy Rust↔PHP communication via direct zval access
- PHP ZTS (Zend Thread Safety) multi-threaded worker pool with bounded queue and 529 backpressure
- Three routing modes: Traditional (direct file mapping), Framework (front controller), SPA (fallback to `index.html`)
- Static file serving with in-memory cache, MIME detection, and HTTP caching (ETag, Last-Modified, 304 responses)
- Brotli compression with configurable quality level (0–11) and minimum size threshold
- TLS support via PEM certificate/key
- PHP 8.4 compatibility

### Worker Mode

- Persistent PHP worker processes (`oxphp_worker()`) that handle multiple requests with soft reset between them
- Early response completion (`oxphp_finish_request()`) for background processing after response is sent
- SSE streaming with real-time chunked delivery (`oxphp_is_streaming()`, `oxphp_stream_flush()`)
- Cooperative timeout and cancellation via `oxphp_request_heartbeat()`
- Worker mode detection (`oxphp_is_worker()`) and introspection (`oxphp_worker_id()`, `oxphp_server_info()`)
- Resilient to `exit`/`die` in PHP 8.4 worker mode

### Worker Pool

- Static pool mode: fixed number of workers (`PHP_WORKERS=N`)
- Dynamic pool mode: auto-scaling between min and max workers (`PHP_WORKERS=MIN:MAX`)
- Auto-detect mode: defaults to CPU/2 workers (`PHP_WORKERS=0`)
- Per-worker memory limits (`WORKER_MAX_MEMORY_MIB`) and request limits (`WORKER_MAX_REQUESTS`)
- Dead worker respawning via health monitor (static) / scale manager (dynamic)
- `catch_unwind` prevents panics from poisoning the channel

### Observability

- Prometheus metrics endpoint (`/metrics`) with request counts, durations, status codes, active connections, queue depth, and worker mode stats
- Health check endpoint (`/health`)
- Runtime configuration endpoint (`/config`)
- Structured JSON access logging with configurable levels (`ACCESS_LOG`: off/error/all)
- Request ID generation (`oxphp_request_id()`) in `{timestamp:08x}{counter:08x}` format
- Structured PHP error logging via `zend_error_cb`

### Security & Limits

- Per-IP rate limiting (`RATE_LIMIT`, `RATE_WINDOW_SECONDS`)
- Header read timeout (`HEADER_TIMEOUT_SECONDS`) and request timeout (`REQUEST_TIMEOUT_SECONDS`)
- Graceful shutdown with drain timeout (`DRAIN_TIMEOUT_SECONDS`)
- Configurable request body limits
- Path traversal protection with canonicalization

### Plugin System

- Plugin trait with lifecycle hooks (init, startup, shutdown)
- Typed event dispatcher with priority ordering at every lifecycle point
- Events: ConnectionAccepted, RequestReceived, RouteResolved, ScriptExecutionComplete, ResponseBuilding, RequestComplete
- Native PHP function registration from plugins (zero-copy zval access, no JSON serialization)
- Plugin context API for handler registration and configuration

### Performance

- mimalloc global allocator for reduced per-alloc latency
- Configurable multi-threaded Tokio runtime (`TOKIO_WORKERS`)
- Route LRU cache for fast path resolution
- Thread-local buffer reuse for server variables
- Single Arc clone per request (reduced from 10)
- OPcache support with correct `sapi_get_request_time()` initialization

### Infrastructure

- Multi-stage Alpine Docker build (`php:8.4-zts-alpine`)
- Multi-platform Docker images (amd64/arm64) published to GHCR
- CI workflows: nightly build, PR checks (fmt, clippy, tests), release tagging
- Best-practice Dockerfile example with separate dev/prod targets
- HTTP QUERY method support (RFC 9110)
- Documentation in English, Russian, Belarusian, and Chinese

### PHP Functions

| Function | Description |
|---|---|
| `oxphp_request_id()` | Current request identifier |
| `oxphp_worker_id()` | Current worker thread ID |
| `oxphp_server_info()` | Server runtime information |
| `oxphp_request_heartbeat()` | Signal liveness for cooperative timeout |
| `oxphp_finish_request()` | Flush response early, continue background work |
| `oxphp_is_worker()` | Whether running in worker mode |
| `oxphp_is_streaming()` | Whether SSE streaming is active |
| `oxphp_stream_flush()` | Flush SSE chunk to client |
| `oxphp_worker(callable)` | Enter persistent worker loop |

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Server listen address |
| `DOCUMENT_ROOT` | `www/public` | Document root path |
| `INDEX_FILE` | `index.php` | Front controller file |
| `PHP_WORKERS` | CPU/2 | Worker pool size (`N`, `MIN:MAX`, or `0`) |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Dynamic pool idle timeout |
| `TOKIO_WORKERS` | CPU/2 | Tokio runtime threads (0 = auto) |
| `QUEUE_CAPACITY` | workers×128 | Bounded queue size |
| `RATE_LIMIT` | `0` (off) | Max requests per window per IP |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window |
| `HEADER_TIMEOUT_SECONDS` | `10` | Header read timeout |
| `REQUEST_TIMEOUT_SECONDS` | `30` | Request execution timeout |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Graceful shutdown drain |
| `COMPRESSION_LEVEL` | `4` | Brotli quality 0–11 (0 = off) |
| `STATIC_CACHE_TTL` | `3600` | Static file Cache-Control max-age |
| `ACCESS_LOG` | `all` | Log level: off/error/all |
| `ERROR_PAGES_DIR` | — | Directory with `{status}.html` pages |
| `TLS_CERT` / `TLS_KEY` | — | TLS certificate/key PEM paths |
| `INTERNAL_ADDR` | `0.0.0.0:9090` | Health/metrics/config endpoint |
| `WORKER_MAX_REQUESTS` | `0` (unlimited) | Max requests per worker before restart |
| `WORKER_MAX_MEMORY_MIB` | `0` (unlimited) | Max worker memory before restart |
| `EXECUTOR` | `sapi` | Executor type: sapi/stub |

[0.6.0]: https://github.com/oxphp/oxphp/releases/tag/v0.6.0
[0.5.0]: https://github.com/oxphp/oxphp/releases/tag/v0.5.0
[0.4.0]: https://github.com/oxphp/oxphp/releases/tag/v0.4.0
[0.3.0]: https://github.com/oxphp/oxphp/releases/tag/v0.3.0
[0.2.0]: https://github.com/oxphp/oxphp/releases/tag/v0.2.0
[0.1.0]: https://github.com/oxphp/oxphp/releases/tag/v0.1.0
