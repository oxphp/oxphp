---
title: Shared\Channel
description: Bounded MPMC channel shared across PHP workers, with fiber-aware send and recv that return value-typed results for explicit dispatch.
---

# Shared\Channel

`OxPHP\Shared\Channel` is a bounded multi-producer multi-consumer channel that lives in the shared registry and is visible to every PHP worker in the process. Use it when a request handler and a background worker — or two workers — need to exchange work items in FIFO order. Inside a fiber, blocking calls suspend cooperatively so the underlying worker thread stays free for other requests.

## Overview

- **Bounded.** Capacity is fixed at construction. Once full, `send` blocks (or suspends a fiber); `trySend` reports the result without waiting.
- **MPMC.** Any number of senders and receivers across threads. Delivery is FIFO.
- **Fiber-aware.** Under worker mode with the async pool, blocking variants suspend the fiber instead of the OS thread. In traditional mode they block the OS thread.
- **Result-typed.** Every send/recv returns a [`Channel\SendResult`](#api-reference) / [`Channel\RecvResult`](#api-reference) — closed / full / timeout are normal outcomes for fan-out dispatchers, so they appear as result variants instead of exceptions.

## API Reference

```php
namespace OxPHP\Shared;

final class Channel implements Shareable, \Countable
{
    public function __construct(int $capacity);

    // ── Receive ──
    public function tryRecv(): Channel\RecvResult;                  // non-blocking
    public function recv(): Channel\RecvResult;                     // block forever / fiber-cancel
    public function recvTimeout(int $ms): Channel\RecvResult;       // bounded ($ms > 0)

    // ── Send ──
    public function trySend(mixed $value): Channel\SendResult;
    public function send(mixed $value): Channel\SendResult;
    public function sendTimeout(mixed $value, int $ms): Channel\SendResult;

    // ── Batch ──
    public function sendMany(array $values, int $ms): int;          // partial count, no throw
    public function recvMany(int $max, int $ms): array;             // partial array, no throw

    // ── Lifecycle ──
    public function close(): bool;
    public function isClosed(): bool;
    public function count(): int;
    public function id(): int;
}

namespace OxPHP\Shared\Channel;

enum RecvStatus { case Ok; case Empty; case Timeout; case Closed; }
enum SendStatus { case Ok; case Full;  case Timeout; case Closed; }

final class RecvResult {
    public function isOk(): bool;
    public function isEmpty(): bool;
    public function isTimeout(): bool;
    public function isClosed(): bool;
    public function value(): mixed;                // throws SharedException if not Ok
    public function valueOr(mixed $default): mixed;
    public function status(): RecvStatus;
}

final class SendResult {
    public function isOk(): bool;
    public function isFull(): bool;
    public function isTimeout(): bool;
    public function isClosed(): bool;
    public function status(): SendStatus;
}
```

## Wait policies

Every direction has three method variants — the suffix encodes the wait policy:

| Suffix       | Behaviour                                                            |
|--------------|----------------------------------------------------------------------|
| `try*`       | Non-blocking. Reports `Empty` / `Full` if the call cannot proceed.   |
| (bare name)  | Block forever, or until the request fiber is cancelled.              |
| `*Timeout`   | Bounded wait. `$ms` must be `> 0` — use one of the other forms for forever or non-blocking. |

`$ms` is **always a positive integer of milliseconds** for `recvTimeout` / `sendTimeout` / `sendMany` / `recvMany`. Zero, negative, non-int, and absent values raise `OxPHP\Shared\TypeException` at the bridge — that constraint moved out of the method body so the trichotomy is self-documenting.

## Result dispatch

`RecvResult` and `SendResult` carry a `status()` discriminant plus (for `RecvResult` only) a payload. Two equivalent idioms:

```php
use OxPHP\Shared\Channel;
use OxPHP\Shared\Channel\RecvStatus;

$ch = new Channel(capacity: 64);

// Boolean accessors — terse when only one outcome matters.
$result = $ch->tryRecv();
if ($result->isOk()) {
    process($result->value());
} elseif ($result->isEmpty()) {
    backoff();
} else {
    break; // closed
}

// Exhaustive match — exhaustiveness checking catches new variants.
$r = $ch->recvTimeout(1500);
match ($r->status()) {
    RecvStatus::Ok      => process($r->value()),
    RecvStatus::Timeout => $logger->debug('idle'),
    RecvStatus::Closed  => break,
    RecvStatus::Empty   => /* unreachable: only tryRecv returns Empty */ ,
};

// Safe accessor — never throws.
$value = $ch->tryRecv()->valueOr('fallback');
```

`RecvResult::value()` throws `OxPHP\Shared\SharedException` if called on a non-Ok variant — use `isOk()` / `valueOr()` / `status()` first. This is intentional: it surfaces the bug of acting on a missing payload as a loud failure rather than a silent `null`.

## Fiber vs blocking behaviour

The same method calls behave differently depending on whether PHP is currently running inside a fiber:

- **Inside a fiber** (worker mode + `oxphp_async(...)`): the blocking variants (`send`, `recv`, `sendTimeout`, `recvTimeout`) allocate a synthetic promise, register a waker with the channel, and suspend the fiber. The worker thread goes back to the scheduler and processes other fibers until the channel notifies the waker.
- **Outside a fiber** (traditional mode, or a non-async call path): the blocking variants block the OS worker thread via `crossbeam_channel`. No other work runs on that thread until the call returns.

Traditional mode still gets the channel semantics — it simply pays with a blocked thread. Worker mode is the recommended deployment for any pipeline that relies on waiting.

```php
<?php
// Traditional mode: this recvTimeout blocks the worker thread for up to 2 seconds.
$ch = new OxPHP\Shared\Channel(16);
$r = $ch->recvTimeout(2000);

// Worker mode: wrap in oxphp_async and recv suspends cooperatively.
oxphp_worker(function () use ($ch) {
    $consumer = oxphp_async(function () use ($ch) {
        for (;;) {
            $r = $ch->recvTimeout(5000);
            if ($r->isOk()) { process($r->value()); continue; }
            if ($r->isClosed()) { break; }
            // Timeout — keep waiting.
        }
    });
    oxphp_async_await($consumer);
});
```

Fiber cancellation (request abort, deadline elapsed at the SAPI level) still surfaces as an `OxPHP\Async\AsyncException` — it is **not** translated into `RecvResult::Closed`. The channel didn't close; the runtime cancelled you. Re-raise or catch as appropriate.

## Close semantics

`close()` is idempotent — calling it a second time is a no-op. After close:

- `send`, `sendTimeout`, `trySend` return `SendResult::Closed`.
- `sendMany` returns the count actually accepted before the close (`0` if already closed).
- `recv`, `recvTimeout`, `tryRecv` continue to drain buffered items as `RecvResult::Ok($value)`, then return `RecvResult::Closed` once empty.
- `recvMany` returns the items it can drain, possibly an empty array.
- `isClosed()` returns `true`.
- Blocked senders wake with `SendResult::Closed`; blocked receivers wake with `RecvResult::Closed`.

```php
<?php
$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->close();

// Drain leftovers.
for (;;) {
    $r = $ch->recv();
    if ($r->isClosed()) { break; }
    echo $r->value(), "\n"; // one, two
}

// Further sends report Closed without throwing.
$result = $ch->send('three');
assert($result->isClosed());
```

The pattern for a graceful pipeline shutdown is: producers stop, producer side calls `close()`, consumers drain in a `for (;;) { $r = $ch->recv(); if ($r->isClosed()) break; … }` loop and exit naturally. Either consumer never observes a `null` payload by mistake — the variant carries that signal.

## Shutdown drain

When the OxPHP process shuts down, the `OxPHP\Shared` registry calls `close()` on every entry, including channels. From PHP's perspective this looks identical to an explicit `close()`:

- Blocked `recv*` calls return `RecvResult::Closed`.
- Blocked `send*` calls return `SendResult::Closed`.

> Always check the result variant. A caller that assumes `recv()->value()` is always safe will throw `SharedException` at shutdown or whenever another holder closes the channel.

## Batched operations

`sendMany` and `recvMany` exist for pipelines that move items in groups. Prefer them when you routinely handle 10+ items at a time: each batch is one FFI round trip instead of N, which meaningfully cuts per-item overhead in throughput-bound loops.

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

// Send an array; returns the count actually accepted.
$sent = $ch->sendMany([1, 2, 3, 4, 5], 100);  // 5 within 100ms budget

// Drain up to 10 items with a 100ms deadline.
$batch = $ch->recvMany(10, 100);
```

Semantics worth noting:

- Both batch methods **return partial results** on timeout or mid-batch close — never an exception. Check the integer / array length to detect partial progress.
- `sendMany` on an already-closed channel returns `0`.
- `recvMany` on closed+empty returns `[]`.
- `$ms` must be `> 0`. There is no "drain whatever is buffered" overload — call `tryRecv()` in a tight loop if that pattern matters; we'll add a dedicated batch variant when a real callsite asks.

## Observability

The internal server (default `INTERNAL_ADDR=127.0.0.1:9090`) exposes channels in the generic shared-registry endpoints:

- **`GET /__ox_shared/summary`** includes a `Channel` bucket with count, bytes, ops, and `pending_total`.
- **`GET /__ox_shared/entries?type=Channel`** lists channel entries with their registry IDs.
- **`GET /__ox_shared/entries/:id`** returns per-channel state: `capacity`, `count`, `pending` *(deprecated alias of `count`)*, `closed`, `senders_blocked`, `receivers_blocked`.

Prometheus exposition on `/metrics`:

```text
oxphp_shared_channel_count{channel_id="<id>"}               gauge
oxphp_shared_channel_pending{channel_id="<id>"}             gauge (deprecated, alias of _count)
oxphp_shared_channel_senders_blocked{channel_id="<id>"}     gauge
oxphp_shared_channel_receivers_blocked{channel_id="<id>"}   gauge
oxphp_shared_channel_items_sent_total{channel_id="<id>"}    counter
oxphp_shared_channel_items_dropped_total{channel_id="<id>"} counter
```

`items_dropped_total` increments for the tail of a partial `sendMany` that could not fit.

## Common patterns

### HTTP producer, async consumer

```php
<?php
$work = new OxPHP\Shared\Channel(256);

$consumer = oxphp_async(function () use ($work) {
    for (;;) {
        $r = $work->recvTimeout(30000);
        if ($r->isOk()) { process_job($r->value()); continue; }
        if ($r->isClosed()) { break; }
        // Timeout — health-check, then keep waiting.
    }
});

oxphp_worker(function () use ($work) {
    $work->send(['url' => $_POST['url'], 'tries' => 3]);
    echo "queued";
});
```

### Fan-out across multiple consumers

Spawn N async consumers on the same channel; the registry ensures exactly one of them receives each item.

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

for ($i = 0; $i < 4; $i++) {
    oxphp_async(function () use ($ch, $i) {
        for (;;) {
            $r = $ch->recvTimeout(60000);
            if ($r->isOk()) { handle($i, $r->value()); continue; }
            if ($r->isClosed()) { break; }
        }
    });
}
```

### Bounded pipeline with backpressure

`trySend` plus a drop counter lets a producer shed load rather than block under overload:

```php
<?php
if ($ch->trySend($event)->isFull()) {
    increment_dropped_metric();
}
```

## Pitfalls

- **`recvTimeout(0)` is a TypeException, not "non-blocking".** Use `tryRecv()` for non-blocking. The dedicated method names removed the timeout-overload ambiguity that the old `?float $timeout` had.
- **Values must be shareable.** Scalars, `null`, and nested arrays of shareables are allowed. Passing an object that is not a `Shared\*` instance raises `TypeException` on send.
- **Clone is forbidden.** `clone $channel` throws `SharedException`; transfer the channel through a closure `use` instead — `oxphp_async(function () use ($ch) { ... })` — so both sides see the same registry entry.
- **`value()` on non-Ok throws.** This is a feature, not a bug — it converts the "I forgot to check isOk" mistake into a loud failure. Use `valueOr($default)` if a default is genuinely fine.
- **Fiber cancellation propagates as an exception**, not a Result variant. A `recv()` interrupted by request cancellation raises `OxPHP\Async\AsyncException`. Channels that close themselves still produce `RecvResult::Closed`.
- **Cancelled waiters with in-flight payloads.** If many fibers wait on `send` / `recv` and are cancelled while their payload was about to cross, the payload can stay referenced until the next wake. Keep waiter counts bounded (e.g. cap concurrency with a `Shared\Counter` or channel-capacity semaphore).

## Migrating from the previous API

| Was                                                          | Is                                                                    |
|--------------------------------------------------------------|-----------------------------------------------------------------------|
| `$ch->tryRecv()` → `null` on empty, throws `ClosedException` | `$ch->tryRecv()` → `RecvResult::Empty` / `Closed`                      |
| `$ch->recv($secs)` → `null` on timeout/close                 | `$ch->recv()` (forever) / `$ch->recvTimeout($ms)` → `RecvResult`        |
| `$ch->trySend($v): bool`                                     | `$ch->trySend($v): SendResult`                                          |
| `$ch->send($v, $secs): bool` (throws TimeoutException/ClosedException) | `$ch->send($v)` / `$ch->sendTimeout($v, $ms)` → `SendResult`  |
| `$ch->sendMany($vs, $secs)` (throws TimeoutException on partial) | `$ch->sendMany($vs, $ms): int` (partial count, no throw)            |
| `?float $timeout` (seconds, NaN/INF tolerated)               | `int $ms` (milliseconds, must be `> 0`)                                |

## Related features

- [Worker Mode](worker-mode.md) — prerequisite for fiber-suspending blocking variants.
- [Async Promises](async-promises.md) — the `oxphp_async()` closure is the normal way to hand a `Channel` to a background fiber.
- [Fiber Multiplexing](fiber-multiplexing.md) — explains how suspension keeps the worker thread productive while channel operations wait.
