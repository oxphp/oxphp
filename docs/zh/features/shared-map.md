---
title: Shared\Map
description: 用于跨 PHP 工作线程协调状态的进程级并发哈希映射——「null 即缺失」模型、单一可线性化的 compareAndSet、惰性迭代，以及对嵌套 Shareable 值的循环安全。
---

# Shared\Map

`OxPHP\Shared\Map` 是一个并发映射，存放于共享注册表中，对进程内每个 PHP 工作线程都可见。当两个工作线程——或请求处理器与后台任务——需要共享能跨越请求生命周期的可变状态时，它是首选原语。

## 概览

- **`int|string → mixed`。** 键是 PHP 整数或字符串，且保持**互不相同**（`123` 与 `"123"` 是不同的键；不存在 PHP 数组式的键强制转换）。字符串键是**二进制安全**的——以不透明字节存储（与 PHP 数组 / Go / Redis 一致），因此非 UTF-8 键（包括内嵌 NUL）也能被忠实地往返保存。值可以是任意标量、标量/数组组成的数组，或另一个 `Shareable` 实例。
- **`null` 表示缺失——绝不会作为存储值。** 写入 `null` 值会抛出 `TypeException`；返回 `null` 始终表示「无此键」。这从根本上消除了经典的「`get()` 返回 null」歧义（与 `java.util.concurrent.ConcurrentHashMap` 和 Go 的 `sync.Map` 相同的选择）。
- **单一可线性化的条件原语。** `compareAndSet` 借助 `null` 缺失哨兵涵盖原子的插入 / 替换 / 移除；任何 read-modify-write 都可在它之上构建。
- **并发。** 来自不同工作线程的写入无需外部加锁；按键操作在分片级别是原子的。
- **循环安全。** 存储会最终反向到达本 Map 的 `Shareable` 会在任何变更发生前以 `CycleException` 被拒绝——被拒路径上不会有泄漏。
- **由近似软上限约束。** `maxEntries` 是 OOM 安全天花板，而非精确计数。

## API 参考

```php
namespace OxPHP\Shared;

final class Map implements Shareable
{
    public function __construct(?int $maxEntries = null);   // null = 不限；<= 0 抛出 TypeException

    // 读取
    public function get(int|string $key): mixed;            // null ⟺ 缺失
    public function getMany(iterable $keys): \Iterator;     // 惰性；跳过缺失的键
    public function count(): int;                           // 分条计数，弱一致
    public function maxEntries(): ?int;

    // 写入
    public function set(int|string $key, mixed $value): void;
    public function setIfAbsent(int|string $key, mixed $value): mixed;  // prev；null ⟺ 已插入
    public function setMany(iterable $entries): int;
    public function remove(int|string $key): bool;          // 是否存在过？
    public function removeMany(iterable $keys): int;
    public function clear(): int;                           // 移除的条目数

    // 返回值
    public function swap(int|string $key, mixed $value): mixed;  // prev；null ⟺ 原本缺失
    public function pop(int|string $key): mixed;                 // prev；null ⟺ 原本缺失

    // 条件（单一可线性化原语）
    public function compareAndSet(int|string $key, mixed $expected, mixed $new): bool;

    // 迭代
    public function forEach(callable $fn): void;            // 弱一致；回调在不持锁的情况下运行

    public function id(): int;
}
```

| 方法          | 使用场景                                                                             |
|---------------|--------------------------------------------------------------------------------------|
| `__construct` | 创建时可选 `maxEntries` 上限（null = 不限；`<= 0` 抛异常）。                         |
| `get`         | 按键取值；`null` ⟺ 缺失。                                                            |
| `getMany`     | 惰性流式返回已知键的 `key => value`；缺失的键被跳过（见下文）。                      |
| `count`       | 近似条目数（并发写入下弱一致）。                                                     |
| `maxEntries`  | 报告配置的上限（不限时为 `null`）。                                                  |
| `set`         | 插入或替换；不会物化先前值。                                                         |
| `setIfAbsent` | 原子的「不存在则插入」；返回已有值，若执行了插入则返回 `null`。                      |
| `setMany`     | 从任意 iterable 批量插入；返回写入的数量。                                           |
| `remove`      | 移除键；返回它是否存在过（不会物化值）。                                             |
| `removeMany`  | 批量移除；返回实际删除的数量。                                                       |
| `clear`       | 删除每个条目（释放对嵌套 `Shareable` 的持有）；返回移除的数量。                      |
| `swap`        | 覆写并返回先前值（`null` ⟺ 原本缺失）。                                              |
| `pop`         | 移除并返回先前值（`null` ⟺ 原本缺失）。                                              |
| `compareAndSet`| 依据当前内容进行原子的插入 / 替换 / 移除（见下文）。                                |
| `forEach`     | 弱一致遍历；回调在不持锁的情况下运行。                                               |
| `id`          | 注册表数字标识；便于日志 + `/__ox_shared/entry?id=…`。                               |

这里没有 `has()`、`update()`、`getOrSet()`、`keys()`、`trySet()`、`updateMany()`，并且本类**不**实现 `Countable` —— 见[从旧接口迁移](#从旧接口迁移)。

## 「null 即缺失」模型

`null` 在各处都被保留为缺失哨兵：

- `set` / `swap` / `setIfAbsent` 传入 `null` 值 → `TypeException`。
- `get` / `swap` / `pop` / `setIfAbsent` 返回 `null` ⟺ 该键缺失。
- 在 `compareAndSet` 中，任一侧的 `null` 都表示「缺失」（而非「存储 null」）。

若你需要记录「无值」，请移除该键（或利用键的缺失），而不要存储 `null`。由于没有 `has()`，也就没有并发的 `has()`+`get()` 竞态，存在性可用单次 `get($k) !== null` 原子地检查。

## compareAndSet —— 条件原语

```php
$map->compareAndSet($key, expected: null, new: $v);    // 仅当缺失时插入   (= setIfAbsent，返回 bool)
$map->compareAndSet($key, expected: $a,   new: $b);    // 仅当当前 === $a 时替换
$map->compareAndSet($key, expected: $a,   new: null);  // 仅当当前 === $a 时移除
```

仅当 swap 被实际应用时返回 `true`。相等性按**内容**判定：标量按值，字符串与数组按其序列化字节，嵌套的 `Shareable` 值按注册表身份。数组相等性在常见场景下与 PHP `===` 一致（列表、纯 int 键或纯 string 键的数组）；而**交错**使用 int 与 string 键的数组按 Map 的归一化存储形式比较，因此不区分具体的 int/string 顺序（这类数组在读回时也会被 Map 重新排序）。请把 read-modify-write 写成显式的重试循环——并保持闭包纯净，因为在竞争下它会运行不止一次：

```php
do {
    $cur = $map->get('counter');          // 缺失则为 null
    $next = ($cur ?? 0) + 1;
} while (!$map->compareAndSet('counter', $cur, $next));
```

这里没有 ABA 隐患：存储是内容寻址的（内容相等的值对值存储而言*就是*同一个值），而嵌套 `Shareable` 的身份使用单调、永不复用的注册表 id。需要抵御 stampede 的惰性初始化请用 [`Shared\Once`](shared-once.md)；需要池化资源请用 [`Shared\Pool`](shared-pool.md)。

## 内存模型 —— 复制发生在何处

值以序列化表示存储，**而非**以 zval 存储——因此「zero-copy」不适用于值：

| 操作                              | 序列化进共享堆 | 把先前值物化为 zval |
|-----------------------------------|:--------------:|:-------------------:|
| `set` / `setMany`                 | 是             | 否                  |
| `remove` / `removeMany`           | —              | 否                  |
| `setIfAbsent`                     | 是             | 仅当存在先前值时     |
| `swap` / `pop`                    | 是 / —         | 是                  |
| `get` / `getMany`                 | （仅键）       | 是                  |
| `compareAndSet`                   | 是（`$new`）   | 否                  |

对任何进入共享内存的值，写入路径的序列化都不可避免。读回到新 zval 的开销**只**由那些返回先前值/查得值的方法承担——所以 `set`/`remove` 是「无返回物化」，而非「免费」。例外是**嵌套 `Shareable`** 值：它按引用存储（一个 id 加一次 refcount 自增），而非深拷贝。

## 并发

- **`count()` 是弱一致的。** 条目计数按分片分条统计，并在读取时求和；当映射处于静止时结果是精确的，并发写入下则是接近的近似值（与 `ConcurrentHashMap::size` 相同的契约）。分条统计使写入避开单个热点计数器。
- **`maxEntries` 是软上限。** 它针对分条求和进行检查，因此在并发插入下，映射在以 `CapacityException` 拒绝新键之前最多可能超出分片数那么多。请把它当作 OOM 安全预算，而非精确计数。在上限处覆写已有键始终成功。**没有驱逐**——带 LRU/TTL 驱逐的缓存是另一种原语。
- **`forEach` 在不持锁的情况下运行回调。** 它一次快照一个分片的键，释放该分片，然后重新取回每个值并调用 `$fn(key, value)`。在快照与调用之间被删除的键会被跳过；在某分片快照之后新增的键可能被遗漏；值可能比快照时刻更新。从回调返回 `false` 可提前停止。由于只快照键，慢速回调绝不会钉住已删除的值。

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

// 任何请求处理器都可无竞争读取；null ⟺ 未配置。
$rpm = $config->get('rate_limit.default_rpm') ?? 60;
```

### 按租户限流器

```php
<?php
$buckets = new OxPHP\Shared\Map(maxEntries: 50_000);

$key  = "tenant:{$tenantId}";
$prev = $buckets->setIfAbsent($key, ['tokens' => 100, 'refill_at' => time() + 60]);
// $prev === null ⟺ 我们创建了桶；否则它持有已有的那个。

$state = $buckets->get($key);
if ($state['tokens'] === 0) {
    throw new RateLimitException();
}
```

### 跨工作线程协调计数器

```php
<?php
$counters = new OxPHP\Shared\Map();
$counters->set('requests_handled', new OxPHP\Shared\Counter());

// 任何工作线程都能通过存储的 Shareable 自增（按引用存储）。
$counters->get('requests_handled')->inc();
```

### 迭代大型映射

```php
<?php
$sessions->forEach(function (int|string $key, mixed $value): bool|null {
    if ($value['expires_at'] < time()) {
        // 安全：forEach 在回调期间不持锁
        return null; // 继续
    }
    return null;
});

// 或者惰性读取已知子集，并提前停止：
foreach ($cache->getMany($hotKeys) as $key => $value) {
    if (enoughCollected()) break;     // 剩余的键永不被物化
    handle($key, $value);
}
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

要原子地更新数组值，请读取它、修改副本、再用 `compareAndSet` 提交（冲突时重试），或把独立变更的字段存为嵌套的 `Shared\Counter` / `Shared\Map`。

### 嵌套 Shareable 的保留是自动的

```php
<?php
$map     = new OxPHP\Shared\Map();
$counter = new OxPHP\Shared\Counter(10);
$map->set('c', $counter);

$retrieved = $map->get('c');           // 同一个 Shareable 身份
$retrieved->inc();                      // 通过 $counter 也能看到变更
echo $counter->get();                   // 11

$counter2 = $map->pop('c');             // Map 释放它的持有，并返回该值
$counter2->inc();                       // 通过返回的包装仍然存活
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

$b->get('a');                           // null —— $b 未被触碰，无泄漏的保留
```

数组内的嵌套引用也会被检查。遍历器受 `SHARED_CYCLE_DETECT_DEPTH`（默认 16）和 `SHARED_CYCLE_DETECT_EDGES`（默认 10 000）约束；非常大的图会以带 `bounds exceeded` 的 `CycleException` 显现——调大环境变量，或拆分图结构。

### 单值大小上限

单个值若其序列化大小超过 `SHARED_MAX_VALUE_SIZE`（默认 1 MiB），会以 `ValueTooLargeException` 被拒绝。这可防范来自 PHP 侧输入的分配炸弹。它适用于每条写入路径（`set`、`setIfAbsent`、`swap`、`compareAndSet`、`setMany`）。

### 批量操作是按键原子，不是按批原子

`setMany`、`getMany` 和 `removeMany` 按键逐个应用操作。如果 `setMany` 在中途遇到 `CapacityException`、`CycleException` 或 `ValueTooLargeException`，先前的键仍被保留——部分成功是故意的。如果你需要全有或全无语义，请用 `Shared\Mutex` 将映射包起来。

## 从旧接口迁移

这是一次破坏性重设计，没有兼容垫片：

| 旧                                    | 新                                                              |
|---------------------------------------|------------------------------------------------------------------|
| `has($k)`                             | `get($k) !== null`（原子——无 `has`/`get` 竞态）                 |
| `get($k, $default)`                   | `get($k) ?? $default`                                            |
| `trySet($k, $v): bool`                | `setIfAbsent($k, $v): mixed`（返回 prev；`null` ⟺ 已插入）       |
| `remove($k)`（返回 prev）             | `remove($k): bool`，或用 `pop($k)` 取值                          |
| `update($k, $fn)`                     | `compareAndSet` 重试循环，或用 `Shared\Once` 做一次性初始化      |
| `getOrSet($k, $fn)`                   | `setIfAbsent`，或按情形用 `Shared\Once` / `Shared\Pool`          |
| `updateMany(...)`                     | 一个 `compareAndSet` 的循环                                      |
| `keys(): array`                       | `forEach(...)`，或 `getMany($knownKeys)`                         |
| `count($map)`（Countable）            | `$map->count()`                                                 |
| 存储 `null` 值                        | 使用键的缺失 / `remove`                                          |

## 异常

所有可能失败的方法都抛出 `OxPHP\Shared\SharedException` 的子类：

| 异常                    | 触发场景                                                                  |
|-------------------------|---------------------------------------------------------------------------|
| `CapacityException`     | 超出 `maxEntries` 的新键（`set` / `setIfAbsent` / `compareAndSet` / `setMany`）。 |
| `ValueTooLargeException`| 超出单值上限的值（`SHARED_MAX_VALUE_SIZE`）。                              |
| `CycleException`        | 会闭合可达性循环的写入（`extends TypeException`）。                        |
| `TypeException`         | `null` 值；不可存储的值（对象/闭包/资源）；非 int/string 的键；`maxEntries <= 0`。 |
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄调用方法。                                       |

## 可观测性

每个 Map 都可通过内部 API 查看：

- `GET /__ox_shared/summary` —— 按类型聚合的计数，包括 `Map`。
- `GET /__ox_shared/entries` —— 列出所有条目，含 id / type / refcount / mem_bytes。
- `GET /__ox_shared/entry?id=N` —— Map 的每实例细节包含 `key_count`、`max_entries`、`saturation` 和 `sample_keys`（按预览上限截断）。
- `GET /__ox_shared/graph?id=N[&depth=D][&edges=E]` —— 出向 Shareable 引用的 BFS 遍历；在 `CycleException` 之后很方便。

Prometheus 在 `/metrics` 暴露每 Map 的仪表：

| 指标                                   | 含义                                      |
|----------------------------------------|-------------------------------------------|
| `oxphp_shared_map_entries{map_id="…"}` | 当前（近似）键数。                        |
| `oxphp_shared_map_max_entries{map_id="…"}` | 配置的上限（不限时为 0）。            |
| `oxphp_shared_map_saturation{map_id="…"}` | `entries / max_entries`，不限时为 0。  |

## 配置

| 环境变量                         | 默认值 | 作用                                                                 |
|---------------------------------|--------|---------------------------------------------------------------------|
| `SHARED_MAX_ENTRIES`            | 100 000 | 所有 Shared 条目的全局上限。                                         |
| `SHARED_MAX_BYTES`              | 1 GiB   | 所有 Shared 条目估算内存的全局上限。                                 |
| `SHARED_MAX_VALUE_SIZE`         | 1 MiB   | 单值序列化大小上限；更大的值抛出 `ValueTooLargeException`。           |
| `SHARED_CYCLE_DETECT_DEPTH`     | 16      | 循环检查中的最大 BFS 深度。对合法深图可调大。                        |
| `SHARED_CYCLE_DETECT_EDGES`     | 10 000  | 循环检查中遍历的最大边数。对合法稠密图可调大。                       |
| `SHARED_PREVIEW_ARRAY_LIMIT`    | 20      | `/entry?id=…` 中 `sample_keys` 采样的条目数。                        |
| `SHARED_INTROSPECTION_ENABLED`  | true    | 开关 `/__ox_shared/*` API。                                          |

## 相关

- [`Shared\Counter`](shared-counter.md) —— 原子整数；存入 Map 以实现按键命中计数。
- [`Shared\Once`](shared-once.md) —— 当 `setIfAbsent` 会重跑昂贵工厂时，用于抵御 stampede 的惰性初始化。
- [`Shared\Channel`](shared-channel.md) —— MPMC 队列；需要 FIFO 流水线而非按键查找时互补。
- [`Shared\Mutex`](shared-mutex.md) —— 需要对存储值严格互斥时。
