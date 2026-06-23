---
title: Shared\Pool
description: 进程级有界对象池，用于昂贵的每线程资源——数据库连接、JSON 解析器、HTTP 客户端——具备严格预算、按线程亲和性和空闲超时驱逐。
---

# Shared\Pool

`OxPHP\Shared\Pool` 是一个有界的每线程资源池。它是用于管理那些「创建昂贵、不能廉价重建、且不应无限存在」的对象的原语——典型如数据库连接、预处理语句缓存、可复用的 JSON 解码器或 HTTP 客户端会话。

Pool 给每个 PHP 工作线程一条专属的就绪资源车道，强制整个池的严格上限，并自动回收空闲槽，让你不必为不用的容量付费。

## 概览

- **严格预算。** `maxSize` 是整个池的硬上限。当池饱和时，获取要么等待、要么返回 `null`、要么抛异常——取决于你调用的是哪个获取方法。
- **按线程亲和性。** 每个工作线程都有自己的空闲队列。获取优先从本地队列取出，且永远不会把线程 A 上铸造的槽交给线程 B（v1）。
- **工厂在获取的工作线程内运行。** 资源在每个线程首次需要时按需铸造，而非池构造时就铸造。
- **驱逐时回调销毁。** 可选的 `destroy($resource)` 闭包会在池丢弃槽（空闲超时、手动驱逐、服务器停止）时运行。
- **空闲超时驱逐。** 闲置时间超过 `idleTimeoutMs` 的槽会被后台任务销毁。设 `idleTimeoutMs: 0` 可彻底关闭空闲驱逐。
- **RAII 句柄。** `acquire()` 返回一个 `Handle`；当句柄离开作用域时（包括抛异常时），槽会自动归还给池，或更早地通过 `$handle->release()` 归还。
- **可共享。** Pool 跨越请求边界存活，并按句柄共享（闭包内 `use ($pool)`）。

## API 参考

```php
namespace OxPHP\Shared;

final class Pool implements Shareable
{
    public function __construct(
        callable  $factory,                  // fn(): object — 创建一个资源
        ?callable $destroy       = null,     // fn(object): void — 拆解一个资源
        int       $maxSize       = 32,       // 活跃槽的硬上限；> 0
        int       $idleTimeoutMs = 300_000,  // 驱逐前的空闲毫秒数；0 表示禁用
    );

    // acquire 家族——毫秒超时三分法
    public function acquire(): Pool\Handle;                 // 永久等待
    public function tryAcquire(): ?Pool\Handle;             // 非阻塞；饱和时返回 null
    public function acquireTimeout(int $ms): Pool\Handle;   // 有界；$ms > 0

    // with 家族——围绕原始资源的作用域守护
    public function with(callable $body): mixed;                 // 永久等待
    public function withTimeout(callable $body, int $ms): mixed; // 有界；$ms > 0

    public function stats(): Pool\Stats;   // 计数器在某一时刻的快照
    public function evict(): int;          // 立即强制驱逐所有空闲槽；返回数量
    public function id(): int;
}

namespace OxPHP\Shared\Pool;

class Handle
{
    public function get(): mixed;     // 底层资源（release 之后调用会抛异常）
    public function release(): void;  // 立即归还槽；幂等；析构时也会运行
}

final class Stats
{
    public function inUse(): int;    // 当前已被取出的槽
    public function idle(): int;     // 可供交出的空闲槽
    public function waiting(): int;  // 阻塞在 acquire 上的调用方
    public function size(): int;     // inUse() + idle()（活跃槽）
    public function maxSize(): int;  // 配置的上限

    public function utilization(): float;  // inUse() / maxSize()，maxSize() == 0 时为 0.0
}
```

| 方法              | 返回值        | 使用场景                                                              |
|-------------------|---------------|----------------------------------------------------------------------|
| `acquire`         | `Handle`      | 取出一个资源，永久等待空闲槽。                                        |
| `tryAcquire`      | `?Handle`     | 非阻塞取出。若池已饱和，立即返回 `null`。                             |
| `acquireTimeout`  | `Handle`      | 在 `$ms`（`> 0`）的有界预算内取出；超时抛 `OperationTimeoutException`。 |
| `with`            | mixed         | 作用域守护：获取（永久），用原始资源运行 `$body($resource)`，即便抛异常也释放。闭包返回值会被透传。 |
| `withTimeout`     | mixed         | 同 `with`，但获取受 `$ms` 约束。                                      |
| `stats`           | `Pool\Stats`  | 池计数器在某一时刻的快照。                                               |
| `evict`           | int           | 立即强制驱逐所有空闲槽（无视 `idleTimeoutMs`）；返回丢弃的数量。      |
| `id`              | int           | 注册表标识；便于日志 / 可观测性。                                     |
| `Handle::get`     | mixed         | 底层资源。release 之后会抛 `StaleHandleException`。                   |
| `Handle::release` | void          | 立即把槽归还给池。幂等；析构时也会自动运行（RAII）。                  |

超时遵循与 `Shared\Mutex` 和 `Shared\Channel` 相同的三分法：裸方法永久等待，`try*` 方法非阻塞，`*Timeout(int $ms)` 方法等待有界的毫秒数。不存在以小数秒为单位的超时。

## 示例

### 数据库连接池

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
        // 无需做什么——PDO 在析构时关闭。回调存在
        // 是为了那些需要显式拆解的资源（套接字、句柄）。
    },
    maxSize: 16,
    idleTimeoutMs: 60_000,     // 空闲连接 1 分钟后释放
);

// 在请求处理器中
$users = $db->with(function (PDO $conn) use ($userId) {
    $stmt = $conn->prepare('SELECT * FROM users WHERE id = ?');
    $stmt->execute([$userId]);
    return $stmt->fetch();
});
```

`with()` 是寿命最短的模式：进入时获取、返回时释放、抛异常时也释放——你不会泄漏句柄。它还会把**原始资源**直接交给你的闭包，因此你跳过了 `Handle::get()` 这一步。

### 手动 acquire / release

```php
<?php
$h = $pool->acquire();        // 永久等待空闲槽

$conn = $h->get();
$conn->beginTransaction();
doWork($conn);
$conn->commit();

$h->release();                // 或者干脆让 $h 离开作用域（RAII）
```

句柄在被销毁时会自动归还其槽——包括异常展开栈的情况——因此显式 `release()` 是可选的。仅当资源必须在处理器序列的多次调用间存活时，才（在 `with()` 之外）选用手动句柄。

### 可复用解析器池

```php
<?php
$parsers = new OxPHP\Shared\Pool(
    factory: fn () => new JsonMachine\Parser(),
    maxSize: 8,
);

$doc = $parsers->with(fn ($p) => $p->parse($body));
```

### 带回退的非阻塞获取

```php
<?php
$h = $pool->tryAcquire();
if ($h === null) {
    // 池已饱和——不等待，优雅降级。
    http_response_code(503);
    header('Retry-After: 1');
    return;
}
// ... 使用 $h->get();槽在离开作用域时归还。
```

### 带超时的有界获取

```php
<?php
try {
    $h = $pool->acquireTimeout(100);   // 最多等待 100 毫秒
} catch (OxPHP\Shared\OperationTimeoutException $e) {
    http_response_code(503);
    header('Retry-After: 1');
    return;
}
// ... 使用 $h->get();
```

## 工厂与销毁语义

工厂在获取的工作线程上**懒**运行。`maxSize: 32` 的池不会预分配 32 个资源；它在需求到来时按需铸造，受跨所有线程合并的 `maxSize` 约束。

- 工厂必须返回一个 PHP 对象。返回非对象会从获取调用处以 `TypeException` 浮现，且该槽不计入预算。
- 抛异常的工厂会把它自己的异常原样传播给获取的调用方，且该槽不计入预算。
- 销毁回调（如已提供）会在池丢弃槽时运行：空闲超时到期、显式 `evict()`、或服务器关停。它在工作线程上运行（不是在驱动驱逐调度器的 Tokio 线程上），因此可以安全调用 PHP。
- 抛出的销毁回调会被记录但不会毒化池——槽本来就在被销毁，没有什么有用的东西需要回滚。

## 按线程亲和性

v1 的池是严格按线程的：在工作线程 A 上铸造的槽不能在工作线程 B 上获取。实务上这意味着工作线程 A 上 `stats()->idle()` 可能非零，而工作线程 B 在 `acquire()` 上阻塞。这能让槽在使用它的线程上保持热（DB 连接、OPcache 预热的对象），并避免在 CPU 核心间搬运资源。

跨线程的工作窃取是 v1.x 候选项。在那之前，请按工作线程数 × 每线程预期并发来设定 `maxSize`，而不是仅仅按聚合需求。

## 空闲超时驱逐

空闲槽由后台调度器驱逐。当槽闲置超过 `idleTimeoutMs` 时，调度器会标记它；拥有它的工作线程会在下一次请求时销毁它（PHP 引擎此时存活，因此 `$destroy` 在正常请求上下文中运行）。预算在同一时刻被释放。

按重建成本调优 `idleTimeoutMs`：

- **重建廉价**（JSON 解码器、字符串池）：设 10_000–60_000 毫秒；流量回落时迅速释放内存。
- **重建昂贵**（DB 连接、TLS 会话）：设 300_000 毫秒（默认）至 900_000 毫秒；少付重建代价。
- **永不驱逐**：传 `0`。空闲槽将一直存活，直到池被丢弃。

`$pool->evict()` 会立即强制驱逐当前从调用工作线程可达的**所有**空闲槽——无视 `idleTimeoutMs`——并返回被丢弃的数量。它是「立即冲掉空闲」的运维逃生口（例如下游服务重启了，你希望下一次获取铸造全新资源）。在用的槽不会被触碰。

## 预算与获取语义

每个获取变体都会先尝试立即满足请求——复用一个空闲槽，或（若池低于 `maxSize`）通过工厂铸造一个新槽。只有当池**饱和**时——既无空闲槽**又**已达 `maxSize`——行为才有差异：

| 调用时状态                                  | `acquire()`           | `tryAcquire()`     | `acquireTimeout($ms)`                     |
|---------------------------------------------|-----------------------|--------------------|-------------------------------------------|
| 本线程队列中存在空闲槽                      | 立即复用              | 立即复用           | 立即复用                                  |
| 无空闲槽，但低于 `maxSize`                  | 工厂铸造一个槽        | 工厂铸造一个槽     | 工厂铸造一个槽                            |
| 饱和（达 `maxSize`，全部在用）              | **永久**等待          | 返回 `null`        | 最多等待 `$ms`，随后抛 `OperationTimeoutException` |

`$ms` 必须 `> 0`；`0` 或负数会抛 `TypeException`。这里故意没有小数秒形式、也没有「无限」哨兵参数——无界等待请使用裸 `acquire()`。

## 异常

| 异常                      | 触发场景                                                             |
|---------------------------|----------------------------------------------------------------------|
| `OperationTimeoutException` | `acquireTimeout` / `withTimeout` 超过 `$ms` 仍无空闲槽。**继承 `Async\AsyncException`，而非 `SharedException`。** |
| `TypeException`           | 非正 `maxSize`、负 `idleTimeoutMs`、`$ms <= 0`，或工厂返回了非对象。 |
| `StaleHandleException`    | 句柄被释放后调用 `Handle::get()`。                                    |
| `UninitializedException`  | 在尚未完成 `__construct` 的 pool 包装上调用方法。                     |

`tryAcquire()` 在饱和时**不**抛异常——它返回 `null`。因为 `OperationTimeoutException` 继承 `OxPHP\Async\AsyncException`（而非 `SharedException`），`catch (SharedException)` **不会**捕获获取超时；请使用 `catch (OxPHP\Async\AsyncException)` 或直接捕获 `OperationTimeoutException`。

**为什么这与 `Mutex::tryWithLock()` 不同。** 二者都是非阻塞的 `try*` 调用，但 `Pool` 在竞争时返回 `null`，而 `Mutex` 抛 `ContentionException`。这种分歧是结构性的，而非风格选择。`Pool` 是 **handle-first**：每次获取都交回一个 `Handle`，因此「饱和」这个结果有天然的载体——`?Handle`，其中 `null` 表示「无槽位」，且永远不会与真实值冲突（`Handle` 本身永远不是用户值）。`Mutex` 在设计上是 **closure-only**——它刻意从不把锁守卫交回 PHP，使得被持有的锁无法泄漏到闭包之外。这导致 `tryWithLock` 没有可作为 nullable 返回的对象，而闭包自身的 `mixed` 结果合法地可能就是 `null`——因此 `null` 无法兼作「未获取」之意。既无句柄又无空闲哨兵值，唯一无歧义的竞争信号就只剩异常。请相应地捕获：`tryAcquire` → 检测 `null`；`tryWithLock` → `catch (ContentionException)`。

工厂内抛出的异常原样传播给获取的调用方，且不消耗预算。`with()` / `withTimeout()` 体内的异常会在槽被释放后传播给调用方。

## 可观测性

完整内容请见 [Shared 可观测性](shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ type: "Pool", size, in_use, idle, waiting, max_size, idle_by_thread, rebalance_strategy }`。
- `GET /__ox_shared/summary` 包含一个带有 `count`、`bytes` 和 `ops` 的 `Pool` 桶。像 `waiting` 这样的每池仪表以及 `evicted_total` 计数器暴露在 `/metrics` 上（见下文），不在 summary 中聚合。
- 每池 Prometheus 指标：
  - `oxphp_shared_pool_size{pool_id="…"}`            —— 仪表，槽总数（在用 + 空闲）。
  - `oxphp_shared_pool_in_use{pool_id="…"}`          —— 仪表。
  - `oxphp_shared_pool_idle{pool_id="…"}`            —— 仪表。
  - `oxphp_shared_pool_waiting{pool_id="…"}`         —— 仪表，排队的获取数。
  - `oxphp_shared_pool_acquire_total{pool_id="…",result="ok|timeout|closed|saturated"}` —— 计数器。`saturated` 统计发现池已满的非阻塞 `tryAcquire` 调用（区别于 `timeout`，后者表示等待已耗尽）。
  - `oxphp_shared_pool_evicted_total{pool_id="…",reason="idle_timeout|evict|shutdown"}` —— 计数器。
  - `oxphp_shared_pool_wait_seconds_*{pool_id="…"}`  —— 获取等待直方图（bucket / sum / count）。

值得告警的组合：`waiting` 上升而 `size` 持平，意味着池已饱和、应该扩容；`acquire_total{result="timeout"}` 上升而 `in_use` 正常，意味着工厂慢（或被阻塞）；`acquire_total{result="saturated"}` 上升，意味着调用方持续在已满的池上调用 `tryAcquire`（背压在触发）。

## 何时不宜使用

- **廉价或不可变资源。** 池的开销大于直接重建简单对象。仅在创建耗时为毫秒级或大小为千字节级的资源上使用。
- **不能安全复用的对象。** 如果资源会累积每次请求的状态（未结束的事务、待处理的读取）且你无法可靠重置，池化会让状态在请求间泄露。请在请求收尾代码中把槽恢复到已知状态，或不要池化。
- **跨主机资源。** 池仅在进程内。多主机连接池请使用连接桶服务或边车（pgbouncer、proxy-sql）。
- **无界扇出。** 如果每个在飞 HTTP 调用都需要一个连接，那不是池——那是「每请求 N 个」的问题。请用 `Shared\Channel` 在有界池后串行化工作。
- **自带池语义的资源。** 许多客户端库已内置池（例如 Guzzle 的连接池）。在其上叠加 `Shared\Pool` 是双重簿记；请优先使用库自身的池化。

## 相关

- [共享状态](shared-state.md) —— 概览与心智模型。
- [Shared\Once](shared-once.md) —— 当你需要的是恰好一个资源（不是 N 个的池）。
- [Shared\Channel](shared-channel.md) —— 配合池构成生产者/消费者流水线。
- [Shared\Map](shared-map.md) —— 按租户名为键的「每租户一个 Pool」。
- [Worker 模式](../features/worker-mode.md) —— 同一工作线程内跨请求复用池句柄。
