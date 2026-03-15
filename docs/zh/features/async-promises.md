---
title: 异步 Promise
description: 在专用工作池上异步执行 PHP 闭包
---

OxPHP 提供了基于 Promise 的异步 API，允许 PHP 代码将闭包分发到独立于 HTTP 工作池的专用线程池上执行。这实现了真正的并行计算，且不会阻塞请求线程。

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `ASYNC_WORKERS` | `0`（禁用） | 专用异步工作线程数。`0` 禁用异步池 |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS * 64` | 待处理异步任务的有界通道大小。队列满时任务将被拒绝 |

启用异步池：

```bash
ASYNC_WORKERS=4
```

设为 `0`（或不设置）将完全禁用异步支持。禁用时调用 `oxphp_async()` 将触发 `E_WARNING` 并返回 `false`。

## PHP API

### `oxphp_async`

将闭包分发到异步工作池进行异步执行。

```php
oxphp_async(Closure $closure, mixed ...$args): int|false
```

**参数：**

| 名称 | 类型 | 说明 |
|------|------|------|
| `$closure` | `Closure` | 要异步执行的闭包 |
| `...$args` | `mixed` | 通过深拷贝传递给闭包的参数 |

**返回值：** 成功时返回 Promise ID（正整数），如果异步池未配置或队列已满则返回 `false`。

**示例：**

```php
<?php
$promise = oxphp_async(function(int $n): int {
    return $n * $n;
}, 42);

$result = oxphp_async_await($promise);
// 1764
```

### `oxphp_async_await`

阻塞当前线程，直到异步任务完成并返回结果。

```php
oxphp_async_await(int $promise_id, ?float $timeout = null): mixed
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$promise_id` | `int` | *(必填)* | `oxphp_async()` 返回的 Promise ID |
| `$timeout` | `?float` | `null` | 超时时间（秒）。`null` 表示无限等待 |

**返回值：** 闭包的返回值。

**抛出异常：**

| 异常 | 条件 |
|------|------|
| `OxPHP\AsyncException` | 闭包抛出了异常或调用了 `die()`/`exit()` |
| `OxPHP\AsyncTimeoutException` | 超时时间到期而任务尚未完成 |

### `oxphp_async_await_all`

等待多个 Promise 并以数组形式返回所有结果。

```php
oxphp_async_await_all(array $promise_ids, ?float $timeout = null): array
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$promise_ids` | `array` | *(必填)* | Promise ID 数组 |
| `$timeout` | `?float` | `null` | 每个 Promise 的超时时间（秒） |

**返回值：** 将 Promise ID 映射到其结果的关联数组。

**抛出异常：** 如果任何 Promise 失败或超时，抛出 `OxPHP\AsyncException` 或 `OxPHP\AsyncTimeoutException`。

### `oxphp_async_await_any`

竞速多个 Promise 并返回最先完成的结果，与数组顺序无关。内部使用 `futures::select_all` 实现真正的并发竞速语义。未获胜的 Promise 仍可通过 `oxphp_async_await()` 单独等待。

```php
oxphp_async_await_any(array $promise_ids, ?float $timeout = null): array
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$promise_ids` | `array` | *(必填)* | Promise ID 数组 |
| `$timeout` | `?float` | `null` | 任何 Promise 完成的整体超时时间（秒） |

**返回值：** 包含 `id`（获胜的 Promise ID）和 `value`（其结果）的关联数组。

**抛出异常：** 如果最先完成的 Promise 抛出了异常，抛出 `OxPHP\AsyncException`。如果在超时时间内没有 Promise 完成，抛出 `OxPHP\AsyncTimeoutException`。

**注意：** 超时后，所有指定的 Promise 都会被取消，之后无法单独等待。

## 数据传输语义

闭包在不同的操作系统线程上执行，该线程拥有自己的 PHP ZTS 引擎状态和 `zend_mm_heap`。数据不能通过指针在线程间共享 —— 任何指向一个线程堆上分配的字符串或数组的指针在另一个线程上都是无效的。OxPHP 使用**可移植二进制序列化**来安全地跨线程边界传输所有数据。

### 工作原理

所有跨线程边界的值（参数、`use` 变量、返回值）都使用系统 `malloc` 序列化为扁平字节缓冲区。缓冲区被传输到目标线程，目标线程使用其自身堆上的 `emalloc` 进行反序列化。这保证了每个 PHP 分配都属于正确的每线程 `zend_mm_heap`。

### 捕获的变量（通过 `use`）

闭包 `use` 子句捕获的变量在源线程上**序列化**，在异步工作线程上**反序列化**为独立副本。源变量在异步执行期间保持不变且完全可用。

```php
<?php
$config = ['db' => 'mysql', 'timeout' => 30];

$p = oxphp_async(function() use ($config): string {
    // $config 是独立副本 — 读写操作正常工作
    return $config['db'];
});

// $config 在此处仍可写 — 它是被复制的，而非冻结
$config['timeout'] = 60; // 安全
$result = oxphp_async_await($p);
```

### 参数

通过 `...$args` 传递的参数以相同方式序列化和反序列化。支持的类型：

| 类型 | 传输方式 |
|------|----------|
| `null`、`bool`、`int`、`float` | 序列化为值（1-9 字节） |
| `string` | 带长度前缀 + 数据序列化 |
| `array` | 递归序列化（键 + 值） |
| `resource` | **拒绝** — 抛出 `OxPHP\AsyncException` |
| `object` | **拒绝** — 抛出 `OxPHP\AsyncException`（对象无法跨线程序列化） |

### 返回值

闭包的返回值在异步工作线程上序列化，在源线程上使用相同机制反序列化。支持所有标量类型、字符串和数组（包括嵌套）。

## 异常处理

异步闭包内抛出的异常会被捕获，并在调用 `oxphp_async_await()` 时重新抛出为 `OxPHP\AsyncException`。原始异常类名和消息保留在 `AsyncException` 消息中：

```php
<?php
$p = oxphp_async(function(): never {
    throw new \DomainException('invalid value', 422);
});

try {
    oxphp_async_await($p);
} catch (\OxPHP\AsyncException $e) {
    echo $e->getMessage();
    // "Async task failed: [DomainException] invalid value"
}
```

### die() 和 exit()

异步闭包内调用 `die()` 或 `exit()` 会被 `zend_try`/`zend_catch` 捕获并转换为 `OxPHP\AsyncException`。异步工作池继续存活 —— 后续任务正常执行：

```php
<?php
// 第一轮：die() — 捕获为 AsyncException
$p1 = oxphp_async(function(): never { die('fatal'); });
try { oxphp_async_await($p1); } catch (\OxPHP\AsyncException $e) { /* 已处理 */ }

// 第二轮：池仍然存活，正常任务正常工作
$p2 = oxphp_async(function(): int { return 42; });
$result = oxphp_async_await($p2); // 42
```

### 异常类

| 类 | 父类 | 条件 |
|----|------|------|
| `OxPHP\AsyncException` | `Exception` | 闭包抛出了异常，或调用了 `die()`/`exit()` |
| `OxPHP\AsyncTimeoutException` | `OxPHP\AsyncException` | 超时时间到期而任务尚未完成 |

## Promise 作用域和生命周期

Promise 存储在创建它们的 PHP 工作线程的**线程本地存储**中。这有三个含义：

1. **线程绑定。** Promise 只能由调用 `oxphp_async()` 的同一工作线程等待。另一个工作线程拥有自己的 Promise 映射表，无法看到外部的 Promise ID。

2. **请求作用域。** 在每个请求结束时（RSHUTDOWN），所有未完成的 Promise 会被自动清理。在工作进程模式下，相同的清理在请求之间运行。Promise 无法跨请求边界存活。

3. **ID 是每线程的，不是全局唯一的。** Promise 计数器在线程内单调递增，不会在请求之间重置。两个不同的工作线程可能都有 ID 为 `0` 的 Promise —— 这些是不同映射表中的独立 Promise。

```
Worker Thread #1                    Worker Thread #2
┌─ Request A ─────────────┐        ┌─ Request C ──────────┐
│ PROMISE_MAP: {0, 1, 2}  │        │ PROMISE_MAP: {0, 1}  │
│ await_any([0, 1]) → OK  │        │ await(0) → OK        │
│ await(2) → OK           │        │ await(1) → OK        │
│ RSHUTDOWN: cleanup      │        │ RSHUTDOWN: cleanup    │
├─ Request B ─────────────┤        └──────────────────────┘
│ PROMISE_MAP: {3, 4}     │  ← counter continues from 3
│ (not awaited)           │
│ RSHUTDOWN: cancel + free│  ← promises 3, 4 cleaned up
└─────────────────────────┘
```

要在请求间共享异步结果，请使用外部机制（Redis、共享内存、数据库）。

## RSHUTDOWN 清理

如果 PHP 请求结束时未等待所有已分发的 Promise，RSHUTDOWN 钩子会自动清理未完成的 Promise。每个未等待的 Promise 将获得 5 秒超时来完成，之后会被取消。这防止了被遗忘的 Promise 导致资源泄漏。

```php
<?php
$p1 = oxphp_async(function(): int { return 1; });
$p2 = oxphp_async(function(): int { return 2; });

$result = oxphp_async_await($p1);
// $p2 从未被等待 — 在请求结束时自动清理
```

## 架构

异步池是与 HTTP PHP 工作池**分离**的一组操作系统线程。这种分离防止了死锁：如果所有 HTTP 工作线程都分发了异步任务然后在 `oxphp_async_await()` 上阻塞，共享池将会死锁。

```
HTTP Worker Thread              Async Worker Thread
─────────────────              ────────────────────
oxphp_async($fn)
  ├─ serialize use-vars
  ├─ serialize args
  ├─ create AsyncTask           recv(AsyncTask)
  ├─ send via channel ──────►     ├─ oxphp_async_reset()
  │                               ├─ deserialize use-vars
  │                               ├─ deserialize args
  │                               ├─ oxphp_execute_async_task()
  │                               ├─ serialize retval
oxphp_async_await($id)                  ├─ send AsyncResult
  ├─ block on oneshot ◄──────     └─ free local data
  ├─ deserialize result
  └─ return value
```

所有跨线程边界的数据都经过可移植二进制序列化：在源线程上使用系统 `malloc` 序列化，传输缓冲区，在目标线程上使用 `emalloc` 反序列化。这保证了每个 PHP 分配都属于正确的每线程 `zend_mm_heap`。

每个异步工作线程：
1. 初始化 TSRM（Zend 线程安全）线程本地存储
2. 在线程启动时调用一次 `php_request_startup()`
3. 循环：接收任务 → 重置状态 → 反序列化数据 → 执行闭包 → 序列化结果 → 发送
4. 退出时调用 `php_request_shutdown()`

## 容量规划与调优

异步工作线程是拥有完整 PHP ZTS 初始化的专用操作系统线程。它们与 HTTP 工作线程和 Tokio 运行时竞争 CPU 核心。正确的容量规划可以防止 CPU 竞争，确保异步任务不会饿死 HTTP 请求处理。

### 线程预算

所有池的总线程数应保持在 CPU 核心数之内：

```
Total threads = TOKIO_WORKERS + PHP_WORKERS + ASYNC_WORKERS ≤ CPU cores
```

超出此预算会导致上下文切换开销，降低所有池的吞吐量。

| 8 核服务器 | TOKIO | PHP | ASYNC | 总计 | 评估 |
|------------|-------|-----|-------|------|------|
| 保守方案 | 4 | 4 | 2 | 10 | 良好 — 轻微超配是可以接受的 |
| 激进方案 | 4 | 4 | 4 | 12 | 可接受（如果异步任务是 I/O 密集型） |
| 超配方案 | 4 | 8 | 8 | 20 | 差 — 上下文切换开销占主导 |

### 按工作负载类型设定 ASYNC_WORKERS

最佳数量取决于异步任务是 CPU 密集型还是 I/O 密集型：

| 工作负载 | 公式 | 原因 |
|----------|------|------|
| **CPU 密集型**（计算、数据处理） | `CPU_cores / 4` | 更多线程无济于事 — 它们与 HTTP 工作线程竞争相同的核心 |
| **I/O 密集型**（sleep、网络调用、文件 I/O） | `PHP_WORKERS` | 线程大部分时间阻塞在 I/O 上，不消耗 CPU |
| **混合型**（典型场景） | `PHP_WORKERS / 2` | 从此开始，根据指标调整 |

### 按延迟要求设定 ASYNC_QUEUE_CAPACITY

队列中的每个任务在内存中持有序列化的参数。队列深度是内存和延迟与突发容忍度之间的权衡：

| 场景 | 建议容量 | 原因 |
|------|----------|------|
| **Web 请求**（延迟敏感） | `ASYNC_WORKERS * 4..8` | 尽早拒绝并回退，而不是排队等待 |
| **后台/批处理**（吞吐敏感） | `ASYNC_WORKERS * 64`（默认值） | 缓冲任务供后续处理是可以接受的 |

### 避免阻塞

当 HTTP 工作线程调用 `oxphp_async_await()` 时，它会阻塞直到异步任务完成。如果所有 HTTP 工作线程同时阻塞且异步工作线程跟不上，请求吞吐量将降至零。

约束条件：

```
ASYNC_WORKERS ≥ max concurrent oxphp_async_await() callers
```

实际操作中，估计使用异步的 HTTP 请求比例：

| 异步使用率 | 容量规则 |
|------------|----------|
| 每个请求都分发异步任务 | `ASYNC_WORKERS ≥ PHP_WORKERS` |
| 约 30% 的请求 | `ASYNC_WORKERS ≥ PHP_WORKERS / 3` |
| 罕见（十分之一） | `ASYNC_WORKERS ≥ PHP_WORKERS / 4` |

### 内存开销

每个异步工作线程消耗：
- 操作系统线程栈：2-8 MB（取决于平台）
- PHP ZTS 堆：约 2-10 MB（取决于扩展和 INI 设置）
- 大致总计：**每个工作线程 4-18 MB**

排队的任务会增加序列化参数的内存开销（取决于负载大小，通常较小）。

### 配置示例

**8 核服务器，Laravel，约 30% 的请求使用异步：**

```bash
TOKIO_WORKERS=0           # auto: 4 (CPU/2)
PHP_WORKERS=4
ASYNC_WORKERS=2           # CPU/4, sufficient for 30% async load
ASYNC_QUEUE_CAPACITY=16   # low-latency: 2 * 8
```

**16 核服务器，批处理，每个请求都进行扇出：**

```bash
TOKIO_WORKERS=0           # auto: 8 (CPU/2)
PHP_WORKERS=6
ASYNC_WORKERS=6           # match PHP workers — all requests use async
ASYNC_QUEUE_CAPACITY=384  # 6 * 64, high throughput
```

**4 核容器，偶尔后台任务：**

```bash
TOKIO_WORKERS=1           # single-threaded, save cores for PHP
PHP_WORKERS=2
ASYNC_WORKERS=1           # minimal — async is rare
ASYNC_QUEUE_CAPACITY=8    # 1 * 8
```

### 监控

部署后，使用以下 Prometheus 查询验证容量配置：

```promql
# Tasks rejected? → increase ASYNC_QUEUE_CAPACITY or ASYNC_WORKERS
rate(oxphp_async_tasks_rejected_total[5m]) > 0

# Task backlog growing? → increase ASYNC_WORKERS
oxphp_async_tasks_dispatched_total
  - oxphp_async_tasks_completed_total
  - oxphp_async_tasks_failed_total
  - oxphp_async_tasks_cancelled_total

# CPU saturated? → reduce ASYNC_WORKERS
# Check system metrics: load average, CPU utilization, steal time
```

## 指标

当异步池处于活跃状态（至少分发了一个任务）时，将输出五个 Prometheus 计数器：

| 指标 | 类型 | 说明 |
|------|------|------|
| `oxphp_async_tasks_dispatched_total` | counter | 通过 `oxphp_async()` 提交的任务总数 |
| `oxphp_async_tasks_completed_total` | counter | 成功返回值的任务数 |
| `oxphp_async_tasks_failed_total` | counter | 抛出异常或调用了 `die()`/`exit()` 的任务数 |
| `oxphp_async_tasks_cancelled_total` | counter | 被取消的任务数（超时或 RSHUTDOWN 清理） |
| `oxphp_async_tasks_rejected_total` | counter | 因异步队列已满而被拒绝的任务数 |

当没有异步任务被分发且没有任务被拒绝时，这些计数器将从 Prometheus 输出中省略。

## 异步工作线程环境

异步工作线程是**隔离的 PHP 引擎**。每个异步工作线程在线程启动时运行一次 `php_request_startup()`，然后循环执行闭包。与 HTTP 工作线程不同，异步工作线程**不会**执行你的应用引导代码 —— 没有 `vendor/autoload.php`，没有框架初始化，没有服务容器。

### 可用的功能

| 类别 | 示例 |
|------|------|
| PHP 内置函数 | `array_map`、`json_encode`、`preg_match`、`hash`、`mb_*`、`date` |
| 内置扩展 | `PDO`（在闭包内创建新连接）、`curl_*`、`file_get_contents` |
| 纯计算 | 数学运算、字符串处理、数组操作、数据转换 |
| 通过 `use` 传递的标量数据 | `string`、`int`、`float`、`bool`、`array`（深拷贝） |

```php
<?php
// ✅ 可用：在闭包内创建新的 PDO 连接
$dsn = 'mysql:host=127.0.0.1;dbname=app';
$user = 'root';
$pass = 'secret';

$p = oxphp_async(function() use ($dsn, $user, $pass): array {
    $pdo = new PDO($dsn, $user, $pass);
    return $pdo->query('SELECT count(*) FROM users')->fetch();
});
```

### 不可用的功能

| 类别 | 原因 |
|------|------|
| `DB::connection()`、`app('cache')`、Facade | 服务容器未初始化 — 这些依赖 Laravel 的引导过程 |
| Composer 自动加载器 | `vendor/autoload.php` 未执行 — 内置扩展之外的类均未定义 |
| Eloquent 模型、Doctrine 实体 | 需要自动加载器 + 框架引导 |
| `$_SERVER`、`$_GET`、`$_POST` | 超全局变量在异步工作线程上未填充 — 它们没有 HTTP 请求上下文 |
| 来自 HTTP 工作线程的静态状态 | `static` 类属性、全局变量 — 每个线程有自己的 ZTS 副本 |
| 通过 `use` 传递对象 | 对象无法跨线程序列化 — 在分发时抛出 `AsyncException` |

```php
<?php
// ❌ 失败：自动加载器不可用，找不到类
$p = oxphp_async(function(): void {
    $user = User::find(1);  // Fatal: Class "User" not found
});

// ❌ 失败：服务容器未初始化
$p = oxphp_async(function(): void {
    $db = app('db');  // Fatal: Function "app" not found
});

// ❌ 失败：对象无法跨线程边界传递
$pdo = new PDO($dsn, $user, $pass);
$p = oxphp_async(function() use ($pdo): array {
    // AsyncException: Cannot pass object values in use-vars
    return $pdo->query('SELECT 1')->fetch();
});
```

### 变通方法：传递连接参数而非连接对象

由于对象无法跨线程边界传递，请传递原始连接参数并在闭包内创建连接：

```php
<?php
$config = [
    'dsn'  => 'mysql:host=127.0.0.1;dbname=app',
    'user' => 'root',
    'pass' => getenv('DB_PASSWORD'),
];

$promises = [];
foreach ($chunks as $chunk) {
    $promises[] = oxphp_async(function() use ($config, $chunk): int {
        // 每个异步工作线程创建自己的连接
        $pdo = new PDO($config['dsn'], $config['user'], $config['pass']);
        $pdo->beginTransaction();
        foreach ($chunk as $row) {
            $pdo->prepare('UPDATE t SET v=? WHERE id=?')->execute([$row['v'], $row['id']]);
        }
        $pdo->commit();
        return count($chunk);
    }, $chunk);
}

$results = oxphp_async_await_all($promises);
$total = array_sum($results);
```

## 使用模式

### 并行计算

```php
<?php
// 分发多个 CPU 密集型任务
$promises = [];
foreach ($chunks as $i => $chunk) {
    $promises[$i] = oxphp_async(function() use ($chunk): array {
        return array_map('process_record', $chunk);
    });
}

// 收集所有结果
$results = oxphp_async_await_all($promises);
```

### 响应后后台工作

```php
<?php
echo json_encode(['status' => 'accepted']);
oxphp_finish_request();

// 响应已发送 — 现在并行执行后台工作
$p1 = oxphp_async(function() use ($data): void { send_email($data); });
$p2 = oxphp_async(function() use ($data): void { update_analytics($data); });
oxphp_async_await_all([$p1, $p2]);
```

### 按完成顺序处理结果

使用 `oxphp_async_await_any()` 在循环中处理 Promise —— 最快完成的先处理：

```php
<?php
$promises = [
    oxphp_async(fn() => call_api_a()),  // 500ms
    oxphp_async(fn() => call_api_b()),  // 100ms
    oxphp_async(fn() => call_api_c()),  // 300ms
];

while (!empty($promises)) {
    $winner = oxphp_async_await_any($promises);
    process_result($winner['id'], $winner['value']);

    // 移除获胜者，继续竞速其余的
    $promises = array_values(
        array_filter($promises, fn($id) => $id !== $winner['id'])
    );
}
// Processing order: api_b (100ms), api_c (300ms), api_a (500ms)
```

每次迭代从内部 Promise 映射表中取出接收器，使用 `select_all` 竞速，返回获胜者，并将剩余的接收器放回。没有 Promise 会丢失或泄漏 —— 每个 Promise 在获胜时被精确清理一次。

### 超时保护

```php
<?php
$p = oxphp_async(function(): array {
    return fetch_from_slow_api();
});

try {
    $result = oxphp_async_await($p, 2.0); // 2-second timeout
} catch (\OxPHP\AsyncTimeoutException $e) {
    $result = cached_fallback();
}
```

## 另请参阅

- [PHP 扩展函数](../php/functions.md) --- 完整函数参考，包括 `oxphp_async`、`oxphp_async_await`、`oxphp_async_await_all`、`oxphp_async_await_any`
- [工作池](../architecture/worker-pool.md) --- HTTP 工作池架构（与异步池分离）
- [指标](../operations/metrics.md#async-tasks) --- Prometheus 指标，包括异步任务计数器
- [配置](../operations/configuration.md#async-pool) --- `ASYNC_WORKERS` 和 `ASYNC_QUEUE_CAPACITY`
