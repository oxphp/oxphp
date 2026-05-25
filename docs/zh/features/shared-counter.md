---
title: Shared\Counter
description: 跨 PHP 工作线程共享的原子 int64 累加器——无锁有符号加法、原子交换、compare-and-set、通过 set(0) 的窗口重置。
---

# Shared\Counter

`OxPHP\Shared\Counter` 是进程级的原子 64 位有符号整数,专门用于**累加**:计数事件、汇总增量、滚动窗口合计。每个操作都是无锁的;两个工作线程并发累加永远不会丢失一次计数。

如需任意原子状态且必须*同步其他内存*——状态机、版本戳、seqlock、bitflag 掩码——请使用 [`Shared\Atomic`](shared-atomic.md)。

## 概览

- **原子 int64。** 范围 `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`。溢出会回绕。
- **无锁。** `add` 编译为单次 `fetch_add`。
- **始终 Relaxed。** 操作是原子的(不丢计数、无撕裂读取),但*不与其他内存建立 happens-before*。Counter 是统计量,而非同步点——若需要顺序,请使用 `Shared\Atomic`。
- **可共享。** 实例可存放在 `Shared\Map` / `Shared\Channel` 内,并通过 `use` 捕获交给 fiber。

## API 参考

```php
namespace OxPHP\Shared;

final class Counter implements Shareable
{
    public function __construct(int $initial = 0);

    public function get(): int;                            // 当前值
    public function set(int $value): int;                  // 返回原值;set(0) = 窗口重置
    public function add(int $delta = 1): int;              // 返回新值;add()=+1, add(-1)=自减
    public function compareAndSet(int $expect, int $new): bool;

    public function id(): int;
}
```

| 方法             | 返回值      | 使用场景                                                        |
|------------------|-------------|-----------------------------------------------------------------|
| `get`            | 当前值      | 无变更的读取。                                                  |
| `set`            | 原值        | 原子交换;`set(0)` 是窗口收尾的读取并清零。                     |
| `add`            | 新值        | `add()` 自增 1,`add(-1)` 自减,其他为任意增量。                |
| `compareAndSet`  | bool        | 通过 CAS 循环实现有界 / 饱和计数器(上限、下限)。              |
| `id`             | 注册表 id   | 日志、追踪、`/__ox_shared/entry?id=…` 关联。                    |

## 示例

### 每工作线程的请求计数器

```php
<?php
$requests = new OxPHP\Shared\Counter();

oxphp_worker(function () use ($requests) {
    $count = $requests->add();          // +1，返回新的合计
    header("X-Request-Count: {$count}");
    echo "ok";
});
```

### 窗口滚动

```php
<?php
$hits = new OxPHP\Shared\Counter();

// 每 N 分钟在 cron/工作循环中:
$prev = $hits->set(0);                   // 原子地读取并置零
logWindowMetric($prev);
```

### 有界计数器(CAS 循环)

```php
<?php
$slots = new OxPHP\Shared\Counter();
$cap   = 100;

// 仅在未达上限时占用一个槽位。
do {
    $cur = $slots->get();
    if ($cur >= $cap) {
        // 已满——拒绝
        break;
    }
} while (!$slots->compareAndSet($cur, $cur + 1));
```

### 批量累加

```php
<?php
$bytes = new OxPHP\Shared\Counter();

// 在 PHP 中汇总一批，然后做一次原子 add(一次 FFI 调用)。
$deltas   = array_map(fn ($req) => strlen($req['body']), $batch);
$newTotal = $bytes->add(array_sum($deltas));
```

## 语义与陷阱

- **`set()` 返回原值,然后存储——原子地。** `set(0)` 是 snapshot-and-zero 模式(`LongAdder::sumThenReset`);`set($n)` 可设定任意新起点。
- **Relaxed 顺序。** 每个操作都是原子的,但 Counter 不发布其他内存。若读者必须看到写者在自增整数*之前*写入的数据,那就是同步——请使用 [`Shared\Atomic`](shared-atomic.md) 配合 `Ordering::Release`/`Acquire`。
- **`compareAndSet` 为 Relaxed/Relaxed,不接受 ordering 参数。** 它适用于基于计数器自身值做出的决策(上限、下限、按值占用)。发布其他状态的 CAS 属于 `Shared\Atomic`。
- **溢出会回绕。** 超过 `INT_MAX` 后会回到 `INT_MIN`。对于可能在月级时间尺度上每秒数千次增长的计数器,把值保持在数十万亿级,或定期重置。
- **不支持小数。** 若你在计数字节数并需要浮点精度的平均值,把分子(Counter)和分母(Counter)分开跟踪,读取时再相除。

## 异常

| 异常                     | 触发场景                                                     |
|-------------------------|--------------------------------------------------------------|
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄调用任何方法。                     |
| `UninitializedException`| 在尚未完成 `__construct` 的包装上调用 `id()`。               |

Counter 在溢出或极值时永远不会抛出——它会回绕。

## 可观测性

完整内容请见 [Shared 可观测性](../operations/shared-observability.md)。速查:

- `GET /__ox_shared/entry?id=N` 暴露 `{ value, type: "Counter" }`。
- Prometheus `oxphp_shared_counter_value{counter_id="…"}` 仪表跟踪当前值。
- 注册表级计数器(`oxphp_shared_ops_total`、`oxphp_shared_objects_total`)通过 `type="Counter"` 标签覆盖 Counter。

## 何时不宜使用

- **浮点或小数。** 使用一对 Counter(分子/分母)或 `Shared\Mutex<array{total_cents: int, count: int}>`。
- **需要丰富上下文的非数值事件。** 如果你需要把 `{count, last_actor, last_reason}` 与一个键绑定,请选 `Shared\Map` 或 `Shared\Mutex`。
- **跨主机累计。** Counter 仅限进程内。多主机聚合请使用指标管道(Prometheus + `rate()`,或中央 Redis `INCR`)。
- **持久化。** Counter 状态在服务器停止时消失。如果总数必须跨重启存活,请在别处持久化快照。

## 相关

- [共享状态](shared-state.md) —— 概览与迁移模式。
- [Shared\Atomic](shared-atomic.md) —— 通用原子 int64,支持 CAS、swap 和完整的内存顺序控制。
- [Shared\Map](shared-map.md) —— 按键计数时(`Map<string, Counter>`)。
- [Shared\Flag](shared-flag.md) —— 仅需 on/off 值时。
- [Shared\Mutex](shared-mutex.md) —— 计数器必须与其他字段同步更新时。
