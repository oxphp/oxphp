---
title: 服务器推送事件（SSE）
description: 在 OxPHP 中使用服务器推送事件向浏览器客户端流式传输实时数据，内置背压支持和连接管理。
---

# 服务器推送事件（SSE）

OxPHP 使用服务器推送事件协议向客户端流式传输实时数据，内置背压支持。在 PHP 脚本中设置 `Content-Type: text/event-stream`，然后调用 `oxphp_stream_flush()`——OxPHP 处理其余的一切。

## 工作原理

1. PHP 脚本通过 `header()` 设置 `Content-Type: text/event-stream`，并使用 `echo` 写入 SSE 格式的行。
2. 首次调用 `oxphp_stream_flush()` 时，将 HTTP 请求头发送给客户端并进入流式模式，客户端连接保持打开状态。
3. 后续每次调用 `oxphp_stream_flush()` 都会将缓冲的输出作为新的数据块刷新，立即传递给客户端。
4. OxPHP 在 PHP Worker 和客户端之间维护最多 64 个数据块的内部缓冲区。当缓冲区已满（因为慢速客户端尚未消费早期数据块）时，`oxphp_stream_flush()` 会阻塞，直到有空间为止。这可防止内存无限增长。
5. PHP 脚本执行完毕后，OxPHP 优雅地关闭连接。如果客户端在流传输过程中断开，OxPHP 会在下一次 flush 时检测到通道已关闭，将 PHP 的 `connection_aborted()` 标志设为 `true`，并触发优雅的 bailout——检查 `connection_aborted()` 的可移植循环会通过常规终止路径干净退出；不检查的循环则会在下一次 flush 时被隐式 bailout 终止。

> **注意：** 保持事件载荷小巧以维持流畅的吞吐量。大载荷会迅速填满 64 个数据块的缓冲区，导致 PHP 在每次 flush 时阻塞。

## PHP 示例

### 基本 SSE 流

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');
header('Connection: keep-alive');

for ($i = 0; $i < 100; $i++) {
    $data = json_encode(['counter' => $i, 'time' => microtime(true)]);
    echo "id: {$i}\n";
    echo "event: tick\n";
    echo "data: {$data}\n\n";
    oxphp_stream_flush();

    sleep(1);

    // 每 15 秒发送一次注释心跳，防止代理因连接空闲而关闭
    if ($i % 15 === 0) {
        echo ": heartbeat\n\n";
        oxphp_stream_flush();
    }
}
```

### 检查流式状态

使用 `oxphp_is_streaming()` 检查当前请求是否已处于流式模式。这在中间件或共享请求处理器中非常有用：

```php
<?php
if (!oxphp_is_streaming()) {
    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');
}

echo "data: {\"status\": \"connected\"}\n\n";
oxphp_stream_flush();
```

### 检测客户端断开

长连接的 SSE 循环应检查 `connection_aborted()`，以便在客户端关闭连接时干净退出。这与标准 PHP / php-fpm 的惯用法一致，让脚本能够在退出前执行清理逻辑（关闭数据库连接、释放锁、执行 `finally` 块）：

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

$db = new PDO(/* ... */);

try {
    while (!connection_aborted()) {
        echo "data: " . json_encode(['ts' => time()]) . "\n\n";
        oxphp_stream_flush();
        sleep(1);
    }
} finally {
    $db = null; // 正常退出和 connection_aborted 退出时都会执行
}
```

如果脚本从不检查 `connection_aborted()`，OxPHP 仍会在客户端断开后通过下一次 flush 上的隐式 bailout 终止它——但绕过 flush 调用的代码路径上的 `finally` 块可能不会运行。对于持有外部资源的代码，请优先使用显式检查。

### 使用原生 flush()

PHP 原生的 `flush()` 也可用于流式传输，但需要先清除所有输出缓冲层。推荐使用 `oxphp_stream_flush()`——它能自动管理输出缓冲区，并与 OxPHP 的背压系统集成。

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

while (ob_get_level()) {
    ob_end_clean();
}

for ($i = 0; $i < 100; $i++) {
    echo "data: " . json_encode(['counter' => $i]) . "\n\n";
    flush();
    sleep(1);
}
```

### SSE 与 Worker 模式

SSE 在标准模式和 Worker 模式下均可使用。在 Worker 模式下，流式连接在整个流传输期间占用该 Worker。Worker 只有在脚本执行完毕后才会处理下一个请求。

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

$redis = new Redis();
$redis->pconnect('redis', 6379);

oxphp_worker(function () use ($redis) {
    if (($_SERVER['HTTP_ACCEPT'] ?? '') !== 'text/event-stream') {
        http_response_code(400);
        echo json_encode(['error' => 'SSE only']);
        return;
    }

    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');

    while (true) {
        $message = $redis->brPop('events', 25);
        if ($message) {
            echo "data: {$message[1]}\n\n";
        } else {
            // 超时内无消息——发送心跳以保持连接活跃
            echo ": heartbeat\n\n";
        }
        oxphp_stream_flush();
    }
});
```

## 故障排除

### 客户端直到脚本结束才收到数据

PHP 输出缓冲区在捕获输出而非流式传输。这发生在 OB 层处于活动状态且未调用 `oxphp_stream_flush()` 时。

**修复：** 在每个事件后调用 `oxphp_stream_flush()`。该函数会刷新所有 PHP 输出缓冲层，并将累积的输出作为一个数据块发送。

### SSE 连接在几分钟后关闭

PHP 的 `max_execution_time` 触发并终止脚本。SSE 流的运行时间必须超过配置的限制。

**修复：** 在流式脚本顶部禁用按请求的执行计时器：

```php
set_time_limit(0);
```

这优于全局设置 `max_execution_time = 0` —— 限制在非 SSE 端点上仍然有效。或者，如果整个实例专用于长连接流：

```ini
; php.ini
max_execution_time = 0
```

### 中间代理关闭空闲的 SSE 连接

负载均衡器和代理通常会关闭 30–60 秒内没有数据传输的连接。

**修复：** 定期发送注释心跳以保持连接活跃：

```php
echo ": heartbeat\n\n";
oxphp_stream_flush();
```

### `oxphp_stream_flush()` 返回 `false`

在同一请求中，`oxphp_finish_request()` 在此之前已被调用。一旦响应结束，就无法进行流式传输。请检查代码，确认在流式传输开始之前没有无意中调用了 `oxphp_finish_request()`。

## Docker 示例

SSE 端点需要禁用 PHP 执行计时器或将其设置得足够高。每个活跃的 SSE 连接在整个流传输期间占用一个 PHP Worker，因此请根据预期的并发流数量调整 Worker 池大小。

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "8080:8080"
    volumes:
      - ./src:/var/www/html
    environment:
      DOCUMENT_ROOT: "/var/www/html/public"
      ENTRY_FILE: "index.php"
      PHP_WORKERS: "32"
```

每个流式脚本应在顶部调用 `set_time_limit(0)`，以避免按请求的计时器在流传输中途触发。这样可保留全局的 `max_execution_time` 对非 SSE 请求的限制。

## 最佳实践

- **使用 `oxphp_stream_flush()` 而非原生 `flush()`**，以自动管理输出缓冲区并与背压系统集成。
- **定期发送注释心跳**（`: heartbeat\n\n`），每 20–30 秒一次，防止中间代理关闭空闲连接，并尽早检测客户端断开。
- **保持事件载荷小巧。** 大载荷会更快填满 64 个数据块的缓冲区，导致 PHP 在每次 flush 时停顿。对于大数据，发送事件 ID 并让客户端通过单独的请求获取完整载荷。
- **按脚本禁用 PHP 执行计时器**（`set_time_limit(0)`）用于长连接的 SSE 端点，或将 `max_execution_time` 设置得足够高以覆盖预期的最长流传输时间。
- **根据峰值并发流数量调整 Worker 池大小。** 每个活跃的 SSE 连接在整个持续期间占用一个 PHP Worker。至少为每个预期并发客户端分配一个 Worker，并额外留出 Worker 用于普通的非 SSE 请求。

## 注意事项

- 流式响应会自动跳过 Brotli 压缩。压缩仅适用于完全缓冲的响应。
- 如果同一请求中已调用 `oxphp_finish_request()`，`oxphp_stream_flush()` 将返回 `false`。
- 在 Worker 模式下，Worker 在整个流传输期间保持占用，只有在 PHP 脚本退出后才处理下一个请求。

## 关闭时的行为

当服务器收到 SIGTERM（滚动部署、`docker stop`、Kubernetes pod 驱逐）时，会优雅地排空连接：

- 每个打开的 SSE 流会在下一次 `flush` 时干净地结束——其处理器如同连接关闭一样退出，因此 `register_shutdown_function()` 回调仍会运行，`error_get_last()['message']` 为 `Request cancelled (shutdown)`。
- HTTP/2 客户端会收到 `GOAWAY` 帧；HTTP/1.1 keep-alive 连接被关闭。浏览器的 `EventSource` 会自动重连——在集群前有负载均衡器时会连到健康的实例。
- 该流**不会**收到 503：其 `200` 响应头已经发送，状态码无法重写。请将客户端设计为从最后一个事件 id 恢复（每个事件都发送 `id:`；重连时浏览器会通过 `Last-Event-ID` 请求头回传，在 PHP 中可用 `$_SERVER['HTTP_LAST_EVENT_ID']` 读取）。

SIGTERM 时正在执行的普通（非流式）请求不会被中断：它们可以利用整个排空窗口正常完成，只有在 `DRAIN_TIMEOUT_SECONDS`（默认 25）到期时仍在运行的请求才会被取消，并有约 2 秒时间收尾。请将编排器的终止宽限期设置为高于 `DRAIN_TIMEOUT_SECONDS` + 2。

这里的"流式"指任何已经开始通过 flush 输出 chunked 内容的响应——服务器无法区分有限的流式下载和无限的事件流，因此 SIGTERM 时正在进行的大文件分块下载同样会被提前结束，而不仅仅是 SSE。在 SIGTERM 之前已调用 `oxphp_finish_request()` 的请求算作普通请求：其响应已经完成，剩余的后台工作可以利用整个排空窗口。

阻塞在单个耗时**原生**调用（原生 `sleep()`、耗时的 `preg_match`、阻塞式数据库查询）中的处理器，在该调用返回前无法被中断。在 worker 模式下，请在流式循环中优先使用协作式的 `oxphp_sleep()`——它会让出给 fiber 调度器并在关闭时立即被唤醒。在 worker 模式之外，`oxphp_sleep()` 退化为普通的阻塞式睡眠，因此流会在睡眠结束后的下一次 `flush` 时才响应关闭。

## 参见

- [Worker 模式](worker-mode.md) -- 减少启动开销的持久化 PHP 进程
- [超时配置](timeouts.md) -- 为长连接配置或禁用请求超时
- [PHP 函数](../php/functions.md) -- `oxphp_stream_flush()` 和 `oxphp_is_streaming()` 的完整参考
- [压缩](compression.md) -- Brotli 压缩行为及哪些响应会被压缩
