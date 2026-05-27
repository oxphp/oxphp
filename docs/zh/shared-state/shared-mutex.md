---
title: Shared\Mutex
description: 对存储值的跨线程互斥——通过 withLock / tryWithLock / withLockTimeout 在 PHP 工作线程间进行带有显式 wait 策略的原子多步更新。
---

# Shared\Mutex

`OxPHP\Shared\Mutex` 是进程级的互斥锁，包裹一个存储的值。你永远不直接操作锁——而是把闭包交给三种方法变体之一，运行时会在闭包执行期间持有锁，即使闭包抛异常也会释放。

## 概览

- **保护的是一个值，而不仅是一个代码段。** 被包裹的值以**引用**方式传入闭包，因此闭包正常返回时，闭包内部的直接修改会被提交回去。
- **三种显式 wait 策略**，取代单一被重载的 `?float $timeout`：
  - `withLock($fn)` —— 永久阻塞（或直到 request fiber 被取消）。
  - `tryWithLock($fn)` —— 非阻塞；若锁被占用则抛 `ContentionException`。
  - `withLockTimeout($fn, int $ms)` —— 有界等待；截止时间到期抛 `OperationTimeoutException`。
- **PHP 异常自由传播。** 若闭包抛出普通 PHP 异常，锁会被释放，异常向上传播。Mutex **不会**损坏——部分更新被认为是可接受的；调用方负责恢复不变量。
- **Rust panic 会损坏 mutex。** 若 Rust panic 穿越 FFI 边界（这是服务器 bug），mutex 进入 sticky 损坏状态，之后每次获取都会抛出 `CorruptedMutexException`。没有恢复 API——丢弃该实例并新建一个。
- **规避死锁。** 在同一线程上重入同一个 mutex（包括该线程上捕获的嵌套异步调用）会抛出 `DeadlockException`，而不是挂起。

## API 参考

```php
namespace OxPHP\Shared;

final class Mutex implements Shareable
{
    public function __construct(mixed $initial = null);

    public function withLock(callable $fn): mixed;
    public function tryWithLock(callable $fn): mixed;
    public function withLockTimeout(callable $fn, int $ms): mixed;

    public function id(): int;
}
```

闭包签名为 `function (mixed &$value): mixed` —— `$value` 以引用传入，因此你可以原地修改它。闭包的正常返回值会被转发给 `withLock` / `tryWithLock` / `withLockTimeout` 的调用方。**返回路径支持标量、`null` 与 `Shared\*` 实例**（string、int、float、bool、byte-string、null 以及任何实现 `OxPHP\Shared\Shareable` 的句柄）。返回 PHP 数组会抛出 `OxPHP\Shared\TypeException` —— 该场景暂不支持，由单独的工单跟踪。要把结构化的数组状态冒泡给调用方，要么通过 `&$value` 原地修改并在调用后重新读取，要么通过 `use (&$captured)` 变量暂存所需内容。

| 方法                        | 行为                                                            |
|-----------------------------|-----------------------------------------------------------------|
| `withLock($fn)`             | 阻塞至获取后执行闭包。永久 / 取消。                             |
| `tryWithLock($fn)`          | 非阻塞。若锁被占用则抛 `ContentionException`。                  |
| `withLockTimeout($fn, $ms)` | 有界等待。要求 `$ms > 0`。截止时间到抛 `OperationTimeoutException`。 |
| `id()`                      | 注册表标识；便于日志 / 可观测性。                               |

`$ms` 是严格为正的整数毫秒。零、负数、非 int 和缺失都会在桥层抛出 `OxPHP\Shared\TypeException`——不要试图用 `$ms` 表达这些策略，而应改用 `withLock`（永久）或 `tryWithLock`（非阻塞）。

## 为什么 Mutex 抛异常而 Channel 返回 Result

对设计良好的 mutex 来说，竞争和超时是**罕见事件**（锁应当只在短临界区中持有；持续的竞争是异味）。对通道来说它们是**例行事件**（扇出分派器在每个繁忙周期都会看到 Full/Closed/Timeout）。因此：

- `Mutex` 用**异常风格**——罕见路径就是例外路径。
- `Channel` 用 **Result 风格**——常见路径不进入 throw/catch 机制。

如果你发现自己在把每个 `withLock` 都包进 `try { … } catch (ContentionException) { … }`，那就是用错了原语。对队列形态的负载请使用 `Shared\Channel`，对单值原子性请使用 `Shared\Counter` / `Shared\Flag`。

同样的结构性原因解释了为什么 `Pool::tryAcquire()` 能返回 `null`，而 `Mutex::tryWithLock()` 却抛异常。`Pool` 是 **handle-first**：`tryAcquire(): ?Handle` 用 `null` 承载「饱和」，而 `Handle` 本身永远不是用户值，因此没有歧义。`Mutex` 是 **closure-only**——它刻意从不把锁守卫交回 PHP（使被持有的锁无法泄漏到闭包之外），于是没有可作为 nullable 返回的对象，而闭包自身的 `mixed` 结果可能本就是 `null`。既无空闲哨兵值，竞争便以 `ContentionException` 浮现。两个 `try*` 表面之所以分歧，源于各类型能交回什么，而非风格偏好。

## 示例

### 原子多字段更新

当值是单个整数时，Counter 就够用。而当若干字段必须同步更新时，Mutex 胜出：

```php
<?php
$stats = new OxPHP\Shared\Mutex(['hits' => 0, 'bytes' => 0]);

$stats->withLock(function (array &$s) use ($responseBytes) {
    $s['hits']  += 1;
    $s['bytes'] += $responseBytes;
});
```

另一个工作线程观察该值时，在同一个临界区中读取两个字段：

```php
$snapshot = ['hits' => 0, 'bytes' => 0];
$stats->withLock(function (array &$s) use (&$snapshot) {
    $snapshot = $s;
});
// $snapshot 看到同一次更新的两个字段，或都没看到——永不会出现
// 'hits' 已增加而 'bytes' 尚未匹配的情况。（我们通过 use(&$x) 捕获，
// 因为闭包自己的 return 目前仅限标量——见上文闭包签名说明。）
```

### 非阻塞探测 + 降级

```php
<?php
use OxPHP\Shared\{Mutex, ContentionException};

$budget = new Mutex(['tokens' => 100, 'refill_at' => time()]);

try {
    $budget->tryWithLock(function (array &$b) {
        if ($b['tokens'] <= 0) {
            // 没有令牌——保持状态不变。
            return;
        }
        $b['tokens'] -= 1;
    });
} catch (ContentionException) {
    // 锁被另一个工作线程持有——削峰放弃请求而非排队。
    http_response_code(503);
    return;
}
```

### 定时获取

```php
<?php
use OxPHP\Shared\{Mutex, OperationTimeoutException};

$counter = new Mutex(0);

try {
    // 返回值为标量——int $next——闭包返回会被转发。
    $next = $counter->withLockTimeout(function (int &$c) {
        $c += 1;
        return $c;
    }, ms: 5000);
} catch (OperationTimeoutException) {
    // 另一方持锁超过 5 秒。
}
```

鼓励使用具名参数——`ms: 5000` 读起来就是「5000 毫秒」，无需读者记住参数顺序。

### 在一处捕获所有并发情况

`OperationTimeoutException`、`ContentionException` 和 `DeadlockException` 都继承自 `OxPHP\Async\AsyncException`。一次 catch 即可横扫 Shared\* 与 Async\* 表面上的所有并发结果：

```php
<?php
use OxPHP\Async\AsyncException;

try {
    $state->withLockTimeout($fn, 100);
} catch (AsyncException) {
    // 超时、竞争、死锁，或任何与 await 相关的并发错误
}
```

### 从损坏 mutex 中灾难性恢复

闭包执行期间发生的 Rust panic（服务器 bug，而非 PHP 代码所致）会让锁处于 sticky 损坏状态。没有 `clearPoison()` 等价物——丢弃实例：

```php
<?php
use OxPHP\Shared\{Mutex, CorruptedMutexException};

try {
    $state->withLock($fn);
} catch (CorruptedMutexException) {
    // 旧实例已死。从持久化真相源重建。
    $state = new Mutex($initialState);
}
```

## 语义与陷阱

- **闭包在持锁状态下运行。** 请保持短小。不要调用 `sleep`、不要在网络 I/O 上阻塞、不要再次进入可能回调该 mutex 的其他 Shared\* 类型。
- **PHP 抛异常不再损坏锁。** 这是相对于先前「任意 throw 即 Poisoned」策略的有意改动：部分更新策略现在是「调用方负责恢复不变量」。如果需要不修改状态的「尝试计算」模式，请在 mutex 外完成，然后仅调用 `withLock` 提交最终值。
- **存储值类标量。** 字符串、整数、浮点、布尔、`null` 以及它们的嵌套数组都可以；对象、闭包、资源会抛出 `TypeException`。
- **闭包返回值覆盖标量、`null` 与 `Shared\*` 实例；数组尚未支持。** 存储值仍可以是数组（通过 `&$value` 修改），但闭包自己的 *return* 路径接受 string/int/float/bool/null/byte-string 以及任何 `OxPHP\Shared\Shareable` 句柄。返回 PHP 数组会抛出 `OxPHP\Shared\TypeException`。数组的变通方法：捕获到 `use (&$x)` 变量，或通过返回标量投影的后续 `withLock` 读取状态。
- **同一线程重入会抛 `DeadlockException`。** 使用不同的 mutex 或重构代码——同一线程重入是 bug，不是特性。
- **Fiber 取消以 `Async\AsyncException` 传播。** 被请求取消打断的 `withLock` 会抛该异常；锁会被干净释放。

## 异常

| 异常                         | 父类                            | 触发场景                                                             |
|------------------------------|---------------------------------|----------------------------------------------------------------------|
| `ContentionException`        | `Async\AsyncException`          | 对已持有的锁调用 `tryWithLock`。                                     |
| `OperationTimeoutException`  | `Async\AsyncException`          | `withLockTimeout` 截止时间到期。                                     |
| `DeadlockException`          | `Async\AsyncException`          | 同一线程重入或检测到 wait-for 环。                                   |
| `CorruptedMutexException`    | `Shared\SharedException`        | 先前一次闭包调用因 Rust panic 崩溃；mutex 不可用。                   |
| `TypeException`              | `Shared\SharedException`        | 构造函数或 `$ms` 参数违反类型契约。                                  |
| `StaleHandleException`       | `Shared\SharedException`        | 对注册表条目已被驱逐的句柄调用方法。                                 |
| `UninitializedException`     | `Shared\SharedException`        | 在尚未完成 `__construct` 的包装上调用 `id()`。                       |

## 可观测性

请见 [Shared 可观测性](shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ type: "Mutex", corrupted, waiters, last_acquire_ms, held_by_thread }`。
- 每实例 Prometheus 指标：
  - `oxphp_shared_mutex_waiters{mutex_id="…"}` —— 当前等待者数。
  - `oxphp_shared_mutex_acquires_total{mutex_id="…"}` —— 生命周期内的获取次数。
  - `oxphp_shared_mutex_contended_total{mutex_id="…"}` —— 曾需等待的获取次数。
  - `oxphp_shared_mutex_corrupted{mutex_id="…"}` —— 0 / 1（由 `_poisoned` 改名）。

## 何时不宜使用

- **单一原子值。** 若受保护的值是一个 int 或一个 bool，请使用 `Shared\Counter` 或 `Shared\Flag`——它们都是无锁的，也更廉价。
- **长时运行的工作。** 不要在 I/O、`sleep` 或 fiber await 期间持有 mutex。请改用 `Shared\Channel` 的生产者/消费者模式。
- **高竞争的热路径。** 如果每个请求都必须拿同一个 mutex，你已经把吞吐量串行化了。请对状态进行分片（例如 `Shared\Map<tenant_id, Mutex>`），或在每工作线程本地预聚合并定期刷写。
- **跨主机互斥。** 仅限进程内。多主机协调请使用分布式锁（Redis `SET NX`、etcd）。

## 从旧 API 迁移

| 之前                                                    | 现在                                                                          |
|---------------------------------------------------------|-------------------------------------------------------------------------------|
| `$m->with($fn)`（永久）                                 | `$m->withLock($fn)`                                                            |
| `$m->with($fn, $secs)`                                  | `$m->withLockTimeout($fn, $ms)`，`$ms` 单位为毫秒                              |
| `$m->tryWith($fn)` → 竞争时 `null`                      | `$m->tryWithLock($fn)` → 抛 `ContentionException`                              |
| `$m->isPoisoned()` / `$m->clearPoison()`                | 已移除；PHP 抛异常不再损坏 mutex                                                |
| `PoisonedException`（Rust panic 路径）                  | `CorruptedMutexException`（无公开清除 API）                                    |
| `Shared\TimeoutException`                               | `Shared\OperationTimeoutException`（现在继承 `Async\AsyncException`）          |
| `DeadlockException extends Shared\TimeoutException`     | `DeadlockException extends Async\AsyncException`                               |

闭包签名也由 `function (mixed $value): mixed`（return-to-commit）变为 `function (mixed &$value): mixed`（按引用修改，正常 return 是闭包自己的值，而不是新状态）。若闭包未返回任何值，存储值保留按引用修改后的内容。**一项先前的限制延续了下来**：闭包的 *return* 必须是标量（string / int / float / bool / null / byte-string）或 `Shared\*` 句柄——返回 PHP 数组会抛出 `OxPHP\Shared\TypeException`。存储值仍然可以是数组；通过 `&$value` 修改它，并用 `use (&$x)` 把结构化数据冒泡上去。

## 相关

- [共享状态](shared-state.md) —— 概览与心智模型。
- [Shared\Counter](shared-counter.md) —— 受保护状态是一个整数时。
- [Shared\Flag](shared-flag.md) —— 受保护状态是一个 bool 时。
- [Shared\Channel](shared-channel.md) —— 需要等待+交接而非互斥（且想要 Result 风格而非异常风格）时。
- [Shared\Map](shared-map.md) —— 按键分片 Mutex 以避免全局竞争。
