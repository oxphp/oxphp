---
title: Shared\Mutex
description: 对存储值进行毒化保护的 Mutex——跨 PHP 工作线程的原子多步更新，带 RAII 风格临界区和基于闭包的访问。
---

# Shared\Mutex

`OxPHP\Shared\Mutex` 是进程级的互斥锁，包裹一个存储的标量值。你永远不直接操作锁——而是把闭包交给 `with()`（阻塞）或 `tryWith()`（非阻塞），运行时会在闭包执行期间持有锁，即使闭包抛异常也会正确释放。

## 概览

- **保护的是一个值，而不仅是一个代码段。** 被包裹的值以传值方式传入闭包，闭包返回的新值会被提交回去。
- **闭包失败则毒化。** 如果闭包抛出异常，mutex 被标记为毒化；后续的 `with()` 调用会因 `PoisonedException` 失败，直到你显式调用 `clearPoison()`。这可以防止其他工作线程对处于更新途中、发生错误的状态继续操作。
- **规避死锁。** 在同一线程上重入同一个 mutex（包括该线程上捕获的嵌套异步调用）会抛出 `DeadlockException`，而不是挂起。
- **定时获取。** `with($fn, $timeout)` 最多等待 `$timeout` 秒；传 0 表示无限等待。`tryWith` 瞬时返回。

## API 参考

```php
namespace OxPHP\Shared;

final class Mutex implements Shareable
{
    public function __construct(mixed $initial = null, ?float $defaultTimeout = null);

    public function isPoisoned(): bool;
    public function clearPoison(): void;

    public function with(callable $fn, float $timeout = 0.0): mixed;
    public function tryWith(callable $fn): mixed;

    public function id(): int;
}
```

闭包签名为 `function (mixed $value): mixed` —— 它以传值方式接收当前存储的值，并返回要存入的新值。若闭包返回 `null`，则存储的值变成 `null`。

| 方法          | 返回                     | 使用场景                                                      |
|---------------|--------------------------|---------------------------------------------------------------|
| `with`        | 闭包返回值               | 对存储值的原子 RMW；最多阻塞 `$timeout`。                     |
| `tryWith`     | 闭包返回值或 `null`      | 同上，但锁被占用时立即返回 `null`。                           |
| `isPoisoned`  | bool                     | 探测 mutex 是否处于毒化状态。                                 |
| `clearPoison` | void                     | 处理完先前失败之后，重置为可用状态。                          |

## 示例

### 原子多字段更新

当值是单个整数时，Counter 就够用。而当若干字段必须同步更新时，Mutex 胜出：

```php
<?php
$stats = new OxPHP\Shared\Mutex(['hits' => 0, 'bytes' => 0]);

$stats->with(function (array $s) use ($responseBytes) {
    $s['hits']  += 1;
    $s['bytes'] += $responseBytes;
    return $s;                     // 原子地提交回去
});
```

另一个工作线程读取 `$stats->with(fn ($s) => $s)` 时，要么同时看到两个字段都更新，要么都没更新——永远不会出现 `hits` 增加但 `bytes` 未匹配更新的状态。

### 非阻塞探测 + 降级

```php
<?php
$budget = new OxPHP\Shared\Mutex(['tokens' => 100, 'refill_at' => time()]);

$allowed = $budget->tryWith(function (array $b) {
    if ($b['tokens'] <= 0) return $b;     // 空操作，仅用于检查
    $b['tokens'] -= 1;
    return $b;
});

if ($allowed === null) {
    // 锁被另一个工作线程持有——削峰放弃请求而不是排队。
    http_response_code(503);
    return;
}
```

### 定时获取

```php
<?php
$cache = new OxPHP\Shared\Mutex(null, defaultTimeout: 2.0);

try {
    $result = $cache->with(function ($c) { /* ... */ return $c; }, timeout: 5.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    // 另一方持有锁超过 5 秒。
}
```

### 恢复被毒化的 mutex

```php
<?php
try {
    $state->with(function ($s) {
        doRiskyThing($s);          // 抛异常
        return $s;
    });
} catch (Throwable $e) {
    // 其他工作线程现在对 $state->with(...) 会得到 PoisonedException。
    if ($state->isPoisoned()) {
        reinitialiseFromPersistentStore($state);
        $state->clearPoison();
    }
    throw $e;
}
```

## 语义与陷阱

- **闭包在持锁状态下运行。** 请保持短小。不要调用 `sleep`、不要在网络 I/O 上阻塞、不要再次进入可能回调该 mutex 的其他 Shared\* 类型。
- **默认严格毒化。** 闭包内部的任何异常都会毒化 mutex，即使存储值未被触动。如果你需要非毒化的「尝试计算」模式，请在 mutex 外完成，然后仅调用 `with` 提交。
- **`$defaultTimeout` 在你给 `with` 传 `0.0` 时生效。** 要在单次调用时覆盖，请显式传 `timeout:` 具名参数。
- **v1 中存储值仅限标量。** 字符串、整数、浮点、布尔以及以上的嵌套数组都可以；对象、闭包和资源会抛出 `TypeException`。
- **同一线程重入会抛异常。** 使用不同的 mutex 或重构代码——重入是 bug，不是特性。

## 异常

| 异常                     | 触发场景                                                            |
|-------------------------|---------------------------------------------------------------------|
| `PoisonedException`     | 对已毒化的 mutex（之前抛过异常）调用 `with` / `tryWith`。           |
| `TimeoutException`      | `with` 超过 `$timeout`（或 `defaultTimeout`）仍未获得锁。           |
| `DeadlockException`     | 在同一线程上对同一 mutex 重入 `with`/`tryWith`。                    |
| `TypeException`         | 构造函数或闭包返回值不可序列化。                                    |
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄调用方法。                                |
| `UninitializedException`| 在尚未完成 `__construct` 的包装上调用 `id()`。                      |

`tryWith` 在竞争时刻意返回 `null` 而不是抛异常——竞争不是错误。

## 可观测性

请见 [Shared 可观测性](../operations/shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ type: "Mutex", poisoned, waiters, last_acquire_ms, held_by_thread }`。
- 每实例 Prometheus 指标：
  - `oxphp_shared_mutex_waiters{mutex_id="…"}` —— 当前等待者数。
  - `oxphp_shared_mutex_acquires_total{mutex_id="…"}` —— 生命周期内的获取次数。
  - `oxphp_shared_mutex_contended_total{mutex_id="…"}` —— 曾需等待的获取次数。
  - `oxphp_shared_mutex_poisoned{mutex_id="…"}` —— 0 / 1。

## 何时不宜使用

- **单一原子值。** 若受保护的值是一个 int 或一个 bool，请使用 `Shared\Counter` 或 `Shared\Flag`——它们都是无锁的，也更廉价。
- **长时运行的工作。** 不要在 I/O、`sleep` 或 fiber await 期间持有 mutex。请改用 `Shared\Channel` 的生产者/消费者模式。
- **高竞争的热路径。** 如果每个请求都必须拿同一个 mutex，你已经把吞吐量串行化了。请对状态进行分片（例如 `Shared\Map<tenant_id, Mutex>`），或在每工作线程本地预聚合并定期刷写。
- **跨主机互斥。** 仅限进程内。多主机协调请使用分布式锁（Redis `SET NX`、etcd）。

## 相关

- [共享状态](shared-state.md) —— 概览与心智模型。
- [Shared\Counter](shared-counter.md) —— 受保护状态是一个整数时。
- [Shared\Flag](shared-flag.md) —— 受保护状态是一个 bool 时。
- [Shared\Channel](shared-channel.md) —— 需要等待+交接而非互斥时。
- [Shared\Map](shared-map.md) —— 按键分片 Mutex 以避免全局竞争。
