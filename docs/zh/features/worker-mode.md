---
title: Worker 模式
description: 持久化 PHP 进程只启动一次并处理多个请求，消除 OxPHP 中每次请求的启动开销。
---

# Worker 模式

Worker 模式运行持久化 PHP 进程，这些进程只启动一次并处理多个请求，从而消除每次请求的启动开销。您的应用无需在每次请求时拆除并重建 PHP 状态，而是只需加载一次自动加载器、配置和数据库连接，并在 Worker 的整个生命周期内复用它们。

## 工作原理

1. **设置 `WORKER_MODE_ENABLED=true`** 并将 **`ENTRY_FILE`** 指向您的引导脚本路径。这将为池中所有 PHP Worker 启用 Worker 模式。
2. **PHP 启动并只运行一次外部作用域** — 自动加载器注册、配置加载、数据库连接以及任何其他初始化代码只执行一次。
3. **调用 `oxphp_worker(callback)`** 进入请求循环。OxPHP 开始将传入的 HTTP 请求分发到您的回调函数。
4. **请求之间**，超全局变量（`$_GET`、`$_POST`、`$_SERVER`、`$_COOKIE`、`$_FILES`、`php://input`）、输出缓冲区和响应头会自动重置。软重置会清理每次请求的状态，同时保留外部作用域中已引导的资源。
5. **外部作用域持久化** — 在 `oxphp_worker()` 之前定义的变量、静态属性、数据库连接和自动加载器在该 Worker 处理的所有请求中保持可用。

> **注意：** Worker 模式还会更改路由行为。所有不匹配磁盘上静态文件的请求都会分发到 Worker，而不是返回 404。详情请参见[路由](routing.md)。

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `WORKER_MODE_ENABLED` | `false` | 启用持久化 Worker 模式。接受 `true`、`1`、`yes`。要求 `ENTRY_FILE` 指向 `.php` 脚本 |
| `ENTRY_FILE` | *(未设置)* | Worker 引导脚本的路径。相对路径基于 `DOCUMENT_ROOT` 解析；允许 `..` 段和绝对路径（Worker 引导位于公开目录之外是受支持的部署方式） |
| `WORKER_MAX_MEMORY_MIB` | `0` | Worker 回收前每个 Worker 的最大 PHP 内存（MiB）。`0` = 无限制 |

> **从 `WORKER_FILE` 迁移：** 旧变量仍会被解析（启动时输出 `WARN`），其行为等同于 `WORKER_MODE_ENABLED=true ENTRY_FILE=$WORKER_FILE`。新部署应使用上述显式组合；旧形式将在后续版本中移除。

应用层主动回收可在请求处理器中调用 [`OxPHP\Server\Worker::scheduleExit()`](../php/worker-class.md#scheduleexit)。当前请求正常完成后 Worker 会安全退出。

## 编写 Worker 脚本

Worker 脚本由两部分组成：在启动时运行一次的外部作用域，以及传递给 `oxphp_worker()` 的回调函数（每次请求时运行）。

```php
<?php
// 外部作用域：在启动时运行一次
require __DIR__ . '/../vendor/autoload.php';

$config = parse_ini_file(__DIR__ . '/../config/app.ini');
$db = new PDO($config['dsn'], $config['user'], $config['pass'], [
    PDO::ATTR_PERSISTENT => true,
]);

$app = new MyApp\Application($config, $db);

// 请求循环：每次请求时运行
oxphp_worker(function () use ($app) {
    $app->handle();
});

// 关闭：Worker 退出时运行
$app->terminate();
```

## 请求之间重置的内容

OxPHP 在请求之间执行软重置。以下状态会自动清理：

- **超全局变量** — `$_GET`、`$_POST`、`$_SERVER`、`$_COOKIE`、`$_FILES` 和 `php://input` 会用新请求数据重新填充
- **输出缓冲区** — 所有输出缓冲区被刷新并清空
- **响应头** — HTTP 状态码和响应头重置为默认值
- **错误状态** — 最后的错误信息（消息、文件、行号、类型）和连接状态被清除。用户注册的错误处理器（`set_error_handler()`）、异常处理器（`set_exception_handler()`）以及 `error_reporting()` 级别在请求之间保持不变

## 持久化的内容

以下状态在同一 Worker 处理的请求之间保持存在：

- **外部作用域中的变量** — 在 `oxphp_worker()` 之前定义并通过 `use` 捕获的任何内容
- **静态属性** — 类的静态属性保留其值
- **数据库连接** — PDO、MySQLi 及其他持久连接保持打开状态
- **自动加载器** — 已注册的自动加载器（Composer、自定义）保持活跃
- **已加载的类和函数** — 所有之前加载的类、接口、trait 和函数

## 回收

当满足以下任一条件时，Worker 会自动回收（以全新 PHP 进程重启）：

- **超出最大内存** — Worker 的 PHP 内存使用量超过 `WORKER_MAX_MEMORY_MIB` MiB
- **应用主动请求退出** — 处理器调用了 [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit)。适用于应用控制的热重载、基于文件 mtime 的重载，或对每个请求重新执行 bootstrap
- **连续错误** — Worker 遇到 3 次连续的处理器失败（致命错误、超时或未处理的异常）。注意 `exit()`/`die()` 调用不计为失败

当 Worker 被回收时，PHP 进程终止，新进程启动，重新执行 Worker 脚本的外部作用域。对于内存回收和 `scheduleExit()` 触发的回收，当前请求会正常完成后 Worker 才退出。对于基于错误的回收，Worker 在失败的请求之后退出。

## 开发模式下的代码热重载

Worker 模式将 bootstrap 状态（自动加载器、DI 容器、数据库连接）保留在内存中，因此仅靠 `opcache.validate_timestamps=1` 不足以加载在外层作用域中执行过的代码的变更。开发循环中有两种选择：

- **每次请求都回收 Worker。** 在每次处理器执行的末尾调用 `OxPHP\Server\Worker::current()->scheduleExit()`（例如可基于 `OXPHP_DEV` 环境标记开关）。当前请求会正常完成，然后 Worker 退出并重新启动，外层作用域会重新执行。你会失去 worker 模式的性能优势，但会获得 FPM 风格的重载语义——是积极开发中最简单、最可靠的方式。
- **保持 Worker 热态，仅刷新请求处理器。** 不调用 `scheduleExit()`，启用 `opcache.validate_timestamps=1`，并保持 bootstrap 最小化。在请求回调内加载的代码会在下次请求时被 OPcache 刷新；在外层作用域仅加载一次的代码则不会。完整的注意事项清单请参阅 [OPcache 与 JIT → 开发环境配置](../php/opcache.md#开发环境配置)。

## 故障排除

### 请求挂起且永不完成

如果引导脚本中从未调用 `oxphp_worker()`，则不会分发任何请求，每个请求都会无限等待。请验证您的脚本在正常代码路径中无条件调用了 `oxphp_worker()`。

### 请求之间的状态泄露

在 `oxphp_worker()` 回调内部定义的变量会被 PHP 的垃圾收集器清理，但在外部作用域中定义的静态属性和全局变量会持续存在。如果您看到某次请求的数据出现在另一次请求中，请检查是否有在调用之间累积状态的静态属性或全局变量。

**修复：** 在每次请求回调开始时显式重置静态状态，或避免在静态变量中存储每次请求的状态。

### Worker 立即回收（内存限制）

Worker 内存限制在每次请求后使用 PHP 报告的内存使用量进行检查。如果您的引导阶段分配了大量内存（如加载大型缓存），初始内存占用可能已经接近限制。

**修复：** 增大 `WORKER_MAX_MEMORY_MIB` 或将大型分配推迟到第一次请求时进行。

### Worker 立即回收（错误限制）

三次连续的处理器失败会触发回收。请检查您的应用日志，查看请求回调中发生的异常或致命错误。

**检查：** 在访问日志或结构化日志输出中查找错误：

```bash
docker logs <container> 2>&1 | grep '"level":"error"'
```

### 空闲后数据库连接断开

如果您的数据库服务器关闭了空闲连接，下一次请求中的重连尝试可能会失败。请使用能处理重连的连接池，或捕获异常并手动重连。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - WORKER_MODE_ENABLED=true
      - ENTRY_FILE=/var/www/html/worker.php
      - WORKER_MAX_MEMORY_MIB=128
```

## PHP API

Worker 内省和 Worker 入口点通过
[`OxPHP\Server\Worker`](../php/worker-class.md) 类提供。

```php
<?php
$worker = OxPHP\Server\Worker::current();
$worker->serve(function () {
    handleRequest();
});
```

**旧的自由函数**（`oxphp_is_worker`、`oxphp_worker_id`、`oxphp_worker`）
仍然可用，并通过相同的内部状态工作。新代码应优先使用类 API。

该类还暴露了运行时自省能力，便于实现优雅自回收、可观测性和健康检查：

| 方法 | 返回值 |
|--------|---------|
| `Worker::isWorkerMode(): bool` | 服务器是否运行在工作进程模式下 |
| `$worker->id(): int` | 稳定的每线程工作进程 ID |
| `$worker->startTime(): float` | 此工作进程启动的 Unix 时间戳 |
| `$worker->requestCount(): int` | 此工作进程已处理的请求数 |
| `$worker->memoryUsage(): int` | 此工作进程当前的 `memory_get_usage(true)` |
| `$worker->rss(): int` | 当前常驻集大小（字节，Linux/macOS） |
| `$worker->maxMemoryBytes(): int` | 回收阈值——`WORKER_MAX_MEMORY_MIB` × 1 MiB，未限制时为 `0` |
| `$worker->isExitScheduled(): bool` | 是否已调用过 `scheduleExit()` |
| `$worker->exitReason(): ?string` | 运行中为 `null`；工作进程即将退出时为 `"scheduled"`、`"max_memory"` 或 `"error"` |

完整签名和示例参见 [`OxPHP\Server\Worker`](../php/worker-class.md)。

## PHP 示例

### 检测 Worker 模式

使用 `OxPHP\Server\Worker::isWorkerMode()` 检查当前进程是否在 Worker 模式下运行。这对于编写能在传统模式和 Worker 模式下都能工作的代码非常有用。

```php
<?php
if (OxPHP\Server\Worker::isWorkerMode()) {
    // 复用持久连接
    $redis = new Redis();
    $redis->pconnect('redis', 6379);
} else {
    // 传统模式：每次请求时连接
    $redis = new Redis();
    $redis->connect('redis', 6379);
}
```

### Symfony Worker 脚本

```php
<?php
use App\Kernel;

require __DIR__ . '/../vendor/autoload.php';

$kernel = new Kernel('prod', false);
$kernel->boot();

oxphp_worker(function () use ($kernel) {
    $request = Symfony\Component\HttpFoundation\Request::createFromGlobals();
    $response = $kernel->handle($request);
    $response->send();
    $kernel->terminate($request, $response);
});

$kernel->shutdown();
```

## 最佳实践

- **设置 `WORKER_MAX_MEMORY_MIB`**（例如 `128`），让出现内存泄漏的 Worker 自动回收，而不是耗尽宿主机内存。在应用层面再叠加 `Worker::scheduleExit()` 实现主动回收。
- **避免在静态属性或全局变量中存储每次请求的状态。** 由于这些会跨请求持久化，某次请求遗留的状态可能泄露到另一次请求中。
- **尽早验证软重置。** 在开发标记下让处理器调用 `Worker::current()->scheduleExit()` 并端到端运行应用——这样能在切换到长寿命 Worker 之前发现状态泄漏类的 bug。
- **处理数据库空闲超时。** 如果您的数据库驱动在空闲一段时间后断开连接，请捕获异常并重连，或使用能自动处理重连的连接池。
- **保持外部作用域简洁。** 只引导真正需要持久化的内容——自动加载器、配置和共享服务。将请求特定的设置推迟到回调函数中。

## 参见

- [路由](routing.md) — Worker 模式如何融入 URL 路由
- [提前响应](early-response.md) — 立即发送响应并继续后台处理
- [PHP 函数](../php/functions.md) — `oxphp_worker()`、`oxphp_is_worker()` 及其他内置函数的完整参考
- [配置参考](../operations/configuration.md) — 完整的环境变量列表
