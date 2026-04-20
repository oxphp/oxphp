---
title: Shared\Counter
description: 跨 PHP 工作线程共享的原子 int64——无锁自增/自减、compare-and-set 与批量加，用于高吞吐计数。
---

# Shared\Counter

`OxPHP\Shared\Counter` 是进程级的原子 64 位有符号整数。每个操作都是无锁且可线性化的；两个工作线程并发自增永远不会丢失一次计数。

## 概览

- **原子 int64。** 范围 `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`。溢出会回绕。
- **无锁。** `inc` / `dec` / `add` 编译为单次 `fetch_add`；`compareAndSet` 是一次 CAS。
- **可共享。** 实例可存放在 `Shared\Map` / `Shared\Channel` 内，并通过 `use` 捕获交给 fiber。

## API 参考

```php
namespace OxPHP\Shared;

final class Counter implements Shareable
{
    public function __construct(int $initial = 0);

    public function get(): int;
    public function set(int $value): int;             // 返回原值
    public function inc(int $by = 1): int;            // 返回新值
    public function dec(int $by = 1): int;            // 返回新值
    public function add(int $delta): int;             // 返回新值
    public function compareAndSet(int $expect, int $new): bool;
    public function addBatch(array $deltas): int;     // 返回新值
    public function reset(int $newValue = 0): int;    // 返回原值

    public function id(): int;
}
```

| 方法             | 返回值      | 使用场景                                                        |
|------------------|-------------|-----------------------------------------------------------------|
| `get`            | 当前值      | 无变更的读取。                                                  |
| `set`            | 原值        | 无条件替换；适合以种子值初始化。                                |
| `inc` / `dec`    | 新值        | 按事件计数；`$by` 允许一次原子操作跳 N 步。                     |
| `add`            | 新值        | 任意正负增量。                                                  |
| `compareAndSet`  | 是否交换    | 乐观状态机（idle → busy → done）。                              |
| `addBatch`       | 新值        | 单次 FFI 往返完成批量累加。                                     |
| `reset`          | 原值        | 窗口结束的归零；`reset()` 置零，`reset(n)` 置种子。             |
| `id`             | 注册表 id   | 日志、追踪、`/__ox_shared/entry?id=…` 关联。                    |

## 示例

### 每工作线程的请求计数器

```php
<?php
$requests = new OxPHP\Shared\Counter();

oxphp_worker(function () use ($requests) {
    $count = $requests->inc();
    header("X-Request-Count: {$count}");
    echo "ok";
});
```

### 乐观状态机

```php
<?php
$state = new OxPHP\Shared\Counter(initial: 0); // 0=idle, 1=busy, 2=done

if (!$state->compareAndSet(expect: 0, new: 1)) {
    throw new RuntimeException('another worker is already processing');
}

try {
    doWork();
    $state->set(2);
} catch (Throwable $e) {
    $state->set(0); // 出错时释放回 idle
    throw $e;
}
```

### 窗口滚动

```php
<?php
$hits = new OxPHP\Shared\Counter();

// 每 N 分钟在 cron/工作循环中：
$prev = $hits->reset();                // 原子地读取并置零
logWindowMetric($prev);
```

### 批量累加

```php
<?php
$bytes = new OxPHP\Shared\Counter();

// 在一次 FFI 调用中统计批次的字节数，而不是 N 次。
$deltas = array_map(fn ($req) => strlen($req['body']), $batch);
$newTotal = $bytes->addBatch($deltas);
```

## 语义与陷阱

- **`set` 返回原值，不是新值。** 这与 `std::atomic<T>::exchange` 语义一致，也与 `reset` 保持一致。若需要刚刚写入的值，请在 `set()` 之后调用 `get()`。
- **`addBatch` 在跨项上不是原子的。** 底层是一轮 `fetch_add` 循环——最终值正确，但其他工作线程在批处理过程中会看到中间总和。如果你需要整批可见性，请用 `Shared\Mutex` 包裹一个 Counter。
- **溢出会回绕。** 加 `INT_MAX + 1` 会回到 `INT_MIN`。对于可能在月级时间尺度上每秒数千次增长的单调计数器，把值保持在数十万亿级，或定期重置。
- **不支持小数。** 若你在计数字节数并需要浮点精度的平均值，把分子（Counter）和分母（Counter）分开跟踪，读取时再相除。

## 异常

| 异常                     | 触发场景                                                     |
|-------------------------|--------------------------------------------------------------|
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄调用任何方法。                     |
| `UninitializedException`| 在尚未完成 `__construct` 的包装上调用 `id()`。               |

Counter 在溢出或极值时永远不会抛出——它会回绕。

## 可观测性

完整内容请见 [Shared 可观测性](../operations/shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ value, type: "Counter" }`。
- Prometheus `oxphp_shared_counter_value{counter_id="…"}` 仪表跟踪当前值。
- 注册表级计数器（`oxphp_shared_ops_total`、`oxphp_shared_objects_total`）通过 `type="Counter"` 标签覆盖 Counter。

## 何时不宜使用

- **浮点或小数。** 使用一对 Counter（分子/分母）或 `Shared\Mutex<array{total_cents: int, count: int}>`。
- **需要丰富上下文的非数值事件。** 如果你需要把 `{count, last_actor, last_reason}` 与一个键绑定，请选 `Shared\Map` 或 `Shared\Mutex`。
- **跨主机累计。** Counter 仅限进程内。多主机聚合请使用指标管道（Prometheus + `rate()`，或中央 Redis `INCR`）。
- **持久化。** Counter 状态在服务器停止时消失。如果总数必须跨重启存活，请在别处持久化快照。

## 相关

- [共享状态](shared-state.md) —— 概览与迁移模式。
- [Shared\Map](shared-map.md) —— 按键计数时（`Map<string, Counter>`）。
- [Shared\Flag](shared-flag.md) —— 仅需 on/off 值时。
- [Shared\Mutex](shared-mutex.md) —— 计数器必须与其他字段同步更新时。
