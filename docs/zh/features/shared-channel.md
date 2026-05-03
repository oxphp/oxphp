---
title: Shared\Channel
description: 跨 PHP 工作线程的有界 MPMC 通道，send 和 recv 感知 fiber，适用于协作式生产者/消费者流水线。
---

# Shared\Channel

`OxPHP\Shared\Channel` 是一个有界的多生产者多消费者通道，存放于共享注册表中，对进程内每个 PHP 工作线程都可见。当请求处理器和后台工作线程——或两个工作线程——需要按 FIFO 顺序交换工作项时，就用它。在 fiber 内部，`send` 和 `recv` 会协作式挂起，底层工作线程因此保持空闲以处理其他请求。

## 概览

- **有界。** 容量在构造时固定。队列已满时 `send` 阻塞或挂起；`trySend` 返回 `false`。
- **MPMC。** 允许任意数量的发送者和接收者跨线程工作。交付是 FIFO。
- **感知 fiber。** 在开启异步池的 Worker 模式下，`send`/`recv` 会挂起 fiber 而不是阻塞工作线程。传统模式下则阻塞 OS 线程。
- **由注册表支撑。** 通道跨越请求边界存活，并按 ID 共享。关闭会传播给所有持有者。

## API 参考

```php
namespace OxPHP\Shared;

final class Channel implements Shareable
{
    public function __construct(int $capacity);

    public function send(mixed $value, float $timeout = 0.0): void;
    public function trySend(mixed $value): bool;

    public function recv(float $timeout = 0.0): mixed;
    public function tryRecv(): mixed;

    public function close(): void;
    public function isClosed(): bool;
    public function pending(): int;

    public function sendMany(array $values, float $timeout = 0.0): int;
    public function recvMany(int $max, float $timeout = 0.0): array;

    public function id(): int;
}
```

| 方法          | 使用场景                                                                             |
|---------------|--------------------------------------------------------------------------------------|
| `send`        | 推送一个条目，最多等待（或 fiber 挂起）`$timeout` 直到有空间。                       |
| `trySend`     | 不等待推送一个条目；队列满或已关闭时返回 `false`。                                   |
| `recv`        | 拉取一个条目，最多等待 `$timeout`。已关闭且为空或超时时返回 `null`。                 |
| `tryRecv`     | 不等待拉取一个条目；为空时返回 `null`；已关闭且为空时抛异常。                        |
| `close`       | 标记通道为已关闭。幂等。唤醒所有被阻塞的发送者/接收者。                              |
| `isClosed`    | 报告通道是否已关闭。                                                                 |
| `pending`     | 当前缓冲项数的参考值。对指标/背压检查有用。                                          |
| `sendMany`    | 推送数组条目；返回在满/关闭/超时前实际放入的数量。                                   |
| `recvMany`    | 拉取最多 `$max` 个条目（`0` 表示不等待、排空当前缓冲）。                             |
| `id`          | 注册表数字标识符；便于日志与可观测性关联。                                           |

## 选择 send/recv 变体

阻塞与非阻塞对在**返回什么与抛什么**上不同，且这种差异是刻意不对称的。

| 结果                 | `send(v, t)`         | `trySend(v)` | `recv(t)`        | `tryRecv()`           |
|----------------------|----------------------|--------------|------------------|-----------------------|
| 成功                 | 返回 `void`          | `true`       | 条目             | 条目                  |
| 满 / 空，未关闭       | 最多等待 `t`          | `false`      | 最多等待 `t`     | `null`                |
| 超时                 | `TimeoutException`   | —            | `null`           | —                     |
| 已关闭（空接收）      | `ClosedException`    | `false`      | `null`           | `ClosedException`     |
| 已关闭（仍有条目）    | `ClosedException`    | `false`      | 返回条目         | 返回条目              |

两个值得记住的结果：

1. **`recv` 在「已关闭且为空」时永不抛异常。** 它返回 `null`。循环必须做 null 检查。
2. **`recv` 在超时时也返回 `null`**，而 `send` 会抛 `TimeoutException`。如果需要区分「没人及时发送」与「通道已关闭」，请在 `null` 接收后检查 `isClosed()`。

```php
<?php
$ch = new OxPHP\Shared\Channel(4);

// 非阻塞探测。
if (!$ch->trySend('job-1')) {
    // 队列已满；丢弃、重试或实施背压。
}

// 带截止时间的阻塞发送。
try {
    $ch->send('job-2', timeout: 1.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    // 1 秒内没有消费者接收。
} catch (OxPHP\Shared\ClosedException $e) {
    // 等待期间通道被关闭。
}
```

## Fiber 与阻塞行为

同一次方法调用的行为会因 PHP 当前是否运行在 fiber 中而不同：

- **在 fiber 内**（Worker 模式 + `oxphp_async(...)`）：`send` / `recv` 分配一个合成 Promise，在通道上注册一个唤醒者，并挂起 fiber。工作线程回到调度器、继续处理其他 fiber，直到通道通知唤醒者。
- **不在 fiber 内**（传统模式，或非异步调用路径）：`send` / `recv` 通过 `crossbeam_channel` 阻塞 OS 工作线程。该线程在调用返回前不会运行其他工作。

传统模式仍然提供通道语义——只是用一个被阻塞的线程作为代价。对任何依赖等待的流水线，推荐部署 Worker 模式。

```php
<?php
// 传统模式：此 recv 最多阻塞工作线程 2 秒。
$ch = new OxPHP\Shared\Channel(16);
$item = $ch->recv(timeout: 2.0);

// Worker 模式：用 oxphp_async 包裹，recv 将协作式挂起。
oxphp_worker(function () use ($ch) {
    $consumer = oxphp_async(function () use ($ch) {
        while (($item = $ch->recv(timeout: 5.0)) !== null) {
            process($item);
        }
    });
    oxphp_async_await($consumer);
});
```

## 关闭语义

`close()` 是幂等的——再次调用是空操作。关闭之后：

- `send` / `sendMany` 抛 `ClosedException`。
- `trySend` 返回 `false`。
- `recv` 继续排空缓冲项，然后在为空时返回 `null`。
- `tryRecv` 返回缓冲项，为空时抛 `ClosedException`。
- `isClosed()` 返回 `true`。
- 被阻塞的发送者以 `ClosedException` 唤醒；被阻塞的接收者以 `null` 唤醒。

```php
<?php
$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->close();

// 排空剩余项。
while (($item = $ch->recv()) !== null) {
    echo $item, "\n"; // one, two
}

// 后续发送被拒绝。
try {
    $ch->send('three');
} catch (OxPHP\Shared\ClosedException $e) {
    // 预期
}
```

优雅关闭流水线的模式是：生产者停止、生产侧调用 `close()`，消费者在 `while (($item = $ch->recv()) !== null)` 循环中排空并自然退出。

## 关停时排空

当 OxPHP 进程关停时，`OxPHP\Shared` 注册表会对每个条目调用 `close()`，包括通道。从 PHP 角度看这与显式 `close()` 相同：

- 被阻塞的 `recv` 调用返回 `null`。
- 被阻塞的 `send` 调用抛 `ClosedException`。

> **始终对 `recv` 做 null 检查。** 把返回当作非空的调用者会在关停时或任何其他持有者关闭通道时崩溃。标准写法是 `while (($item = $ch->recv(timeout: T)) !== null) { ... }`。

## 批量操作

`sendMany` 和 `recvMany` 为需要成组处理条目的流水线而存在。当你经常一次处理 10+ 条时，请优先使用它们：每批只有一次 FFI 往返而非 N 次，这在吞吐密集的循环里能显著降低单项开销。

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

// 一次发送数组；返回实际缓冲的数量。
$sent = $ch->sendMany([1, 2, 3, 4, 5]);   // 5

// 在 100ms 截止时间内拉取最多 10 项。
$batch = $ch->recvMany(10, 0.1);

// max = 0 表示「排空当前缓冲，不等待」。
$snapshot = $ch->recvMany(0);
```

值得注意的语义：

- 在已关闭的通道上 `sendMany` 返回 `0`（不抛异常）。它不会发送部分批次。
- `recvMany(0)` 永不阻塞。它返回当前缓冲的内容。
- 部分返回是正常的：若在接收期间超时到达，调用会返回已经拿到的条目。

## 可观测性

内部服务器（默认 `INTERNAL_ADDR=127.0.0.1:9090`）在通用 shared-registry 端点中暴露通道：

- **`GET /__ox_shared/summary`** 包含 `Channel` 分桶，含 count、bytes、ops 和 `pending_total`。
- **`GET /__ox_shared/entries?type=Channel`** 列出通道条目及其注册表 ID。
- **`GET /__ox_shared/entries/:id`** 返回按通道的状态：`capacity`、`pending`、`closed`、`senders_blocked`、`receivers_blocked`。

`/metrics` 上的 Prometheus 曝露：

```text
oxphp_shared_channel_pending{channel_id="<id>"}             gauge
oxphp_shared_channel_senders_blocked{channel_id="<id>"}     gauge
oxphp_shared_channel_receivers_blocked{channel_id="<id>"}   gauge
oxphp_shared_channel_items_sent_total{channel_id="<id>"}    counter
oxphp_shared_channel_items_dropped_total{channel_id="<id>"} counter
```

`items_dropped_total` 会在 `sendMany` 部分成功、未能容纳的尾部递增。

## 常见模式

### HTTP 生产者、异步消费者

在通道上暴露一个队列，并把工作者运行在异步池内：

```php
<?php
// worker.php（Worker 引导）
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

### 跨多个消费者扇出

在同一通道上启动 N 个异步消费者；注册表确保每个条目恰好被其中一个接收。

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

### 带背压的有界流水线

用 `trySend` 搭配丢弃计数器，让生产者在过载时削峰而非阻塞：

```php
<?php
if (!$ch->trySend($event)) {
    increment_dropped_metric();
}
```

## 陷阱

- **`timeout = 0.0` 表示无限等待**，而不是「立即返回」。需要非阻塞探测请用 `trySend` / `tryRecv`。这与 `oxphp_async_await` 语义一致。
- **值必须是可共享的。** 标量、`null`、以及可共享值的嵌套数组都允许。传入非 `Shared\*` 实例的对象会在 send 时抛出 `TypeException`。
- **禁止 clone。** `clone $channel` 会抛异常；请通过闭包 `use` 传递通道——`oxphp_async(function () use ($ch) { ... })`——这样两侧看到同一个注册表条目。
- **始终对 `recv` 做 null 检查。** 把返回当作非空会在关停、另一持有者关闭通道和超时时崩溃。
- **超时与关闭的歧义。** `recv` 两种情况都返回 `null`。若需要区分，请在 `null` 返回后调用 `isClosed()`。
- **被取消等待者的在途负载。** 如果许多 fiber 在 `send` / `recv` 上等待并在其负载即将穿越时被取消，该负载可能保持被引用直到下次唤醒。保持等待者数量有界（例如用 `Shared\Counter` 或通道容量信号量限制并发）。

## 相关特性

- [Worker 模式](worker-mode.md) —— fiber 挂起式 `send` / `recv` 的前置条件。
- [异步 Promise](async-promises.md) —— `oxphp_async()` 闭包是把 `Channel` 交给后台 fiber 的常规方式。
- [Fiber 多路复用](fiber-multiplexing.md) —— 阐述挂起如何在通道操作等待时让工作线程继续产出。
