---
title: Shared\Pool
description: 进程级有界对象池，用于昂贵的每线程资源——数据库连接、JSON 解析器、HTTP 客户端——具备严格预算、按线程亲和性和空闲超时驱逐。
---

# Shared\Pool

`OxPHP\Shared\Pool` 是一个有界的每线程资源池。它是用于管理那些「创建昂贵、不能廉价重建、且不应无限存在」的对象的原语——典型如数据库连接、预处理语句缓存、可复用的 JSON 解码器或 HTTP 客户端会话。

Pool 给每个 PHP 工作线程一条专属的就绪资源车道，强制整个池的严格上限，并自动回收空闲槽，让你不必为不用的容量付费。

## 概览

- **严格预算。** `maxSize` 是整个池的硬上限。超过上限的获取会阻塞，直到有槽被释放或超时。
- **按线程亲和性。** 每个工作线程都有自己的空闲队列。获取优先从本地队列取出，且永远不会把线程 A 上铸造的槽交给线程 B（v1）。
- **工厂在获取的工作线程内运行。** 资源在每个线程首次需要时按需铸造，而非池构造时就铸造。
- **驱逐时回调销毁。** 可选的 `destroy($resource)` 闭包会在池丢弃槽（空闲超时、池驱逐、服务器停止）时运行。
- **空闲超时驱逐。** 闲置时间超过 `idleTimeout` 的槽会被每 500 毫秒触发一次的后台任务销毁。
- **可共享。** Pool 跨越请求边界存活，并按句柄共享（闭包内 `use ($pool)`）。

## API 参考

```php
namespace OxPHP\Shared;

final class Pool implements Shareable, \Countable
{
    public function __construct(
        callable  $factory,
        ?callable $destroy = null,
        int       $maxSize = 32,
        float     $idleTimeout = 300.0,
        ?float    $defaultAcquireTimeout = 5.0,
    );

    public function acquire(float $timeout = 0.0): Pool\Handle;
    public function release(Pool\Handle $handle): void;
    public function with(callable $body, float $timeout = 0.0): mixed;

    public function count(): int;
    public function inUse(): int;
    public function idle(): int;
    public function waiting(): int;
    public function maxSize(): int;

    public function evict(): int;
    public function id(): int;
}

namespace OxPHP\Shared\Pool;

final class Handle
{
    public function get(): mixed;     // 底层资源
}
```

| 方法           | 返回值    | 使用场景                                                          |
|----------------|-----------|-------------------------------------------------------------------|
| `acquire`      | `Handle`  | 取出一个资源；最多阻塞 `$timeout`。`0.0` 使用默认值。             |
| `release`      | void      | 把句柄归还给池。在句柄生命周期内幂等；重复释放会被拒绝。          |
| `with`         | mixed     | 围绕闭包做作用域守护的获取 + 释放。闭包返回值会被透传。优先于手动 `acquire`/`release`。 |
| `count`        | int       | 当前所有线程上的槽数（在用 + 空闲）。实现 `Countable` — 可直接使用 `count($pool)`。 |
| `inUse`        | int       | 当前已被取出的槽数。                                              |
| `idle`         | int       | 在按线程队列中坐等的槽数。                                        |
| `waiting`      | int       | 因等待空闲槽而被挂起的获取数。                                    |
| `maxSize`      | int       | 配置的硬上限。                                                    |
| `evict`        | int       | 强制驱逐调度器立即扫描；返回被丢弃的槽数。                        |
| `id`           | int       | 注册表标识；便于日志 / 可观测性。                                 |

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
    idleTimeout: 60.0,        // 空闲连接 1 分钟后释放
);

// 在请求处理器中
$users = $db->with(function (PDO $conn) use ($userId) {
    $stmt = $conn->prepare('SELECT * FROM users WHERE id = ?');
    $stmt->execute([$userId]);
    return $stmt->fetch();
});
```

`with()` 是寿命最短的模式：进入时获取、返回时释放、抛异常时也释放——你不会泄漏句柄。

### 手动 acquire / release

```php
<?php
$h = $pool->acquire(timeout: 2.0);

try {
    $conn = $h->get();
    $conn->beginTransaction();
    doWork($conn);
    $conn->commit();
} finally {
    $pool->release($h);
}
```

只有当作用域跨越 `with()` 无法表达的边界（例如资源需要在处理器序列的多次调用间存活）时，才需要手动获取。

### 可复用解析器池

```php
<?php
$parsers = new OxPHP\Shared\Pool(
    factory: fn () => new JsonMachine\Parser(),
    maxSize: 8,
);

$doc = $parsers->with(fn ($p) => $p->parse($body));
```

### 短获取超时 + 回退

```php
<?php
try {
    $h = $pool->acquire(timeout: 0.1);    // 100ms
} catch (OxPHP\Shared\TimeoutException $e) {
    // 池已饱和——优雅降级
    http_response_code(503);
    header('Retry-After: 1');
    return;
}

try {
    // ...
} finally {
    $pool->release($h);
}
```

## 工厂与销毁语义

工厂在获取的工作线程上**懒**运行。`maxSize: 32` 的池不会预分配 32 个资源；它在需求到来时按需铸造，受跨所有线程合并的 `maxSize` 约束。

- 工厂应返回非 null 值。返回 `null` 或抛异常会作为异常从 `acquire()` 抛出，且该槽不计入预算。
- 销毁回调（如已提供）会在池丢弃槽时运行：空闲超时到期、显式 `evict()`、服务器关停或池句柄驱逐。它在工作线程上运行（不是在驱动驱逐调度器的 Tokio 线程上），因此可以安全调用 PHP。
- 抛出的销毁回调会被记录但不会毒化池——槽本来就在被销毁，没有什么需要回滚。

## 按线程亲和性

v1 的池是严格按线程的：在工作线程 A 上铸造的槽不能在工作线程 B 上获取。实务上这意味着工作线程 A 上 `idle()` 可能非零，而工作线程 B 在 `acquire()` 上阻塞。这能让槽在使用它的线程上保持热（DB 连接、OPcache 预热的对象），并避免在 CPU 核心间搬运资源。

跨线程的工作窃取是 v1.x 候选项。在那之前，请按工作线程数 × 每线程预期并发来设定 `maxSize`，而不是仅仅按聚合需求。

## 空闲超时驱逐

空闲槽由每 500 毫秒触发一次的后台调度器驱逐。当槽闲置超过 `idleTimeout` 时，调度器会标记它；拥有它的工作线程会在下一次进入 `execute_request` 时销毁它（PHP 引擎此时存活，因此 `$destroy` 在正常请求上下文中运行）。预算在同一时刻被释放。

按重建成本调优 `idleTimeout`：

- **重建廉价**（JSON 解码器、字符串池）：设 10–60 秒；流量回落时迅速释放内存。
- **重建昂贵**（DB 连接、TLS 会话）：设 300 秒（默认）至 900 秒；少付重建代价。
- **永不驱逐**：传一个非常大的数字。不推荐；某个一时热门、之后无限期闲置的槽会成为永远收不回来的内存。

`$pool->evict()` 强制立即扫描并返回被丢弃的槽数。在测试与按需削峰的管理端点中很有用。

## 预算与获取超时

`acquire(timeout)` 行为如下：

| 调用时状态                                  | 结果                                         |
|---------------------------------------------|----------------------------------------------|
| 本线程队列中存在空闲槽                      | 立即复用，工厂不会被调用。                   |
| 无空闲槽，但 `count() < maxSize`            | 工厂运行，铸造新槽。                         |
| `count() == maxSize` 且所有槽 `inUse`       | 最多阻塞 `timeout`，随后抛 `TimeoutException`。 |

`timeout` 传 `0.0` 会使用 `$defaultAcquireTimeout`（默认 5 秒）。要无限等待，请传一个非常大的数字——故意没有「无限」哨兵值，因为永远死等的池比超时抛异常的池更难诊断。

## 异常

| 异常                     | 触发场景                                                             |
|-------------------------|----------------------------------------------------------------------|
| `TimeoutException`      | `acquire` 超过 `$timeout` 仍无空闲槽。                               |
| `CapacityException`     | 即便预算核对后，创建仍会突破 `maxSize`（罕见）。                     |
| `TypeException`         | 非正 `maxSize`、工厂返回非 PHP 对象等。                              |
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄、或对已经被释放的 `Pool\Handle` 调用方法。 |
| `UninitializedException`| 在尚未完成 `__construct` 的 pool 包装上调用 `id()`。                 |

工厂内抛出的异常原样传播给 `acquire()` 调用者，且不消耗预算。`with()` 体内的异常会在句柄被释放后传播给调用者。

## 可观测性

完整内容请见 [Shared 可观测性](../operations/shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ type: "Pool", count, size, in_use, idle, waiting, max_size, idle_timeout_ms }` —— `size` 是 `count` 的已弃用别名。
- `GET /__ox_shared/summary` 统计 Pool 实例和聚合的 `waiting_total`、`evicted_total`。
- 每池 Prometheus 指标：
  - `oxphp_shared_pool_count{pool_id="…"}`           —— 仪表，槽总数。
  - `oxphp_shared_pool_size{pool_id="…"}`            —— 仪表，**已弃用别名** `_count`；将于未来某个发布移除。
  - `oxphp_shared_pool_in_use{pool_id="…"}`          —— 仪表。
  - `oxphp_shared_pool_idle{pool_id="…"}`            —— 仪表。
  - `oxphp_shared_pool_waiting{pool_id="…"}`         —— 仪表，排队的获取数。
  - `oxphp_shared_pool_acquires_total{pool_id="…"}`  —— 计数器。
  - `oxphp_shared_pool_evicted_total{pool_id="…",reason="idle_timeout|manual|shutdown"}` —— 计数器。
  - `oxphp_shared_pool_factory_errors_total{pool_id="…"}` —— 工厂异常计数器。
  - `oxphp_shared_pool_acquire_timeouts_total{pool_id="…"}` —— 获取超时计数器。

值得告警的组合：`waiting` 上升而 `size` 持平，意味着池已饱和、应该扩容；`acquire_timeouts_total` 上升而 `in_use` 正常，意味着工厂慢（或被阻塞）。

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
- [Worker 模式](worker-mode.md) —— 同一工作线程内跨请求复用池句柄。
