---
title: 基于 Fiber 的请求多路复用
description: 协作式多任务处理，让 PHP 工作线程同时处理多个并发请求
---

OxPHP 的 Fiber 调度器使每个 PHP 工作线程能够同时处理多个并发 HTTP 请求。当请求调用挂起点（如 `oxphp_async_await()` 或 `oxphp_sleep()`）时，Fiber 让出控制权，工作线程转而处理其他请求，而非阻塞等待。

## 工作原理

每个 HTTP 请求在自己的 Fiber（PHP 8.4 底层 Fiber API 中的 `zend_fiber_context`）中运行。Fiber 的 C 栈通过 `mmap` 分配一次，并通过**循环协程**在请求间复用 —— 协程在循环中处理请求，每次请求之间挂起。这避免了每次请求都进行 `mmap`/`munmap` 的开销。

工作线程运行一个事件循环：

1. **接收新请求** —— 从有界通道非阻塞接收（`try_recv`）
2. **检查已完成的异步结果** —— 用于等待 `oxphp_async_await()` 的 Fiber
3. **检查已到期的计时器** —— 用于等待 `oxphp_sleep()` / `oxphp_usleep()` 的 Fiber
4. **恢复就绪的 Fiber** —— 恢复其状态并切换到对应的 Fiber 上下文

每个 Fiber 拥有自己的隔离状态：
- **PHP VM 状态**：`EG(vm_stack)`、`EG(execute_data)`、`EG(bailout)` —— 每次上下文切换时保存和恢复
- **PHP 超全局变量**：`$_SERVER`、`$_GET`、`$_POST` 等 —— 在 TLS 和每 Fiber 存储之间移动（非复制）
- **SAPI 头部**：响应头和 HTTP 状态码
- **C 栈限制**：`EG(stack_base)` / `EG(stack_limit)` —— 按 Fiber 设置，防止误判栈溢出
- **Rust TLS**：响应输出缓冲区、EARLY_TX（响应通道）、请求开始时间

从 PHP 脚本的角度来看，执行是连续的 —— 挂起和恢复对用户代码不可见。

### 顺序请求（无多路复用）

当请求处理器在未调用任何挂起点的情况下完成时，循环协程处理它并立即等待下一个请求。只有一个 Fiber 处于活跃状态，其 C 栈在每个请求中复用。性能等同于 Fiber 之前的工作循环 —— 无额外 `mmap`、无状态保存/恢复、无事件循环轮询。

### 多路复用请求

当处理器调用挂起点时，Fiber 挂起，调度器进入事件循环。事件循环接收新请求（按需创建额外的 Fiber）并恢复挂起条件已满足的 Fiber。每个额外的 Fiber 分配自己的 C 栈（一次），并通过循环协程复用。

```
Worker Thread
===========================
                          ┌─ try_recv ──► new request? ──► create/reuse fiber, start handler
                          │
loop ─────────────────────┤─ poll awaits ► result ready? ─► resume waiting fiber
                          │
                          └─ poll timers ► timer expired? ► resume sleeping fiber

                          (fibers that complete are finalized and their response is sent)
```

## 挂起点

以下函数在 worker 处理器内部调用时会触发 Fiber 挂起：

| 函数 | 行为 |
|------|------|
| `oxphp_async_await(int $promise_id, ?float $timeout = null)` | 挂起直到异步任务完成或超时 |
| `oxphp_async_await_all(array $promise_ids, ?float $timeout = null)` | v1 回退为阻塞（未来版本支持 Fiber 感知） |
| `oxphp_async_await_any(array $promise_ids, ?float $timeout = null)` | v1 回退为阻塞（未来版本支持 Fiber 感知） |
| `oxphp_sleep(float $seconds)` | 挂起指定时间（协作式） |
| `oxphp_usleep(int $microseconds)` | 以微秒为单位挂起指定时间（协作式） |

在 Fiber 外部调用时（传统模式），这些函数回退为各自的阻塞等效实现。

## 配置

启用工作进程模式后，Fiber 多路复用自动生效。无需额外的环境变量。

| 常量 | 值 | 说明 |
|------|-----|------|
| `OXPHP_MAX_FIBERS` | `256` | 每个工作线程的最大并发 Fiber 数（编译时常量） |

Fiber 限制防止单个工作线程积累过多挂起的请求。达到限制时，事件循环停止接收新请求，直到有活跃的 Fiber 完成。

### 多工作线程扩展

每个 PHP 工作线程运行自己的独立 Fiber 调度器。所有状态（Fiber、计时器、TLS 槽位）都是线程本地的。多个工作线程线性扩展：

| 工作线程数 | `/bench`（req/s） | `/sleep?ms=20` c40（req/s） |
|-----------|------------------|---------------------------|
| 1 | 40,144 | 867 |
| 2 | 66,619 | 1,378 |

设置 `PHP_WORKERS=N` 控制工作线程数。默认值：`CPU / 2`。

## PHP API

### `oxphp_sleep`

协作式 sleep，挂起当前 Fiber，允许工作线程在等待期间处理其他请求。

```php
oxphp_sleep(float $seconds): void
```

**参数：**

| 名称 | 类型 | 说明 |
|------|------|------|
| `$seconds` | `float` | 休眠时间（秒），例如 `0.5` 表示 500ms |

**行为：**
- 在工作进程模式下：注册计时器并挂起 Fiber。调度器在指定时间后恢复它。
- 在 Fiber 外部（传统模式）：回退为阻塞 `usleep()`。
- 小于或等于零的值立即返回，无任何效果。

### `oxphp_usleep`

协作式微秒级 sleep。与 `oxphp_sleep()` 功能相同，但接受微秒整数作为参数，与 PHP 内置 `usleep()` 保持一致。

```php
oxphp_usleep(int $microseconds): void
```

**参数：**

| 名称 | 类型 | 说明 |
|------|------|------|
| `$microseconds` | `int` | 休眠时间（微秒） |

**行为：** 与 `oxphp_sleep()` 相同，但使用微秒粒度。小于或等于零的值立即返回。

## 向后兼容性

Fiber 多路复用完全向后兼容：

- **不调用挂起点的处理器**通过循环协程运行，零额外开销。Fiber 的 C 栈被复用，不进行状态保存/恢复。性能等同于 Fiber 之前的工作循环。
- **`oxphp_sleep()` 和 `oxphp_usleep()` 在工作进程模式外部**回退为阻塞 `usleep()`。
- **`oxphp_async_await()` 在 Fiber 外部**回退为在 oneshot 通道上阻塞。
- **无需新的环境变量**。
- **多工作线程配置**行为完全相同 —— 每个工作线程拥有自己的独立 Fiber 调度器。

## 限制（v1）

- **带自定义回调的 `ob_start()`** 在挂起点之间可能有意外行为。输出缓冲区在挂起时会刷新到 Rust 响应缓冲区，因此自定义 OB 回调在挂起边界处看到的是部分输出。
- **共享可变闭包变量**（`use (&$var)`）在挂起点处可能交错执行。两个挂起点之间的代码不会被中断。在每个挂起点，其他请求可能在同一工作线程上执行。这与 Node.js 的 `async`/`await` 并发模型相同。
- **`oxphp_async_await_all` 和 `oxphp_async_await_any`** 在 Fiber 模式下回退为阻塞（v1）。未来版本将添加 Fiber 感知的实现。
- **每个工作线程最多 256 个并发 Fiber。** 这是编译时常量（`OXPHP_MAX_FIBERS`）。
- **REQUEST_DATA**（服务器变量、cookie、请求体）尚未按 Fiber 保存/恢复。当多个 Fiber 处于活跃状态时，它们共享最近加载的请求数据。这会影响多路复用场景中挂起点之后的 `$_SERVER`、`$_GET`、`$_POST`。

## 示例

### 使用协作式 sleep 的 SSE

此示例在事件间延迟期间让出工作线程，流式传输 Server-Sent Events。在每次 `oxphp_sleep()` 调用期间处理其他请求。

```php
<?php
oxphp_worker(function() {
    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');

    for ($i = 0; $i < 10; $i++) {
        echo "data: " . json_encode(['counter' => $i]) . "\n\n";
        oxphp_stream_flush();
        oxphp_sleep(1.0); // yields fiber, worker handles other requests
    }

    echo "event: done\ndata: {}\n\n";
    oxphp_stream_flush();
});
```

### 带多路复用的异步等待

当 `oxphp_async_await()` 挂起 Fiber 时，工作线程在等待异步结果期间处理其他 HTTP 请求：

```php
<?php
oxphp_worker(function() {
    $config = ['dsn' => 'mysql:host=localhost;dbname=app', 'user' => 'root', 'pass' => ''];

    // Dispatch async task
    $promise = oxphp_async(function() use ($config): array {
        $pdo = new PDO($config['dsn'], $config['user'], $config['pass']);
        return $pdo->query('SELECT count(*) as c FROM users')->fetch();
    });

    // Fiber suspends here — worker handles other requests
    $result = oxphp_async_await($promise);

    header('Content-Type: application/json');
    echo json_encode($result);
});
```

## 另请参阅

- [异步 Promise](async-promises.md) --- 使用 `oxphp_async()` 和 `oxphp_async_await()` 进行并行执行
- [工作池](../architecture/worker-pool.md) --- HTTP 工作池架构和工作进程模式
- [PHP 扩展函数](../php/functions.md) --- 完整函数参考，包括 `oxphp_sleep` 和 `oxphp_usleep`
