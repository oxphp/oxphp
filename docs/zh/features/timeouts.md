---
title: 超时配置
description: 在 OxPHP 中配置请求头读取超时和 PHP 执行时间限制，防止慢速客户端和失控 PHP 脚本占用资源。
---

# 超时配置

OxPHP 强制执行两个独立的超时机制，防止慢速客户端和失控请求占用资源。请求头超时在服务器层面保护连接阶段。PHP 执行时间由 PHP 自身的 `max_execution_time` ini 指令（以及 `set_time_limit()` 运行时函数）限制，与其他 SAPI 的行为完全一致。

## 工作原理

每个请求经历以下阶段：

1. **连接已接受** — 请求头超时开始计时。OxPHP 等待客户端发送完整的 HTTP 请求头。
2. **请求头已接收** — 请求头超时结束。请求被分发到 PHP Worker。
3. **PHP 处理请求** — 应用代码在 PHP 自身的 `max_execution_time`（基于 SIGALRM）下运行。达到限制时，请求被取消，触发统一的 `Request cancelled (timeout)` 致命错误。
4. **响应已发送** — 在 keep-alive 连接上，循环从第 1 步重新开始。

```text
TCP 连接（如启用 TLS 则包含 TLS 握手）
  |
  +-- [HEADER_TIMEOUT_SECONDS] --> 请求头已接收
                                   |
                                   +-- [max_execution_time] --> 响应已发送
                                                                    |
                                                                    +-- 下一个请求（keep-alive）
```

在 keep-alive 连接上，两个超时均独立应用于连接中的每个请求。

> **注意：** 启用 TLS 时，请求头超时从 TLS 握手完成后开始计时，而非从 TCP 连接建立时开始。

请求头超时可防御 slowloris 风格的攻击——攻击者每次只发送一个字节的请求头，以无限期地占用连接。

PHP 执行计时完全委托给 PHP。当 `max_execution_time` 超时时，OxPHP 的统一取消路径会：

- 设置 `connection_status() & PHP_CONNECTION_TIMEOUT`，使用户代码可以检测原因。
- 运行所有 `register_shutdown_function()` 回调，与 PHP-FPM 完全一致。
- 返回 HTTP `504 Gateway Timeout`，错误日志中写入消息 `Request cancelled (timeout)`。

### 取消请求时的状态码

OxPHP 因多种不同原因取消请求。每种原因都映射为反映实际情况的 HTTP 状态，而不是统一的 `500`：

| 原因 | 状态码 | 说明 |
|------|--------|------|
| 超出 `max_execution_time` / `set_time_limit()` | `504 Gateway Timeout` | 服务端执行时间耗尽。 |
| 服务器优雅关停时中断了请求 | `503 Service Unavailable` | 附加 `Retry-After: 5` 响应头，提示客户端在已恢复或新启动的实例上重试。 |
| 客户端在请求处理过程中关闭连接 | `499` | nginx 风格的 "Client Closed Request"。连接已断开，因此该状态仅出现在访问日志和指标中——永远不会写入响应。把客户端引发的中断从 `5xx` 中区分出来，避免污染服务器错误告警。 |
| Worker 被监督进程判定为卡死 | `500 Internal Server Error` | 通用服务端错误——具体原因（死锁、阻塞 syscall 等）未知。 |
| 用户代码主动触发的取消 | `500 Internal Server Error` | 用户代码可在触发取消之前通过 `http_response_code()` 设置自己的状态码——该显式状态会被保留。 |

如果 `ERROR_PAGES_DIR` 中只放了 `500.html`，请额外添加 `504.html`、`503.html` 以及（可选的）`499.html`，让所有取消原因下的错误页保持一致的样式。

## 配置

| 变量 | 默认值 | 说明 |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | 连接建立后接收请求头的最长秒数。防御 slowloris 攻击。`0` **不会**被特殊处理——它会作为零秒超时直接传给 hyper，从而立即触发。要禁用该超时，请取消该变量的设置，而不是将其设为 `0` |

PHP 执行时间通过 `php.ini` 配置，而非 OxPHP 的环境变量：

```ini
; php.ini
max_execution_time = 30
```

或通过运行时按脚本配置：

```php
set_time_limit(60);  // 从当前时刻起 60 秒
set_time_limit(0);   // 为本请求禁用
```

## 推荐值

| 场景 | 请求头超时 | `max_execution_time` |
|----------|----------------|----------------------|
| API 服务器 | 5s | 30s |
| 通用 Web 服务 | 5s | 60s |
| 文件上传 | 10s | 300s |
| SSE / 长轮询 | 5s | 0（按脚本禁用） |

请根据应用特点调整这些值。对于 SSE 端点，应在流式脚本顶部调用 `set_time_limit(0)`，而非全局禁用 `max_execution_time`。

## 故障排除

### 客户端意外收到 504 并附带 `Request cancelled (timeout)`

PHP 执行时间限制在脚本完成前触发。

**修复：** 提高受影响脚本的 `max_execution_time`，或在运行时调用 `set_time_limit($seconds)` 进行扩展：

```php
// 在慢速脚本顶部
set_time_limit(300);
```

对于需要无限期保持连接的 SSE 或流式端点，为该脚本禁用计时器：

```php
set_time_limit(0);
```

### 连接在请求头到达前被断开

对于高延迟链路或慢速负载均衡器后面的客户端，请求头超时设置过短。

**修复：** 增大请求头超时值：

```bash
HEADER_TIMEOUT_SECONDS=15
```

### OPcache 导致脚本变更后的第一个请求超时

脚本文件变更后，首次请求时 OPcache 重新编译会增加延迟。在包含大量文件的开发环境中更为常见。可适当增大 `max_execution_time` 或在开发期间禁用。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "8080:8080"
    environment:
      HEADER_TIMEOUT_SECONDS: "5"
    volumes:
      - ./app:/var/www/html:ro
      - ./php.ini:/usr/local/etc/php/conf.d/zz-app.ini:ro
```

`php.ini` 内容如下：

```ini
max_execution_time = 30
```

## 最佳实践

- **生产环境中绝不全局设置 `max_execution_time = 0`**，除非有需要无限期连接的 SSE 或长轮询端点。优先使用按脚本的 `set_time_limit(0)`。
- **为 API 服务器使用较短的限制值。** API 的响应时间通常可预测。30 秒的 `max_execution_time` 可快速捕获卡死的请求，同时不影响正常流量。
- **结合速率限制使用。** 超时保护单个请求，速率限制防御高请求量。两者结合可全面防御慢速和快速两种攻击模式。

## 参见

- [速率限制](rate-limiting.md) -- 基于 IP 的请求速率限制
- [服务器推送事件（SSE）](sse.md) -- 流式端点禁用超时的指导说明
- [配置参考](../operations/configuration.md) -- 完整的环境变量参考
