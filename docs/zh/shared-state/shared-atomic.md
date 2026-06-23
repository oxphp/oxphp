---
title: Shared\Atomic
description: 跨 PHP 工作线程共享的通用原子 int64——load/store、swap、CAS、fetch 算术与 fetch 位运算，并提供显式的内存序控制。
---

# Shared\Atomic

`OxPHP\Shared\Atomic` 是进程级的原子 64 位有符号整数，提供完整的原语：`load`、`store`、`swap`、`compareAndSet`，以及 `fetchAdd`/`Sub`/`And`/`Or`/`Xor`。每个操作都是无锁的；内存序显式指定，默认 `SeqCst`。

## 概览

- **原子 int64 原语。** 范围 `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`。溢出会回绕。
- **无锁。** 每个操作都编译为单条 CPU 原子指令（`load`、`store`、`xchg`、`cmpxchg`、`xadd` 等）。
- **内存序由你选择。** 当需要 `Relaxed` / `Acquire` / `Release` / `AcqRel` / `SeqCst` 时，传入 `OxPHP\Shared\Ordering` 枚举值。默认 `SeqCst`，所以不关心内存序的调用者也能获得最强保证。

何时选择 Atomic 而非 `Shared\Counter`：

- **状态机** —— 用 `compareAndSet` 实现 `idle → busy → done`。
- **版本戳 / 代次计数器** —— `fetchAdd(1)` 返回先前的版本号；读者据此检测竞态。
- **CAS 循环** —— `load` 读取，计算新值，`compareAndSet` 重试直至成功。
- **位标志掩码** —— `fetchOr` 置位，`fetchAnd` 清位。

`Counter` 适合累加（`add`）；`Atomic` 适合任意原子状态。

## API 参考

```php
namespace OxPHP\Shared;

final class Atomic implements Shareable
{
    public function __construct(int $initial = 0);

    public function load(Ordering $order = Ordering::SeqCst): int;
    public function store(int $value, Ordering $order = Ordering::SeqCst): void;
    public function swap(int $value, Ordering $order = Ordering::SeqCst): int;            // 返回原值

    public function compareAndSet(
        int $expect,
        int $new,
        Ordering $success = Ordering::SeqCst,
        Ordering $failure = Ordering::SeqCst,
    ): bool;

    public function fetchAdd(int $delta, Ordering $order = Ordering::SeqCst): int;        // 返回原值
    public function fetchSub(int $delta, Ordering $order = Ordering::SeqCst): int;        // 返回原值
    public function fetchAnd(int $mask,  Ordering $order = Ordering::SeqCst): int;        // 返回原值
    public function fetchOr (int $mask,  Ordering $order = Ordering::SeqCst): int;        // 返回原值
    public function fetchXor(int $mask,  Ordering $order = Ordering::SeqCst): int;        // 返回原值

    public function id(): int;
}
```

| 方法              | 返回值        | 使用场景                                                       |
|-------------------|---------------|----------------------------------------------------------------|
| `load`            | 当前值        | 以指定内存序读取值。                                           |
| `store`           | void          | 写入新值，丢弃旧值。                                           |
| `swap`            | 原值          | 原子替换；`swap(0)` 即 snapshot-and-zero 模式。                |
| `compareAndSet`   | 是否交换      | 乐观状态转换与 CAS 循环。                                      |
| `fetchAdd`/`Sub`  | 原值          | 代次计数器、CAS 实现的有界计数、增量。                         |
| `fetchAnd`/`Or`/`Xor` | 原值      | 位标志掩码：置位、清位、翻转。                                 |
| `id`              | 注册表 id     | 日志、追踪、`/__ox_shared/entry?id=…` 关联。                   |

## 内存序

简短入门：

- **Relaxed** —— 仅原子性，不与其他内存访问形成顺序关系。
- **Acquire**（用于 load）—— 与 `Release` store 配对；此操作之后的读能观察到 releaser 已完成的写。
- **Release**（用于 store）—— 与 `Acquire` load 配对；此操作之前的写对 acquirer 可见。
- **AcqRel**（用于 read-modify-write）—— 同时具备 Acquire load 和 Release store 两面。
- **SeqCst** —— 所有 `SeqCst` 操作之间存在单一全局总序。

每个操作只接受对它有意义的内存序：

| 操作 | 允许 |
|---|---|
| `load` | `Relaxed`、`Acquire`、`SeqCst` |
| `store` | `Relaxed`、`Release`、`SeqCst` |
| `swap`、`fetchAdd`、`fetchSub`、`fetchAnd`、`fetchOr`、`fetchXor` | 任意 |
| `compareAndSet` `success` | 任意 |
| `compareAndSet` `failure` | `Relaxed`、`Acquire`、`SeqCst` |

各处默认均为 `Ordering::SeqCst`，因此不思考内存序的调用者也能得到安全的行为。无效组合会在 FFI 调用前抛出 `OxPHP\Shared\InvalidOrderingException`。

C++/Rust 内存模型的深入讲解，参见 [Rust `std::sync::atomic::Ordering` 文档](https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html)。

## 示例

### 通过 compareAndSet 实现状态机

```php
<?php
use OxPHP\Shared\Atomic;

$state = new Atomic(initial: 0); // 0=idle, 1=busy, 2=done

if (!$state->compareAndSet(expect: 0, new: 1)) {
    throw new RuntimeException('another worker is already processing');
}

try {
    doWork();
    $state->store(2);
} catch (Throwable $e) {
    $state->store(0); // 出错时释放回 idle
    throw $e;
}
```

### 代次计数器 / 版本戳

```php
<?php
$version = new OxPHP\Shared\Atomic();

// 每个写者都对版本递增，并得到刚刚被取代的版本号。
$prev = $version->fetchAdd(1);
publishUpdate($prev + 1, $payload);
```

### 通过 CAS 循环做乐观更新

```php
<?php
use OxPHP\Shared\Atomic;
use OxPHP\Shared\Ordering;

$cell = new Atomic(initial: 100);

// 饱和加法：永不超过 1000。
do {
    $cur = $cell->load(Ordering::Acquire);
    $next = min($cur + 7, 1000);
    if ($cur === $next) {
        break; // 已达上限
    }
} while (!$cell->compareAndSet($cur, $next, Ordering::AcqRel, Ordering::Acquire));
```

### 位标志掩码

```php
<?php
const FLAG_READY    = 1 << 0;
const FLAG_DRAINING = 1 << 1;
const FLAG_FAILED   = 1 << 2;

$flags = new OxPHP\Shared\Atomic();

$flags->fetchOr(FLAG_READY);                  // 置位
$flags->fetchAnd(~FLAG_DRAINING);             // 清位
$snapshot = $flags->load();
if ($snapshot & FLAG_FAILED) {
    raiseAlert();
}
```

## 语义与陷阱

- **`fetchAdd` 返回原值，不是新值。** 这与 `Counter::add` 返回新总数形成有意的对比。不同抽象、不同返回约定——按你想表达的语义选择类。
- **溢出会回绕。** `i64::MIN.fetchSub(1)` 得到 `i64::MAX`，不会抛出异常。
- **默认内存序是 `SeqCst`。** 这是最安全也是最慢的选择。仅在能说清理由时再降到 `Acquire`/`Release`/`Relaxed`。
- **仅支持单个 int64。** 对于多字段耦合的复合状态，请用 `Shared\Mutex`。

## 异常

| 异常                          | 触发场景                                                              |
|-------------------------------|------------------------------------------------------------------------|
| `StaleHandleException`        | 对注册表条目已被驱逐的句柄调用任何方法。                              |
| `UninitializedException`      | 在尚未完成 `__construct` 的包装上调用 `id()`。                        |
| `InvalidOrderingException`    | 操作收到对它无效的内存序。                                            |

## 可观测性

完整内容请见 [Shared 可观测性](shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ value, type: "Atomic" }`。
- 注册表级计数器（`oxphp_shared_operations_total`、`oxphp_shared_objects_total`）通过 `type="Atomic"` 标签覆盖 Atomic。

## 何时不宜使用

- **复合状态。** 必须同步更新的多个字段 → `Shared\Mutex`。
- **计数 / 累加。** 选 `Shared\Counter` —— 其 `add` 返回新总数与该领域语义一致。
- **浮点或小数。** 不支持；将结构体放入 `Shared\Mutex`，或用一对 Counter（分子/分母）。
- **跨主机协调。** Atomic 仅限进程内。多主机状态请使用 Redis、数据库或指标管道。
- **持久化。** Atomic 状态在服务器停止时消失。若必须跨重启存活，请在别处持久化快照。

## 相关

- [共享状态](shared-state.md) —— 概览与迁移模式。
- [Shared\Counter](shared-counter.md) —— 值是领域累加器时。
- [Shared\Mutex](shared-mutex.md) —— 状态超出单个 int64 时。
- [Shared\Flag](shared-flag.md) —— 仅需 on/off 值时。
