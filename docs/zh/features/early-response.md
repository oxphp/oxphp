---
title: 提前响应
description: 使用 oxphp_finish_request() 立即发送 HTTP 响应，并在 OxPHP 中继续运行后台任务。
---

# 提前响应

`oxphp_finish_request()` 立即向客户端发送完整的 HTTP 响应，并允许 PHP 脚本继续运行以执行后台工作。这是 OxPHP 中等同于 PHP-FPM 的 `fastcgi_finish_request()` 的函数。

## 工作原理

1. 脚本照常设置请求头、状态码并输出响应体。
2. 调用 `oxphp_finish_request()`——OxPHP 刷新所有输出缓冲区，将请求标记为已完成，并将完整的 HTTP 响应传递给客户端。
3. 脚本继续执行后台工作，例如发送邮件、写入缓存条目或分发 Webhook。
4. 调用后产生的任何输出（`echo`、`print`、`var_dump`）都会被静默丢弃。
5. `oxphp_finish_request()` 在首次调用时返回 `true`，在同一请求内的后续调用中返回 `false`。

## 使用场景

提前响应适用于任何需要立即确认请求同时延迟非关键工作的场景：

- **发送邮件** — 立即返回"已接受"，在后台发送邮件
- **缓存预热** — 返回缓存数据，然后重新生成缓存条目
- **分析与日志记录** — 确认请求，然后写入详细的分析记录
- **Webhook 分发** — 向调用方确认接收，然后分发 Webhook
- **图像处理** — 立即返回 URL，然后处理原始尺寸的图像

## PHP 示例

### 基本用法

```php
<?php
header('Content-Type: application/json');
echo json_encode(['status' => 'accepted', 'id' => uniqid()]);

// 立即发送响应——客户端在此时收到完整响应
oxphp_finish_request();

// 后台工作在此处运行；客户端不再等待
file_put_contents('/tmp/audit.log', date('c') . " request processed\n", FILE_APPEND);
send_notification_email($user);
```

### 防止重复调用

`oxphp_finish_request()` 在第二次及后续调用时返回 `false`。在中间件层较多的应用中，多个层可能都会调用该函数，此时应检查返回值：

```php
<?php
function finish_and_cleanup(): void
{
    if (!oxphp_finish_request()) {
        // 已完成——后台工作已安排
        return;
    }

    // 首次调用——可安全运行清理操作
    flush_metrics_buffer();
    close_external_connections();
}
```

### 条件式后台工作

```php
<?php
header('Content-Type: application/json');
$payload = json_decode(file_get_contents('php://input'), true);

$result = handle_request($payload);
echo json_encode($result);

if ($result['needs_sync']) {
    oxphp_finish_request();
    sync_to_external_service($result);
}
// 如果不需要同步，则不提前结束——脚本正常退出
```

### Worker 模式

在 Worker 模式下，PHP Worker 在整个脚本（包括所有后台工作）完成之前保持占用。Worker 只有在回调返回后才接受新请求。

```php
<?php
oxphp_worker(function () {
    $order = json_decode(file_get_contents('php://input'), true);

    $result = process_order($order);
    header('Content-Type: application/json');
    echo json_encode(['order_id' => $result['id'], 'status' => 'accepted']);
    oxphp_finish_request();

    // Worker 在执行以下后台工作期间仍处于占用状态
    send_confirmation_email($result);
    update_inventory($result);
    notify_warehouse($result);
    // 此后 Worker 才变为可用状态
});
```

> **注意：** 在调整 Worker 池大小时，需将后台处理时间考虑在内。每次请求后需要 3 秒处理后续工作的 Worker，实际能处理的并发请求数会相应减少。

## 故障排除

### 后台工作未能完成

PHP 的 `max_execution_time` 在调用 `oxphp_finish_request()` 后仍继续计时。如果脚本总执行时间（包括后台工作）超过限制，请求将被取消并触发 `Request cancelled (timeout)` 致命错误。

**修复：** 提高 `max_execution_time`（在 `php.ini` 中或通过脚本调用 `set_time_limit()`），或将耗时较长的后台任务移至消息队列：

```php
set_time_limit(300);
oxphp_finish_request();
// ... 长时间运行的工作 ...
```

对于经常需要超过几秒的工作，请向 Redis、RabbitMQ 或类似队列发布消息，让专用消费者异步处理。

### Session 变更丢失

必须在调用 `oxphp_finish_request()` 之前写入 Session 数据。调用后的变更会被丢弃。

**修复：** 在调用 `oxphp_finish_request()` 之前调用 `session_write_close()`：

```php
<?php
$_SESSION['last_seen'] = time();
session_write_close();      // 在结束前持久化 Session
oxphp_finish_request();     // 发送响应
```

### 调用 `oxphp_finish_request()` 后响应体为空

如果在没有任何 `echo` 输出的情况下调用 `oxphp_finish_request()`，客户端将收到空正文。请先构建并输出响应，再调用该函数。

## 注意事项

- `oxphp_finish_request()` 在首次调用时返回 `true`，在同一请求内的后续调用中返回 `false`。
- 首次调用后的所有输出（`echo`、`print`、`var_dump`）都会被静默丢弃。
- 在 Worker 模式下，Worker 在整个回调（包括所有响应后代码）完成之前保持占用。
- 请求超时继续适用于 `oxphp_finish_request()` 之后运行的后台代码。
- `oxphp_finish_request()` 和 `oxphp_stream_flush()` 互斥：在流式传输开始之前调用 `oxphp_finish_request()` 会阻止流式传输；在 `oxphp_stream_flush()` 之后调用则会关闭流。

## 参见

- [Worker 模式](worker-mode.md) -- 持久化 PHP 进程以及提前响应与请求循环的交互方式
- [超时配置](timeouts.md) -- 请求超时如何适用于后台工作
- [PHP 函数](../php/functions.md) -- `oxphp_finish_request()` 及其他内置函数的完整参考
