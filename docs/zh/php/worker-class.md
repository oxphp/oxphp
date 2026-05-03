---
title: Worker 类
description: OxPHP\Server\Worker 类参考——用于 Worker 内省、Worker 入口点和单线程指标的统一运行时句柄。
---

# Worker 类

`OxPHP\Server\Worker` 是与单个 OxPHP OS Worker 线程相关的所有内容的统一运行时句柄。它是一个无状态的 final 类，本质上是对桥接 thread-local 状态的薄封装，由 SAPI 扩展自身注册，因此始终可用——既可在传统模式下使用，也可在 [Worker 模式](../features/worker-mode.md) 下使用。每次调用都会直接读取运行时的实时状态；对象本身不缓存任何内容。

`Worker::current()` 在每个 OS 线程上返回一个单例：在同一线程上的两次调用始终返回相同的实例。

## 快速参考

| 方法 | 说明 |
|------|------|
| `Worker::current(): self` | 返回当前 OS 线程的单例句柄。 |
| `Worker::isWorkerMode(): bool` | 服务器运行在 Worker 模式（即设置了 `WORKER_FILE`）时返回 `true`。 |
| `getId(): int` | 当前 OS 线程的 Worker 数字标识符，范围为 `0..N-1`。 |
| `getStartTime(): float` | 此 OS Worker 线程启动时的 Unix 时间戳（秒）。 |
| `getRequestCount(): int` | 此 OS 线程已处理的请求数，从 1 开始计数。在两种模式下都会增长。 |
| `getMemoryUsage(): int` | 当前 PHP 内存使用量（字节，`zend_memory_usage(0)`）。 |
| `getRss(): int` | 进程常驻内存集大小（字节）。不缓存——每次请求最多调用一次。 |
| `getMaxMemoryBytes(): int` | 配置的内存上限（字节）。`0` 表示无限制。 |
| `scheduleExit(): void` | 标记 Worker 在当前请求完成后优雅退出。在传统模式下为 no-op。 |
| `isExitScheduled(): bool` | 如果已对当前 Worker 调用过 `scheduleExit()`，则返回 `true`。在传统模式下始终为 `false`。 |
| `getExitReason(): ?string` | 待退出原因：`'scheduled'`、`'max_memory'`、`'error'`，无待退出时为 `null`。在传统模式下始终为 `null`。 |
| `serve(callable $h): void` | 进入请求循环。在非 Worker 模式下抛出 `InvalidServeContextException`。 |

## 模式矩阵

| 方法 | 传统模式 | Worker 模式 |
|------|----------|-------------|
| `current()` | 每个 OS 线程的单例。 | 每个 OS 线程的单例。 |
| `isWorkerMode()` | `false` | `true` |
| `getId()` | Worker 池中的 OS 线程索引。 | Worker 池中的 OS 线程索引。 |
| `getStartTime()` | OS 线程启动时间（通常是服务器启动时间）。 | OS 线程启动时间。 |
| `getRequestCount()` | 从 1 开始，复用同一 OS 线程时跨请求递增（`1, 2, 3, …`）。 | 从 1 开始，每次 Worker 处理请求时递增。 |
| `getMemoryUsage()` | 调用时的实时 PHP 内存。 | 调用时的实时 PHP 内存。 |
| `getRss()` | 进程实时 RSS。 | 进程实时 RSS。 |
| `getMaxMemoryBytes()` | `0`（不应用回收上限）。 | `WORKER_MAX_MEMORY_MIB` × 1 MiB 的值；未设置则为 `0`。 |
| `scheduleExit()` | No-op（脚本即将结束）。 | 设置退出标志；当前请求处理器返回后，请求循环退出。 |
| `isExitScheduled()` | 始终为 `false`。 | 在该线程调用过 `scheduleExit()` 后为 `true`。 |
| `getExitReason()` | 始终为 `null`。 | 未安排退出时为 `null`；待退出时为 `'scheduled'`、`'max_memory'` 或 `'error'`。 |
| `serve(callable)` | 抛出 `OxPHP\Server\Exception\InvalidServeContextException`。 | 进入请求循环。 |

## 示例

### 按 Worker 的日志上下文

为每条日志打上 Worker id 和单线程请求计数器，以便将请求流量与特定 Worker 关联起来。

```php
<?php
$worker = OxPHP\Server\Worker::current();

$logger->info('handling request', [
    'worker_id'      => $worker->getId(),
    'request_number' => $worker->getRequestCount(),
]);
```

### 每个 OS 线程仅初始化一次

`getRequestCount()` 从 1 开始计数，因此任何线程处理的第一个请求都会看到值 `1`。这是一个执行延迟初始化的可移植入口，每个线程只会运行一次。

```php
<?php
$worker = OxPHP\Server\Worker::current();

if ($worker->getRequestCount() === 1) {
    bootstrap();
}
```

### scheduleExit

应用层主动触发 Worker 回收。当前请求正常完成后，循环检查 `isExitScheduled()` 并退出。监督进程会重新拉起新的 Worker，重新执行 Worker 文件的外层作用域。

```php
<?php
$worker = OxPHP\Server\Worker::current();

handleRequest();

// 本地开发时让 bootstrap 在每次请求后重新加载。
if (getenv('OXPHP_DEV') === '1') {
    $worker->scheduleExit();
}
```

`scheduleExit()` 是幂等的，且在非 Worker 模式下为 no-op。使用场景：

- **开发期热重载** — 每次请求后退出，让外层 bootstrap 再次执行。
- **基于 RSS 的回收** — `WORKER_MAX_MEMORY_MIB` 仅衡量 Zend 分配器。在涉及 curl、mysqli 等重型扩展的场景下，可在进程 RSS 超过你设定的阈值时触发回收：

  ```php
  if ($worker->getRss() > 256 * 1024 * 1024) {
      $worker->scheduleExit();
  }
  ```

- **协调式滚动重启** — 将该调用挂在哨兵文件或外部信号上，让外部编排系统能干净地腾空 Worker。

### Worker 入口点

在 `WORKER_FILE` 脚本中，调用 `serve()` 进入请求循环。

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

OxPHP\Server\Worker::current()->serve(function () {
    handleRequest();
});
```

### RSS 可观测性

`getRss()` 以字节为单位返回进程的实时常驻内存集大小。底层是一次真实的系统调用——开销低，但并非零成本。每次请求最多调用一次。

```php
<?php
$worker = OxPHP\Server\Worker::current();
$rss = $worker->getRss();

$metrics->gauge('php_worker_rss_bytes', $rss, [
    'worker_id' => (string) $worker->getId(),
]);
```

## 从 `oxphp_*` 函数迁移

旧的自由函数仍然可用，并通过相同的内部状态工作——它们并未被弃用。新代码应优先使用类 API，以获得更好的可发现性和一致性。

| 旧函数 | 类 API |
|--------|--------|
| `oxphp_is_worker()` | `OxPHP\Server\Worker::isWorkerMode()` |
| `oxphp_worker_id()` | `OxPHP\Server\Worker::current()->getId()` |
| `oxphp_worker(callable)` | `OxPHP\Server\Worker::current()->serve(callable)` |

## 注意事项

- **`getRss()` 不缓存。** 每次调用都会执行系统调用（Linux 上读取 `/proc/self/statm`，macOS 上调用 `getrusage(RUSAGE_SELF)`）。开销低但非零——每次请求最多调用一次，通常在指标处理器内部，而不是每行日志都调用。
- **禁止克隆。** `clone $worker` 会抛出 `\Error("Cloning OxPHP\\Server\\Worker is not allowed")`。Worker 句柄代表 OS 线程身份；克隆会造成同一线程存在第二个句柄的错觉。
- **在 OxPHP 宿主之外**（例如，链接到 SAPI 的扩展被加载到 PHP CLI 中时），`Worker::current()` 仍会返回一个实例，但所有访问器都返回零状态值：`getId()` 为 `0`，`getStartTime()` 为进程启动时间，`getRequestCount()` 为 `0`，`getRss()` 为实时 RSS，而 `serve()` 抛出 `InvalidServeContextException`。

## 参见

- [Worker 模式](../features/worker-mode.md) — 持久 PHP 进程与"仅初始化一次"模式概览
- [PHP 函数](functions.md) — 旧自由函数 `oxphp_*` 的参考
- [Request API](request-api.md) — `OxPHP\Http\RequestInterface::startTime()` 用于按请求计时
