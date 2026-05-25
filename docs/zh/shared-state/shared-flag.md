---
title: Shared\Flag
description: 跨 PHP 工作线程共享的原子布尔值——用于终止开关、熔断器和一次性标记，提供无锁的 load/store/swap/compareAndSet 以及显式内存顺序。
---

# Shared\Flag

`OxPHP\Shared\Flag` 是进程级的原子布尔值，是 [`Shared\Atomic`](shared-atomic.md) 的 bool 孪生体。每个操作都是无锁的；两个工作线程并发翻转标志时不会观察到中间状态。

## 概览

- **原子 bool。** 单比特状态，支持 `load` / `store` / `swap` / `compareAndSet`。
- **显式内存顺序。** 每个操作都接受可选的 [`Ordering`](shared-atomic.md)，默认 `SeqCst`——与 `Shared\Atomic` 完全一致。
- **无锁。** 所有变更都是单条 CPU 原子指令。在竞争下安全。
- **可共享。** 实例存放在注册表中，可放入 `Shared\Map`、通过 `use` 捕获传递等。

## API 参考

```php
namespace OxPHP\Shared;

final class Flag implements Shareable
{
    public function __construct(bool $initial = false);

    public function load(Ordering $order = Ordering::SeqCst): bool;             // Relaxed | Acquire | SeqCst
    public function store(bool $value, Ordering $order = Ordering::SeqCst): void; // Relaxed | Release | SeqCst
    public function swap(bool $value, Ordering $order = Ordering::SeqCst): bool;   // 任意 ordering；返回原值
    public function compareAndSet(
        bool $expect,
        bool $new,
        Ordering $success = Ordering::SeqCst,
        Ordering $failure = Ordering::SeqCst,                                  // Relaxed | Acquire | SeqCst
    ): bool;

    public function id(): int;
}
```

| 方法            | 返回值    | 使用场景                                                         |
|-----------------|-----------|------------------------------------------------------------------|
| `load`          | 当前值    | 纯读取。                                                         |
| `store`         | void      | 无条件设置为显式值。                                             |
| `swap`          | 原值      | 设置为显式值；返回值告知你是否改动了它。`swap(true)` 即 test-and-set（「我胜出了吗？」）。 |
| `compareAndSet` | 是否交换  | 一次性初始化：仅当标志为期望值时成功。                           |

## 示例

### 终止开关

```php
<?php
use OxPHP\Shared\Flag;

$maintenance = new Flag();

// 请求处理器中
if ($maintenance->load()) {
    http_response_code(503);
    header('Retry-After: 60');
    echo 'under maintenance';
    return;
}

// 管理端点中
$maintenance->store(true);   // 启用
$maintenance->store(false);  // 关闭
```

### 一次性初始化的胜者

```php
<?php
use OxPHP\Shared\Flag;

$migrated = new Flag();

if ($migrated->compareAndSet(expect: false, new: true)) {
    // 第一个到达的工作线程胜出——运行一次迁移。
    runSchemaMigration();
} else {
    // 已经有别人运行过了。
}
```

### 熔断器触发

```php
<?php
use OxPHP\Shared\Flag;

$tripped = new Flag();

try {
    callDownstream();
} catch (DownstreamFailedException $e) {
    $wasAlreadyTripped = $tripped->swap(true);   // 置为 true，并获知此前的状态
    if (!$wasAlreadyTripped) {
        alertOncall($e);        // 仅在首次触发时发送告警
    }
    throw $e;
}
```

要实现完整的熔断器，通常还需要一个 `Shared\Counter` 用于失败窗口，加上一个 `Shared\Flag` 用于触发状态——窗口冷却后通过 `store(false)` 重置标志。

### 先发布数据，再用更廉价的顺序发出信号

```php
<?php
use OxPHP\Shared\Flag;
use OxPHP\Shared\Map;
use OxPHP\Shared\Ordering;

$ready = new Flag();
$config = new Map();

// 生产者：先写入数据，再用 Release 发布。
$config->set('dsn', $dsn);
$ready->store(true, Ordering::Release);

// 消费者：观察到 `true` 的 Acquire 读取也能观察到数据。
if ($ready->load(Ordering::Acquire)) {
    $dsn = $config->get('dsn');
}
```

## 语义与陷阱

- **`swap` 返回的是*原*值。** 这是最有用的返回：「我是否改动了任何东西？」即 `$prev !== $new`。`swap(true)` 是规范的 test-and-set。
- **`store` 返回 `void`。** 若需要原值，请使用 `swap`。
- **`compareAndSet` 是表达「先到先得」的方式。** 普通 `store(true)` 总是成功，因此无法表达「如果已设置则不覆盖」。
- **内存顺序与 `Shared\Atomic` 一致。** `load` 拒绝 `Release`/`AcqRel`，`store` 拒绝 `Acquire`/`AcqRel`，`compareAndSet` 的 `$failure` 拒绝 `Release`/`AcqRel`——每种情况都会抛出 `InvalidOrderingException`。默认的 `SeqCst` 始终安全。
- **不等待。** Flag 不会阻塞。如果你需要等待状态转移，请配合 `Shared\Channel`，或使用 `Shared\Once`。

## 异常

| 异常                       | 触发场景                                                     |
|----------------------------|--------------------------------------------------------------|
| `StaleHandleException`     | 对注册表条目已被驱逐的句柄调用任何方法。                     |
| `UninitializedException`   | 在尚未完成 `__construct` 的包装上调用 `id()`。               |
| `InvalidOrderingException` | 对操作不允许的 `Ordering`（见上文）。                        |

## 可观测性

请见 [Shared 可观测性](shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ value: true|false, type: "Flag" }`。
- Prometheus `oxphp_shared_flag_value{flag_id="…"}` 仪表（0 或 1）。
- 注册表级指标通过 `type="Flag"` 标签覆盖 Flag。

## 何时不宜使用

- **多态逻辑。** Flag 只有两个值。若需要 idle/busy/done 或任意三态机，请使用 `Shared\Counter`（用整数枚举值）或对类枚举数组使用 `Shared\Mutex`。
- **等待状态转移。** Flag 不阻塞。当工作线程需要等待标志翻转时，请配合 `Shared\Channel`（或轮询 `compareAndSet` 的 `Shared\Counter`）。
- **计数事件。** Flag 不是计数器。计数请使用 `Shared\Counter`。
- **整数状态。** 如果这个开关其实是个小整数，请直接使用 [`Shared\Atomic`](shared-atomic.md)。

## 相关

- [共享状态](shared-state.md) —— 概览与心智模型。
- [Shared\Atomic](shared-atomic.md) —— int64 孪生体，相同的 ordering 模型。
- [Shared\Counter](shared-counter.md) —— 当你需要超过 on/off 的状态时。
- [Shared\Once](shared-once.md) —— 当一次性计算得到的值比 bool 更丰富时。
- [Shared\Mutex](shared-mutex.md) —— 当标志翻转必须与其他状态同时提交时。
