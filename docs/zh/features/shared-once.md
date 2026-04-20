---
title: Shared\Once
description: 一次性容器——仅一个工作线程的工厂回调产生值，其他所有调用者都看到已记忆的结果，覆盖整个 OxPHP 进程。
---

# Shared\Once

`OxPHP\Shared\Once` 在整个进程中仅运行一次初始化闭包，并把结果暴露给所有后续调用者。它是「无论并发启动多少工作线程，都最多发生一次的昂贵动作」的原语。

## 概览

- **跨工作线程仅运行一次。** 两个工作线程同时进入 `init($factory)` 时，只有其中之一会运行工厂；落败方等待并看到胜出方的值。
- **永远记忆。** 一旦初始化，`get()` 不再运行任何内容就返回值。
- **可重入安全。** 从工厂内部对同一个 Once 调用 `init()` 会抛出 `DeadlockException`，而不是挂起。
- **可共享。** 实例存放于注册表，并通过 `use` 捕获与 `Shared\Map` 条目流转。

## API 参考

```php
namespace OxPHP\Shared;

final class Once implements Shareable
{
    public function __construct();

    public function get(): mixed;                   // 未设置时为 null
    public function isInitialized(): bool;
    public function trySet(mixed $value): bool;     // 本次调用胜出时返回 true
    public function init(callable $factory): mixed; // 工厂最多运行一次

    public function id(): int;
}
```

| 方法            | 返回值        | 使用场景                                                         |
|-----------------|---------------|------------------------------------------------------------------|
| `get`           | 值或 null     | 纯读取；无人初始化时返回 `null`。                                |
| `isInitialized` | bool          | 探测是否初始化，而不取值。                                       |
| `trySet`        | 是否胜出      | 直接以值初始化，适合已有值在手的场景。                           |
| `init`          | 存储的值      | 基于工厂的初始化；每次调用都返回已记忆的值。                     |
| `id`            | 注册表 id     | 日志 / 可观测性关联。                                            |

## 示例

### 每进程只加载一次的昂贵配置

```php
<?php
$config = new OxPHP\Shared\Once();

oxphp_worker(function () use ($config) {
    $cfg = $config->init(function () {
        // 恰好在一个工作线程内运行；其他所有工作线程看到结果。
        return json_decode(file_get_contents('/etc/myapp.json'), true);
    });

    echo $cfg['greeting'];
});
```

### 值已知时的值优先初始化

```php
<?php
$buildSha = new OxPHP\Shared\Once();

// 通常来自构建期常量，而非运行时计算。
if ($buildSha->trySet(getenv('GIT_SHA') ?: 'unknown')) {
    // 我们存储了它。
}

// 所有人读取已记忆的值。
$sha = $buildSha->get();   // 上面首次 trySet 之后永不为 null
```

### 数据库连接引导

```php
<?php
$pool = new OxPHP\Shared\Once();

$conn = $pool->init(function () {
    return new PDO(getenv('DB_DSN'), getenv('DB_USER'), getenv('DB_PASS'), [
        PDO::ATTR_PERSISTENT => true,
    ]);
});
```

如果需要多槽位的连接池，请见 [Shared\Pool](shared-pool.md) —— `Once` 给你*一个*值；`Pool` 给你 N 个。

## 语义与陷阱

- **在 `init()` / `trySet()` 成功之前 `get()` 返回 `null`。** 用 `isInitialized()` 区分「尚未设置」与「已设为 null」。
- **工厂每进程最多运行一次。** 即使抛出异常，也算作一次尝试——`Once` 不会重试。需要可重试的逻辑请在工厂内部自己包裹。
- **可重入会抛异常。** 在工厂内部对同一个 Once 调用 `init()` 会抛出 `DeadlockException`，消息中带 id。请重构图结构，让内层调用使用不同的 `Once`，或以其他方式获取尚未存储的值。
- **工厂的返回值会被序列化为 shared-safe 形式。** 标量和嵌套的标量数组可直接通过；闭包、资源以及非 `Shareable` 的 PHP 对象会抛出 `TypeException`。

## 异常

| 异常                     | 触发场景                                                        |
|-------------------------|-----------------------------------------------------------------|
| `DeadlockException`     | 从工厂内部对同一 Once 递归调用 `init()`。                       |
| `TypeException`         | 工厂返回不可序列化的值（闭包、资源）。                          |
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄调用任何方法。                        |
| `UninitializedException`| 在尚未完成 `__construct` 的包装上调用 `id()`。                  |

如果工厂本身抛出异常，该异常原样向上传播；Once 保持未初始化状态，下次 `init` 调用会重试。

## 可观测性

请见 [Shared 可观测性](../operations/shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ initialized: bool, type: "Once" }`，并在可得时附带已存储值的预览。
- Prometheus `oxphp_shared_once_initialized{once_id="…"}` 仪表（0 或 1）。

## 何时不宜使用

- **创建后会变化的值。** `Once` 是一次写入。若存储的状态会变更，请使用 `Shared\Mutex` 或 `Shared\Map`。
- **每工作线程本地状态。** 不需要共享时，类静态属性或模块全局变量更廉价。
- **昂贵的*每次请求*计算。** 请在请求内部缓存，而非共享状态里——否则会泄漏内存。

## 相关

- [共享状态](shared-state.md) —— 概览与心智模型。
- [Shared\Mutex](shared-mutex.md) —— 一次性值之后会变更时。
- [Shared\Pool](shared-pool.md) —— 一次性初始化 *N* 个等价资源。
- [Shared\Map](shared-map.md) —— 使用 `getOrSet($key, $factory)` 的按键初始化。
