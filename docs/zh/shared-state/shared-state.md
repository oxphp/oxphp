---
title: 共享状态
description: 进程级原语——计数器、标志、映射、通道、资源池——让 PHP 工作线程无需依赖 Redis 或 APCu 等外部存储即可协调。
---

# 共享状态

`OxPHP\Shared\*` 是一组并发原语，存活于服务器进程内部，对每个 PHP 工作线程可见。它们让工作线程无需借助 Redis、Memcached 或 APCu，就能协调可变状态——计数器、特性开关、缓存、工作队列、连接池。

此处介绍的一切都完全在进程内。服务器停止时共享状态随之丢失。如果你需要持久化或跨主机协调，请参见[迁移到外部存储](migrating-to-external-store.md)。

## 为什么需要共享状态

在传统 SAPI 下，PHP 工作线程不共享内存。每个工作线程都有自己的 opcode、自己的静态类属性、自己的全局变量。跨工作线程协调状态历来意味着一个进程外依赖：同主机缓存用 APCu，计数器用 Redis，扇出分发用专门的队列代理。

OxPHP 将 PHP 引擎作为多线程的进程内 SAPI 运行。这意味着，一组精心设计的原语*可以*向工作线程提供对同一份状态的安全视图。这正是 `OxPHP\Shared\*` 所提供的：

- **零延迟访问。** 没有网络往返；没有到套接字的序列化。单次操作的开销在微秒量级。
- **无外部依赖。** 少一个需要部署、监控和维护存活的服务。
- **类型化原语。** `Shared\Counter` 是一个原子整数。`Shared\Map` 是一个并发哈希映射。你不必在 `INCR` 语义之上自己实现正确性。
- **循环与生命周期安全。** 运行时跟踪引用，确保句柄不会比其注册表条目存活更久，并拒绝会造成内存泄漏的图结构。

共享状态不是银弹。当你需要跨重启的持久化时，它无法替代 Redis；当你需要多主机时，它也无法替代真正的消息代理。其适用场景在下文 [何时不宜使用](#何时不宜使用) 中说明。

## 心智模型

每个 `Shared\*` 实例背后都对应进程级**注册表**中的一个条目。你持有的 PHP 对象（句柄）携带一个注册表 ID；注册表拥有真正的状态。

```
 ┌──────────────────── PHP 工作线程 1 ────────────────────┐
 │  $counter (Shared\Counter, id=7) ──┐                    │
 └────────────────────────────────────┼───────────────────┘
                                      │
                                      ▼
                            ┌─────────────────────┐
                            │     共享注册表       │
                            │                     │
                            │   id=7: Counter = 42│
                            │   id=8: Map { … }   │
                            │   id=9: Channel(16) │
                            └─────────▲───────────┘
                                      │
 ┌────────────────────────────────────┼───────────────────┐
 │  $counter (Shared\Counter, id=7) ──┘                    │
 └──────────────────── PHP 工作线程 2 ────────────────────┘
```

由此带来的结果：

- **句柄按引用共享状态。** 两个工作线程持有「同一个」计数器时，能立即看到彼此的写入。
- **生命周期随引用。** 当最后一个句柄被释放且没有其他 Shared 条目指向它时，注册表条目才会被释放。
- **禁止 `clone`。** 克隆句柄会产生两个看似独立、实则变更同一注册表条目的 PHP 对象——容易引起混淆和 bug。所有类型在 `clone` 时都会抛出异常。
- **跨线程移交是显式的。** 要把 Shared 值交给后台 fiber，请使用 `oxphp_async(fn () use ($thing) { ... })`。`use` 导入会转移句柄；注册表条目本身是线程安全的。

## v1 原语

OxPHP 0.3 提供七种类型。请按语义而非熟悉度来选择：

| 类型                                     | 形态                      | 适用场景                                                              |
|------------------------------------------|--------------------------|-----------------------------------------------------------------------|
| [`Shared\Counter`](shared-counter.md)    | int64 累加器              | 请求计数、按租户的使用量、特性开关命中跟踪                            |
| [`Shared\Atomic`](shared-atomic.md)      | 通用原子 int64            | 状态机、版本戳、CAS 循环、bitflag 掩码                                |
| [`Shared\Flag`](shared-flag.md)          | 原子 bool                 | 终止开关、一次性初始化标记、熔断器状态                                |
| [`Shared\Once`](shared-once.md)          | 一次性初始化容器          | 跨工作线程的昂贵单例初始化（仅一次运行会胜出）                        |
| [`Shared\Mutex`](shared-mutex.md)        | 带毒化的互斥              | 对非原子值的临界区                                                    |
| [`Shared\Channel`](shared-channel.md)    | 有界 MPMC 队列            | 生产者/消费者流水线、工作扇出                                         |
| [`Shared\Map`](shared-map.md)            | 并发 string→mixed         | 按键缓存、按租户状态、注册表式查询                                    |
| [`Shared\Pool`](shared-pool.md)          | 有界对象池                | 昂贵的每线程资源（DB 句柄、解析器）                                   |

> **优先选择最简单、最契合的类型。** 仅有一个键时，`Counter` 比 `Map<string, int>` 更好；true/false 状态用 `Flag` 胜过 `Counter`；`Mutex<T>` 胜过临时拼凑的 compare-and-set 链。

## 快速开始——并发下的原子计数器

```php
<?php
// worker.php —— Worker 模式入口脚本，每个 PHP 工作线程运行一次
require __DIR__ . '/vendor/autoload.php';

// Registry::counter 把计数器绑定到一个键上。每个运行该引导脚本的工作线程
// 都汇聚到「同一个」条目——整个进程范围内只有一个计数器。
$requests = OxPHP\Shared\Registry::counter(
    'request-counter',
    fn() => new OxPHP\Shared\Counter(),
);

oxphp_worker(function () use ($requests) {
    $requests->add();                 // 跨所有工作线程的原子操作
    header('X-Request-Count: ' . $requests->get());
    echo 'hello';
});
```

[Worker 模式](../features/worker-mode.md) 的外层作用域**每个工作线程运行一次**。`use ($requests)` 捕获使同一个句柄在该工作线程处理的每次请求中都保持存活。`Registry::counter('request-counter', …)` 才是让所有工作线程本地句柄都指向同一个共享条目的关键——工厂在整个进程范围内恰好运行一次，其他工作线程的调用都会找到那个已绑定的条目。

若不使用 `Registry`，直接 `new OxPHP\Shared\Counter()` 的模式会**为每个工作线程产生一个不同的计数器**（每个引导脚本都会创建自己的匿名条目）。这对按工作线程累计的场景没问题，但若要进程级总计，请通过 `Registry`。

同样的形态在传统模式（未启用 `WORKER_MODE_ENABLED`）下也适用。首次触及 `'request-counter'` 的请求会创建条目；后续每个请求——在任意工作线程上——都能看到它。完整说明见 [Shared\Registry](shared-registry.md)。

当 fiber 也需要看到该计数器时，通过 `use` 传入：

```php
<?php
oxphp_async(function () use ($requests) {
    $requests->add();                 // 在任意拾取该 fiber 的工作线程上运行
});
```

> 在两处分别执行 `new OxPHP\Shared\Counter()` 会得到两个独立的计数器，`id()` 不同。共享状态是*按句柄*（构造函数路径）共享，或者*按名称*——通过 `Registry::counter(...)`。请二者择一并坚持下去；不要在每个工作线程里重新构造并期望句柄合并。

## 典范示例——迁移手写计数器

团队常在 APCu 或静态数组上手写协调逻辑。以下以按 IP 的限流器为例，给出迁移到 `Shared\*` 的模式。

### 迁移前：静态 + 外部锁

```php
<?php
// 脆弱：并发下不是原子的、无法在重载后存活
// 在某些 APCu 构建中，还会在同一个池里的无关主机间共享计数器。
final class NaiveRateLimiter
{
    public function __construct(
        private int $maxRequests,
        private int $windowSeconds,
    ) {}

    public function allow(string $ip): bool
    {
        $key     = "rl:{$ip}";
        $now     = time();
        $current = apcu_fetch($key);

        if ($current === false || $now - $current['start'] >= $this->windowSeconds) {
            apcu_store($key, ['count' => 1, 'start' => $now], $this->windowSeconds * 2);
            return true;
        }

        apcu_store(
            $key,
            ['count' => $current['count'] + 1, 'start' => $current['start']],
            $this->windowSeconds * 2,
        );

        return $current['count'] + 1 <= $this->maxRequests;
    }
}
```

该片段隐藏了三个 bug：`apcu_fetch` + `apcu_store` 的组合不是原子的；紧挨着的竞争会丢失窗口起点；清理实际上完全取决于 APCu 的 TTL 驱逐策略。

### 迁移后：Shared\Map + compareAndSet 循环

```php
<?php
final class RateLimiter
{
    public function __construct(
        private OxPHP\Shared\Map $buckets,
        private int $maxRequests,
        private int $windowSeconds,
    ) {}

    public function allow(string $ip): bool
    {
        $now = time();

        // 通过 compareAndSet 实现原子的读-改-写。读取当前桶，
        // 计算下一个状态，并 CAS 写回。若有并发写入者抢先，
        // 就用新值重试。
        while (true) {
            $current = $this->buckets->get($ip);
            if ($current === null || $now - $current['start'] >= $this->windowSeconds) {
                $next = ['count' => 1, 'start' => $now];
            } else {
                $next = ['count' => $current['count'] + 1, 'start' => $current['start']];
            }

            if ($this->buckets->compareAndSet($ip, $current, $next)) {
                return $next['count'] <= $this->maxRequests;
            }
            // 输了竞争——重新读取并重试。
        }
    }

    /** 后台清理——从定时工作线程或 oxphp_async 循环里调用。 */
    public function sweep(): void
    {
        $now    = time();
        $cutoff = $this->windowSeconds * 2;
        $this->buckets->forEach(function (string $ip, array $state) use ($now, $cutoff): void {
            if ($now - $state['start'] >= $cutoff) {
                $this->buckets->remove($ip);
            }
        });
    }
}

// 引导阶段——Registry::map 把 buckets 绑定到一个名称上，让所有工作线程和
// 所有请求都汇聚到「同一个」map。若不使用 Registry，这里裸写的
// `new Shared\Map(...)` 会在 worker 模式下每个工作线程创建一个独立的 map
//（传统模式下则是每次请求一个），实际的限流阈值会随工作线程池规模放大。
$limiter = new RateLimiter(
    buckets:       OxPHP\Shared\Registry::map(
        'rate-limit-buckets',
        fn() => new OxPHP\Shared\Map(maxEntries: 50_000),
    ),
    maxRequests:   100,
    windowSeconds: 60,
);

// 每次请求
if (!$limiter->allow($_SERVER['REMOTE_ADDR'])) {
    http_response_code(429);
    header('Retry-After: 60');
    echo '429 Too Many Requests';
    return;
}
```

这次迁移给你带来的好处：

- **原子性。** `compareAndSet($key, $expected, $next)` 要么提交新值，要么报告竞争让调用方重试——不存在先读后写的覆盖。`setIfAbsent` 一次调用就能覆盖"若不存在则创建，否则保留"的场景。
- **确定性清理。** `sweep()` 可预测，且按你掌控的计划执行。
- **少一个依赖。** APCu 不再出现在部署故事中。
- **高负载下的精确计数。** 同一 IP 上的两次并发命中永远不会彼此覆盖。
- **真正的进程级共享。** `Registry::map` 绑定让 buckets 成为一个共享实例——限流阈值作用于整个服务器，而非按工作线程。

> 内置的 [限流](../features/rate-limiting.md) 功能（`RATE_LIMIT=...`）继续在连接层运行，比 PHP 层限流器更快。只有当你需要 PHP 层的策略（按租户、按路由、按用户 ID，而非按 IP）时，才需要自定义限流器。

### 何时选 Map，何时选 Counter

上例使用 `Map<string, array{count,start}>`，因为每个 IP 需要两个字段保持同步。若只需要没有窗口状态的累计数，`Counter` 更廉价：

```php
<?php
$hits = new OxPHP\Shared\Counter();

// 每次请求都自增，不需要读-改-写循环。
$current = $hits->add();          // 原子地「自增后取值」
```

当你需要按键累计但不涉及窗口逻辑时，选用 `Map<string, Counter>`（计数器的映射）：

```php
<?php
// 按名称分键的按租户计数器。`Registry::counter` 把每个租户的
// 计数器绑定到一个稳定名字上，使所有工作线程都汇聚到同一个
// 条目——同一名字下工厂最多只会执行一次。
$counter = OxPHP\Shared\Registry::counter(
    "tenant:{$tenantId}",
    fn () => new OxPHP\Shared\Counter(),
);
$counter->add();
```

## 句柄语义

每个 `Shared\*` 对象都是对注册表 ID 的一层轻薄 PHP 包装。由此派生几条规则：

1. **身份即注册表 ID，而非 PHP 对象。** 两个 ID 相同的句柄指向同一份状态。相等性测试是 `$a->id() === $b->id()`。
2. **禁止序列化。** `serialize($counter)` 会抛出异常。注册表仅存活于本进程——放到网络上没有任何用处。当你确实需要跨越进程边界时，请使用[迁移指南](migrating-to-external-store.md)。
3. **禁止 `clone`。** 原因同上——克隆后的包装看似独立实则共享状态。如果你想要独立值，请构造新实例。
4. **`id()` 是仅在本进程内有效的不透明令牌。** 在最后一个引用被释放之前保持稳定；可以安全地写入日志、附到 trace span，或在同一进程内传给内部服务器的 [可观测性](shared-observability.md) 端点。该值在进程启动时随机生成，进程之外没有任何意义——不要把它保存到外部存储、会话、cookie 或另一个 OxPHP worker 中。没有 `fromId()` 构造函数；来自其他进程的 id 无法被解析为任何对象。

## 生命周期

条目是引用计数的。只要以下任一条件成立，注册表条目就保持存活：

- 任何工作线程中有 PHP 包装引用它。
- 它作为值存储在另一个存活的 `Shared\Map` / `Shared\Channel` 里。
- 某个待处理的异步操作通过闭包捕获了它。

一旦引用计数归零，注册表会调用类型特定的 `on_drop`（释放嵌套引用、关闭通道、丢弃池槽等），然后释放该槽位。

大多数类型没有显式的 `close()`。`Shared\Channel` 有一个，因为发送方和接收方需要一种方式来通告「没有更多项了」；它并**不**提前释放注册表条目——那仍然需要所有引用都被释放。`Shared\Pool::evict()` 丢弃空闲槽，但保留 pool 本身。

## 循环安全

把一个 Shareable 存进另一个是允许的；但把 A 存进 B 时，B（直接或传递地）已经能到达 A，就会形成循环并泄漏。每一次新增引用的变更都会先运行有界的 BFS，并在触及状态之前以 `OxPHP\Shared\CycleException` 拒绝该写入：

```php
<?php
$a = new OxPHP\Shared\Map();
$b = new OxPHP\Shared\Map();
$a->set('b', $b);                // 正常

try {
    $b->set('a', $a);            // 闭环——被拒绝
} catch (OxPHP\Shared\CycleException $e) {
    // $b 未被修改，没有部分状态、没有对 $a 的泄漏保留
}
```

遍历预算可调：`SHARED_CYCLE_DETECT_DEPTH`（默认 16）和 `SHARED_CYCLE_DETECT_EDGES`（默认 10 000）。超出预算的合法大图会以 `bounds exceeded` 消息抛出 `CycleException`。

## 跨线程移交

共享句柄可以在任意工作线程和异步上下文中安全使用。唯一规则：**通过 `use` 而非 `global` 传递。** 被异步操作捕获的闭包需要显式导入，以便运行时能正确维护引用计数。

```php
<?php
$queue = new OxPHP\Shared\Channel(256);

oxphp_async(function () use ($queue) {              // ← 显式 use
    while (($job = $queue->recvTimeout(30_000)) !== null) {
        process($job);
    }
});

$queue->send(['url' => $_POST['url']]);
```

`Shared\*` 实例**不**可序列化，因此不要尝试通过 shell 命令、HTTP body 或会话存储来传输它们。

## 可观测性

每个注册表条目都可通过[内部服务器](../features/internal-server.md)查看。完整内容请见 [Shared 可观测性](shared-observability.md)。概览：

- `GET /__ox_shared/summary` —— 按类型聚合的计数、内存和操作量。
- `GET /__ox_shared/entries` —— 每个存活条目的 id、类型、引用计数和大小。
- `GET /__ox_shared/entry?id=N` —— 单个条目的类型特定细节。
- `GET /__ox_shared/graph?id=N` —— 出向引用的 BFS 遍历（发生 `CycleException` 后很有用）。
- `/metrics` —— 以 `oxphp_shared_*` 为前缀的 Prometheus 计数器和仪表。

在面向不受信任租户的生产部署中，可通过 `SHARED_INTROSPECTION_ENABLED=false` 关闭内省；指标仍保持开启。

## 配置

所有环境变量都在启动时读取。默认值适用于单机数百个条目；注册表密集的部署可相应调大。

| 环境变量                         | 默认值 | 作用                                                                |
|---------------------------------|--------|---------------------------------------------------------------------|
| `SHARED_MAX_ENTRIES`            | 100 000 | 所有 Shared 条目的全局上限。超过时插入会失败。                      |
| `SHARED_MAX_BYTES`              | 1 GiB   | 所有 Shared 条目估算内存的全局上限。                                 |
| `SHARED_SOFT_LIMIT_RATIO`       | 0.7     | 使用量超过该比例时开始对低优先级工作进行削峰。                       |
| `SHARED_CYCLE_DETECT_DEPTH`     | 16      | 循环检查时的 BFS 深度。对合法深图可调大。                            |
| `SHARED_CYCLE_DETECT_EDGES`     | 10 000  | 循环检查时遍历的边数。对合法稠密图可调大。                           |
| `SHARED_PREVIEW_ARRAY_LIMIT`    | 20      | `/entry?id=…` 预览中采样的条目数。                                   |
| `SHARED_PREVIEW_STRING_LIMIT`   | 256     | 预览时每个字符串的截断长度。                                         |
| `SHARED_INTROSPECTION_ENABLED`  | true    | 开关 `/__ox_shared/*` API。                                          |
| `SHARED_METRICS_ENABLED`        | true    | 开关 `oxphp_shared_*` Prometheus 曝露。                              |

## 何时不宜使用

共享状态是进程内的，这限制了它的适用场景。

- **多主机。** 如果你运行超过一个 OxPHP 进程（任何超出单机的场景都属于这种情况），进程 A 中的工作线程无法看到进程 B 中的 `Shared\*` 条目。请使用 Redis、NATS 或任何你已在使用的方案。[迁移到外部存储](migrating-to-external-store.md) 阐述了常见模式。
- **持久化。** 共享状态在进程重启时消失。如果你的计数器必须在部署后存活，请在别处持久化。
- **无界值。** 没有 `maxEntries` 的 `Shared\Map` 可能被攻击者耗尽内存。任何由用户输入作为键的数据都务必设置上限。
- **大型负载。** 值在读取时会跨 FFI 边界复制。把 10 MB 数组塞进 `Shared\Map` 是形态错误——把大对象放到对象存储，并共享其 URL。
- **替代 OPcache / APCu 缓存。** OPcache 已缓存已编译的脚本；APCu 按工作线程缓存请求范围数据（不需要跨工作线程可见性时更廉价）。

## 常见陷阱

- **忘记给 `Shared\Map` 设上限。** 以用户 IP / 用户 ID / 会话 token 为键的无界 map 是自伤 OOM 的第一大来源。务必传入 `maxEntries` 并捕获 `CapacityException`。
- **每次请求都读取整个数组。** `Map::get` 会复制。如果你在一次请求中对一个大数组访问几十次，请把副本缓存到请求级变量。
- **把 `recv` / `get` 当作非空。** 每次读取都可能合法地返回 `null`（通道已关、键缺失）。务必做 null 检查。
- **把 `global` 用于异步。** 由 `oxphp_async` 启动的 fiber 需要把捕获写在 `use (…)` 子句中。`global` 引用不会被跟踪。
- **`clone` 惊喜。** `clone $counter` 会抛异常。新用户常尝试这么做；请学会替代方案（`new Shared\Counter($counter->get())`）。

## 相关

- [Shared\Counter](shared-counter.md) —— 领域累加器。
- [Shared\Atomic](shared-atomic.md) —— 通用原子 int64,支持完整的内存顺序控制。
- [Shared\Flag](shared-flag.md) —— 原子 bool / 终止开关。
- [Shared\Once](shared-once.md) —— 跨工作线程仅运行一次。
- [Shared\Mutex](shared-mutex.md) —— 对值的带毒化互斥。
- [Shared\Channel](shared-channel.md) —— 有界 MPMC 队列。
- [Shared\Map](shared-map.md) —— 并发按键存储。
- [Shared\Pool](shared-pool.md) —— 有界对象池。
- [Shared\Registry](shared-registry.md) —— 按名键的进程级全局句柄（跨工作线程 / 跨请求的关键）。
- [Shared 可观测性](shared-observability.md) —— 内省、指标、诊断。
- [迁移到外部存储](migrating-to-external-store.md) —— 当共享状态超出单机时。
