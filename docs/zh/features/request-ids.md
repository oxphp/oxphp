---
title: 请求 ID
description: OxPHP 自动为每个请求分配唯一标识符，用于分布式追踪和跨服务的日志关联。
---

# 请求 ID

OxPHP 处理的每个请求都会获得一个用于追踪和日志关联的唯一标识符。该 ID 出现在响应头和访问日志中，让你可以用单个值追踪请求在整个服务栈中的完整路径。

## 工作原理

1. OxPHP 在任何其他处理之前为每个传入请求分配唯一 ID。
2. 如果客户端发送了 `X-Request-ID` 请求头（例如来自负载均衡器或 API 网关），OxPHP 会对该值进行验证并予以保留。通过验证的条件：长度在 1–64 个字符之间，且仅包含字母数字、连字符（`-`）、下划线（`_`）或点（`.`）。验证失败时，OxPHP 会生成新的 ID。
3. 当不存在有效的 `X-Request-ID` 请求头时，OxPHP 生成一个 20 个字符的小写十六进制 ID（例如 `67890abc12341a2b0042`）。该 ID 编码了时间戳、进程唯一值和单调递增计数器，使跨容器和重启的碰撞极不可能发生。
4. 每个响应的 `X-Request-ID` 响应头中都会包含该 ID。
5. 启用访问日志时，该 ID 会出现在每条访问日志条目的 `request_id` 字段中。
6. PHP 脚本可通过 `oxphp_request_id()` 读取该 ID。

## 响应头

OxPHP 的每个 HTTP 响应都包含 `X-Request-ID` 响应头：

```http
HTTP/1.1 200 OK
X-Request-ID: 67890abc12341a2b0042
Content-Type: text/html; charset=utf-8
```

当上游负载均衡器或网关在传入请求中提供了 `X-Request-ID`，同一值会在响应中回传，从而在整个基础架构中保持端到端的可追踪性。

## PHP 示例

使用 `oxphp_request_id()` 从 PHP 读取当前请求 ID：

```php
<?php
$requestId = oxphp_request_id();

// 在应用日志中包含以便关联
$logger->info('Processing order', [
    'request_id' => $requestId,
    'order_id'   => $orderId,
]);
```

将请求 ID 转发给下游服务，以保持跨 API 调用的可追踪性：

```php
<?php
$requestId = oxphp_request_id();

$ch = curl_init('https://api.example.com/users');
curl_setopt($ch, CURLOPT_HTTPHEADER, [
    "X-Request-ID: $requestId",
]);
$response = curl_exec($ch);
curl_close($ch);
```

## 示例

启用访问日志时，每条日志条目都包含 `request_id` 字段：

```json
{
  "timestamp": "2026-02-11T12:34:56.789Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc12341a2b0042",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1234,
    "remote_addr": "10.0.0.1:54321",
    "message": "request completed"
  }
}
```

在日志聚合器中按 `request_id` 过滤，可以追踪单个请求的完整生命周期，包括引用相同 ID 的 PHP 错误或应用日志条目。

## 故障排除

### 响应中缺少 `X-Request-ID` 头

这是异常情况——OxPHP 会为每个响应添加该请求头。如果该头不存在，可能是中间代理将其过滤掉了。

**检查：** 不经过任何代理，直接对 OxPHP 进行测试：

```bash
curl -v http://localhost:8080/ 2>&1 | grep -i x-request-id
```

### 上游 ID 未被保留

传入的 `X-Request-ID` 值可能未通过验证。OxPHP 会拒绝空值、长度超过 64 个字符，或包含字母数字、连字符、下划线、点以外字符的 ID。

**检查：** 查看上游发送的值，验证其是否符合字符和长度要求。常见的失败情况包括值中含有斜杠、空格或大括号字符。

### `oxphp_request_id()` 返回空字符串

此函数仅在 OxPHP 内部可用。如果在 PHP-FPM 或 CLI 下运行相同的 PHP 代码，该函数不存在。请添加兼容性检查：

```php
<?php
$requestId = function_exists('oxphp_request_id')
    ? oxphp_request_id()
    : ($_SERVER['HTTP_X_REQUEST_ID'] ?? uniqid('', true));
```

## 参见

- [访问日志](access-logging.md) -- 每条日志条目均包含 `request_id` 字段
- [PHP 函数](../php/functions.md) -- `oxphp_request_id()` 及其他内置函数的完整参考
