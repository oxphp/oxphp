---
title: Shared\Once
description: 一次性容器——仅一个工作线程的工厂回调产生值，其他所有调用者都看到已记忆的结果，覆盖整个 OxPHP 进程。
---

# Shared\Once

`OxPHP\Shared\Once` 针对**单个 `Once` 单元**只运行一次初始化闭包，并把结果暴露给该单元的所有后续调用者。它是「最多发生一次的昂贵动作」的原语。

要在整个进程范围内获得真正的「跨工作线程 / 跨请求恰好一次」语义，请通过 [`Shared\Registry::once(...)`](shared-registry.md) 把 `Once` 绑定到一个名称上，让所有工作线程都汇聚到同一个单元。直接 `new Shared\Once()` 的构造模式在 worker 模式下每个工作线程产生一个独立单元（每个工作线程的引导阶段都会执行构造函数），在传统模式下每次请求产生一个独立单元——这给你的是「单元级仅一次」，而非「进程级仅一次」。

## 概览

- **跨工作线程仅运行一次 _（针对同一个单元）_。** 两个工作线程同时进入同一个 `Once` 单元的 `getOrInit($factory)` 时，只有其中之一会运行工厂；落败方阻塞并收到胜出方的值。配合 `Shared\Registry::once` 即可让「同一个单元」对应「所有工作线程上同一个名称」。
- **四状态机。** 一个单元处于 `Uninitialized`、`Pending`（工厂正在运行）、`Ready` 或 `Poisoned`。用 `status()` 读取。
- **没有歧义的 null。** `get()` 在未设置的单元上抛异常，而非返回 `null`，因此存储的 `null` 是真实的值，而非「缺失」。
- **可重入安全。** 从工厂内部对同一个 Once 调用 `getOrInit()` 会抛出 `DeadlockException`，而不是挂起。
- **可配置的失败策略。** 默认情况下失败的工厂会重置单元，使后续调用可重试。选择 `Poison` 则让失败的工厂永久禁用该单元。
- **可共享。** 实例存放于注册表，并通过 `use` 捕获与 `Shared\Map` 条目流转。

## API 参考

```php
namespace OxPHP\Shared;

final class Once implements Shareable
{
    public function __construct(Once\FailureMode $onFactoryError = Once\FailureMode::Reset);

    public function get(): mixed;                    // 非 Ready 时抛异常
    public function status(): Once\Status;           // 永不抛异常
    public function trySet(mixed $value): bool;      // 本次调用胜出时返回 true
    public function getOrInit(callable $factory): mixed; // 工厂最多运行一次

    public function id(): int;
}

namespace OxPHP\Shared\Once;

enum Status { case Uninitialized; case Pending; case Ready; case Poisoned; }
enum FailureMode: int { case Reset = 0; case Poison = 1; }
```

| 方法        | 返回值          | 使用场景                                                          |
|-------------|-----------------|------------------------------------------------------------------|
| `get`       | 存储的值        | 读取已知为 `Ready` 的值。在 uninit / pending / poison 上抛异常。  |
| `status`    | `Once\Status`   | 内省 / 诊断。永不抛异常（poison 的安全观察者）。                  |
| `trySet`    | 是否胜出        | 已有值在手的 push 式初始化（无副作用资源获取）。                  |
| `getOrInit` | 存储的值        | pull 式初始化；规范的无竞态原语。                                 |
| `id`        | 注册表 id       | 日志 / 可观测性关联。                                             |

## 示例

### 每进程只加载一次的昂贵配置

```php
<?php
// Registry::once 把单元绑定到一个名称上，让每个工作线程的引导阶段都汇聚到
// 同一个单元。若不使用 Registry，这里裸写的 `new Once()` 会在每个工作线程上
// 各创建一个单元，工厂会按工作线程运行一次，而非按进程运行一次。
$config = OxPHP\Shared\Registry::once(
    'app-config',
    fn() => new OxPHP\Shared\Once(),
);

oxphp_worker(function () use ($config) {
    $cfg = $config->getOrInit(function () {
        // 恰好在进程内的一个工作线程中运行；其他每个工作线程
        //（以及传统模式下的每次后续请求）都会在此阻塞并看到结果。
        return json_decode(file_get_contents('/etc/myapp.json'), true);
    });

    echo $cfg['greeting'];
});
```

`getOrInit()` 是抵御 cache-stampede 的模式：在并发首次访问的高峰下，工厂*在成功时*只运行一次，且每个调用者——包括竞态落败者——都收到胜出方的值。若胜出方的工厂在 `Reset` 模式下抛出，下一个被阻塞的调用者会成为初始化者并重试——因此在负载下*持续*失败的工厂是串行重试，而非并行铺开。若希望失败是终态，请改用 `Poison` 模式（见下文）。

### 不触发初始化地按状态分支

```php
<?php
use OxPHP\Shared\Once\Status;

$cfg = new OxPHP\Shared\Once();

$report = match ($cfg->status()) {
    Status::Ready        => $cfg->get(),
    Status::Pending      => '正在初始化…',
    Status::Uninitialized => '尚未开始',
    Status::Poisoned     => '配置加载失败',
};
```

`status()` 用于内省：它永不触发工厂，也永不抛异常，即使在已中毒的单元上。要真正无竞态地取值，请调用 `getOrInit()`。

### 值已知时的值优先初始化

```php
<?php
$buildSha = new OxPHP\Shared\Once();

// 一个没有获取副作用的普通值——这里用 trySet 是合适的。
$buildSha->trySet(getenv('GIT_SHA') ?: 'unknown');

$sha = $buildSha->get();   // 上面 trySet 之后为 Ready
```

仅对没有副作用获取的值使用 `trySet()`。对于资源（连接、文件句柄、套接字）请改用 `getOrInit()`：竞态落败的 `trySet()` 只是把普通值交给垃圾回收，而在竞态落败*之前*获取的资源会泄漏。

返回 `false` 表示单元已经是 `Ready` **或** `Pending`——它*不*保证随后的 `get()` 会成功：另一线程上的 `Pending` 工厂仍可能失败并重置单元（在 `Reset` 模式下）。不要写 `if (!$o->trySet($v)) { $x = $o->get(); }`；若需要取值，请调用 `getOrInit()`。

### 数据库连接引导

```php
<?php
// 给单元命名，使整个 OxPHP 进程范围内只打开一个 PDO 连接。
// 工厂会获取资源——这恰好是 `getOrInit` 的「落败者阻塞」
// 语义所要保护的场景。
$pool = OxPHP\Shared\Registry::once('db-conn', fn() => new OxPHP\Shared\Once());

$conn = $pool->getOrInit(function () {
    return new PDO(getenv('DB_DSN'), getenv('DB_USER'), getenv('DB_PASS'), [
        PDO::ATTR_PERSISTENT => true,
    ]);
});
```

如果需要多槽位的连接池，请见 [Shared\Pool](shared-pool.md) —— `Once` 给你*一个*值；`Pool` 给你 N 个。

### 在损坏的先决条件上快速失败

```php
<?php
use OxPHP\Shared\Once\FailureMode;

// 若此初始化失败，应用无法恢复——给单元下毒，
// 让后续每次访问都响亮地失败，而非重试注定失败的工厂。
$secrets = new OxPHP\Shared\Once(onFactoryError: FailureMode::Poison);

$secrets->getOrInit(fn () => loadSecretsOrThrow());
```

## 语义与陷阱

- **单元非 `Ready` 时 `get()` 抛异常。** 空或 `Pending` 单元抛 `UninitializedException`，已中毒单元抛 `PoisonedException`。用 `status()` 无异常地分支，或用 `getOrInit()` 安全取值。
- **工厂在每次成功初始化中最多运行一次。** 并发调用者阻塞在胜出方上；它们不会运行自己的副本。
- **失败策略在构造时设定，而非按调用。** `Reset`（默认）在工厂失败时把单元还原为 `Uninitialized`，使后续调用重试；`Poison` 使单元终态 `Poisoned`。两种模式下工厂的异常都会重新抛给*当前*调用者。
- **Poison 跨线程诚实，但并非对象一致。** PHP 异常对象无法跨工作线程，因此中毒单元会捕获失败的类名、消息与代码。任意线程上的后续调用者会收到一个携带该信息的全新 `PoisonedException`——细节相同，但不是同一个对象。
- **可重入会抛异常。** 在工厂内部对同一个 Once 调用 `getOrInit()` 会抛出 `DeadlockException`。请重构，使内层调用使用不同的 `Once`。
- **完整的值范围。** 标量、数组以及嵌套的 `Shareable` 值都可存储并读回。闭包、资源以及非 `Shareable` 的 PHP 对象会抛出 `TypeException`。

## 异常

| 异常                     | 触发场景                                                          |
|-------------------------|-------------------------------------------------------------------|
| `UninitializedException`| 在 `Uninitialized` 或 `Pending` 单元上调用 `get()`。              |
| `PoisonedException`     | 在 `Poisoned` 单元上调用 `get()` / `getOrInit()` / `trySet()`。   |
| `DeadlockException`     | 从工厂内部对同一 Once 递归调用 `getOrInit()`。                    |
| `TypeException`         | 存储的值不可序列化（闭包、资源）。                                |
| `StaleHandleException`  | 对注册表条目已被驱逐的句柄调用任何方法。                          |

如果工厂本身抛出异常，该异常原样向上传播给当前调用者。在 `Reset` 模式下单元保持未初始化，下次 `getOrInit` 会重试；在 `Poison` 模式下单元变为已中毒。

## 可观测性

请见 [Shared 可观测性](shared-observability.md)。速查：

- `GET /__ox_shared/entry?id=N` 暴露 `{ status: "uninitialized" | "pending" | "ready" | "poisoned", type: "Once" }`，并在 `ready` 时附带已存储值的预览。

## 何时不宜使用

- **创建后会变化的值。** `Once` 是一次写入。若存储的状态会变更，请使用 `Shared\Mutex` 或 `Shared\Map`。
- **每工作线程本地状态。** 不需要共享时，类静态属性或模块全局变量更廉价。
- **昂贵的*每次请求*计算。** 请在请求内部缓存，而非共享状态里——否则会泄漏内存。

## 相关

- [共享状态](shared-state.md) —— 概览与心智模型。
- [Shared\Mutex](shared-mutex.md) —— 一次性值之后会变更时。
- [Shared\Pool](shared-pool.md) —— 一次性初始化 *N* 个等价资源。
- [Shared\Map](shared-map.md) —— 使用 `getOrSet($key, $factory)` 的按键初始化。
