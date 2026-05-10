---
title: 异步 Promise
description: 在后台线程中运行 PHP 闭包并等待结果，而不会阻塞 Worker 池。
---

# 异步 Promise

OxPHP 提供异步执行系统，在专用线程池（与 HTTP Worker 池相互独立）上运行 PHP 闭包。这可以防止耗时的后台任务阻塞请求处理。

## 工作原理

1. **分发** — 调用 `oxphp_async()`，传入一个闭包和可选参数。OxPHP 序列化闭包的 `use` 变量和参数，将其发送到异步池，并立即返回一个 Promise ID。
2. **执行** — 专用的异步 Worker 线程反序列化数据，运行闭包，并序列化结果。
3. **等待** — 使用 Promise ID 调用 `oxphp_async_await()`。在带有 fiber 的 Worker 模式下，当前 fiber 挂起，同一线程上的其他请求继续处理。在传统模式下，Worker 线程阻塞直到结果就绪。
4. **清理** — 未显式等待的 Promise 会在请求结束时自动取消并清理。

## 配置

| 变量 | 默认值 | 说明 |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0`（禁用） | 专用异步 Worker 线程数。设为 `0` 可完全禁用异步池 |
| `ASYNC_QUEUE_CAPACITY` | `0`（自动） | 最大待处理异步任务数。为 `0` 时默认为 `ASYNC_WORKERS × 64` |

> **注意：** 异步池默认禁用（`ASYNC_WORKERS=0`）。池被禁用时，四个异步函数均已注册，但调用时会抛出 `OxPHP\Async\AsyncException`。将 `ASYNC_WORKERS` 设置为大于 `0` 的值即可启用后台执行。

## 分发任务

向 `oxphp_async()` 传入一个闭包和可选参数，它会立即返回一个 Promise ID（整数）：

```php
<?php
$promise = oxphp_async(function (string $url) {
    return file_get_contents($url);
}, 'https://api.example.com/data');

// 闭包正在后台运行。
// 在此处执行其他工作...

$result = oxphp_async_await($promise);
echo $result;
```

### 向闭包传递数据

使用 `use` 变量或函数参数传递数据。仅支持标量类型和数组：

```php
<?php
$apiKey = 'sk-abc123';
$ids = [1, 2, 3];

$promise = oxphp_async(function () use ($apiKey, $ids) {
    // $apiKey 和 $ids 在此处可用
    return count($ids);
});
```

## 等待结果

### 单个 Promise

```php
<?php
$result = oxphp_async_await($promise);           // 无限等待
$result = oxphp_async_await($promise, 5.0);      // 最多等待 5 秒
```

超时值为 `0.0`（默认值）时无限等待。超时时抛出 `OxPHP\Async\TimeoutException`。

### 所有 Promise

`oxphp_async_await_all()` 等待所有 Promise，并返回以 Promise ID 为键的关联数组：

```php
<?php
$p1 = oxphp_async(fn() => file_get_contents('https://api.example.com/users'));
$p2 = oxphp_async(fn() => file_get_contents('https://api.example.com/orders'));

$results = oxphp_async_await_all([$p1, $p2], 10.0);

$users  = $results[$p1];
$orders = $results[$p2];
```

> **注意：** `oxphp_async_await_all()` 按数组顺序依次等待各 Promise。所有闭包在异步池上并发运行，但调用线程逐一收集结果。

### 最先完成的 Promise（竞速）

`oxphp_async_await_race()` 在任意一个 Promise 完成（无论成功还是失败）时立即返回：

```php
<?php
$p1 = oxphp_async(fn() => fetch_from_primary_db());
$p2 = oxphp_async(fn() => fetch_from_replica_db());

$winner = oxphp_async_await_race([$p1, $p2], 5.0);
// $winner = ['id' => int, 'value' => mixed]

echo "Promise {$winner['id']} 赢得竞速：{$winner['value']}";
```

`oxphp_async_await_race()` 返回后，未获胜的 Promise 仍可单独等待。如果获胜的 Promise 以失败告终，则抛出 `OxPHP\Async\AsyncException`，未获胜的 Promise 仍然可以等待。这是 JavaScript `Promise.race` 的对应函数。

### 最先成功完成的 Promise

`oxphp_async_await_any()` 在任意一个 Promise 成功完成时立即返回。失败会被累积，只有当所有 Promise 都失败时才会暴露出来。这是 JavaScript `Promise.any` 的对应函数——适用于回退 / 冗余场景（"任何能响应的镜像"）。

```php
<?php
$mirror_a = oxphp_async(fn() => fetch('https://mirror-a.example.com/data'));
$mirror_b = oxphp_async(fn() => fetch('https://mirror-b.example.com/data'));
$mirror_c = oxphp_async(fn() => fetch('https://mirror-c.example.com/data'));

try {
    $winner = oxphp_async_await_any([$mirror_a, $mirror_b, $mirror_c], 5.0);
    // ['id' => 输入数组中某个 Promise 的 id, 'value' => 它的结果]
} catch (\OxPHP\Async\AggregateAsyncException $e) {
    foreach ($e->getErrors() as $i => $err) {
        // $err 按输入位置 0..N-1 索引
    }
    foreach ($e->getErrorMap() as $promise_id => $err) {
        // $err 按 Promise ID 索引
    }
} catch (\OxPHP\Async\TimeoutException $e) {
    foreach ($e->getPartialErrors() as $promise_id => $err) {
        // 在截止时间之前已经失败的 Promise
    }
    $cancelled = $e->getCancelledPromiseIds();
    // 截止时间到达时仍未完成的 Promise ID。每个 Promise 的取消标志已被
    // 设置，其 receivers 已被丢弃——之后将这些 ID 中的任何一个传给
    // oxphp_async_await*() 都会抛出 "unknown or already-awaited promise id"。
    // 该列表用于审计记录，而非可恢复的工作队列。
}
```

获胜时仍处于挂起状态的非获胜 Promise 仍可单独等待。在获胜者出现之前已经失败的 Promise 不再可等待——它们的结果在被累积为候选错误时已被消费。

### 异常类型

| 类 | 抛出于 | 说明 |
|----|--------|------|
| `OxPHP\Async\AsyncException`           | `oxphp_async_await()`、`oxphp_async_await_all()`、`oxphp_async_await_race()` | 单个错误，包含消息和可选的原始异常详情。 |
| `OxPHP\Async\TimeoutException`         | 全部四个 `await_*` 函数在到达截止时间时 | 继承自 `AsyncException`。对于 `oxphp_async_await_any()` 的超时，`getPartialErrors()` 和 `getCancelledPromiseIds()` 已填充；其他调用站点二者均返回 `[]`。 |
| `OxPHP\Async\AggregateAsyncException`  | `oxphp_async_await_any()` 在所有 Promise 都失败时 | 继承自 `AsyncException`。提供 `getErrors()`（按位置，键为 0..N-1）、`getErrorMap()`（按 ID 索引）、`getPromiseIds()`。 |

## 错误处理

异步闭包内抛出的异常会被捕获，并在等待时重新以 `OxPHP\Async\AsyncException` 抛出：

```php
<?php
$promise = oxphp_async(function () {
    throw new \RuntimeException('Something failed');
});

try {
    $result = oxphp_async_await($promise);
} catch (\OxPHP\Async\AsyncException $e) {
    // "Async task failed: [RuntimeException] Something failed"
    echo $e->getMessage();
}
```

异步闭包内的 `exit()` 和 `die()` 也会被捕获并转换为 `OxPHP\Async\AsyncException`。异步 Worker 可以继续存活并处理新任务。

### 异常层级

```text
\Exception
  └── OxPHP\Async\AsyncException              # 所有异步错误
        └── OxPHP\Async\TimeoutException  # 超时专用
```

## Fiber 集成

在 Worker 模式下，`oxphp_async_await()` 与 OxPHP 的 fiber 调度器协作。等待结果时，当前 fiber 挂起而非阻塞 Worker 线程，调度器在结果就绪时恢复它，允许同一线程上处理其他请求。

在传统模式（无 Worker 文件）下，`oxphp_async_await()` 同步阻塞 Worker 线程，这意味着 Worker 在等待期间无法处理其他请求。

为获得最佳性能，请将异步 Promise 与 Worker 模式结合使用：

```php
<?php
// worker.php
require __DIR__ . '/../vendor/autoload.php';

oxphp_worker(function () {
    // 这两个 API 调用在异步池上并发运行
    // 同时 fiber 挂起——Worker 线程可以处理其他请求
    $p1 = oxphp_async(fn() => file_get_contents('https://api.example.com/users'));
    $p2 = oxphp_async(fn() => file_get_contents('https://api.example.com/orders'));

    $results = oxphp_async_await_all([$p1, $p2]);
    echo json_encode($results);
});
```

## 限制

异步闭包在独立线程上运行，这对可以跨线程传递的数据施加了限制：

| 允许 | 不允许 |
|---------|-------------|
| `null`、`bool`、`int`、`float`、`string` | 对象（任何类实例） |
| 标量类型数组 | 资源（文件句柄、数据库连接、流） |
| 嵌套标量数组 | 在 `use` 中引用对象的闭包 |

其他限制：

- **不支持嵌套异步** — 在异步闭包内调用 `oxphp_async()` 会抛出 `OxPHP\Async\AsyncException`
- **仅限用户定义函数** — 闭包必须是用户定义的，不能是对内置函数的包装
- **序列化开销** — 参数和返回值在线程边界间序列化。大数组或字符串会增加延迟
- **无共享状态** — 每个异步 Worker 有独立的 PHP 环境，分发线程和异步线程之间没有共享变量

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
      - ASYNC_WORKERS=4
      - ASYNC_QUEUE_CAPACITY=256
```

## 故障排除

### "Async pool is disabled. Set ASYNC_WORKERS > 0 to enable."

异步池未配置。当 `ASYNC_WORKERS=0`（默认值）时，异步函数已注册，但每次调用都会抛出 `OxPHP\Async\AsyncException`。

**修复：** 将 `ASYNC_WORKERS` 设置为正整数：

```bash
ASYNC_WORKERS=4
```

### "Failed to dispatch async task (pool full)"

异步池正在运行，但所有队列槽位均已占满。

**检查：** 验证池是否正在接受任务：

```bash
curl -s http://localhost:9090/config | jq '.async_workers'
```

**修复：** 增大 `ASYNC_WORKERS` 或 `ASYNC_QUEUE_CAPACITY`：

### "Cannot pass object values in use-vars to async closure"

对象无法跨线程边界序列化。

**修复：** 在分发之前提取所需的标量数据：

```php
<?php
// 错误：传递对象
$promise = oxphp_async(function () use ($user) { ... });

// 正确：传递从对象中提取的标量数据
$userId = $user->getId();
$userName = $user->getName();
$promise = oxphp_async(function () use ($userId, $userName) { ... });
```

### 传统模式下等待挂起

在传统模式下，`oxphp_async_await()` 会阻塞 Worker 线程。如果所有 PHP Worker 都在等待异步结果，服务器将停止处理请求。

**修复：** 使用 Worker 模式（`WORKER_MODE_ENABLED=true`），使 `oxphp_async_await()` 挂起 fiber 而不是阻塞线程。

### 异步超时不会终止正在运行的任务

`OxPHP\Async\TimeoutException` 在等待方抛出——闭包继续在异步池上运行，直到完成或请求结束。任务会在请求结束清理期间被取消。

## 最佳实践

- **始终为生产环境的 `oxphp_async_await()` 调用设置超时**，以防止无限等待
- **使用 Worker 模式** 以获得基于 fiber 的非阻塞等待，而非阻塞 Worker 线程
- **保持闭包小巧** — 分发专注的工作单元，而非整个请求处理器
- **分发前提取标量** — 在传递给闭包之前，从对象中提取 ID、字符串和配置值
- **监控异步池** — 在 Prometheus 指标中检查 `oxphp_async_tasks_rejected_total`。如果拒绝数持续增加，请增大 `ASYNC_WORKERS` 或 `ASYNC_QUEUE_CAPACITY`

## 参见

- [Worker 模式](worker-mode.md) -- 基于 fiber 并发的持久化 PHP 进程
- [PHP 函数](../php/functions.md) -- `oxphp_async()`、`oxphp_async_await()` 及相关函数参考
- [指标](../operations/metrics.md) -- 异步池 Prometheus 指标
- [配置参考](../operations/configuration.md) -- `ASYNC_WORKERS` 和 `ASYNC_QUEUE_CAPACITY`
