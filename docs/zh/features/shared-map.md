---
title: Shared\Map
description: 用于跨 PHP 工作线程协调状态的进程级并发哈希映射——原子读取、批量写入、对嵌套 Shareable 值的循环安全。
---

# Shared\Map

`OxPHP\Shared\Map` 是一个并发的字符串键映射，存放于共享注册表中，对进程内每个 PHP 工作线程都可见。当两个工作线程——或请求处理器与后台任务——需要共享能跨越请求生命周期的可变状态时，它是首选原语。

## 概览

- **String → mixed。** 键是 PHP 字符串。值可以是任意标量、数组（含嵌套数组）或另一个 `Shareable` 实例。
- **并发。** 来自不同工作线程的写入无需外部加锁。按键操作在分片级别是原子的。
- **循环安全。** 存储会最终反向到达本 Map 的 `Shareable` 会在任何变更发生前以 `CycleException` 被拒绝——被拒路径上不会有泄漏。
- **可选的每实例上限。** 构造时传入 `maxEntries` 可获得严格天花板；对已有键的覆写始终允许，新键在达到上限后会以 `CapacityException` 被拒绝。
- **由注册表支撑。** 每个 Map 都有稳定的数字 `id()`；它跨越请求边界存活，并按句柄共享。

## API 参考

```php
namespace OxPHP\Shared;

final class Map implements Shareable
{
    public function __construct(?int $maxEntries = null);

    public function get(string $key, mixed $default = null): mixed;
    public function set(string $key, mixed $value): void;
    public function has(string $key): bool;
    public function remove(string $key): mixed;
    public function clear(): void;
    public function count(): int;
    public function keys(): array;
    public function maxEntries(): ?int;

    public function setIfAbsent(string $key, mixed $value): bool;

    public function setMany(array $kv): int;
    public function getMany(array $keys): array;
    public function removeMany(array $keys): int;

    public function id(): int;
}
```

| 方法          | 使用场景                                                                             |
|---------------|--------------------------------------------------------------------------------------|
| `__construct` | 创建时可选 `maxEntries` 上限（null = 不限）。                                        |
| `get`         | 按键取值；缺失时返回 `$default`（默认 `null`）。                                     |
| `set`         | 插入或替换；覆盖已有值。                                                             |
| `has`         | 不取值的存在性检查。                                                                 |
| `remove`      | 移除键并返回其原值（缺失时为 `null`）。                                              |
| `clear`       | 删除每个条目并释放 Map 对嵌套 `Shareable` 的持有。                                   |
| `count`       | 当前条目数。                                                                         |
| `keys`        | 调用时所有键的快照。迭代顺序未定义（分片顺序）。                                     |
| `maxEntries`  | 报告配置的上限（未设置时为 `null`）。                                                |
| `setIfAbsent` | 原子的「不存在则插入」。存入时返回 `true`，键已存在时返回 `false`。                  |
| `setMany`     | 批量插入；返回在出现任何错误前已存入的键值对数。                                     |
| `getMany`     | 批量读取；缺失的键在按键结果数组中返回 `null`。                                      |
| `removeMany`  | 批量移除；返回实际删除的键数。                                                       |
| `id`          | 注册表数字标识；便于日志 + `/__ox_shared/entry?id=…`。                               |

## 示例

### 共享配置缓存

```php
<?php
$config = new OxPHP\Shared\Map(maxEntries: 1024);

// 在应用引导时预热一次。
$config->setMany([
    'rate_limit.default_rpm' => 600,
    'feature.new_checkout'   => true,
    'timeout.downstream_ms'  => 250,
]);

// 任何请求处理器都可无竞争读取。
$rpm = $config->get('rate_limit.default_rpm', 60);
```

### 按租户限流器

```php
<?php
$buckets = new OxPHP\Shared\Map(maxEntries: 50_000);

$key = "tenant:{$tenantId}";
$created = $buckets->setIfAbsent($key, ['tokens' => 100, 'refill_at' => time() + 60]);
// 若另一请求抢先了，$created 为 false —— 已有桶胜出。

$state = $buckets->get($key);
if ($state['tokens'] === 0) {
    throw new RateLimitException();
}
```

### 跨工作线程协调计数器

```php
<?php
$counters = new OxPHP\Shared\Map();

// 将一个可共享计数器存入键下；跨工作线程的处理器会变更它。
$counters->set('requests_handled', new OxPHP\Shared\Counter());

// 任何工作线程都能通过存储的 Shareable 自增。
$counters->get('requests_handled')->inc();
```

## 语义与陷阱

### 数组在读取时被复制

```php
<?php
$m = new OxPHP\Shared\Map();
$m->set('cfg', ['timeout' => 5, 'retries' => 3]);

$cfg = $m->get('cfg');
$cfg['timeout'] = 10;     // 只变更返回的副本
// $m->get('cfg')['timeout'] 仍然是 5
```

要原子地更新数组值，请「移除 + 设置新形态」，或对独立变更的字段使用嵌套的 `Shared\Counter` / `Shared\Map`。基于闭包的 `update($key, fn)` 会在后续提交中提供。

### 嵌套 Shareable 的保留是自动的

当你 `set($key, $shareable)` 时，Map 会在条目存活期间保留该 Shareable。`remove`、`clear` 或驱逐会释放这份保留。你传入的 PHP 包装独立保持有效：

```php
<?php
$map     = new OxPHP\Shared\Map();
$counter = new OxPHP\Shared\Counter(10);
$map->set('c', $counter);

$retrieved = $map->get('c');           // 同一个 Shareable 身份
$retrieved->inc();                      // 通过 $counter 也能看到变更
echo $counter->get();                   // 11

$map->remove('c');                      // Map 释放它的持有
$counter->inc();                        // $counter 仍通过 PHP 变量存活
```

### 循环检测在变更之前拒绝

```php
<?php
$a = new OxPHP\Shared\Map();
$b = new OxPHP\Shared\Map();
$a->set('b', $b);                       // 正常

try {
    $b->set('a', $a);                   // 闭环
} catch (OxPHP\Shared\CycleException $e) {
    // 消息："cycle would form: #… → #… (inserting into #…)"
}

// $b 未被修改——没有部分状态、没有泄漏的保留。
$b->has('a');                           // false
```

数组内的嵌套引用也会被检查：

```php
try {
    $b->set('shape', ['self' => $a]);
} catch (OxPHP\Shared\CycleException $e) { /* 被拒绝 */ }
```

遍历器受 `SHARED_CYCLE_DETECT_DEPTH`（默认 16）和 `SHARED_CYCLE_DETECT_EDGES`（默认 10 000）约束。非常大的图可能会以消息中带 `bounds exceeded` 的 `CycleException` 显现；调大环境变量，或拆分图结构。

### 每实例上限与覆写

```php
<?php
$m = new OxPHP\Shared\Map(maxEntries: 3);
$m->set('a', 1);
$m->set('b', 2);
$m->set('c', 3);

try {
    $m->set('d', 4);                    // 第 4 个*新*键
} catch (OxPHP\Shared\CapacityException $e) { /* … */ }

$m->set('a', 99);                       // 满上限时覆写始终 OK
```

上限冲突会抛 `CapacityException`。消息会指明上限，运维可据此在构造时调大。

### 批量操作是按键原子，不是按批原子

`setMany`、`getMany` 和 `removeMany` 按键逐个应用操作。如果 `setMany` 在中途遇到 `CapacityException` 或 `CycleException`，先前的键仍被保留——部分成功是故意的，符合规范。如果你需要全有或全无语义，请用 `Mutex<Map>` 将整批包起来（后续版本提供）。

## 异常

所有可能失败的方法都抛出 `OxPHP\Shared\SharedException` 的子类：

| 异常                    | 触发场景                                |
|-------------------------|-----------------------------------------|
| `CapacityException`     | `set` / `setIfAbsent` / `setMany` 超出 `maxEntries`。 |
| `CycleException`        | 任何会闭合可达性循环的写入（`extends TypeException`）。 |
| `TypeException`         | 构造函数接收非正 `maxEntries`；不可序列化值（闭包、资源）；批量键非字符串。 |
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄调用方法。    |
| `UninitializedException`| 在尚未完成 `__construct` 的包装上调用 `id()`。 |

## 可观测性

每个 Map 都可通过内部 API 查看：

- `GET /__ox_shared/summary` —— 按类型聚合的计数，包括 `Map`。
- `GET /__ox_shared/entries` —— 列出所有条目，含 id / 类型 / 引用计数 / mem_bytes。
- `GET /__ox_shared/entry?id=N` —— Map 的每实例细节包含 `key_count`、`max_entries`、`saturation` 和 `sample_keys`（按预览上限截断）。
- `GET /__ox_shared/graph?id=N[&depth=D][&edges=E]` —— 出向 Shareable 引用的 BFS 遍历。当 `CycleException` 触发、你想看遍历器走过的路径时很方便。

Prometheus 在 `/metrics` 暴露每 Map 的仪表：

| 指标                                   | 含义                                      |
|----------------------------------------|-------------------------------------------|
| `oxphp_shared_map_entries{map_id="…"}` | 当前键数。                                |
| `oxphp_shared_map_max_entries{map_id="…"}` | 配置的上限（不限时为 0）。            |
| `oxphp_shared_map_saturation{map_id="…"}` | `entries / max_entries`，不限时为 0。  |

注册表级仪表（`oxphp_shared_objects_total`、`oxphp_shared_bytes`、`oxphp_shared_capacity_saturation`）通过 `type="Map"` 标签自动覆盖 Map。

## 配置

| 环境变量                         | 默认值 | 作用                                                                 |
|---------------------------------|--------|---------------------------------------------------------------------|
| `SHARED_MAX_ENTRIES`            | 100 000 | 所有 Shared 条目的全局上限。                                         |
| `SHARED_MAX_BYTES`              | 1 GiB   | 所有 Shared 条目估算内存的全局上限。                                 |
| `SHARED_CYCLE_DETECT_DEPTH`     | 16      | 循环检查中的最大 BFS 深度。对合法深图可调大。                        |
| `SHARED_CYCLE_DETECT_EDGES`     | 10 000  | 循环检查中遍历的最大边数。对合法稠密图可调大。                       |
| `SHARED_PREVIEW_ARRAY_LIMIT`    | 20      | `/entry?id=…` 中 `sample_keys` 采样的条目数。                        |
| `SHARED_INTROSPECTION_ENABLED`  | true    | 开关 `/__ox_shared/*` API。                                          |

## 相关

- [`Shared\Counter`](shared-counter.md) —— 原子整数；存入 Map 以实现按键命中计数。
- [`Shared\Channel`](shared-channel.md) —— MPMC 队列；需要 FIFO 流水线而非按键查找时互补。
- [`Shared\Mutex`](shared-mutex.md) —— 需要对存储值严格互斥时。
