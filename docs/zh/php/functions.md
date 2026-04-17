---
title: PHP 函数
description: OxPHP 提供的所有 oxphp_* PHP 函数完整参考，包括异步、流式传输、Worker 模式和装饰器 API。
---

# PHP 函数

OxPHP 通过 `oxphp_sapi` 扩展注册其函数，该扩展在服务器执行每个 PHP 脚本时自动加载。无需配置 `extension=` 指令，也无需手动加载——以下所有函数从 PHP 代码的第一行起即可使用。

## 目录

- [oxphp_http_request()](#oxphp_http_request)
- [oxphp_superglobals_enabled()](#oxphp_superglobals_enabled)
- [oxphp_request_id()](#oxphp_request_id)
- [oxphp_worker_id()](#oxphp_worker_id)
- [oxphp_server_info()](#oxphp_server_info)
- [oxphp_request_heartbeat()](#oxphp_request_heartbeat)
- [oxphp_finish_request()](#oxphp_finish_request)
- [oxphp_is_worker()](#oxphp_is_worker)
- [oxphp_worker()](#oxphp_worker)
- [oxphp_is_streaming()](#oxphp_is_streaming)
- [oxphp_stream_flush()](#oxphp_stream_flush)
- [oxphp_sleep()](#oxphp_sleep)
- [oxphp_usleep()](#oxphp_usleep)
- [oxphp_async()](#oxphp_async)
- [oxphp_async_await()](#oxphp_async_await)
- [oxphp_async_await_all()](#oxphp_async_await_all)
- [oxphp_async_await_any()](#oxphp_async_await_any)
- [oxphp_register_decorator()](#oxphp_register_decorator)
- [oxphp_apm_trace()](#oxphp_apm_trace)
- [oxphp_apm_start()](#oxphp_apm_start)
- [oxphp_apm_end()](#oxphp_apm_end)
- [oxphp_apm_attribute()](#oxphp_apm_attribute)
- [oxphp_apm_event()](#oxphp_apm_event)
- [oxphp_apm_error()](#oxphp_apm_error)
- [oxphp_apm_status()](#oxphp_apm_status)
- [oxphp_apm_trace_id()](#oxphp_apm_trace_id)
- [oxphp_apm_span_id()](#oxphp_apm_span_id)
- [oxphp_apm_header()](#oxphp_apm_header)
- [类与接口](#类与接口)
- [异常](#异常)

---

## oxphp_http_request()

```php
oxphp_http_request(): \OxPHP\Http\Request
```

返回当前 HTTP 请求的请求对象。该对象提供对 HTTP 方法、URI、查询参数、解析后的请求体、请求头、Cookie、上传文件、客户端 IP 和请求时间的类型化访问。

**返回值：** 一个 `\OxPHP\Http\Request` 实例，由当前 PHP Worker 线程中的请求数据支撑。

**抛出异常：** 在没有活跃请求的上下文中调用时，抛出 `OxPHP\Http\Exception` 命名空间下的异常：

| 异常 | 触发情形 |
|------|----------|
| `\OxPHP\Http\Exception\WorkerIdleException` | Worker 模式下，两次请求之间 |
| `\OxPHP\Http\Exception\AsyncContextException` | 在 `oxphp_async()` 回调内部 |
| `\OxPHP\Http\Exception\NoActiveRequestException` | 其他无活跃请求的上下文 |

在普通的请求处理代码中，不需要异常处理。

**示例：**

```php
<?php
$request = oxphp_http_request();

$method  = $request->method();             // "POST"
$path    = $request->path();               // "/api/users"
$email   = $request->payload('email');     // 来自 JSON 或表单请求体
$token   = $request->header('Authorization');
$theme   = $request->cookie('theme', 'light');
```

完整的接口参考请参阅 [HTTP 请求对象 API](request-api.md) 文档。

---

## oxphp_superglobals_enabled()

```php
oxphp_superglobals_enabled(): bool
```

返回当前服务器实例是否启用了超全局变量填充。该值反映 `SUPERGLOBALS_ENABLED` 环境变量的设置，在服务器运行期间不会改变。

当返回 `false` 时，`$_GET`、`$_POST`、`$_COOKIE`、`$_FILES` 和 `$_SERVER` 均为空数组。HTTP 对象 API（`oxphp_http_request()`）、`php://input` 和 PHP session 函数不受影响。

**返回值：** `SUPERGLOBALS_ENABLED` 为 `true`（默认值）时返回 `true`，否则返回 `false`。

**示例：**

```php
<?php
if (oxphp_superglobals_enabled()) {
    $query = $_GET['page'] ?? 1;
} else {
    $query = oxphp_http_request()->query('page', 1);
}
```

---

## oxphp_request_id()

```php
oxphp_request_id(): string
```

返回当前请求的唯一请求标识符。该值与响应头 `X-Request-ID` 中发送的值相同。如果客户端发送了 `X-Request-ID` 请求头，OxPHP 将原样转发，而不生成新的标识符。

**返回值：** 当 OxPHP 生成 ID 时，返回 20 位十六进制字符串（例如 `"67890abc12341a2b0042"`）。当客户端发送 `X-Request-ID` 请求头时，直接返回该值（1–64 个字符，由字母数字及 `-`、`_`、`.` 组成）。

**示例：**

```php
<?php
$id = oxphp_request_id();
error_log("[$id] Processing order #1234");

// 将 ID 传播到下游服务
header("X-Correlation-ID: $id");
```

---

## oxphp_worker_id()

```php
oxphp_worker_id(): int
```

返回处理当前请求的 PHP Worker 线程的从零开始的索引。Worker 索引范围为 `0` 到 `PHP_WORKERS - 1`。

**返回值：** 标识当前 Worker 线程的整数。

**示例：**

```php
<?php
$workerId = oxphp_worker_id();

// 使用每个 Worker 专属的临时文件以避免冲突
$tmp = "/tmp/worker_{$workerId}_buffer.dat";

error_log("Worker $workerId handling request");
```

---

## oxphp_server_info()

```php
oxphp_server_info(): array
```

返回包含服务器和请求元数据的关联数组。

**返回值：** 包含以下键的数组：

| 键 | 类型 | 描述 |
|----|------|------|
| `sapi` | `string` | 始终为 `"oxphp"` |
| `version` | `string` | 服务器版本（例如 `"0.1.0"`） |
| `worker_id` | `int` | 与 `oxphp_worker_id()` 返回值相同 |
| `request_time` | `float` | 请求开始时的 Unix 时间戳，精确到微秒 |
| `worker_mode` | `bool` | 当前进程是否以 Worker 模式运行 |

**示例：**

```php
<?php
$info = oxphp_server_info();
// [
//     "sapi"         => "oxphp",
//     "version"      => "0.1.0",
//     "worker_id"    => 3,
//     "request_time" => 1738800000.123456,
//     "worker_mode"  => true,
// ]

$elapsed = microtime(true) - $info['request_time'];
echo "Processing took {$elapsed}s so far";
```

---

## oxphp_finish_request()

```php
oxphp_finish_request(): bool
```

将响应刷新发送给客户端，并在后台继续执行 PHP 脚本。客户端立即收到完整的 HTTP 响应；脚本继续运行直至自然退出。这是 OxPHP 中等价于 PHP-FPM 的 `fastcgi_finish_request()` 的函数。

**返回值：** 成功时返回 `true`，若本次请求已调用过则返回 `false`。

> **注意：** PHP Worker 线程在脚本结束前保持占用状态。请保持后台工作简短，或将繁重的处理卸载到队列中。

**示例：**

```php
<?php
http_response_code(202);
echo json_encode(['status' => 'accepted']);
oxphp_finish_request();

// 客户端已收到 202 响应；继续后台工作
send_notification_email($user);
update_analytics($event);
```

---

## oxphp_request_heartbeat()

```php
oxphp_request_heartbeat(int $time = 10): bool
```

从调用时刻起将 `REQUEST_TIMEOUT_SECONDS` 截止时间延长 `$time` 秒。在长时间运行的循环中定期调用此函数，以防止 OxPHP 在处理过程中中止请求。

**参数：**
- `$time` — 延长超时截止时间的秒数。默认值：`10`

**返回值：** 成功时返回 `true`，若 `$time` 为零或负数则返回 `false`。

> **注意：** 每次调用都会相对于当前时间设置新的截止时间，而非相对于原始请求开始时间。在请求的第 100 秒时调用 `oxphp_request_heartbeat(30)`，截止时间将设置为从当前时刻起 30 秒后（即从请求开始起第 130 秒）。

**示例：**

```php
<?php
foreach ($large_dataset as $row) {
    oxphp_request_heartbeat(30); // 从现在起延长 30 秒
    process($row);
}
```

---

## oxphp_is_worker()

```php
oxphp_is_worker(): bool
```

返回服务器是否以 Worker 模式运行。当设置了 `WORKER_FILE` 时，Worker 模式将被激活。

**返回值：** Worker 模式下返回 `true`，传统模式下返回 `false`。

**示例：**

```php
<?php
if (oxphp_is_worker()) {
    // 跨请求复用持久连接
    $db = $GLOBALS['db'] ??= new PDO($dsn);
} else {
    // 传统模式：每次请求创建新连接
    $db = new PDO($dsn);
}
```

---

## oxphp_worker()

```php
oxphp_worker(callable $handler): bool
```

进入持久 Worker 模式循环。OxPHP 对每个传入的 HTTP 请求调用一次 `$handler`。请求之间会进行软重置，清除每次请求的状态——输出缓冲区、响应头和超全局变量——但不销毁 PHP 堆，因此在处理程序外部声明的变量会跨请求持久存在。

**参数：**
- `$handler` — 每次请求调用一次。处理程序不接收参数。在处理程序内部使用超全局变量（`$_SERVER`、`$_GET`、`$_POST` 等）或 `oxphp_http_request()` 访问请求数据。

**返回值：** 优雅关闭时返回 `true`，非 Worker 模式时返回 `false`。

以下任一条件满足时，Worker 循环退出：
- 服务器优雅关闭
- 处理程序连续抛出 3 个未捕获的异常或致命错误
- Worker 达到 `WORKER_MAX_REQUESTS` 限制
- Worker 超出 `WORKER_MAX_MEMORY_MIB` 限制

> **注意：** `oxphp_worker()` 仅在配置了 `WORKER_FILE` 时有效。在传统模式下，它会记录警告并返回 `false`。

**示例：**

```php
<?php
// worker.php — 在每个 Worker 进程生命周期内运行一次

// 引导阶段：仅在启动时执行一次
require __DIR__ . '/vendor/autoload.php';
$app = new App();

// 在循环中处理请求
oxphp_worker(function () use ($app) {
    $app->handle();
});

// oxphp_worker() 之后的代码在关闭期间运行
$app->terminate();
```

---

## oxphp_is_streaming()

```php
oxphp_is_streaming(): bool
```

返回当前请求是否处于流式传输模式。首次调用 `oxphp_stream_flush()` 时，或 PHP 设置 `Content-Type: text/event-stream` 时，流式传输模式会自动激活。

**返回值：** 流式传输模式激活时返回 `true`，否则返回 `false`。

**示例：**

```php
<?php
if (oxphp_is_streaming()) {
    echo "data: " . json_encode($event) . "\n\n";
    oxphp_stream_flush();
} else {
    echo json_encode($allData);
}
```

---

## oxphp_stream_flush()

```php
oxphp_stream_flush(): bool
```

激活流式传输模式，并将缓冲的输出以 HTTP 分块形式刷新发送给客户端。首次调用时，HTTP 响应头立即发送并开始流式传输。后续每次调用都会刷新自上次刷新以来写入的输出。

**返回值：** 成功时返回 `true`，若已调用过 `oxphp_finish_request()` 则返回 `false`。

> **注意：** 当 PHP 设置 `Content-Type: text/event-stream` 时，流式传输模式也会自动激活。在这种情况下可以使用 PHP 内置的 `flush()`，但需要先调用 `ob_end_flush()` 以绕过 PHP 的输出缓冲层。

**示例：**

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

for ($i = 0; $i < 10; $i++) {
    echo "id: $i\n";
    echo "data: " . json_encode(['counter' => $i]) . "\n\n";
    oxphp_stream_flush();
    oxphp_sleep(1.0); // 使用 oxphp_sleep 代替 sleep——在 fiber 模式下不会阻塞 Worker
}
```

---

## oxphp_sleep()

```php
oxphp_sleep(float $seconds): void
```

休眠指定的时长。在以 fiber 方式运行的 Worker 模式处理程序内，此调用是协作式的——它挂起当前 fiber，使其他请求在等待期间得以处理。在 fiber 外部，则退化为标准的阻塞式 `usleep()`。

**参数：**
- `$seconds` — 休眠时长（秒）。接受小数值（例如 `0.5` 表示 500 毫秒）。值为 `0` 或负数时立即返回。

**返回值：** `void`

**示例：**

```php
<?php
oxphp_worker(function () {
    // 在启用了 fiber 多路复用的 Worker 模式下：
    // 这会挂起 fiber 而非阻塞线程
    oxphp_sleep(1.0);
    echo json_encode(['done' => true]);
});
```

---

## oxphp_usleep()

```php
oxphp_usleep(int $microseconds): void
```

休眠指定的微秒数。与 `oxphp_sleep()` 类似，在 fiber 内部是协作式的，否则退化为阻塞式 `usleep()`。

**参数：**
- `$microseconds` — 休眠时长（微秒）。值为 `0` 或负数时立即返回。

**返回值：** `void`

**示例：**

```php
<?php
oxphp_worker(function () {
    // 每 100ms 轮询一次条件，不阻塞其他请求
    while (!$condition_met()) {
        oxphp_usleep(100_000);
    }
    echo "ready";
});
```

---

## oxphp_async()

```php
oxphp_async(Closure $closure, mixed ...$args): int
```

将闭包分发到专用异步 Worker 线程上执行，并立即返回一个 Promise ID。调用方无需等待闭包完成即可继续执行。使用 `oxphp_async_await()` 获取结果。

**参数：**
- `$closure` — 要在异步 Worker 线程上运行的用户定义 `Closure`
- `...$args` — 传递给闭包的参数。仅接受标量值（`null`、`bool`、`int`、`float`、`string`）及标量数组。对象和资源不能跨线程传递。

**返回值：** 整数类型的 Promise ID。将其传递给 `oxphp_async_await()`、`oxphp_async_await_all()` 或 `oxphp_async_await_any()`。

**抛出异常：** 在以下情况下抛出 `OxPHP\Async\Exception`：
- 异步池已禁用（`ASYNC_WORKERS=0`）
- 闭包不是用户定义的
- 异步池已满（所有队列槽位被占用）
- 参数或 `use` 变量包含对象或资源

> **注意：** 通过 `use` 捕获的变量遵循相同限制——对象和资源会被拒绝。

**示例：**

```php
<?php
// 并发分发两个独立任务
$p1 = oxphp_async(function () {
    return fetch_from_api('/users');
});

$p2 = oxphp_async(function () {
    return fetch_from_api('/posts');
});

// 获取两个结果
$users = oxphp_async_await($p1);
$posts = oxphp_async_await($p2);
```

---

## oxphp_async_await()

```php
oxphp_async_await(int $promise_id, float $timeout = 0.0): mixed
```

阻塞直至指定的异步 Promise 完成并返回其结果。在 Worker 模式的 fiber 内部，这会以协作方式挂起当前 fiber，而非阻塞线程。

**参数：**
- `$promise_id` — `oxphp_async()` 返回的 Promise ID
- `$timeout` — 最长等待秒数。`0.0` 表示无限等待。默认值：`0.0`

**返回值：** 异步闭包的返回值。

**抛出异常：**
- 若异步池已禁用（`ASYNC_WORKERS=0`）或异步任务抛出异常，则抛出 `OxPHP\Async\Exception`
- 若超过 `$timeout`，则抛出 `OxPHP\Async\TimeoutException`

**示例：**

```php
<?php
$promise = oxphp_async(function (int $n) {
    return array_sum(range(1, $n));
}, 1_000_000);

$result = oxphp_async_await($promise);
echo $result; // 500000500000

// 带超时
try {
    $result = oxphp_async_await($promise, 5.0);
} catch (\OxPHP\Async\TimeoutException $e) {
    echo "Task took too long";
}
```

---

## oxphp_async_await_all()

```php
oxphp_async_await_all(array $promise_ids, float $timeout = 0.0): array
```

等待数组中的所有 Promise 完成，并返回一个关联数组，将每个 Promise ID 映射到其结果。Promise 按数组顺序依次等待。

**参数：**
- `$promise_ids` — `oxphp_async()` 返回的整数 Promise ID 数组
- `$timeout` — 每个 Promise 的最长等待秒数。`0.0` 表示无限等待。默认值：`0.0`

**返回值：** 关联数组，每个键为 Promise ID（整数），每个值为该 Promise 的结果。

**抛出异常：**
- 若异步池已禁用（`ASYNC_WORKERS=0`）或任意 Promise 失败，则抛出 `OxPHP\Async\Exception`
- 若任意 Promise 超过 `$timeout`，则抛出 `OxPHP\Async\TimeoutException`

**示例：**

```php
<?php
$promises = [
    oxphp_async(fn() => slow_query('users')),
    oxphp_async(fn() => slow_query('orders')),
    oxphp_async(fn() => slow_query('products')),
];

$results = oxphp_async_await_all($promises);

foreach ($results as $promiseId => $result) {
    // 处理 $result
}
```

---

## oxphp_async_await_any()

```php
oxphp_async_await_any(array $promise_ids, float $timeout = 0.0): array
```

竞争多个 Promise，返回最先完成的那个。其他 Promise 不会被取消——它们继续运行，仍可通过 `oxphp_async_await()` 等待。

**参数：**
- `$promise_ids` — 至少包含一个 `oxphp_async()` 返回的整数 Promise ID 的数组。不得为空。
- `$timeout` — 等待任意 Promise 完成的最长秒数。`0.0` 表示无限等待。默认值：`0.0`

**返回值：** 包含两个键的关联数组：
- `id`（`int`）— 获胜 Promise 的 ID
- `value`（`mixed`）— 获胜 Promise 的返回值

**抛出异常：**
- 若异步池已禁用（`ASYNC_WORKERS=0`）或获胜 Promise 失败，则抛出 `OxPHP\Async\Exception`
- 若在 `$timeout` 内没有 Promise 完成，则抛出 `OxPHP\Async\TimeoutException`

**示例：**

```php
<?php
// 尝试两个镜像节点；使用最先响应的那个
$p1 = oxphp_async(fn() => fetch('https://mirror-1.example.com/data'));
$p2 = oxphp_async(fn() => fetch('https://mirror-2.example.com/data'));

$winner = oxphp_async_await_any([$p1, $p2], timeout: 10.0);
echo "Mirror {$winner['id']} won: " . json_encode($winner['value']);
```

---

## oxphp_register_decorator()

```php
oxphp_register_decorator(string $class): bool
```

将一个 PHP 类注册为装饰器，用于包装函数和方法调用。该类必须实现 `OxPHP\Decorator\AttributeInterface`。注册后，OxPHP 会在每次与装饰器 `#[Attribute]` 目标匹配的函数或方法调用前后，分别触发装饰器的 `before()` 和 `after()` 钩子。

**参数：**
- `$class` — 要注册的装饰器的完全限定类名

**返回值：** 成功时返回 `true`，若类不存在或未实现 `OxPHP\Decorator\AttributeInterface` 则返回 `false`。

**示例：**

```php
<?php
use OxPHP\Decorator\AttributeInterface;
use OxPHP\Decorator\Context;

#[\Attribute(\Attribute::TARGET_METHOD)]
class LogDecorator implements AttributeInterface
{
    public function before(Context $ctx): void
    {
        error_log("Calling {$ctx->target} (request {$ctx->requestId})");
    }

    public function after(Context $ctx): void
    {
        error_log("Finished {$ctx->target}");
    }
}

// 在引导阶段（或 Worker 启动时）注册一次
oxphp_register_decorator(LogDecorator::class);
```

---

## oxphp_apm_trace()

```php
oxphp_apm_trace(string $name, callable $callback, ?array $attributes = null): void
```

在一个命名 Span 内执行回调。Span 在回调运行前打开，在回调返回后关闭。为未来增强的回调集成而预留。

**参数：**
- `$name` — Span 名称
- `$callback` — 在 Span 内执行的可调用对象
- `$attributes` — 可选的字符串键值属性关联数组

**返回值：** `void`

---

## oxphp_apm_start()

```php
oxphp_apm_start(string $name, ?array $attributes = null): int
```

打开一个新的 Span 并返回一个本地 ID 以供后续引用。该 Span 将成为当前活跃 Span 的子 Span（如果没有活跃 Span，则成为请求根 Span 的子 Span）。使用 `oxphp_apm_end()` 关闭它。

**参数：**
- `$name` — Span 名称（例如 `"cache.warm"`、`"payment.process"`）
- `$attributes` — 可选的字符串键值属性关联数组，在创建时设置到 Span 上

**返回值：** 整数类型的本地 Span ID。将其传递给 `oxphp_apm_end()`、`oxphp_apm_attribute()` 或其他接受 `$span_id` 的函数。APM 禁用时返回 `0`。

**示例：**

```php
<?php
$spanId = oxphp_apm_start('order.validate', [
    'order.type' => 'subscription',
]);

validateOrder($order);

oxphp_apm_end($spanId);
```

---

## oxphp_apm_end()

```php
oxphp_apm_end(int $span_id): void
```

关闭由 `oxphp_apm_start()` 打开的 Span。记录 Span 的结束时间，并将其从活跃栈移至已完成列表，准备导出。

**参数：**
- `$span_id` — `oxphp_apm_start()` 返回的本地 Span ID

**返回值：** `void`

> **注意：** 始终按反向顺序关闭 Span。如果你先打开 Span A 再打开 Span B，则应先关闭 B 再关闭 A。未关闭的 Span 会在请求结束时自动关闭，并标记 `oxphp.span.leaked=true`。

---

## oxphp_apm_attribute()

```php
oxphp_apm_attribute(string $key, mixed $value, ?int $span_id = null): void
```

在 Span 上设置键值属性。值会被转换为字符串。如果未提供 `$span_id`，属性将添加到当前活跃的 Span 上。

**参数：**
- `$key` — 属性键（例如 `"user.id"`、`"cache.hit"`）
- `$value` — 属性值（string、int、float、bool 或 null——转换为字符串）
- `$span_id` — 可选的本地 Span ID。省略时指向当前 Span

**返回值：** `void`

**示例：**

```php
<?php
$spanId = oxphp_apm_start('db.query');

oxphp_apm_attribute('db.system', 'mysql');
oxphp_apm_attribute('db.statement', 'SELECT * FROM users WHERE id = ?');
oxphp_apm_attribute('db.row_count', $rowCount, $spanId);

oxphp_apm_end($spanId);
```

---

## oxphp_apm_event()

```php
oxphp_apm_event(string $name, ?array $attributes = null, ?int $span_id = null): void
```

在 Span 上记录一个带时间戳的事件。事件用于记录 Span 生命周期内的离散事件（例如缓存未命中、重试尝试、鉴权检查）。

**参数：**
- `$name` — 事件名称（例如 `"cache.miss"`、`"retry"`）
- `$attributes` — 可选的字符串键值事件属性关联数组
- `$span_id` — 可选的本地 Span ID。省略时指向当前 Span

**返回值：** `void`

**示例：**

```php
<?php
$spanId = oxphp_apm_start('payment.process');

oxphp_apm_event('payment.authorized', [
    'provider' => 'stripe',
    'amount' => '49.99',
]);

oxphp_apm_end($spanId);
```

---

## oxphp_apm_error()

```php
oxphp_apm_error(mixed $exception, ?int $span_id = null): void
```

将 Span 的状态标记为错误（状态码 2）。用于标记发生异常或故障的 Span。

**参数：**
- `$exception` — 异常或错误（用于上下文信息；状态设置与类型无关）
- `$span_id` — 可选的本地 Span ID。省略时指向当前 Span

**返回值：** `void`

**示例：**

```php
<?php
$spanId = oxphp_apm_start('external.api');

try {
    $result = callExternalApi();
} catch (\Throwable $e) {
    oxphp_apm_error($e, $spanId);
    throw $e;
} finally {
    oxphp_apm_end($spanId);
}
```

---

## oxphp_apm_status()

```php
oxphp_apm_status(int $code, ?string $description = null, ?int $span_id = null): void
```

设置 Span 的状态码和可选描述。

**参数：**
- `$code` — 状态码：`0` = 未设置，`1` = 正常，`2` = 错误
- `$description` — 可选的可读状态描述
- `$span_id` — 可选的本地 Span ID。省略时指向当前 Span

**返回值：** `void`

**示例：**

```php
<?php
$spanId = oxphp_apm_start('validation');

if ($valid) {
    oxphp_apm_status(1, 'Validation passed', $spanId);
} else {
    oxphp_apm_status(2, 'Invalid input: missing email', $spanId);
}

oxphp_apm_end($spanId);
```

---

## oxphp_apm_trace_id()

```php
oxphp_apm_trace_id(): string
```

返回当前请求追踪上下文的 W3C trace ID（32 位十六进制字符）。该值与 `$_SERVER['OXPHP_TRACE_ID']` 相同，无需超全局变量即可获取。

**返回值：** 32 位十六进制字符的 trace ID 字符串。APM 禁用或没有活跃追踪上下文时返回空字符串。

**示例：**

```php
<?php
$traceId = oxphp_apm_trace_id();
error_log("Processing request in trace {$traceId}");
```

---

## oxphp_apm_span_id()

```php
oxphp_apm_span_id(): string
```

返回当前活跃 Span 的 Span ID（16 位十六进制字符）。如果存在嵌套 Span，返回最内层打开的 Span 的 ID。

**返回值：** 16 位十六进制字符的 Span ID 字符串。没有活跃 Span 时返回空字符串。

---

## oxphp_apm_header()

```php
oxphp_apm_header(): string
```

返回当前 Span 上下文的 W3C `traceparent` 请求头值。用于将追踪上下文传播到下游 HTTP 调用。

**返回值：** 格式为 `00-{trace_id}-{span_id}-01` 的字符串。没有活跃追踪上下文时返回空字符串。

**示例：**

```php
<?php
$spanId = oxphp_apm_start('http.call');

$traceparent = oxphp_apm_header();

$response = file_get_contents('https://api.example.com/data', false,
    stream_context_create([
        'http' => [
            'header' => "traceparent: {$traceparent}\r\n",
        ],
    ])
);

oxphp_apm_end($spanId);
```

---

## 类与接口

`oxphp_sapi` 扩展注册了以下类：

### HTTP

| 类 | 描述 |
|----|------|
| `OxPHP\Http\Request` | `oxphp_http_request()` 返回的请求对象。`final`——不可继承。 |
| `OxPHP\Http\Attributes` | 可变的请求属性容器（用于中间件）。`final`。 |
| `OxPHP\Http\Session` | 通过 `$request->session()` 访问的会话对象。`final`。 |
| `OxPHP\Http\UploadedFile` | 来自 `$request->files()` 的上传文件对象。`final`。 |

### 装饰器

| 类 / 接口 | 描述 |
|-----------|------|
| `OxPHP\Decorator\AttributeInterface` | 装饰器接口。要求实现 `before(Context $ctx)` 和 `after(Context $ctx)` 方法。 |
| `OxPHP\Decorator\Context` | 传递给装饰器钩子的上下文对象。`final`。包含 `target`、`requestId`、参数和返回值。 |

### 追踪

| 类 | 描述 |
|----|------|
| `OxPHP\Apm\Trace` | 用于自动创建 Span 的内置属性。应用于函数或方法。 |

### Async

| 类 | 描述 |
|----|------|
| `OxPHP\Async\BorrowedProxy` | 用于线程间借用值的代理对象。 |

---

## 异常

扩展注册的所有异常：

| 异常 | 继承自 | 触发时机 |
|------|--------|----------|
| `OxPHP\Async\Exception` | `\Exception` | 异步任务中的错误（`oxphp_async_await()`）或 `oxphp_async()` 中的无效参数 |
| `OxPHP\Async\TimeoutException` | `OxPHP\Async\Exception` | `oxphp_async_await()`、`oxphp_async_await_all()` 或 `oxphp_async_await_any()` 中超时 |
| `OxPHP\Async\BorrowException` | `\Exception` | 线程间借用值时出错 |
| `OxPHP\Http\Exception\NoActiveRequestException` | `\RuntimeException` | 在没有活跃请求时调用 `oxphp_http_request()` |
| `OxPHP\Http\Exception\AsyncContextException` | `NoActiveRequestException` | 在 `oxphp_async()` 回调内部调用 `oxphp_http_request()` |
| `OxPHP\Http\Exception\WorkerIdleException` | `NoActiveRequestException` | 在 Worker 模式下两次请求之间调用 `oxphp_http_request()` |
| `OxPHP\Decorator\RejectedException` | `\Exception` | 装饰器拒绝了函数/方法调用 |

---

## 扩展验证

你可以验证 OxPHP 扩展是否已加载并查看所有已注册的函数：

```php
<?php
if (extension_loaded('oxphp_sapi')) {
    echo "OxPHP extension is loaded\n";
}

$functions = get_extension_funcs('oxphp_sapi');
print_r($functions);
// Array
// (
//     [0]  => oxphp_http_request
//     [1]  => oxphp_superglobals_enabled
//     [2]  => oxphp_request_id
//     [3]  => oxphp_worker_id
//     [4]  => oxphp_server_info
//     [5]  => oxphp_request_heartbeat
//     [6]  => oxphp_finish_request
//     [7]  => oxphp_is_worker
//     [8]  => oxphp_is_streaming
//     [9]  => oxphp_stream_flush
//     [10] => oxphp_sleep
//     [11] => oxphp_usleep
//     [12] => oxphp_worker
//     [13] => oxphp_async
//     [14] => oxphp_async_await
//     [15] => oxphp_async_await_all
//     [16] => oxphp_async_await_any
//     [17] => oxphp_register_decorator
//     [18] => oxphp_apm_trace
//     [19] => oxphp_apm_start
//     [20] => oxphp_apm_end
//     [21] => oxphp_apm_attribute
//     [22] => oxphp_apm_event
//     [23] => oxphp_apm_error
//     [24] => oxphp_apm_status
//     [25] => oxphp_apm_trace_id
//     [26] => oxphp_apm_span_id
//     [27] => oxphp_apm_header
// )
```

## 与 PHP-FPM 的兼容性

如果你的代码需要同时在 OxPHP 和 PHP-FPM 上运行，可使用回退包装器：

```php
<?php
function finish_request(): bool
{
    if (function_exists('oxphp_finish_request')) {
        return oxphp_finish_request();
    }
    if (function_exists('fastcgi_finish_request')) {
        return fastcgi_finish_request();
    }
    return false;
}

// Worker 感知的引导逻辑
if (function_exists('oxphp_is_worker') && oxphp_is_worker()) {
    // OxPHP Worker 模式
} else {
    // PHP-FPM 或 OxPHP 传统模式
}
```

> **注意：** `oxphp_async()` 系列函数在 OxPHP 中始终会被注册，因此即使 `ASYNC_WORKERS=0`，`function_exists('oxphp_async')` 也会返回 `true`。池被禁用时，调用任意异步函数均会抛出 `OxPHP\Async\Exception`。如果你的代码需要同时兼容两种配置，请捕获异常，而非依赖 `function_exists()` 进行检测。

## 参见

- [HTTP 请求对象 API](request-api.md) -- 通过 `oxphp_http_request()` 以面向对象方式访问请求数据
- [Worker 模式](../features/worker-mode.md) -- 持久 Worker 循环与请求生命周期
- [Server-Sent Events](../features/sse.md) -- 使用 `oxphp_stream_flush()` 实现实时流式传输
- [提前响应](../features/early-response.md) -- 使用 `oxphp_finish_request()` 进行后台处理
- [超全局变量](superglobals.md) -- OxPHP 如何填充 `$_SERVER`、`$_GET`、`$_POST` 及其他超全局变量
- [分布式追踪与 APM](../features/distributed-tracing.md) -- W3C Trace Context、OTel 导出和 `oxphp_apm_*()` SDK
- [配置参考](../operations/configuration.md) -- `WORKER_FILE`、`PHP_WORKERS`、`REQUEST_TIMEOUT_SECONDS` 及其他环境变量
