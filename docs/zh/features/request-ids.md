---
title: 请求 ID
description: 用于追踪和关联的唯一请求标识符
---

OxPHP 处理的每个请求都会获得一个唯一的请求 ID。此 ID 出现在访问日志、错误日志和响应头中，提供一个单一值来关联请求在所有层级中的处理过程。

## 配置

请求 ID 生成始终启用，无需配置。

## 工作原理

`RequestIdGenerator` 事件处理器以优先级 **-100**（最低优先级值，意味着最先运行）运行。它要么保留传入的请求 ID，要么生成新的。

### 透传

如果传入请求包含 `X-Request-ID` 头，其值将原样使用。这允许上游负载均衡器或 API 网关分配的请求 ID 通过 OxPHP 传播。

### 生成

当没有 `X-Request-ID` 头时，OxPHP 以如下格式生成 ID：

```
{timestamp:08x}{counter:08x}
```

- **timestamp**（8 个十六进制字符）：当前 Unix 时间戳（秒），截断为 32 位
- **counter**（8 个十六进制字符）：进程级原子计数器，使用完整的 `u32` 范围

这会生成一个 16 字符的小写十六进制字符串。例如：`67890abc00000042`。

计数器使用 `Relaxed` 内存排序，因为唯一性由原子递增保证 -- 不需要与其他数据建立先行发生关系。

### 响应头

请求 ID 作为 `X-Request-ID` 头包含在每个 HTTP 响应中。这由服务器头处理器在 `ResponseBuilding` 事件期间设置。

## 在 PHP 中访问请求 ID

请求 ID 可通过 OxPHP PHP 扩展提供的 `oxphp_request_id()` 函数在 PHP 中获取：

```php
<?php
$requestId = oxphp_request_id();
header("X-Correlation-ID: $requestId");
error_log("Processing request $requestId");
```

该函数返回与响应头和访问日志中相同的 16 字符十六进制字符串（或透传值）。

## 访问日志关联

每条访问日志条目都包含 `request_id` 字段：

```json
{
  "request_id": "67890abc00000042",
  "method": "GET",
  "path": "/api/users",
  "status": 200,
  "duration_us": 1234,
  "remote_addr": "10.0.0.1:54321"
}
```

您可以按请求 ID 过滤日志，追踪单个请求的完整生命周期，包括引用相同 ID 的 PHP 错误。

## 追踪衍生请求 ID

当 OpenTelemetry 启用时（`OTEL_ENABLED=true`），请求 ID 格式变为追踪衍生格式：

```
{trace_id[0:16]}-{span_id[0:8]}
```

这会生成一个 25 字符的字符串（例如 `4bf92f3577b16e82-a1b2c3d4`），可在 Jaeger、Grafana Tempo 或其他追踪后端中直接关联请求 ID 和分布式追踪。

当 OTel 禁用时，请求 ID 使用标准的 16 字符十六进制格式。

> **注意：** 当 OTel 激活时，客户端提供的 `X-Request-ID` 值将被追踪衍生格式替换，以确保跨服务的一致追踪关联。

## 另请参阅

- [访问日志](access-logging.md) -- 每条日志条目都包含 `request_id` 字段
- [分布式追踪](distributed-tracing.md) -- W3C Trace Context 与 OpenTelemetry 集成
- [速率限制](rate-limiting.md) -- 被限速的响应包含 `X-Request-ID` 头
