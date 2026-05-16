---
title: Shared\Channel
description: Bounded MPMC channel shared across PHP workers, with fiber-aware send and recv for cooperative producer/consumer pipelines.
---

# Shared\Channel

`OxPHP\Shared\Channel` is a bounded multi-producer multi-consumer channel that lives in the shared registry and is visible to every PHP worker in the process. Use it when a request handler and a background worker — or two workers — need to exchange work items in FIFO order. Inside a fiber, `send` and `recv` suspend cooperatively so the underlying worker thread stays free to process other requests.

## Overview

- **Bounded.** Capacity is fixed at construction. Once full, `send` blocks or suspends; `trySend` returns `false`.
- **MPMC.** Any number of senders and receivers across threads. Delivery is FIFO.
- **Fiber-aware.** Under worker mode with the async pool, `send`/`recv` suspend the fiber instead of blocking the worker thread. In traditional mode they block the OS thread.
- **Registry-backed.** Channels survive request boundaries and are shared by ID. Close propagates to all holders.

## API Reference

```php
namespace OxPHP\Shared;

final class Channel implements Shareable, \Countable
{
    public function __construct(int $capacity);

    public function send(mixed $value, float $timeout = 0.0): void;
    public function trySend(mixed $value): bool;

    public function recv(float $timeout = 0.0): mixed;
    public function tryRecv(): mixed;

    public function close(): void;
    public function isClosed(): bool;
    public function count(): int;

    public function sendMany(array $values, float $timeout = 0.0): int;
    public function recvMany(int $max, float $timeout = 0.0): array;

    public function id(): int;
}
```

| Method        | Use case                                                                             |
|---------------|--------------------------------------------------------------------------------------|
| `send`        | Push one item, waiting (or fiber-suspending) up to `$timeout` for space.             |
| `trySend`     | Push one item without waiting; returns `false` if full or closed.                    |
| `recv`        | Pull one item, waiting up to `$timeout`. Returns `null` on closed+empty or timeout.  |
| `tryRecv`     | Pull one item without waiting; returns `null` when empty; throws when closed+empty.  |
| `close`       | Mark the channel closed. Idempotent. Wakes all blocked senders/receivers.            |
| `isClosed`    | Reports whether the channel has been closed.                                         |
| `count`       | Advisory count of buffered items right now. `Countable` — `count($ch)` works directly. |
| `sendMany`    | Push an array of items; returns how many actually went in before full/closed/timeout.|
| `recvMany`    | Pull up to `$max` items (`0` = drain what is currently buffered without waiting).    |
| `id`          | Numeric registry identifier; useful for logging and observability correlation.       |

## Choosing between send/recv variants

Blocking and non-blocking pairs differ in **what they return versus what they throw**, and the behaviour is deliberately asymmetric.

| Outcome             | `send(v, t)`         | `trySend(v)` | `recv(t)`       | `tryRecv()`           |
|---------------------|----------------------|--------------|------------------|-----------------------|
| Success             | returns `void`       | `true`       | item             | item                  |
| Full / empty, open  | waits up to `t`      | `false`      | waits up to `t`  | `null`                |
| Timeout             | `TimeoutException`   | —            | `null`           | —                     |
| Closed (empty recv) | `ClosedException`    | `false`      | `null`           | `ClosedException`     |
| Closed (still items)| `ClosedException`    | `false`      | returns item     | returns item          |

Two consequences worth memorising:

1. **`recv` never throws on closed+empty.** It returns `null`. Loops must null-check.
2. **`recv` also returns `null` on timeout**, whereas `send` throws `TimeoutException`. If you need to distinguish "nobody sent in time" from "channel shut down", check `isClosed()` after a `null` recv.

```php
<?php
$ch = new OxPHP\Shared\Channel(4);

// Non-blocking probe.
if (!$ch->trySend('job-1')) {
    // Queue is full; drop, retry, or apply backpressure.
}

// Blocking send with a deadline.
try {
    $ch->send('job-2', timeout: 1.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    // No consumer picked up within 1s.
} catch (OxPHP\Shared\ClosedException $e) {
    // Channel was closed while we were waiting.
}
```

## Fiber vs blocking behaviour

The same method calls behave differently depending on whether PHP is currently running inside a fiber:

- **Inside a fiber** (worker mode + `oxphp_async(...)`): `send` / `recv` allocate a synthetic promise, register a waker with the channel, and suspend the fiber. The worker thread goes back to the scheduler and processes other fibers until the channel notifies the waker.
- **Outside a fiber** (traditional mode, or a non-async call path): `send` / `recv` block the OS worker thread via `crossbeam_channel`. No other work runs on that thread until the call returns.

Traditional mode still gets the channel semantics — it simply pays with a blocked thread. Worker mode is the recommended deployment for any pipeline that relies on waiting.

```php
<?php
// Traditional mode: this recv blocks the worker thread for up to 2 seconds.
$ch = new OxPHP\Shared\Channel(16);
$item = $ch->recv(timeout: 2.0);

// Worker mode: wrap in oxphp_async and recv suspends cooperatively.
oxphp_worker(function () use ($ch) {
    $consumer = oxphp_async(function () use ($ch) {
        while (($item = $ch->recv(timeout: 5.0)) !== null) {
            process($item);
        }
    });
    oxphp_async_await($consumer);
});
```

## Close semantics

`close()` is idempotent — calling it a second time is a no-op. After close:

- `send` / `sendMany` throw `ClosedException`.
- `trySend` returns `false`.
- `recv` continues to drain buffered items, then returns `null` once empty.
- `tryRecv` returns buffered items, then throws `ClosedException` on empty.
- `isClosed()` returns `true`.
- Blocked senders wake with `ClosedException`; blocked receivers wake with `null`.

```php
<?php
$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->close();

// Drain leftovers.
while (($item = $ch->recv()) !== null) {
    echo $item, "\n"; // one, two
}

// Further sends are rejected.
try {
    $ch->send('three');
} catch (OxPHP\Shared\ClosedException $e) {
    // expected
}
```

The pattern for a graceful pipeline shutdown is: producers stop, producer side calls `close()`, consumers drain in a `while (($item = $ch->recv()) !== null)` loop and exit naturally.

## Shutdown drain

When the OxPHP process shuts down, the `OxPHP\Shared` registry calls `close()` on every entry, including channels. From PHP's perspective this looks identical to an explicit `close()`:

- Blocked `recv` calls return `null`.
- Blocked `send` calls throw `ClosedException`.

> **Always null-check `recv`.** A caller that treats the return as non-null will crash at shutdown or whenever another holder closes the channel. The standard idiom is `while (($item = $ch->recv(timeout: T)) !== null) { ... }`.

## Batched operations

`sendMany` and `recvMany` exist for pipelines that move items in groups. Prefer them when you routinely handle 10+ items at a time: each batch is one FFI round trip instead of N, which meaningfully cuts per-item overhead in throughput-bound loops.

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

// Send an array in one call; returns how many were actually buffered.
$sent = $ch->sendMany([1, 2, 3, 4, 5]);   // 5

// Drain up to 10 items with a 100ms deadline.
$batch = $ch->recvMany(10, 0.1);

// max = 0 means "drain what is currently buffered, no wait".
$snapshot = $ch->recvMany(0);
```

Semantics worth noting:

- `sendMany` on a closed channel returns `0` (no exception). It does not send a partial batch.
- `recvMany(0)` never blocks. It returns whatever is currently buffered.
- A partial return is normal: if the timeout elapses while receiving, the call returns the items it already got.

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

Expose a queue on the channel and run the worker inside the async pool:

```php
<?php
// worker.php (worker bootstrap)
$work = new OxPHP\Shared\Channel(256);

$consumer = oxphp_async(function () use ($work) {
    while (($job = $work->recv(timeout: 30.0)) !== null) {
        process_job($job);
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
        while (($job = $ch->recv(timeout: 60.0)) !== null) {
            handle($i, $job);
        }
    });
}
```

### Bounded pipeline with backpressure

Using `trySend` plus a drop counter lets a producer shed load rather than block under overload:

```php
<?php
if (!$ch->trySend($event)) {
    increment_dropped_metric();
}
```

## Pitfalls

- **`timeout = 0.0` means wait indefinitely**, not "return immediately". Use `trySend` / `tryRecv` for non-blocking probes. This matches `oxphp_async_await` semantics.
- **Values must be shareable.** Scalars, `null`, and nested arrays of shareables are allowed. Passing an object that is not a `Shared\*` instance raises a `TypeException` on send.
- **Clone is forbidden.** `clone $channel` throws; transfer the channel through a closure `use` instead — `oxphp_async(function () use ($ch) { ... })` — so both sides see the same registry entry.
- **Always null-check `recv`.** Treating the return as non-null breaks at shutdown, when another holder closes the channel, and on timeout.
- **Timeout vs close ambiguity.** `recv` returns `null` for both. If you need to tell them apart, call `isClosed()` after a `null` return.
- **Cancelled waiters with in-flight payloads.** If many fibers wait on `send` / `recv` and are cancelled while their payload was about to cross, the payload can stay referenced until the next wake. Keep waiter counts bounded (e.g. cap concurrency with a `Shared\Counter` or channel-capacity semaphore).

## Related features

- [Worker Mode](worker-mode.md) — prerequisite for fiber-suspending `send` / `recv`.
- [Async Promises](async-promises.md) — the `oxphp_async()` closure is the normal way to hand a `Channel` to a background fiber.
- [Fiber Multiplexing](fiber-multiplexing.md) — explains how suspension keeps the worker thread productive while channel operations wait.
