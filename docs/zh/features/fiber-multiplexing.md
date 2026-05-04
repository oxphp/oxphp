---
title: Fiber 多路复用
description: 使用协作式多任务处理，在单个 PHP Worker 线程上处理数百个并发请求。
---

# Fiber 多路复用

OxPHP 使用 PHP Fiber 在单个 Worker 线程上并发处理多个 HTTP 请求。当某个请求调用 `oxphp_sleep()` 或 `oxphp_async_await()`（当异步池启用时）时，它会挂起，Worker 线程立即处理下一个请求。这使得一个 Worker 无需额外线程就能管理数百个正在处理中的请求。

## 工作原理

1. 请求到达后，调度器将其分配给一个 fiber——一个拥有独立栈和 PHP 状态的轻量级执行上下文
2. fiber 运行 `oxphp_worker()` 处理器。如果处理器完成时没有挂起，响应会被发送，fiber 被回收——与单请求 Worker 相比开销为零
3. 如果处理器调用了挂起函数（`oxphp_sleep()`、`oxphp_usleep()`、`oxphp_async_await()`），fiber 会将控制权交回调度器
4. 调度器接收新的传入请求（创建新 fiber），并恢复等待条件已满足的挂起 fiber（计时器到期、异步结果就绪）
5. 每个 fiber 的 PHP 状态——超全局变量、响应头、输出缓冲区、虚拟机栈——在挂起时保存，在恢复时还原。fiber 之间完全隔离

```text
Worker 线程
  │
  ├─ Fiber A: 处理 /api/users ──── oxphp_sleep(0.5) ──── [已挂起] ──── [已恢复] ──── 响应
  │
  ├─ Fiber B: 处理 /api/orders ──── oxphp_async_await($p) ──── [已挂起] ──── [已恢复] ──── 响应
  │
  └─ Fiber C: 处理 /health ──── 响应（无挂起，零开销）
```

## 配置

当 Worker 模式启用时，fiber 多路复用会自动激活。没有额外的环境变量。

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `WORKER_MODE_ENABLED` | `false` | 设为 `true` 并将 `ENTRY_FILE` 指向 `.php` 引导脚本，以启用 Worker 模式和 fiber 多路复用 |
| `PHP_WORKERS` | CPU / 2（最少 1） | Worker 线程数。每个线程运行其独立的调度器，最多支持 256 个并发 fiber |

每个 Worker 线程的最大并发 fiber 数为 256。使用 4 个 Worker 线程时，OxPHP 最多可以同时处理 1,024 个正在处理中的请求。

## 挂起点

以下函数会挂起当前 fiber，让其他请求在同一线程上运行：

| 函数 | 行为说明 |
|------|----------|
| `oxphp_sleep(float $seconds)` | 将 fiber 挂起指定时长。其他 fiber 继续运行 |
| `oxphp_usleep(int $microseconds)` | 与 `oxphp_sleep()` 相同，但精度为微秒（最小 1 毫秒） |
| `oxphp_async_await(int $promise_id)` | 挂起 fiber 直到异步任务在后台线程池上完成 |

以下函数**不会**挂起 fiber：

| 函数 | 行为说明 |
|------|----------|
| `oxphp_stream_flush()` | 立即将数据块发送给客户端并返回。在 SSE 循环中与 `oxphp_sleep()` 配合使用 |
| `oxphp_finish_request()` | 发送完整响应并继续 PHP 执行。不产生让步 |

> **注意：** PHP 内置的 `sleep()` 和 `usleep()` 会阻塞整个 Worker 线程。请始终使用 `oxphp_sleep()` 和 `oxphp_usleep()` 以获得协作式行为。

## PHP 示例

### 基本并发处理

不挂起的请求以全速运行，fiber 开销为零：

```php
<?php
oxphp_worker(function () {
    $path = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);

    if ($path === '/health') {
        echo json_encode(['status' => 'ok']);
        return; // 无挂起——以全速运行
    }

    if ($path === '/slow') {
        oxphp_sleep(2.0); // 让步 2 秒——其他请求继续运行
        echo "Done after 2s delay";
        return;
    }

    echo "Hello";
});
```

### 非阻塞 API 调用

将 `oxphp_async()` 与 `oxphp_async_await()` 结合使用，在不阻塞 Worker 的情况下发起外部 API 调用：

```php
<?php
oxphp_worker(function () {
    // 将两个 API 调用分发到异步线程池
    $p1 = oxphp_async(fn() => file_get_contents('https://api.example.com/users'));
    $p2 = oxphp_async(fn() => file_get_contents('https://api.example.com/orders'));

    // 等待两个结果——fiber 挂起，同一线程上的其他请求继续运行
    $users  = oxphp_async_await($p1);
    $orders = oxphp_async_await($p2);

    header('Content-Type: application/json');
    echo json_encode(['users' => json_decode($users), 'orders' => json_decode($orders)]);
});
```

### 使用协作式睡眠的 SSE

使用 `oxphp_stream_flush()` 和 `oxphp_sleep()` 将 Server-Sent Events 与其他请求交错处理：

```php
<?php
oxphp_worker(function () {
    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');

    for ($i = 0; $i < 30; $i++) {
        echo "data: " . json_encode(['count' => $i, 'time' => time()]) . "\n\n";
        oxphp_stream_flush(); // 立即发送数据块（不挂起）
        oxphp_sleep(1.0);     // 让步 1 秒（其他请求继续运行）
    }
});
```

## 阻塞 I/O

Fiber 多路复用是**协作式的，不是抢占式的**。调用阻塞函数的 fiber 会冻结整个 Worker 线程——该线程上的其他 fiber 无法继续执行。

### 会阻塞 Worker 的函数

- `file_get_contents()`、`fopen()`、`fread()`
- `curl_exec()`、`curl_multi_exec()`
- PDO 查询、`mysqli_query()`
- PHP 的 `sleep()`、`usleep()`（请改用 `oxphp_sleep()`）
- DNS 解析（`gethostbyname()`）
- 任何同步网络或磁盘 I/O

### 如何避免阻塞

将阻塞操作封装在 `oxphp_async()` 中，在异步线程池上运行：

```php
<?php
// 错误——阻塞整个 Worker 线程
$html = file_get_contents('https://example.com');

// 正确——在异步池上运行，fiber 让步
$promise = oxphp_async(fn() => file_get_contents('https://example.com'));
$html = oxphp_async_await($promise);
```

对于数据库查询：

```php
<?php
$db = new PDO('mysql:host=db;dbname=app', 'root', 'secret');

// 错误——阻塞 Worker
$users = $db->query('SELECT * FROM users WHERE active = 1')->fetchAll();

// 正确——查询在异步线程上运行，fiber 让步
$promise = oxphp_async(function () {
    $db = new PDO('mysql:host=db;dbname=app', 'root', 'secret');
    return $db->query('SELECT * FROM users WHERE active = 1')->fetchAll();
});
$users = oxphp_async_await($promise);
```

> **注意：** 数据库连接不能传递给 `oxphp_async()`，因为对象无法跨线程序列化。请在异步闭包内部创建连接，或者如果查询足够快速以至于阻塞是可以接受的，则直接在 fiber 中使用查询。

> **重要：** `oxphp_async()` 需要 `ASYNC_WORKERS > 0`。当异步池被禁用时（默认），调用 `oxphp_async()` 会抛出 `OxPHP\Async\Exception`。

## Fiber 的回收方式

Fiber 的 C 栈只分配一次，并跨请求复用。当 fiber 完成一次请求处理后，它不会被销毁——而是挂起回调度器并被添加到空闲列表中。下一个请求复用现有的 C 栈，避免了昂贵的内存分配。

PHP 虚拟机栈（用于函数调用帧）在每次请求时全新分配，并在处理器返回时释放。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "80:80"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - WORKER_MODE_ENABLED=true
      - ENTRY_FILE=worker.php
      - PHP_WORKERS=4
      - ASYNC_WORKERS=8
```

使用此配置，4 个 Worker 线程中的每个都可以处理最多 256 个并发 fiber，阻塞 I/O 被卸载到 8 个异步 Worker 线程。

## 故障排除

### 某个请求执行大量 I/O 时所有请求都变慢

某个 fiber 在没有使用 `oxphp_async()` 的情况下调用了阻塞函数（数据库查询、HTTP 请求、文件读取）。这会阻塞整个 Worker 线程。

**修复：** 将阻塞调用封装在 `oxphp_async()` 中：

```php
<?php
$promise = oxphp_async(fn() => file_get_contents($url));
$result = oxphp_async_await($promise);
```

### 使用 oxphp_async() 时出现"Async pool is disabled"

异步池未配置。当 `ASYNC_WORKERS=0`（默认值）时，所有异步函数均会抛出 `OxPHP\Async\Exception`。

**修复：** 将 `ASYNC_WORKERS` 设置为正整数：

```bash
ASYNC_WORKERS=8
```

### 使用 oxphp_async() 时出现"Failed to dispatch async task"

异步池正在运行，但容量已满。

**修复：** 增大 `ASYNC_WORKERS` 或 `ASYNC_QUEUE_CAPACITY`：

```bash
ASYNC_WORKERS=8
ASYNC_QUEUE_CAPACITY=512
```

### oxphp_sleep() 没有向其他请求让步

Fiber 多路复用仅在 Worker 模式下工作。在传统模式下，`oxphp_sleep()` 回退到阻塞式 `usleep()`。

**修复：** 启用 Worker 模式（`WORKER_MODE_ENABLED=true`）。

### 并发请求多时内存使用量高

每个 fiber 使用一个 C 栈（默认 8 MiB，由 PHP 的 `fiber.stack_size` ini 设置配置）以及每次请求的 PHP 虚拟机栈。在 256 个并发 fiber 的情况下，每个 Worker 线程最坏情况下的 C 栈内存为 2 GiB。

**修复：** 如果您的应用不使用深层递归，请在 `php.ini` 中减小 `fiber.stack_size`：

```ini
fiber.stack_size = 512K
```

## 限制

- **仅限 Worker 模式** — fiber 多路复用在传统模式下不可用
- **每个 Worker 最多 256 个 fiber** — 硬性限制，运行时不可配置
- **仅协作式** — CPU 密集型代码（紧密循环、大量计算）会使其他 fiber 饥饿。没有抢占机制
- **阻塞 I/O 会阻塞线程** — 所有阻塞调用必须封装在 `oxphp_async()` 中才能实现真正的并发
- **PHP 原生的 `sleep()`/`usleep()` 不感知 fiber** — 请使用 `oxphp_sleep()`/`oxphp_usleep()`
- **`oxphp_async_await_all()` 和 `oxphp_async_await_any()` 不产生让步** — 即使在 fiber 内部它们目前也会阻塞。对于 fiber 友好的行为，请使用顺序的 `oxphp_async_await()` 调用

## 参见

- [Worker 模式](worker-mode.md) — 持久化 PHP 进程和 `oxphp_worker()` API
- [异步 Promise](async-promises.md) — 用于卸载阻塞 I/O 的后台线程池
- [SSE](sse.md) — 结合基于 fiber 的协作式睡眠的实时流式传输
- [PHP 函数](../php/functions.md) — `oxphp_sleep()`、`oxphp_usleep()` 及其他感知 fiber 的函数
- [配置参考](../operations/configuration.md) — `WORKER_MODE_ENABLED`、`ENTRY_FILE`、`PHP_WORKERS`、`ASYNC_WORKERS`
