---
title: Shared\Channel
description: 跨 PHP 工作线程的有界 MPMC 通道，send 和 recv 感知 fiber，返回类型化的 Result 值以便显式分派。
---

# Shared\Channel

`OxPHP\Shared\Channel` 是一个有界的多生产者多消费者通道，存放于共享注册表中，对进程内每个 PHP 工作线程都可见。当请求处理器和后台工作线程——或两个工作线程——需要按 FIFO 顺序交换工作项时，就用它。在 fiber 内部，阻塞调用会协作式挂起，底层工作线程因此保持空闲以处理其他请求。

## 概览

- **有界。** 容量在构造时固定。队列已满时 `send` 阻塞（或挂起 fiber）；`trySend` 不等待便给出结果。
- **MPMC。** 允许任意数量的发送者和接收者跨线程工作。交付是 FIFO。
- **感知 fiber。** 在开启异步池的 Worker 模式下，阻塞变体会挂起 fiber 而非 OS 线程。传统模式下则阻塞 OS 线程。
- **类型化结果。** 每次 send/recv 都返回 [`Channel\SendResult`](#api-参考) / [`Channel\RecvResult`](#api-参考)——closed / full / timeout 对扇出分派器来说是正常结果，因此它们以 Result 变体而非异常的形式呈现。

## API 参考

```php
namespace OxPHP\Shared;

final class Channel implements Shareable, \Countable
{
    public function __construct(int $capacity);

    // ── Receive ──
    public function tryRecv(): Channel\RecvResult;                  // 非阻塞
    public function recv(): Channel\RecvResult;                     // 永久等待 / fiber-cancel
    public function recvTimeout(int $ms): Channel\RecvResult;       // 有界 ($ms > 0)

    // ── Send ──
    public function trySend(mixed $value): Channel\SendResult;
    public function send(mixed $value): Channel\SendResult;
    public function sendTimeout(mixed $value, int $ms): Channel\SendResult;

    // ── Batch ──
    public function sendMany(array $values, int $ms): int;          // 部分计数，不抛
    public function recvMany(int $max, int $ms): array;             // 部分数组，不抛

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
    public function value(): mixed;                // 非 Ok 时抛 SharedException
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

## Wait 策略

每个方向都有三种方法变体——后缀编码 wait 策略：

| 后缀          | 行为                                                                  |
|---------------|-----------------------------------------------------------------------|
| `try*`        | 非阻塞。无法继续时报告 `Empty` / `Full`。                             |
| (裸名)        | 永久阻塞，或直到 request fiber 被取消。                              |
| `*Timeout`    | 有界等待。`$ms` 必须 `> 0`——「永远」与「非阻塞」请使用其他形式。     |

`$ms` 对 `recvTimeout` / `sendTimeout` / `sendMany` / `recvMany` **始终是正整数毫秒**。零、负数、非 int 与缺失都会在桥层抛出 `OxPHP\Shared\TypeException`——该约束从方法体迁出，让三分法自解释。

### 每个方法可达的结果变体

`SendResult::Full` 与 `RecvResult::Empty` 仅由非阻塞 `try*` 调用产生——阻塞变体要么取到槽/项，要么用尽预算，此时结果为 `Timeout` 而非 `Full` / `Empty`。完整可达性矩阵：

| 方法            | `Ok` | `Full` / `Empty` | `Timeout` | `Closed` |
|-----------------|------|------------------|-----------|----------|
| `trySend`       | ✓    | `Full` ✓         | —         | ✓        |
| `send`          | ✓    | —                | —         | ✓        |
| `sendTimeout`   | ✓    | —                | ✓         | ✓        |
| `tryRecv`       | ✓    | `Empty` ✓        | —         | ✓        |
| `recv`          | ✓    | —                | —         | ✓        |
| `recvTimeout`   | ✓    | —                | ✓         | ✓        |

对阻塞调用的返回结果做 `isFull()` / `isEmpty()` 检查属于死代码。下面的 `match` 示例将这些分支保留为 `unreachable` 注释，以便读者一眼看到这种非对称。

## Result 分派

`RecvResult` 与 `SendResult` 携带一个 `status()` 判别值，外加（仅 `RecvResult`）一个负载。两种等价写法：

```php
use OxPHP\Shared\Channel;
use OxPHP\Shared\Channel\RecvStatus;

$ch = new Channel(capacity: 64);

// 布尔访问器——只关心单一结果时简洁。
$result = $ch->tryRecv();
if ($result->isOk()) {
    process($result->value());
} elseif ($result->isEmpty()) {
    backoff();
} else {
    break; // closed
}

// 穷尽 match——穷尽性检查能捕获新变体。
$r = $ch->recvTimeout(1500);
match ($r->status()) {
    RecvStatus::Ok      => process($r->value()),
    RecvStatus::Timeout => $logger->debug('idle'),
    RecvStatus::Closed  => break,
    RecvStatus::Empty   => /* unreachable: 只有 tryRecv 返回 Empty */ ,
};

// 对称的 send 侧分派——仅 Ok / Timeout / Closed 可达。
use OxPHP\Shared\Channel\SendStatus;
$s = $ch->sendTimeout($value, 1500);
match ($s->status()) {
    SendStatus::Ok      => /* 已交付 */ ,
    SendStatus::Timeout => $logger->debug('backpressure'),
    SendStatus::Closed  => break,
    SendStatus::Full    => /* unreachable: 只有 trySend 返回 Full */ ,
};

// 安全访问器——永不抛异常。
$value = $ch->tryRecv()->valueOr('fallback');
```

`RecvResult::value()` 在非 Ok 变体上调用时会抛出 `OxPHP\Shared\SharedException`——请先用 `isOk()` / `valueOr()` / `status()`。这是有意为之：它将「忘记检查 isOk」这一 bug 转为响亮的失败，而不是悄无声息的 `null`。

## Fiber 与阻塞行为

同一次方法调用的行为会因 PHP 当前是否运行在 fiber 中而不同：

- **在 fiber 内**（Worker 模式 + `oxphp_async(...)`）：阻塞变体（`send`、`recv`、`sendTimeout`、`recvTimeout`）分配一个合成 Promise，在通道上注册一个唤醒者，并挂起 fiber。工作线程回到调度器、继续处理其他 fiber，直到通道通知唤醒者。
- **不在 fiber 内**（传统模式，或非异步调用路径）：阻塞变体通过 `crossbeam_channel` 阻塞 OS 工作线程。该线程在调用返回前不会运行其他工作。

传统模式仍然提供通道语义——只是用一个被阻塞的线程作为代价。对任何依赖等待的流水线，推荐部署 Worker 模式。

```php
<?php
// 传统模式：此 recvTimeout 最多阻塞工作线程 2 秒。
$ch = new OxPHP\Shared\Channel(16);
$r = $ch->recvTimeout(2000);

// Worker 模式：用 oxphp_async 包裹，recv 将协作式挂起。
oxphp_worker(function () use ($ch) {
    $consumer = oxphp_async(function () use ($ch) {
        for (;;) {
            $r = $ch->recvTimeout(5000);
            if ($r->isOk()) { process($r->value()); continue; }
            if ($r->isClosed()) { break; }
            // 超时——继续等待。
        }
    });
    oxphp_async_await($consumer);
});
```

Fiber 取消（请求中止、SAPI 层截止时间到期）仍以 `OxPHP\Async\AsyncException` 形式呈现——它**不会**转译为 `RecvResult::Closed`。通道并未关闭；是运行时取消了你。视情况重抛或捕获。

## 关闭语义

`close()` 是幂等的——再次调用是空操作。关闭之后：

- `send`、`sendTimeout`、`trySend` 返回 `SendResult::Closed`。
- `sendMany` 返回关闭前实际接受的数量（已关闭则为 `0`）。
- `recv`、`recvTimeout`、`tryRecv` 继续将缓冲项作为 `RecvResult::Ok($value)` 返回，排空后返回 `RecvResult::Closed`。
- `recvMany` 返回能排空的项，可能是空数组。
- `isClosed()` 返回 `true`。
- 被阻塞的发送者以 `SendResult::Closed` 唤醒；被阻塞的接收者以 `RecvResult::Closed` 唤醒。

```php
<?php
$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->close();

// 排空剩余项。
for (;;) {
    $r = $ch->recv();
    if ($r->isClosed()) { break; }
    echo $r->value(), "\n"; // one, two
}

// 后续发送不抛异常地报告 Closed。
$result = $ch->send('three');
assert($result->isClosed());
```

优雅关闭流水线的模式：生产者停止，生产侧调用 `close()`，消费者在 `for (;;) { $r = $ch->recv(); if ($r->isClosed()) break; … }` 循环中排空并自然退出。任何消费者都不会错把缺失负载当成 `null`——变体本身就携带这一信号。

## 关停时排空

当 OxPHP 进程关停时，`OxPHP\Shared` 注册表会对每个条目调用 `close()`，包括通道。从 PHP 角度看这与显式 `close()` 相同：

- 被阻塞的 `recv*` 调用返回 `RecvResult::Closed`。
- 被阻塞的 `send*` 调用返回 `SendResult::Closed`。

> 始终检查 Result 变体。把 `recv()->value()` 当作总是安全的调用方会在关停或任何其他持有者关闭通道时抛出 `SharedException`。

## 批量操作

`sendMany` 和 `recvMany` 为需要成组处理条目的流水线而存在。当你经常一次处理 10+ 条时，请优先使用它们：每批只有一次 FFI 往返而非 N 次，这在吞吐密集的循环里能显著降低单项开销。

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

// 发送数组；返回实际接受的数量。
$sent = $ch->sendMany([1, 2, 3, 4, 5], 100);  // 100ms 预算内 5 个

// 在 100ms 截止时间内拉取最多 10 项。
$batch = $ch->recvMany(10, 100);
```

值得注意的语义：

- 两个批量方法在超时或批中关闭时**返回部分结果**——绝不抛异常。请检查 int / 数组长度以判断部分进展。
- `sendMany` 在已关闭的通道上返回 `0`。
- `recvMany` 在 closed+empty 时返回 `[]`。
- `$ms` 必须 `> 0`。没有「排空当前缓冲」重载——若该模式重要，请在紧凑循环中调用 `tryRecv()`；待真实调用点出现时再加专用批量变体。

## 可观测性

内部服务器（默认 `INTERNAL_ADDR=127.0.0.1:9090`）在通用 shared-registry 端点中暴露通道：

- **`GET /__ox_shared/summary`** 包含 `Channel` 分桶，含 count、bytes、ops 和 `pending_total`。
- **`GET /__ox_shared/entries?type=Channel`** 列出通道条目及其注册表 ID。
- **`GET /__ox_shared/entries/:id`** 返回按通道的状态：`capacity`、`count`、`pending`（已弃用别名 `count`）、`closed`、`senders_blocked`、`receivers_blocked`。

`/metrics` 上的 Prometheus 曝露：

```text
oxphp_shared_channel_count{channel_id="<id>"}               gauge
oxphp_shared_channel_pending{channel_id="<id>"}             gauge（已弃用，_count 的别名）
oxphp_shared_channel_senders_blocked{channel_id="<id>"}     gauge
oxphp_shared_channel_receivers_blocked{channel_id="<id>"}   gauge
oxphp_shared_channel_items_sent_total{channel_id="<id>"}    counter
oxphp_shared_channel_items_dropped_total{channel_id="<id>"} counter
```

`items_dropped_total` 会在 `sendMany` 部分成功、未能容纳的尾部递增。

## 常见模式

### HTTP 生产者、异步消费者

```php
<?php
$work = new OxPHP\Shared\Channel(256);

$consumer = oxphp_async(function () use ($work) {
    for (;;) {
        $r = $work->recvTimeout(30000);
        if ($r->isOk()) { process_job($r->value()); continue; }
        if ($r->isClosed()) { break; }
        // 超时——做健康检查，然后继续等待。
    }
});

oxphp_worker(function () use ($work) {
    $work->send(['url' => $_POST['url'], 'tries' => 3]);
    echo "queued";
});
```

### 跨多个消费者扇出

在同一通道上启动 N 个异步消费者；注册表确保每个条目恰好被其中一个接收。

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

### 带背压的有界流水线

用 `trySend` 搭配丢弃计数器，让生产者在过载时削峰而非阻塞：

```php
<?php
if ($ch->trySend($event)->isFull()) {
    increment_dropped_metric();
}
```

## 陷阱

- **`recvTimeout(0)` 是 TypeException，不是「非阻塞」。** 非阻塞请用 `tryRecv()`。专门的方法名消除了旧 `?float $timeout` 带来的 timeout 重载歧义。
- **值必须是可共享的。** 标量、`null`、以及可共享值的嵌套数组都允许。传入非 `Shared\*` 实例的对象会在 send 时抛出 `TypeException`。
- **禁止 clone。** `clone $channel` 会抛 `SharedException`；请通过闭包 `use` 传递通道——`oxphp_async(function () use ($ch) { ... })`——这样两侧看到同一个注册表条目。
- **在非 Ok 上调用 `value()` 会抛异常。** 这是特性而非缺陷——它把「忘记检查 isOk」的错误转为响亮失败。若 default 确实合适，请用 `valueOr($default)`。
- **Fiber 取消以异常形式传播**，而不是 Result 变体。被请求取消打断的 `recv()` 会抛 `OxPHP\Async\AsyncException`。自身关闭的通道仍然产出 `RecvResult::Closed`。
- **被取消等待者的在途负载。** 如果许多 fiber 在 `send` / `recv` 上等待并在其负载即将穿越时被取消，该负载可能保持被引用直到下次唤醒。保持等待者数量有界（例如用 `Shared\Counter` 或通道容量信号量限制并发）。

## 从旧 API 迁移

| 之前                                                                | 现在                                                                   |
|---------------------------------------------------------------------|------------------------------------------------------------------------|
| `$ch->tryRecv()` → 空时 `null`，关闭时抛 `ClosedException`          | `$ch->tryRecv()` → `RecvResult::Empty` / `Closed`                       |
| `$ch->recv($secs)` → 超时/关闭时 `null`                             | `$ch->recv()`（永远）/ `$ch->recvTimeout($ms)` → `RecvResult`           |
| `$ch->trySend($v): bool`                                            | `$ch->trySend($v): SendResult`                                          |
| `$ch->send($v, $secs): bool`（抛 TimeoutException/ClosedException） | `$ch->send($v)` / `$ch->sendTimeout($v, $ms)` → `SendResult`            |
| `$ch->sendMany($vs, $secs)`（部分成功时抛 TimeoutException）        | `$ch->sendMany($vs, $ms): int`（部分计数，不抛异常）                    |
| `?float $timeout`（秒，允许 NaN/INF）                               | `int $ms`（毫秒，必须 `> 0`）                                            |

## 相关特性

- [Worker 模式](worker-mode.md) —— fiber 挂起式阻塞变体的前置条件。
- [异步 Promise](async-promises.md) —— `oxphp_async()` 闭包是把 `Channel` 交给后台 fiber 的常规方式。
- [Fiber 多路复用](fiber-multiplexing.md) —— 阐述挂起如何在通道操作等待时让工作线程继续产出。
