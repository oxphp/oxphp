---
title: PHP 扩展函数
description: oxphp_sapi PHP 扩展的 API 参考
---

`oxphp_sapi` PHP 扩展注册了十五个内置函数，使你的 PHP 代码能够访问 OxPHP 服务器内部信息。这些函数在 OxPHP 执行的每个 PHP 脚本中均可用 --- 无需 `extension=` 指令，因为该扩展已编译到自定义 SAPI 中。

插件可以在启动时注册额外的 PHP 函数。这些由插件提供的函数在 `MINIT` 期间通过 C 桥接注册，与内置函数一起出现。

## `oxphp_request_id`

返回分配给当前请求的唯一请求标识符。

```php
oxphp_request_id(): string
```

**参数：** 无。

**返回值：** 一个 20 字符的十六进制字符串。格式为 `{timestamp:08x}{process:04x}{counter:08x}`，前 8 个字符是 Unix 时间戳，接下来 4 个是进程标识符（PID XOR 启动纳秒），后 8 个是单调递增计数器。如果未设置请求 ID（正常请求处理中不应发生），则返回空字符串。

**示例：**

```php
<?php
$requestId = oxphp_request_id();
// "67a3b1c4a1f200000042"

header("X-Request-Id: $requestId");

// 在应用日志中使用
error_log("[$requestId] Processing payment for order #1234");
```

**注意事项：**
- 请求 ID 在 PHP 执行开始前由服务器设置。
- 同一 ID 在 Rust 端也可用于访问日志和响应头。
- 由于进程标识符组件，ID 在不同进程和重启之间保持唯一。

---

## `oxphp_worker_id`

返回处理当前请求的 PHP 工作线程索引。

```php
oxphp_worker_id(): int
```

**参数：** 无。

**返回值：** 一个从零开始的整数，标识工作线程。静态工作线程的值范围为 `0` 到 `PHP_WORKERS - 1`。动态工作线程会获得超出初始范围的 ID。

**示例：**

```php
<?php
$workerId = oxphp_worker_id();
// 3

// 用于工作线程特定的临时目录或调试
$tmpDir = "/tmp/oxphp-worker-$workerId";
```

**注意事项：**
- 工作线程 ID 在工作线程的生命周期内保持不变。
- 在动态伸缩模式下，启动后创建的工作线程会获得超过初始池大小的递增 ID。
- 适用于调试并发问题或划分工作线程特定的资源。

---

## `oxphp_server_info`

返回包含服务器和工作线程元数据的关联数组。

```php
oxphp_server_info(): array
```

**参数：** 无。

**返回值：** 包含以下键的关联数组：

| 键 | 类型 | 说明 |
|----|------|------|
| `sapi` | `string` | 始终为 `"oxphp"` |
| `version` | `string` | 服务器版本（当前为 `"0.1.0"`） |
| `worker_id` | `int` | 与 `oxphp_worker_id()` 相同的值 |
| `request_time` | `float` | 请求开始时的 Unix 时间戳，精确到微秒 |
| `worker_mode` | `bool` | 在 worker 模式下运行时为 `true`，传统模式下为 `false` |

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

// 计算已用时间
$elapsed = microtime(true) - $info['request_time'];
echo "Request processing took {$elapsed}s so far";
```

**注意事项：**
- `request_time` 从 C 桥接库的线程本地存储中读取，该值在 `php_request_startup()` 之前设置。
- 此值也被 OPcache 的 `file_update_protection` 检查使用。

---

## `oxphp_request_heartbeat`

从当前时间起将执行截止时间延长指定的秒数。在长时间运行的脚本中使用此函数，以防止协作式看门狗终止请求。

```php
oxphp_request_heartbeat(int $time = 10): bool
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$time` | `int` | `10` | 从当前时间起延长截止时间的秒数 |

**返回值：** 成功时返回 `true`。如果 `$time` 为零或负数，或未设置执行截止时间，返回 `false`。

**示例：**

```php
<?php
// 长时间运行的数据导入
foreach ($records as $record) {
    process($record);
    oxphp_request_heartbeat(30); // 延长截止时间 30 秒
}
```

**注意事项：**
- 截止时间每 128 次 `ub_write` 调用（即每 128 次输出操作）协作式检查一次。这不是硬实时保证。
- 初始截止时间在 PHP 执行开始前从 `REQUEST_TIMEOUT_SECONDS` 设置。
- 如果未配置超时（截止时间为 0），调用此函数无效并返回 `false`。

---

## `oxphp_finish_request`

将当前请求标记为完成，允许服务器将响应发送给客户端，同时 PHP 脚本继续在后台执行。这是 OxPHP 版本的 `fastcgi_finish_request()`。

```php
oxphp_finish_request(): bool
```

**参数：** 无。

**返回值：** 第一次调用返回 `true`。如果请求已经完成（即在同一请求中调用了多次），返回 `false`。

**示例：**

```php
<?php
// 立即将响应发送给客户端
echo json_encode(['status' => 'accepted']);
oxphp_finish_request();

// 继续后台工作 — 客户端已经收到响应
send_notification_email($userId);
update_analytics($eventData);
cleanup_temp_files();
```

**注意事项：**
- 调用此函数后，`echo` 或 `print` 的任何后续输出将从客户端响应中丢弃。
- PHP 工作线程在脚本完成执行前一直被占用，因此长时间的后台任务会减少可用的工作池。
- 在同一请求中第二次调用此函数将返回 `false`。

---

## `oxphp_is_worker`

检查服务器是否运行在 worker 模式下。

```php
oxphp_is_worker(): bool
```

**参数：** 无。

**返回值：** 如果当前请求由持久化 worker 进程处理（即设置了 `WORKER_FILE`）返回 `true`，在传统模式（每个请求启动新的 PHP 进程）下返回 `false`。

**示例：**

```php
<?php
if (oxphp_is_worker()) {
    // Worker 模式：复用持久化数据库连接
    $db = $GLOBALS['db'] ?? ($GLOBALS['db'] = new PDO($dsn));
} else {
    // 传统模式：每请求建立连接
    $db = new PDO($dsn);
}
```

**注意事项：**
- 此函数可在 `oxphp_worker()` 处理器回调内外调用。
- 同一值可通过 `oxphp_server_info()['worker_mode']` 获取。
- 适用于需要根据执行模型调整行为的库和框架（例如连接池、静态缓存、会话处理）。

---

## `oxphp_is_streaming`

检查当前请求是否处于流模式。

```php
oxphp_is_streaming(): bool
```

**参数：** 无。

**返回值：** 如果流模式已激活返回 `true`，否则返回 `false`。

**示例：**

```php
<?php
if (oxphp_is_streaming()) {
    // 增量刷新输出
    echo "data: " . json_encode($event) . "\n\n";
    flush();
} else {
    // 缓冲完整响应
    echo json_encode($allData);
}
```

**注意事项：**
- 当设置 `Content-Type: text/event-stream` 头时，流模式会自动激活，也可通过 `oxphp_stream_flush()` 手动激活。
- 此函数适用于需要根据传输模式调整输出行为的脚本。

---

## `oxphp_stream_flush`

激活流模式（如果尚未激活）并将当前输出缓冲区作为块刷新到客户端。这是在 OxPHP 中实现 Server-Sent Events (SSE) 的主要函数。

```php
oxphp_stream_flush(): bool
```

**参数：** 无。

**返回值：** 成功时返回 `true`。如果请求已通过 `oxphp_finish_request()` 完成，返回 `false`。

**示例：**

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

for ($i = 0; $i < 10; $i++) {
    echo "id: $i\n";
    echo "data: " . json_encode(['counter' => $i, 'time' => microtime(true)]) . "\n\n";
    oxphp_stream_flush();
    sleep(1);
}
```

**工作原理：**

1. 首次调用时，通过 C 桥接激活流模式（`oxphp_bridge_set_stream_mode`）
2. 刷新所有 PHP 输出缓冲区（`php_output_flush_all`）
3. 触发 SAPI flush 回调，将缓冲输出作为 HTTP 块发送到客户端

**注意事项：**
- 首次刷新时将头部发送到客户端。后续调用仅发送正文块。
- 也可以使用原生 PHP `flush()` 配合 `Content-Type: text/event-stream` — OxPHP 会自动检测 SSE 内容类型并激活流模式。在这种情况下，先调用 `ob_end_flush()` 以禁用 PHP 的输出缓冲层。
- 如果之前已调用 `oxphp_finish_request()`，此函数返回 `false` 且不执行任何操作。
- 当 PHP 脚本结束且流通道关闭时，HTTP 连接会自动关闭。
- 通过有界通道（容量 64）实现背压 — 如果客户端读取缓慢，`oxphp_stream_flush()` 会阻塞，直到客户端赶上。

---

## `oxphp_sleep`

协作式 sleep，挂起当前 Fiber，允许工作线程在等待期间处理其他请求。在 Fiber 外部调用时回退为阻塞 `usleep()`。

```php
oxphp_sleep(float $seconds): void
```

**参数：**

| 名称 | 类型 | 说明 |
|------|------|------|
| `$seconds` | `float` | 休眠时间（秒），例如 `0.5` 表示 500ms |

**返回值：** 无（void）。

**示例：**

```php
<?php
oxphp_worker(function() {
    header('Content-Type: text/event-stream');
    for ($i = 0; $i < 10; $i++) {
        echo "data: " . json_encode(['counter' => $i]) . "\n\n";
        oxphp_stream_flush();
        oxphp_sleep(1.0); // yields fiber, worker handles other requests
    }
});
```

**注意事项：**
- 在启用了 Fiber 调度器的工作进程模式下，此函数注册计时器并挂起当前 Fiber。调度器在指定时间后恢复 Fiber。
- 在 Fiber 外部（传统模式）回退为阻塞 `usleep()`。
- 小于或等于零的值立即返回，无任何效果。
- 计时器分辨率为毫秒粒度（时间向上取整到最近的毫秒）。

---

## `oxphp_usleep`

协作式微秒级 sleep。与 `oxphp_sleep()` 功能相同，但接受微秒整数作为参数。

```php
oxphp_usleep(int $microseconds): void
```

**参数：**

| 名称 | 类型 | 说明 |
|------|------|------|
| `$microseconds` | `int` | 休眠时间（微秒） |

**返回值：** 无（void）。

**示例：**

```php
<?php
oxphp_worker(function() {
    oxphp_usleep(50000); // 50ms cooperative sleep
    echo "done";
});
```

**注意事项：**
- 行为与 `oxphp_sleep()` 相同，使用微秒粒度。
- 在 Fiber 模式下，计时器以毫秒精度注册（向上取整）。
- 小于或等于零的值立即返回，无任何效果。

---

## `oxphp_worker`

进入持久化工作进程模式循环。对每个传入的 HTTP 请求调用提供的处理器回调。请求之间会进行软重置，清理每请求状态（超全局变量、输出缓冲区、响应头、错误状态），而不销毁 PHP 堆，因此引导状态（自动加载器、数据库连接、缓存配置）在请求间保持不变。

```php
oxphp_worker(callable $handler): bool
```

**参数：**

| 名称 | 类型 | 说明 |
|------|------|------|
| `$handler` | `callable` | 每个 HTTP 请求调用一次的回调。不接收参数。 |

**返回值：** 优雅关闭（服务器停止）时返回 `true`。如果未启用工作进程模式（即未设置 `WORKER_FILE`），立即返回 `false`。

**示例：**

```php
<?php
// worker.php — 持久化工作进程入口点

// 引导：每个工作进程生命周期运行一次
require __DIR__ . '/vendor/autoload.php';
$db = new PDO('mysql:host=localhost;dbname=app', 'root', '');
$config = json_decode(file_get_contents(__DIR__ . '/config.json'), true);

// 在循环中处理请求
oxphp_worker(function () use ($db, $config) {
    $uri = $_SERVER['REQUEST_URI'];
    $method = $_SERVER['REQUEST_METHOD'];

    // 路由并处理请求
    if ($uri === '/api/users' && $method === 'GET') {
        $users = $db->query('SELECT id, name FROM users')->fetchAll();
        header('Content-Type: application/json');
        echo json_encode($users);
    } else {
        http_response_code(404);
        echo 'Not Found';
    }
});
```

**工作原理：**

1. 处理器回调对从 Rust 层接收的每个 HTTP 请求被调用。
2. 请求之间发生软重置：
   - 超全局变量（`$_GET`、`$_POST`、`$_SERVER`、`$_COOKIE`、`$_FILES`）从新请求数据重新填充
   - 输出缓冲区被清除
   - HTTP 响应头被重置
   - 通过 `register_shutdown_function()` 注册的关闭函数被调用并清除
3. 垃圾收集定期运行（每 100 个请求），在不影响每请求延迟的情况下回收循环引用。
4. 循环在以下情况退出：
   - 服务器关闭（优雅关闭信号）
   - 处理器连续失败 3 次（退出原因：`consecutive_errors`）— 单独的错误可以容忍，成功时计数器重置
   - 工作进程达到 `WORKER_MAX_REQUESTS`（退出原因：`max_requests`）
   - 工作进程超过 `WORKER_MAX_MEMORY_MIB`（退出原因：`max_memory`）

**注意事项：**
- 此函数仅在设置了 `WORKER_FILE` 时有效。从普通 PHP 脚本调用会触发 `E_WARNING` 并返回 `false`。
- 在处理器闭包外声明的变量在请求间保持不变。用于数据库连接、配置和其他昂贵的初始化操作。
- 处理器的 `use` 子句按引用或按值捕获变量，行为与平常相同。按引用捕获的变量在请求间共享状态。
- 工作进程回收（通过 `WORKER_MAX_REQUESTS` 或 `WORKER_MAX_MEMORY_MIB`）会导致工作进程退出并重新生成，重新执行整个工作脚本（包括引导代码）。
- 工作进程模式指标（`oxphp_worker_requests_handled_total`、`oxphp_worker_recycles_total` 等）在内部服务器运行时可通过 `/metrics` 端点获取。

---

## `oxphp_async`

将闭包分发到专用异步工作池进行异步执行。闭包的 `use` 变量和参数在源线程上序列化，在异步工作线程上反序列化为独立副本。

```php
oxphp_async(Closure $closure, mixed ...$args): int|false
```

**参数：**

| 名称 | 类型 | 说明 |
|------|------|------|
| `$closure` | `Closure` | 要在异步工作线程上执行的闭包 |
| `...$args` | `mixed` | 序列化到工作线程的参数。支持标量、字符串和数组。资源和对象将被拒绝并触发 `E_WARNING` |

**返回值：** 成功时返回 Promise ID（正整数）。如果异步池未配置（`ASYNC_WORKERS=0`）或队列已满，返回 `false`。

**示例：**

```php
<?php
$promise = oxphp_async(function(int $x, int $y): int {
    return $x + $y;
}, 10, 20);

$result = oxphp_async_await($promise);
// 30
```

**注意事项：**
- 闭包在拥有自己 PHP ZTS 状态的独立操作系统线程上执行。通过 `use` 捕获的变量被序列化为独立副本 — 源变量保持可写。
- 异步池必须通过 `ASYNC_WORKERS` 环境变量启用。
- 当队列已满时，任务将被拒绝，`oxphp_async()` 返回 `false`。

---

## `oxphp_async_await`

阻塞当前线程，直到指定的异步任务完成并返回其结果。

```php
oxphp_async_await(int $promise_id, ?float $timeout = null): mixed
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$promise_id` | `int` | *(必填)* | `oxphp_async()` 返回的 Promise ID |
| `$timeout` | `?float` | `null` | 最大等待秒数。`null` 表示无限等待 |

**返回值：** 闭包的返回值。支持所有标量类型、字符串和数组（包括嵌套）。

**抛出异常：**

| 异常 | 条件 |
|------|------|
| `OxPHP\AsyncException` | 闭包抛出了异常，或调用了 `die()` / `exit()` |
| `OxPHP\AsyncTimeoutException` | 超时时间到期而任务尚未完成 |

**示例：**

```php
<?php
$p = oxphp_async(function(): string {
    usleep(100_000);
    return 'done';
});

try {
    $result = oxphp_async_await($p, 0.5); // 500ms timeout
} catch (\OxPHP\AsyncTimeoutException $e) {
    $result = 'timed out';
}
```

**注意事项：**
- 返回值从异步工作线程反序列化到当前线程的堆上。
- 每个 Promise ID 只能被等待一次。两次等待同一 ID 的行为未定义。
- 未等待的 Promise 在请求结束时（RSHUTDOWN）自动清理，清理超时为 5 秒。

---

## `oxphp_async_await_all`

等待多个 Promise 并以关联数组形式返回所有结果。

```php
oxphp_async_await_all(array $promise_ids, ?float $timeout = null): array
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$promise_ids` | `array` | *(必填)* | `oxphp_async()` 返回的 Promise ID 数组 |
| `$timeout` | `?float` | `null` | 每个 Promise 的超时时间（秒） |

**返回值：** 将每个 Promise ID 映射到其结果值的关联数组。

**抛出异常：** 如果任何 Promise 失败或超时，抛出 `OxPHP\AsyncException` 或 `OxPHP\AsyncTimeoutException`。

**示例：**

```php
<?php
$p1 = oxphp_async(function(): int { return 1; });
$p2 = oxphp_async(function(): int { return 2; });
$p3 = oxphp_async(function(): int { return 3; });

$results = oxphp_async_await_all([$p1, $p2, $p3]);
// [$p1 => 1, $p2 => 2, $p3 => 3]
```

---

## `oxphp_async_await_any`

竞速多个 Promise 并返回最先完成的结果，与数组顺序无关。内部使用 `futures::select_all` 实现真正的并发竞速语义。

```php
oxphp_async_await_any(array $promise_ids, ?float $timeout = null): array
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$promise_ids` | `array` | *(必填)* | `oxphp_async()` 返回的 Promise ID 数组 |
| `$timeout` | `?float` | `null` | 任何 Promise 完成的整体超时时间（秒） |

**返回值：** 包含两个键的关联数组：`id`（最先完成的 Promise ID）和 `value`（其结果）。

**抛出异常：** 如果获胜的 Promise 抛出了异常，抛出 `OxPHP\AsyncException`。如果在超时时间内没有 Promise 完成，抛出 `OxPHP\AsyncTimeoutException`。

**剩余 Promise：** 未获胜的 Promise 仍在执行中，可通过 `oxphp_async_await()` 单独等待。超时时，所有指定的 Promise 将被取消。

**示例：**

```php
<?php
$p1 = oxphp_async(function(): int { sleep(2); return 1; });
$p2 = oxphp_async(function(): int { usleep(100_000); return 2; });
$p3 = oxphp_async(function(): int { sleep(1); return 3; });

$winner = oxphp_async_await_any([$p1, $p2, $p3]);
// ['id' => $p2, 'value' => 2]  (fastest to complete, ~100ms)

// Non-winning promises are still awaitable
$r1 = oxphp_async_await($p1); // 1
$r3 = oxphp_async_await($p3); // 3
```

---

## 插件函数

插件可以注册自定义 PHP 函数，供脚本调用。这些函数在 PHP 模块初始化（`MINIT`）期间注册，通过 C 桥接分发到 Rust 处理代码。

插件函数使用原生桥接进行零序列化分发。参数和返回值作为原始 `zval` 指针传递 --- Rust 通过 C 访问器函数直接读写它们，无 JSON 编码开销。如果处理器返回错误，将触发 PHP `E_WARNING` 并返回 `NULL`。

```php
<?php
// 示例：调用插件注册的函数
$result = some_plugin_function('arg1', 42, ['key' => 'value']);
```

插件函数在 `phpinfo()` 输出中与内置函数一起列出，但它们是全局注册的（不在 `oxphp_sapi` 扩展下），因此不会出现在 `get_extension_funcs('oxphp_sapi')` 中。

## 扩展信息

扩展元数据在 `phpinfo()` 输出中可见：

| 字段 | 值 |
|------|-----|
| 扩展名称 | `oxphp_sapi` |
| 版本 | `0.1.0` |

你可以验证扩展是否已加载：

```php
<?php
var_dump(extension_loaded('oxphp_sapi'));
// bool(true)

// 列出内置扩展函数
print_r(get_extension_funcs('oxphp_sapi'));
// Array
// (
//     [0] => oxphp_request_id
//     [1] => oxphp_worker_id
//     [2] => oxphp_server_info
//     [3] => oxphp_request_heartbeat
//     [4] => oxphp_finish_request
//     [5] => oxphp_is_worker
//     [6] => oxphp_is_streaming
//     [7] => oxphp_stream_flush
//     [8] => oxphp_sleep
//     [9] => oxphp_usleep
//     [10] => oxphp_worker
//     [11] => oxphp_async
//     [12] => oxphp_async_await
//     [13] => oxphp_async_await_all
//     [14] => oxphp_async_await_any
// )
```

## 另请参阅

- [异步 Promise](/features/async-promises.md) --- 使用 `oxphp_async()` 和 `oxphp_async_await()` 进行并行执行
- [基于 Fiber 的请求多路复用](/features/fiber-multiplexing.md) --- 使用 `oxphp_sleep()` 进行协作式多任务处理，以及支持 Fiber 的 `oxphp_async_await()`
- [超全局变量](superglobals.md) --- OxPHP 如何填充 `$_SERVER`、`$_GET`、`$_POST` 及其他超全局变量
- [OPcache 兼容性](opcache.md) --- `request_time` 回调如何启用 OPcache
- [请求 ID](/features/request-ids.md) --- 请求 ID 如何生成和传播
- [SAPI 桥接](/architecture/sapi-bridge.md) --- 连接 Rust 和 PHP 的 C 桥接
- [工作池](/architecture/worker-pool.md#worker-mode-persistent-php) --- 工作进程模式架构、回收和指标
- [配置](/operations/configuration.md#worker-mode) --- `WORKER_FILE`、`WORKER_MAX_REQUESTS`、`WORKER_MAX_MEMORY_MIB`
