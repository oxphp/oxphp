---
title: 分布式追踪
description: W3C Trace Context 传播与 OpenTelemetry Span 导出
---

OxPHP 通过两个层级支持分布式追踪：W3C Trace Context 传播（内置，零依赖）和 OpenTelemetry Span 导出（通过 `plugin-otel` 特性启用）。

## 架构

### 第一层：W3C Trace Context（内置）

当 `TRACE_CONTEXT=true` 时，OxPHP：

1. 按照 [W3C Trace Context](https://www.w3.org/TR/trace-context/) 规范解析传入的 `traceparent` 和 `tracestate` 头
2. 当没有 `traceparent` 时自动生成新的 trace ID 和 span ID
3. 在 HTTP 响应中注入 `traceparent` 和 `tracestate`
4. 通过 `$_SERVER` 超全局变量向 PHP 暴露 trace ID
5. 在访问日志条目中包含 `trace_id` 和 `span_id`

此层无外部依赖，不会向构建中添加任何第三方 crate。

### 第二层：OpenTelemetry 导出（`plugin-otel` 特性）

使用 `--features plugin-otel` 构建并设置 `OTEL_ENABLED=true` 时，OxPHP 还会：

1. 为每个请求创建符合语义化 HTTP 约定的 OpenTelemetry Span
2. 通过 gRPC 或 HTTP/protobuf 将 Span 导出到 OTLP 收集器
3. 支持可配置的采样策略、资源属性和认证头

启用 `OTEL_ENABLED` 会自动设置 `TRACE_CONTEXT=true`。

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `TRACE_CONTEXT` | `false` | 启用 W3C Trace Context 传播 |
| `OTEL_ENABLED` | `false` | 启用 OpenTelemetry Span 导出（隐含 `TRACE_CONTEXT=true`） |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP 收集器端点 |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | 导出协议：`grpc` 或 `http/protobuf` |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | 导出超时（毫秒） |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(无)* | 认证头（`key=value,key=value`） |
| `OTEL_SERVICE_NAME` | `oxphp` | 导出追踪中的服务名称 |
| `OTEL_SERVICE_VERSION` | *(无)* | 导出追踪中的服务版本 |
| `OTEL_RESOURCE_ATTRIBUTES` | *(无)* | 资源属性（`key=value,key=value`） |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | 采样策略 |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | 采样比率（0.0-1.0） |

参见[配置](/operations/configuration.md)获取完整的环境变量参考。

## PHP 集成

当 trace context 激活时，每个请求都会填充四个 `$_SERVER` 变量：

| 变量 | 说明 |
|------|------|
| `$_SERVER['OXPHP_TRACE_ID']` | W3C trace ID（32 个十六进制字符） |
| `$_SERVER['OXPHP_SPAN_ID']` | 当前 span ID（16 个十六进制字符） |
| `$_SERVER['OXPHP_PARENT_SPAN_ID']` | 来自传入 `traceparent` 的父 span ID（无父级时为空） |
| `$_SERVER['HTTP_TRACEPARENT']` | 原始 `traceparent` 头值 |

### 日志关联

在应用日志中使用 trace ID，将 PHP 层级的日志与分布式追踪关联：

```php
<?php
$traceId = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId  = $_SERVER['OXPHP_SPAN_ID'] ?? '';

error_log(json_encode([
    'trace_id' => $traceId,
    'span_id'  => $spanId,
    'message'  => 'Processing payment',
    'order_id' => $orderId,
]));
```

### 下游传播

发起出站 HTTP 调用时，转发 `traceparent` 头以维持追踪链：

```php
<?php
$traceId  = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId   = $_SERVER['OXPHP_SPAN_ID'] ?? '';

// 为下游调用生成新的 span ID
$childSpanId = bin2hex(random_bytes(8));
$traceparent = "00-{$traceId}-{$childSpanId}-01";

$ch = curl_init('https://api.internal/orders');
curl_setopt($ch, CURLOPT_HTTPHEADER, [
    "traceparent: {$traceparent}",
]);
curl_exec($ch);
```

## 访问日志关联

当 `TRACE_CONTEXT` 启用时，每条访问日志条目都包含 `trace_id` 和 `span_id`：

```json
{
  "timestamp": "2026-02-11T12:34:56.789Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc00000042",
    "trace_id": "4bf92f3577b16e8264cabd64a999f321",
    "span_id": "a1b2c3d4e5f6a7b8",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1234,
    "remote_addr": "10.0.0.1:54321",
    "message": "request completed"
  }
}
```

当 `TRACE_CONTEXT` 禁用时，这些字段将被省略。

## 快速开始

### Jaeger（本地开发）

```yaml
services:
  jaeger:
    image: jaegertracing/all-in-one:latest
    ports:
      - "16686:16686"   # Jaeger UI
      - "4317:4317"     # OTLP gRPC

  oxphp:
    image: oxphp:latest
    environment:
      OTEL_ENABLED: "true"
      OTEL_EXPORTER_OTLP_ENDPOINT: "http://jaeger:4317"
      OTEL_SERVICE_NAME: "my-app"
    ports:
      - "8080:8080"
```

打开 `http://localhost:16686` 查看追踪数据。

### Datadog

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://datadog-agent:4317
OTEL_SERVICE_NAME=my-app
OTEL_RESOURCE_ATTRIBUTES=deployment.environment=production
```

Datadog Agent 在配置 `DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT` 后接受端口 4317 上的 OTLP。

### New Relic

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=https://otlp.nr-data.net:4317
OTEL_EXPORTER_OTLP_HEADERS=api-key=YOUR_INGEST_LICENSE_KEY
OTEL_SERVICE_NAME=my-app
```

### Grafana Tempo

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://tempo:4317
OTEL_SERVICE_NAME=my-app
```

对于 Grafana Cloud，使用 HTTPS 端点并附带认证头：

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=https://tempo-us-central1.grafana.net:443
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_HEADERS=Authorization=Basic YOUR_BASE64_CREDENTIALS
```

## 采样

`OTEL_TRACES_SAMPLER` 变量控制哪些请求生成 Span：

| 采样器 | 行为 |
|--------|------|
| `always_on` | 导出每个请求 |
| `always_off` | 不导出任何请求（trace context 仍然传播） |
| `traceidratio` | 基于 trace ID 哈希导出一定百分比的请求 |
| `parentbased_traceidratio` | 遵循父级的采样决策；按比率采样根 Span |

使用 `OTEL_TRACES_SAMPLER_ARG` 设置基于比率的采样器的比率。例如，采样 10% 的根追踪：

```bash
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1
```

`parentbased_traceidratio` 采样器（默认）推荐在生产环境中使用。它遵循上游的采样决策，同时对本地发起的追踪应用比率。

## 另请参阅

- [请求 ID](request-ids.md) -- 当 OTel 激活时的追踪衍生请求 ID
- [访问日志](access-logging.md) -- 日志条目中的 `trace_id` 和 `span_id` 字段
- [请求生命周期](/architecture/request-lifecycle.md) -- 事件管道中的 TraceContextHandler
- [配置](/operations/configuration.md) -- 完整环境变量参考
