---
title: PHP 扩展函数
description: oxphp_sapi PHP 扩展的 API 参考
---

`oxphp_sapi` PHP 扩展注册了八个内置函数，使你的 PHP 代码能够访问 OxPHP 服务器内部信息。这些函数在 OxPHP 执行的每个 PHP 脚本中均可用 --- 无需 `extension=` 指令，因为该扩展已编译到自定义 SAPI 中。

插件可以在启动时注册额外的 PHP 函数。这些由插件提供的函数在 `MINIT` 期间通过 C 桥接注册，与内置函数一起出现。

## `oxphp_request_id`

返回分配给当前请求的唯一请求标识符。

```php
oxphp_request_id(): string
```

**参数：** 无。

**返回值：** 一个 16 字符的十六进制字符串。格式为 `{timestamp_hex}{counter_hex}`，前 8 个字符是 Unix 时间戳，后 8 个是单调递增计数器。如果未设置请求 ID（正常请求处理中不应发生），则返回空字符串。

**示例：**

```php
<?php
$requestId = oxphp_request_id();
// "67a3b1c400000042"

header("X-Request-Id: $requestId");

// 在应用日志中使用
error_log("[$requestId] Processing payment for order #1234");
```

**注意事项：**
- 请求 ID 在 PHP 执行开始前由服务器设置。
- 同一 ID 在 Rust 端也可用于访问日志和响应头。
- ID 在单个服务器进程生命周期内唯一。

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

**示例：**

```php
<?php
$info = oxphp_server_info();
// [
//     "sapi"         => "oxphp",
//     "version"      => "0.1.0",
//     "worker_id"    => 3,
//     "request_time" => 1738800000.123456,
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

为未来的看门狗超时延长机制预留的占位函数。当前为空操作。

```php
oxphp_request_heartbeat(int $time = 10): bool
```

**参数：**

| 名称 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `$time` | `int` | `10` | 请求的超时延长秒数 |

**返回值：** 当前实现始终返回 `true`。

**示例：**

```php
<?php
// 长时间运行的数据导入
foreach ($records as $record) {
    process($record);
    oxphp_request_heartbeat(30); // 通知脚本仍然存活
}
```

**注意事项：**
- 此函数是为前向兼容性而存在的。未来版本中，它将重置请求超时看门狗定时器。
- 现在调用它是安全的，没有性能开销。

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

## `oxphp_worker`

进入持久化工作进程模式循环。对每个传入的 HTTP 请求调用提供的处理器回调。请求之间会进行软重置，清理每请求状态（超全局变量、输出缓冲区、`$_SESSION`），而不销毁 PHP 堆，因此引导状态（自动加载器、数据库连接、缓存配置）在请求间保持不变。

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
   - 处理器抛出未捕获的异常或致命错误（退出原因：`error`）
   - 工作进程达到 `WORKER_MAX_REQUESTS`（退出原因：`max_requests`）
   - 工作进程超过 `WORKER_MAX_MEMORY`（退出原因：`max_memory`）

**注意事项：**
- 此函数仅在设置了 `WORKER_FILE` 时有效。从普通 PHP 脚本调用会触发 `E_WARNING` 并返回 `false`。
- 在处理器闭包外声明的变量在请求间保持不变。用于数据库连接、配置和其他昂贵的初始化操作。
- 处理器的 `use` 子句按引用或按值捕获变量，行为与平常相同。按引用捕获的变量在请求间共享状态。
- 工作进程回收（通过 `WORKER_MAX_REQUESTS` 或 `WORKER_MAX_MEMORY`）会导致工作进程退出并重新生成，重新执行整个工作脚本（包括引导代码）。
- 工作进程模式指标（`oxphp_worker_requests_handled_total`、`oxphp_worker_recycles_total` 等）在内部服务器运行时可通过 `/metrics` 端点获取。

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
//     [5] => oxphp_is_streaming
//     [6] => oxphp_stream_flush
//     [7] => oxphp_worker
// )
```

## 另请参阅

- [超全局变量](superglobals.md) --- OxPHP 如何填充 `$_SERVER`、`$_GET`、`$_POST` 及其他超全局变量
- [OPcache 兼容性](opcache.md) --- `request_time` 回调如何启用 OPcache
- [请求 ID](/features/request-ids.md) --- 请求 ID 如何生成和传播
- [SAPI 桥接](/architecture/sapi-bridge.md) --- 连接 Rust 和 PHP 的 C 桥接
- [工作池](/architecture/worker-pool.md#worker-mode-persistent-php) --- 工作进程模式架构、回收和指标
- [配置](/operations/configuration.md#worker-mode) --- `WORKER_FILE`、`WORKER_MAX_REQUESTS`、`WORKER_MAX_MEMORY`
