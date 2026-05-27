---
title: Shared\* 命名约定
description: OxPHP\Shared\* 并发 API 的命名约定 —— 每个共享原语都遵循的规范方法词汇（get/set、try*/timeout 等待策略、is*、fetch*、compareAndSet）。
---

# `OxPHP\Shared\*` 命名约定

`OxPHP\Shared\*` 命名空间是应用级并发 API：`Atomic`、`Counter`、
`Flag`、`Map`、`Channel`、`Mutex`、`Once`、`Pool`。方法名遵循单一
的一组规则，使用者无需逐个查阅文档也能预测 API。

本文档是规范参考。新增原语以及对现有原语的更改都**必须**遵循它。

## 规则

### 1. 读取值 — `get()`

PHP 约定。用于 `Map::get()`、`Counter::get()`、`Once::get()`。

`Atomic::load(?Ordering $order = null)` 是有意保留的例外：方法
本身携带 ordering 参数，表明此读取属于内存模型契约，而不是普通
的 getter。

### 2. 写入值 — `set()`，原子类型用 `store()`

`Map::set()`、`Mutex` 通过 `with` 重置值、`Once::getOrInit()`。
`Atomic::store($value, ?Ordering)` 与 `load` 对称，原因相同。

### 3. 元素数量 — `count(): int`

每个暴露当前规模的容器都以 `count(): int` 命名该方法。`Channel`
还实现了 `\Countable`，因此 `count($ch)` 是已排队项数的原生
惯用法。`Map` 与 `Pool` 把 `count(): int` 作为方法暴露，但
**不**实现 `\Countable` —— 直接调用即可：

```php
$ch  = new OxPHP\Shared\Channel(1024);
$map = new OxPHP\Shared\Map();
$pool = new OxPHP\Shared\Pool($factory);

count($ch);       // 已排队项数（Channel 实现了 \Countable）
$map->count();    // 条目数
$pool->count();   // 全部活跃槽（in-use + idle）
```

不允许 `size()`、`len()`、`pending()` — 无论实现者的肌肉记忆来自
哪种语言，公开接口都禁止使用它们。

### 4. 布尔 getter — `is*()` 前缀

`Channel::isClosed()`。

不使用裸动词（`test`、`check`），也不使用领域专有名
（`closed`）。`is` 前缀标记对布尔属性的纯读取。

状态比单个布尔更丰富的类型，用返回枚举的 `status()` 方法暴露状态，
而非 `is*()` getter —— `Channel` 的 `RecvResult::status()` 与
`Once::status(): Once\Status`（Uninitialized/Pending/Ready/Poisoned）
即如此。当答案多于两种情形时，请使用 `status()`。

`Mutex` 故意**不**提供 `isCorrupted()` —— 损坏是 sticky、不可恢复
的，并通过下一次获取时抛出的 `CorruptedMutexException` 暴露。除了
再次获取并 catch，对该探测结果没有其他有用的操作。

### 5. Wait-policy 三分法 — `try*` / 裸名 / `*Timeout`

阻塞原语（Channel、Mutex）通过方法**名**而非重载的 `?float $timeout`
参数来表达 **wait policy**：

| 后缀          | 行为                                                          | 示例                                                     |
|---------------|---------------------------------------------------------------|----------------------------------------------------------|
| `try*`        | 非阻塞；立即报告失败变体。                                    | `Channel::trySend`、`Channel::tryRecv`、`Mutex::tryWithLock` |
| (裸名)        | 永久阻塞（或直到 request fiber 被取消）。                     | `Channel::send`、`Channel::recv`、`Mutex::withLock`       |
| `*Timeout`    | 有界等待。接受强制的 `int $ms > 0`。                          | `Channel::sendTimeout`、`Channel::recvTimeout`、`Mutex::withLockTimeout` |

三分法把三种含糊的策略（`null` = 永久、`0` = try、正数 = 有界）从
一个参数里挪到三个名字自解释的方法上。`*Timeout` 方法的 `$ms` 参数
**严格为正** —— 零、负数、非 int 和缺失都会在桥层抛出
`OxPHP\Shared\TypeException`。

条件性成功操作位于 `Map` 上，名称为 `setIfAbsent`，而非
`try*`：`Map::setIfAbsent` 仅在键缺失时提交并返回 `bool`
（对应 `HashMap::try_insert`）。`setIfAbsent` 这个名字专门
保留给这种语义，不要在别处复用。

`try*` 的统一不变量：它要么返回带值的 Result（Channel），要么抛出
`ContentionException`（Mutex）。它从不用 `null` 来编码「未成功」 ——
那是旧 API 的做法，会带来三分法所要消除的 null-coalescing 歧义。

### 6. Compare-and-swap — `compareAndSet()`

`Atomic::compareAndSet()`、`Flag::compareAndSet()`。始终返回 `bool`
（交换是否发生）。

### 7. 替换并返回原值 — `swap()`

`Atomic::swap()` 用于 int，`Flag::swap()` 用于 bool。返回原值。

### 8. 返回原值的原子 RMW — `fetch*()` 前缀

`Atomic::fetchAdd()`、`fetchSub()`、`fetchAnd()`、`fetchOr()`、
`fetchXor()`。

`fetch` 前缀编码了返回契约：**操作前的值**。这与 `Counter::add()`
形成对比 — 后者返回**新**值（LongAdder 风格的聚合计数器）。

为新 RMW 方法命名时，先选契约，再起名：

- 返回 prev → `fetchVerb(args)`
- 返回 new → 裸 `verb(args)`

不要混用。

### 9. 重置为默认 — `clear()`

`Map::clear()` —— 清空容器；返回 `void`。

`Counter` 没有 `clear()` —— `set(0)` 即其窗口重置。`Counter::set()`
是已记录的例外，返回**原**值（而非 `void`）：它是原子交换，而
`set(0)` 读取先前总和正是 LongAdder 的 `sumThenReset` 惯用法。
（`Atomic` 将同一操作命名为 `swap()`；Counter 保留
`set`，因为 `set($n)` 用于初始化和窗口重置时读起来更自然。）

### 10. 注册表标识 — `id(): int`

每个 `Shared\*` 实例都暴露 `id(): int`，便于日志记录与
`/__ox_shared/entries/:id` 可观测性端点关联。

## 速查表

| 概念                        | 规范命名                 | 示例                                    |
| --------------------------- | ------------------------ | --------------------------------------- |
| 读取值                      | `get()`                  | `Map::get`、`Counter::get`              |
| 读取原子                    | `load($order)`           | `Atomic::load`                          |
| 写入值                      | `set()`                  | `Map::set`                              |
| 写入原子                    | `store($v, $order)`      | `Atomic::store`                         |
| 元素数量                    | `count(): int`           | `Map::count`、`Channel::count`、`Pool::count` |
| 布尔属性                    | `is*(): bool`            | `Channel::isClosed`                     |
| 条件性插入                  | `setIfAbsent($k, $v)`    | `Map::setIfAbsent`                      |
| 非阻塞等待                  | `try*()`                 | `Channel::trySend`、`Mutex::tryWithLock` |
| 永久等待                    | 裸动词                   | `Channel::send`、`Channel::recv`、`Mutex::withLock`     |
| 有界等待                    | `*Timeout(int $ms)`      | `Channel::sendTimeout`、`Mutex::withLockTimeout`        |
| Compare-and-swap            | `compareAndSet()`        | `Atomic::compareAndSet`                 |
| 替换并返回 prev             | `swap()`                 | `Atomic::swap`、`Flag::swap`            |
| 原子 RMW，返回 prev         | `fetch*()`               | `Atomic::fetchAdd`                      |
| 原子 RMW，返回 new          | 裸动词                   | `Counter::add`                          |
| 重置为默认                  | `clear()`                | `Map::clear`                            |
| 注册表标识                  | `id(): int`              | 所有 `Shared\*` 类型                    |

## 添加新的 `Shared\*` 类型

提交新原语前请逐项检查：

- [ ] 每个方法对应速查表中的一行，或有 ADR 说明例外（参见上文
  `Atomic::load/store` 与 `Counter::set`）。
- [ ] 若该类型存放值的集合，则实现 `\Countable` 并暴露
  `count(): int`。
- [ ] 读取方法为 `get` 或 `load`（仅原子）。
- [ ] 布尔 getter 使用 `is*` 前缀。
- [ ] Wait-policy 变体遵循 `try*` / 裸名 / `*Timeout(int $ms)` 三分
  法。`*Timeout` 变体接受 `int $ms > 0`，并以 `TypeException` 拒绝
  零 / 负数 / 非 int 输入。Wait-policy 的 `try*` 方法要么返回带值
  的 Result，要么抛出领域异常 —— 永远不用 `null`-编码。条件性成功
  操作改用专门的 `setIfAbsent` 命名，而不是 `try*`。
- [ ] 不出现 `len`、`size`、`pending`、`test` 等临时命名。
- [ ] 领域专有动词（`evict`、`drain`、`flush` 等）仅在速查表中没有
  规范对应项时出现。

## 可观测性命名滞后于 PHP API

面向运维的接口 — Prometheus 指标名与 `/__ox_shared/entries/:id` 的
JSON — 与 PHP API 是相互独立的契约。重命名会破坏仪表盘和告警规则。
为避免悄无声息的不一致，受影响的名字会在一个发布周期内**同时**
发出：

| 接口        | 已弃用（仍发出）               | 规范命名                    |
| ----------- | ------------------------------ | --------------------------- |
| Prometheus  | `oxphp_shared_channel_pending` | `oxphp_shared_channel_count` |
| Prometheus  | `oxphp_shared_pool_size`       | `oxphp_shared_pool_count`    |
| JSON entry  | `Channel.pending`              | `Channel.count`             |
| JSON entry  | `Pool.size`                    | `Pool.count`                |

已弃用指标的 `# HELP` 行带有前缀 `(deprecated, removed in a future
release; use *_count)`；当启用 introspection 或 metrics 时，
`ox_shared` 插件会在启动时输出 `WARN`。

请在 deprecation 周期关闭前把仪表盘和告警规则迁移到 `_count` 名称。
一旦移除，只会发出规范命名，引用旧名的 Prometheus / Grafana 面板
将开始返回空序列。

## 稳定性

这些规则是 `OxPHP\Shared\*` 1.0 契约的一部分。1.0 发布之后，重命名
属于破坏性变更，需要走 deprecation 周期。1.0 之前规则同样具有约束
力 — 违反规则的新方法会在评审中被拒绝。
