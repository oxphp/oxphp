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

`Map::set()`、`Mutex` 通过 `with` 重置值、`Once::init()`。
`Atomic::store($value, ?Ordering)` 与 `load` 对称，原因相同。

### 3. 元素数量 — `count(): int` + `\Countable`

每个容器都暴露 `count(): int` 并实现 `\Countable`。这样可直接使用
`count($obj)`：

```php
$ch  = new OxPHP\Shared\Channel(1024);
$map = new OxPHP\Shared\Map();
$pool = new OxPHP\Shared\Pool($factory);

count($ch);    // 已缓冲的项
count($map);   // 条目数
count($pool);  // 全部活跃槽（in-use + idle）
```

不允许 `size()`、`len()`、`pending()` — 无论实现者的肌肉记忆来自
哪种语言，公开接口都禁止使用它们。

### 4. 布尔 getter — `is*()` 前缀

`Channel::isClosed()`、`Mutex::isPoisoned()`、`Once::isInitialized()`、
`Flag::isSet()`。

不使用裸动词（`test`、`check`），也不使用领域专有名（`poisoned`、
`closed`）。`is` 前缀标记对布尔属性的纯读取。

### 5. 可能失败的尝试 — `try*()` 前缀

`Channel::trySend()`、`Channel::tryRecv()`、`Mutex::tryWith()`、
`Map::trySet()`。

语义：一个可能合理失败的操作 —— 因为阻塞变体需要等待、因为某个
逻辑前置条件不满足，或者因为容量耗尽 —— 并通过返回 `bool` /
`null` 而非抛出异常来报告失败。当调用方必须区分「未成功」与
「成功」但又不想走 `try`/`catch` 时，请使用 `try*`。

同一前缀下并存两种不同的子语义；两者都是有意为之，并与 Rust
stdlib 的 `try_*` 用法一致：

- **阻塞操作的非阻塞变体。** `trySend` / `tryRecv` / `tryWith` 等
  价于零截止期的阻塞变体（will-block → `false` / `null`，不抛
  `TimeoutException`）。对应 `mpsc::Sender::try_send`、
  `Mutex::try_lock`。
- **条件性成功操作。** `Map::trySet` 仅在键缺失时成功；冲突
  → `false`，不抛异常。对应 `HashMap::try_insert`。

统一的不变量：`try*` 返回值而非抛出异常。不要发明替代名
（`setIfAbsent`、`lockNonblocking`、`pushIfRoom`）。

### 6. Compare-and-swap — `compareAndSet()`

`Atomic::compareAndSet()`、`Flag::compareAndSet()`。始终返回 `bool`
（交换是否发生）。

### 7. 替换并返回原值 — `swap()`、`exchange()`

`Atomic::swap()` 用于 int，`Flag::exchange()` 用于 bool。

命名上的不对称是历史的、刻意保留的：`swap` 在底层语境里读作
「交换两处内容」；`exchange` 在 PHP 中更常用于「换为新值」。
两者都返回原值。

### 8. 返回原值的原子 RMW — `fetch*()` 前缀

`Atomic::fetchAdd()`、`fetchSub()`、`fetchAnd()`、`fetchOr()`、
`fetchXor()`。

`fetch` 前缀编码了返回契约：**操作前的值**。这与 `Counter::add()`、
`Counter::inc()`、`Counter::dec()` 形成对比 — 后者返回**新**值
（LongAdder 风格的聚合计数器）。

为新 RMW 方法命名时，先选契约，再起名：

- 返回 prev → `fetchVerb(args)`
- 返回 new → 裸 `verb(args)`

不要混用。

### 9. 重置为默认 — `clear()`

`Map::clear()`、`Flag::clear()`（含义为「设为 false」）。普通重置
返回 `void`；当调用方合理需要原值时返回原值（`Flag::clear()`、
`Counter::reset()`）。

`Counter::reset()` 是已记录的例外，保留 `reset`：LongAdder 约定为
`sumThenReset`，重命名会误导熟悉 Java `LongAdder` 或 Go
`atomic.Int64.Swap(0)` 的用户。

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
| 键/元素存在性               | `has($key): bool`        | `Map::has`                              |
| 布尔属性                    | `is*(): bool`            | `Flag::isSet`、`Channel::isClosed`      |
| 可能失败的尝试              | `try*()`                 | `Channel::trySend`、`Map::trySet`       |
| Compare-and-swap            | `compareAndSet()`        | `Atomic::compareAndSet`                 |
| 替换并返回 prev             | `swap()` / `exchange()`  | `Atomic::swap`、`Flag::exchange`        |
| 原子 RMW，返回 prev         | `fetch*()`               | `Atomic::fetchAdd`                      |
| 原子 RMW，返回 new          | 裸动词                   | `Counter::inc`、`Counter::add`          |
| 重置为默认                  | `clear()`                | `Map::clear`、`Flag::clear`             |
| 注册表标识                  | `id(): int`              | 所有 `Shared\*` 类型                    |

## 添加新的 `Shared\*` 类型

提交新原语前请逐项检查：

- [ ] 每个方法对应速查表中的一行，或有 ADR 说明例外（参见上文
  `Atomic::load/store` 与 `Counter::reset`）。
- [ ] 若该类型存放值的集合，则实现 `\Countable` 并暴露
  `count(): int`。
- [ ] 读取方法为 `get` 或 `load`（仅原子）。
- [ ] 布尔 getter 使用 `is*` 前缀。
- [ ] 可能失败的变体（非阻塞、条件性成功、容量）使用 `try*` 前缀，
  并返回 `bool` / `null` 而非抛出异常。
- [ ] 不出现 `len`、`size`、`pending`、`test`、`setIfAbsent` 等
  临时命名。
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
