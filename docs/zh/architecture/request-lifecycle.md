---
title: 请求生命周期
description: 逐步详解 OxPHP 如何处理 HTTP 请求 — 从 TCP 接受到响应发送
---

OxPHP 中的每个 HTTP 请求都会经过一系列管道阶段，从 TCP 接受到响应交付。本页追踪该管道在 `src/server/connection.rs` 中的实际代码流程。

## 管道概览

```
  Client
    │
    ▼
┌──────────────────┐
│ TCP Accept       │  main.rs: listener.accept()
│ + TLS Handshake  │  server/mod.rs: handle_connection()
└────────┬─────────┘
         ▼
┌──────────────────┐
│ HTTP Parse       │  hyper-util auto::Builder
│ (http1/http2)    │  service_fn → handle_request()
└────────┬─────────┘
         ▼
┌──────────────────┐
│ RequestReceived  │  Event dispatch (priority order):
│ Event            │    -100  RequestIdGenerator
│                  │    -50   RateLimitHandler
│                  │      0   MetricsRequestHandler
└────────┬─────────┘
         │
    ┌────┴────┐
    │ Early   │──── Yes ──▶ 429 Too Many Requests
    │ Response│              (skip to RequestComplete)
    │ ?       │
    └────┬────┘
         │ No
         ▼
┌───────────────────┐
│ Plugin Cookie     │  plugin::cookies::strip_plugin_cookies()
│ Strip             │
└────────┬──────────┘
         ▼
┌───────────────────┐
│ Route Resolution  │  routing.rs: resolve_request()
│ Serve / Execute / │  sanitize, validate, file cache
│ NotFound          │
└────────┬──────────┘
         │
    ┌────┴─────────┐
    │              │
    ▼              ▼
┌────────┐  ┌──────────┐
│ Static │  │ PHP      │
│ File   │  │ Execution│
│ Serve  │  │ (worker) │
└───┬────┘  └────┬─────┘
    │            │
    └─────┬──────┘
          ▼
┌───────────────────┐
│ ResponseBuilding  │  Event dispatch (priority order):
│ Event             │     60   ErrorPagesHandler
│                   │    100   ServerHeaderHandler
└────────┬──────────┘
         ▼
┌───────────────────┐
│ Brotli            │  compression.rs: maybe_compress()
│ Compression       │  (if Accept-Encoding: br)
└────────┬──────────┘
         ▼
┌───────────────────┐
│ RequestComplete   │  Event dispatch (priority order):
│ Event             │      0   MetricsResponseHandler
│                   │    100   AccessLogHandler
└────────┬──────────┘
         ▼
  Response sent
```

## 逐阶段详解

### 1. TCP 接受与连接建立

`main.rs` 中的接受循环对每个传入连接调用 `listener.accept()`。一个具有 `max_connections` 许可的 `Semaphore` 限制总并发数。每个连接生成一个 Tokio 任务：

```rust
let (stream, remote_addr) = listener.accept().await?;
let permit = semaphore.clone().acquire_owned().await?;
tokio::spawn(async move {
    let _permit = permit;
    server_clone.handle_connection(stream, remote_addr).await;
});
```

在 `Server::handle_connection()`（`src/server/mod.rs`）中，服务器通过 `ConnectionGuard`（RAII — drop 时自动递减）在指标中记录连接，并可选地执行 TLS 握手：

```rust
self.metrics.connection_opened();
let _guard = ConnectionGuard(Arc::clone(&self.metrics));

if let Some(ref acceptor) = self.tls_acceptor {
    let tls_stream = acceptor.accept(stream).await?;
    // ... 通过 TLS 服务
} else {
    // ... 通过明文服务
}
```

### 2. HTTP 解析

`hyper-util` 的 `auto::Builder` 处理 HTTP/1.1 和 HTTP/2 协议检测。`header_read_timeout` 用于防御慢头攻击（需要在 builder 上设置 `TokioTimer`）。builder 调用 `service_fn`，对连接上的每个 HTTP 请求调用 `handle_request()`。

### 3. 请求分解

在 `handle_request()` 开始时，请求被拆分为 parts 和 body：

```rust
let start = Instant::now();
let (parts, body) = req.into_parts();
```

此处通过 `is_some_and(compression::accepts_brotli)` 进行非分配检查以判断 `Accept-Encoding` 头是否支持 Brotli。

### 4. RequestReceived 事件

第一次事件分发按优先级顺序运行三个处理器：

| 优先级 | 处理器 | 操作 |
|---|---|---|
| -100 | `RequestIdGenerator` | 生成 `{timestamp_hex:08x}{counter:08x}`（16 位十六进制字符）或保留传入的 `X-Request-ID` |
| -50 | `RateLimitHandler` | 检查每 IP 滑动窗口；超出限制时设置 `early_response` |
| 0 | `MetricsRequestHandler` | 调用 `metrics.record_request(&method)` |

`RequestReceived` 事件包含一个 `metadata: Vec<(String, String)>` 字段，插件处理器可用于附加键值数据。

请求 ID 通过 `std::mem::take` 提取（零拷贝移动，无克隆）：

```rust
let request_id = std::mem::take(&mut received_event.request_id);
```

### 5. 提前响应短路

如果任何处理器在 `RequestReceived` 事件上设置了 `early_response`（限流器设置 429 响应），管道直接跳到 `RequestComplete`：

```rust
if let Some(early_resp) = received_event.early_response {
    // 为指标/日志分发 RequestComplete，然后返回
    return Ok(early_resp);
}
```

这确保被限流的请求仍然被计入指标并出现在访问日志中。方法和路径字符串仅在提前路径中分配（从第 3 步延迟到此处，以避免 `early_response` 未设置时的不必要分配）。

### 6. 插件 Cookie 剥离与字符串分配

在提前响应检查之后，管道：

1. 从事件中获取请求 parts
2. 分配方法和路径字符串（`method_str`、`path_str`）— 延迟到此时以避免请求被短路时的分配
3. 调用 `plugin::cookies::strip_plugin_cookies()` 从请求头中移除插件内部 cookie，然后再转发给 PHP

### 7. 请求超时

如果配置了 `REQUEST_TIMEOUT_SECS`（非零），则剩余管道被包装在 `tokio::time::timeout` 中。超时触发时返回 504 Gateway Timeout：

```rust
match tokio::time::timeout(server.request_timeout, dispatch_request(...)).await {
    Ok(inner_result) => inner_result,
    Err(_) => Ok(Response::builder()
        .status(StatusCode::GATEWAY_TIMEOUT)
        .body(full_body(Bytes::from_static(b"504 Gateway Timeout")))
        .unwrap()),
}
```

### 8. 路由解析

`src/server/routing.rs` 中的 `RouteConfig::resolve_request()` 将 URI 路径解析为三种结果之一：

| 结果 | 含义 |
|---|---|
| `Serve(PathBuf)` | 从磁盘提供静态文件 |
| `Execute(PathBuf)` | 发送到 PHP 工作线程 |
| `NotFound` | 返回 404 |

路由过程：

1. 对 URI 进行百分号解码
2. 清理路径（移除 `..` 和 `.` 段）
3. 在框架模式下阻止直接访问 `INDEX_FILE` 和 `.php` 文件
4. 在文件缓存中检查是否存在
5. 如已配置则回退到 `INDEX_FILE`（框架/SPA 模式）
6. 验证解析后的路径未通过符号链接逃出文档根目录

### 9a. 静态文件服务

对于 `Serve` 结果，`static_file::serve()` 从磁盘读取文件（使用文件缓存获取元数据），检测 MIME 类型，并返回带有适当 `Content-Type` 和 `Content-Length` 头的响应。

### 9b. PHP 执行

对于 `Execute` 结果，请求体以 **10 MB 限制**（`MAX_POST_BODY`）收集。仅对 POST、PUT 和 PATCH 请求进行体收集 — 所有其他方法（GET、HEAD、DELETE 等）接收空 `Bytes` 而不从 body 流读取。如果体超过此限制，立即返回 413 Payload Too Large 响应。

```rust
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

let limited = Limited::new(body, MAX_POST_BODY);
let body_bytes = match BodyExt::collect(limited).await {
    Ok(collected) => collected.to_bytes(),
    Err(e) => {
        if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
            return Ok(Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(...)?);
        }
        return Err(e);
    }
};
```

构造 `ScriptRequest` 并发送到执行器：

```rust
let script_request = ScriptRequest {
    request_id: request_id.to_string(),
    script_path,
    method: parts.method,
    uri: parts.uri,
    query_string,
    headers: parts.headers,
    body: body_bytes,
    remote_addr,
    document_root: ctx.route_config.document_root_arc(),
};

ctx.metrics.request_queued();
let response_rx = ctx.executor.execute(script_request);
```

Tokio 任务等待 `oneshot::Receiver`。当 PHP 工作线程完成时，它发回包含状态码、头、体和执行时间的 `ScriptResponse`。如果工作线程 channel 断开，返回 500 错误并调用 `metrics.request_dropped()`。

### 10. ResponseBuilding 事件

在响应构建完成后（无论是静态文件服务还是 PHP 执行），`ResponseBuilding` 事件触发：

| 优先级 | 处理器 | 操作 |
|---|---|---|
| 60 | `ErrorPagesHandler` | 对状态码 >= 400 的响应，用自定义 HTML 页面替换响应体 |
| 100 | `ServerHeaderHandler` | 添加 `Server: OxPHP/{version}` 和 `X-Request-ID` 头 |

这是整个管道中 `request_id` 唯一一次被克隆的地方，因为在 `RequestComplete` 事件中还需要用到它。

### 11. Brotli 压缩

如果客户端发送了 `Accept-Encoding: br` 且压缩已启用，`compression::maybe_compress()` 在 ResponseBuilding 事件之后运行：

- 检查内容类型是否可压缩（text/html、application/json 等）
- 跳过已有 `Content-Encoding` 的响应
- 跳过小于 256 字节或大于 3 MB 的响应体
- 使用 Brotli 质量 4、窗口大小 20 进行压缩
- 仅在压缩结果确实更小时才使用压缩版本
- 更新 `Content-Encoding`、`Content-Length`，并添加 `Vary: Accept-Encoding`

### 12. RequestComplete 事件

最终事件携带完整的请求元数据：

```rust
let mut complete_event = RequestComplete {
    request_id,    // 移动，无克隆
    method,        // http::Method（移动，无克隆）
    path: path_str,
    status,
    duration: elapsed,
    remote_addr,
};
```

| 优先级 | 处理器 | 操作 |
|---|---|---|
| 0 | `MetricsResponseHandler` | 调用 `metrics.record_response(status, duration)` |
| 100 | `AccessLogHandler` | 通过 `tracing::info!` 输出结构化 JSON 日志条目 |

### 13. 响应交付

`Ok(response)` 返回给 hyper-util，后者将其序列化到网络。对于 keep-alive 连接，`service_fn` 闭包会在同一连接上为下一个请求再次被调用。

## 错误处理

各阶段的错误产生相应的 HTTP 状态码：

| 错误 | 状态码 | 来源 |
|---|---|---|
| 被限流 | 429 | `RateLimitHandler` 通过提前响应 |
| 请求体过大 | 413 | `Limited` 体收集 |
| 请求超时 | 504 | `tokio::time::timeout` |
| PHP 工作线程错误 | 500 | oneshot channel 断开 |
| 队列已满 | 503 | `SapiExecutor::execute()` 通过 `try_send` |
| 文件未找到 | 404 | 路由解析 |
| 内部错误 | 500 | `handle_request` 中的兜底处理 |

## 分配预算

管道设计为最小化每请求分配：

- 整个管道中 `request_id` **零克隆**（使用 `std::mem::take`）
- 在 `ResponseBuilding` 事件处 `request_id` **1 次克隆**（需要在 `RequestComplete` 中复用）
- `method`（`http::Method`）和 `path_str` **零克隆**（在管道中移动传递）
- 方法和路径字符串**延迟**到提前响应检查之后 — 被限流的请求完全避免了分配
- `Accept-Encoding` 通过非分配的 `is_some_and` 调用检查
- `RouteConfig` 使用预计算的根路径 `/` 索引路径，避免每次请求都执行 `PathBuf::join`

## 另请参阅

- [架构概览](./overview.md) — 组件全景与高层数据流
- [事件系统](./event-system.md) — 事件类型、优先级与处理器注册
- [工作线程池](./worker-pool.md) — PHP 工作线程如何处理 `ScriptRequest`
- [SAPI 与 Bridge](./sapi-bridge.md) — PHP 工作线程内部执行流程
- [路由](../features/routing.md) — 三种路由模式与路径清理
- [压缩](../features/compression.md) — Brotli 压缩配置
- [超时](../features/timeouts.md) — 请求与头读取超时行为
- [限流](../features/rate-limiting.md) — 每 IP 限流与 429 响应
