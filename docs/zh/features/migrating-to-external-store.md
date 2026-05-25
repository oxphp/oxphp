---
title: 将 Shared\* 迁移到外部存储
description: 何时把进程内的 Shared\* 状态升级到 Redis、NATS 或其他持久存储——具体的模式、语义差异，以及每种 Shared\* 类型的迁移形态。
---

# 将 Shared\* 迁移到外部存储

`OxPHP\Shared\*` 在进程内。这让它快、且无依赖，但也把你限制在一台主机和一个进程生命周期。本文是逃生口：当你需要多主机协调或跨重启的持久化时，下面的内容讲述如何把每种 Shared 类型迁移到 Redis 或 NATS（或类似）后端，而无需重写应用。

## 何时迁移

你大概率不需要迁移。`Shared\*` 的甜蜜区——单主机、临时性、微秒级延迟的协调——比人们想象覆盖更多生产场景。仅当下列之一为真时才迁移到外部存储：

1. **你运行多个 OxPHP 进程。** 多主机、有重叠期的蓝绿部署、需要看到同一状态的边车。`Shared\*` 是进程本地的，无法跨越进程边界。
2. **状态必须跨重启存活。** 滚动部署、崩溃或例行重启都会丢失每个 `Shared\*` 条目。如果丢失不可接受（计费计数、每日配额、工作队列位置），你需要持久化。
3. **状态必须跨主机存活。** 如果你的任何主机都可能消失而状态仍需存在，那它就该住在本机之外的地方。
4. **你想要跨语言读取者。** 外部存储可以被 Go 写的后台作业、指标管道或管理工具读取。`Shared\*` 仅限 PHP。

如果上述都不适用，进程内原语几乎一定是正确选择。把迁移方案放在口袋里，而不是放在热路径上。

## 抽象

大多数团队采用相同的形态：一个接口配两个后端，由配置选择。

```php
<?php
interface CounterBackend
{
    public function inc(string $key, int $by = 1): int;
    public function get(string $key): int;
    public function reset(string $key): int;
}

final class SharedCounterBackend implements CounterBackend
{
    public function __construct(private OxPHP\Shared\Map $counters) {}

    public function inc(string $key, int $by = 1): int
    {
        $counter = $this->counters->getOrSet(
            $key,
            fn () => new OxPHP\Shared\Counter(),
        );
        return $counter->add($by);
    }

    public function get(string $key): int
    {
        $counter = $this->counters->get($key);
        return $counter?->get() ?? 0;
    }

    public function reset(string $key): int
    {
        $counter = $this->counters->get($key);
        return $counter?->set(0) ?? 0;
    }
}

final class RedisCounterBackend implements CounterBackend
{
    public function __construct(private Redis $redis) {}

    public function inc(string $key, int $by = 1): int
    {
        return (int) $this->redis->incrBy("counter:{$key}", $by);
    }

    public function get(string $key): int
    {
        return (int) ($this->redis->get("counter:{$key}") ?? 0);
    }

    public function reset(string $key): int
    {
        // GETSET 是原子的：一次往返，返回原值。
        return (int) ($this->redis->getSet("counter:{$key}", 0) ?? 0);
    }
}
```

在引导阶段一次性接入选定后端，业务里到处使用 `CounterBackend`。迁移就变成配置切换，而非改写。

## 按类型的迁移说明

每种 `Shared\*` 类型都有一些不能简单平移到任何外部存储的语义癖好。下面的说明指出差异和惯用替代品。

### `Shared\Counter` → Redis / NATS JetStream KV

- **Redis：** `INCR` / `INCRBY` / `GET`。原子、持久，并在 Redis Cluster 中复制。
- **NATS JetStream KV：** `KV.put` 配基于版本号的 CAS 同时覆盖 `set` 和 `compareAndSet`。自增需要 `KV.get` + `KV.update(revision)` 循环。

语义差异：

- 批量累加即 `add(array_sum($deltas))`，在 `Shared\*` 中是单次 FFI 往返。Redis 中先求和再做一次 `INCRBY`（一次 RTT）；NATS 中是一次 `KV.update`。
- Redis 的整数溢出会返回错误；`Shared\Counter` 静默回绕。

### `Shared\Flag` → Redis / NATS 特性开关服务

- **Redis：** `SET` / `GET` / `SETNX` 提供类似 `compareAndSet` 的语义。字符串值 `"1"` / `"0"` 即可工作；通过 `GETSET` + 字符串比较表达布尔更整洁。
- **专用开关服务：**（LaunchDarkly、Unleash、ConfigCat）开箱即用地处理缓存、灰度定位和审计追踪。一旦你越过 `Shared\*` 阈值，对运维型终止开关，这通常是正解。

语义差异：

- `exchange($new)` → Redis `GETSET`。原子。
- `compareAndSet($expect, $new)` → Lua 脚本或 `WATCH`/`MULTI`。值得封装为帮助函数。
- 外部开关服务通常会在本地缓存值；你的读取并不总是网络往返。这通常没问题，但变更时要预期最终一致。

### `Shared\Once` → 数据库引导表

- **模式：** 带唯一约束的幂等 INSERT，冲突时再 SELECT。
- **SQL：** `INSERT INTO once (key, value) VALUES (?, ?) ON CONFLICT (key) DO NOTHING; SELECT value FROM once WHERE key = ?`。
- **Redis：** `SETNX` + `GET`。

语义差异：

- `Shared\Once::getOrInit(callable)` 在胜出时进程内运行工厂。在外部存储中，工厂必须幂等（两个写入者可能都运行它，只有一个值胜出），或者你需要一个领导选举的包装。
- 重入时的 `DeadlockException` 没有外部等价物——你继承存储本身的行为，通常是无。

### `Shared\Mutex` → Redis 分布式锁

- **Redis：** “Redlock”模式，或者保证更宽松时使用更简单的 `SET NX EX` 单键锁。`cheprasov/php-redis-lock` 等库对此做了封装。
- **etcd / Consul / Zookeeper：** 基于会话的锁与租约续约。运维负担更重，但保证更强。

语义差异：

- **这是最难的迁移。** 进程内 mutex 瞬时且正确；分布式锁慢、且仅尽力而为。请假定语义会变化——按至少一次、幂等的临界区来设计。
- `Shared\Mutex` 的 `with($fn)` 会原子地把闭包返回值提交回受保护的存储。Redis 锁下你必须显式读取、计算、写入，且写入可能与无关操作竞争。
- 毒化：外部锁没有「毒化」状态。如果分布式临界区中你的闭包抛异常，你会释放锁并让下一个调用者看到半提交的状态。请通过补偿动作来处理一致性，而不是模仿 `isPoisoned()`。

### `Shared\Channel` → NATS JetStream / Redis Streams / SQS / Kafka

- **NATS JetStream：** 语义上最贴近。持久、有界、MPMC，带消费者偏移和至少一次投递。
- **Redis Streams：** `XADD` / `XREADGROUP` 覆盖基本队列模式。消费者组对应 `Shared\Channel` 的多消费者语义。
- **SQS / Kafka：** 业界主力。Kafka 适合高吞吐事件流；SQS 适合简单工作队列。

语义差异：

- **阻塞 `recv` 被替换为长轮询。** 你的消费者代码从「关闭时返回 null」变成「带超时轮询、处理重连」。
- **`sendMany` 批处理** 映射到 Kafka 的 linger/batch 配置或 Redis 流水线。
- **`close()`** 没有外部对应物。优雅地停止生产者并让消费者排空；没有「永远不再有项」的信号。
- **进程内有序性** 在跨网络时变成至少一次投递。消费侧的幂等键是必备的。

### `Shared\Map` → Redis 哈希 / KV 服务 / 数据库

- **Redis 哈希：** `HGET` / `HSET` / `HDEL` / `HSCAN` 覆盖按键映射形态。
- **键化字符串值：** `SET key:<k> value`，通过 LRU 驱逐强制 `maxEntries`。
- **带 TTL 列的数据库表：** 行即条目；后台清扫器处理驱逐。当值大于几百字节时，这是你想要的。

语义差异：

- `update($key, $fn)` 在 Redis 中必须变成服务端 Lua 脚本（以保持 RMW 原子），或在 SQL 中使用 `SELECT ... FOR UPDATE`。普通 `HGET` + 计算 + `HSET` 失去原子性。
- Map 的**循环安全**在外部不存在。你不会闭合循环，因为没有可闭合的 Shareable 图。
- **嵌套 Shareable** 变成「单独键、值中编码指针」。簿记由你负责。

### `Shared\Pool` → 客户端库自带的池

- **优先使用库自身的池。** PDO、Guzzle、HTTP 客户端和大多数数据库驱动都有成熟的池化。不要用 `Shared\Pool` 重新发明。
- **代理服务：** 对每主机的 Postgres/MySQL 池化，pgbouncer / proxysql 把池化边界放到基础设施层。你的 PHP 侧重新无状态。

语义差异：

- 池的空闲超时驱逐被库自身的健康检查替代。
- 工厂/销毁回调被库的连接生命周期替代。
- 跨主机时，你可能需要**按服务**的池（每个下游一个），而不是一个大池。

## 一个具体案例：按租户限流器

下面是 `shared-state.md` 中的[限流器示例](shared-state.md#典范示例迁移手写计数器)，改写在后端接口背后：

```php
<?php
interface RateLimiterBackend
{
    public function allow(string $key, int $max, int $windowSecs): bool;
}

final class SharedRateLimiterBackend implements RateLimiterBackend
{
    public function __construct(private OxPHP\Shared\Map $buckets) {}

    public function allow(string $key, int $max, int $windowSecs): bool
    {
        $now = time();
        $state = $this->buckets->update($key, function ($current) use ($now, $windowSecs) {
            if ($current === null || $now - $current['start'] >= $windowSecs) {
                return ['count' => 1, 'start' => $now];
            }
            return ['count' => $current['count'] + 1, 'start' => $current['start']];
        });
        return $state['count'] <= $max;
    }
}

final class RedisRateLimiterBackend implements RateLimiterBackend
{
    /**
     * 原子的固定窗口计数器。在引导时通过
     * `$redis->script('load', $lua)` 加载一次该脚本，并保留得到的 SHA。
     */
    private const SCRIPT = <<<'LUA'
        local current = redis.call('GET', KEYS[1])
        if current then
            local c = tonumber(current) + 1
            redis.call('SET', KEYS[1], c, 'KEEPTTL')
            return c
        end
        redis.call('SET', KEYS[1], 1, 'EX', ARGV[1])
        return 1
    LUA;

    public function __construct(
        private Redis $redis,
        private string $scriptSha,
    ) {}

    public static function withLoadedScript(Redis $redis): self
    {
        $sha = $redis->script('load', self::SCRIPT);
        return new self($redis, $sha);
    }

    public function allow(string $key, int $max, int $windowSecs): bool
    {
        $count = (int) $this->redis->evalSha($this->scriptSha, ["rl:{$key}"], [$windowSecs]);
        return $count <= $max;
    }
}
```

单主机和多主机部署之间唯一变化的，是引导时接入哪个后端。应用其余部分一律对 `RateLimiterBackend` 编程。

## 混合模式

### 在外部状态前加本地缓存

读密集型负载常把 `Shared\Map` 当作外部存储前的 TTL 缓存。你每 N 秒命中一次 Redis；每秒命中数千次 `Shared\Map`。

```php
<?php
$cfg = $cache->getOrSet($tenantId, fn () => loadFromRedis($tenantId));
```

通过所有 OxPHP 进程都订阅的 Redis pub/sub 通道使其失效，或在本地 Map 中用 TTL 过期。

### 直写缓冲

写密集型负载在 `Shared\Channel` 中缓冲，由后台消费者刷写到外部存储。你在进程内吸收突发，并摊销网络开销。

```php
<?php
$writes = new OxPHP\Shared\Channel(capacity: 10_000);

oxphp_async(function () use ($writes) {
    while (($batch = $writes->recvMany(max: 100, timeout: 0.5))) {
        writeBatchToRedis($batch);
    }
});

// 热路径
$writes->trySend([$key, $value]);
```

权衡：如果进程在刷写完成前死掉，你会丢失缓冲项。适合分析数据，不适合计费。

## 检查清单

切换之前：

- [ ] 确定迁移背后的那一个 `Shared\*` 原语。不要一次「全部」迁移。
- [ ] 抽出接口；接入两个后端。
- [ ] 决定一致性——至多一次还是至少一次——并在接口中显式表达。
- [ ] 用同一套集成测试套件测试两个后端。
- [ ] 测量延迟。外部存储每次操作增加 0.1–5 毫秒——验证你的应用在热路径上能吸收它。
- [ ] 为外部存储下线做准备：失败开放（放行请求）还是失败关闭（返回 503）？正确答案因业务而异。
- [ ] 在切换前后开启 `Shared\*` 后端上的 `oxphp_shared_*` 指标，便于对比。

## 相关

- [共享状态](shared-state.md) —— 概览；何时留在进程内。
- [Shared 可观测性](../operations/shared-observability.md) —— 用同样的方式给两个后端打点。
- [限流](rate-limiting.md) —— 内置的按 IP 限流器（在 PHP 之前运行；与 PHP 层限流正交）。
