---
title: 分布式追踪
description: OxPHP 的 W3C Trace Context 传播、OpenTelemetry 集成与端到端可观测性。
---

# 分布式追踪

OxPHP 支持 W3C Trace Context 传播和 OpenTelemetry（OTel）导出。传入的 `traceparent` 请求头会被解析并延续，trace ID 可通过 `$_SERVER` 在 PHP 中访问，访问日志包含 trace 字段，span 可以导出到 Jaeger、Grafana Tempo、Zipkin 或任何兼容 OTLP 的后端。

## 工作原理

1. **传入请求** — OxPHP 按照 W3C Trace Context 规范读取 `traceparent` 和 `tracestate` 请求头
2. **新 span** — 为当前跳生成新的 span ID，传入的 span ID 成为父级
3. **传播到 PHP** — trace ID 注入到 `$_SERVER['OXPHP_TRACE_ID']`、`$_SERVER['OXPHP_SPAN_ID']` 和 `$_SERVER['OXPHP_PARENT_SPAN_ID']`
4. **访问日志** — 结构化 JSON 日志包含 `trace_id` 和 `span_id` 字段，用于日志关联
5. **响应头** — 更新后的 `traceparent` 请求头（包含 OxPHP 的 span ID）被添加到响应中，使下游服务能够继续追踪
6. **OTel 导出**（可选）— 启用 OTel 插件时，每个请求成为一个通过 OTLP 导出的 span，包含 HTTP 语义约定属性

如果不存在 `traceparent` 请求头，OxPHP 生成新的 trace ID 和 span ID，开始一个全新的追踪。

## 配置

### W3C Trace Context（内置）

| 变量 | 默认值 | 说明 |
|----------|---------|-------------|
| `TRACE_CONTEXT` | `false` | 启用 W3C Trace Context 传播。设置为 `true` 或 `1` |

### OpenTelemetry 插件

OTel 插件是编译时特性（`plugin-otel`）。启用时会自动设置 `TRACE_CONTEXT=true`。

| 变量 | 默认值 | 说明 |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | 启用 OpenTelemetry 插件。设置为 `true` 或 `1` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | 导出协议：`grpc` 或 `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317`（gRPC）或 `http://localhost:4318`（HTTP） | OTLP 收集器端点 |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | 导出超时（毫秒） |
| `OTEL_EXPORTER_OTLP_HEADERS` | *（未设置）* | 认证请求头：`key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | 导出 span 中的服务名称 |
| `OTEL_SERVICE_VERSION` | *（未设置）* | 服务版本属性 |
| `OTEL_RESOURCE_ATTRIBUTES` | *（未设置）* | 额外资源属性：`env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | 采样策略：`always_on`、`always_off`、`traceidratio`、`parentbased_always_on`、`parentbased_always_off`、`parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | 基于比例的采样器的采样率（0.0–1.0） |

## PHP 中的 Trace Context

`TRACE_CONTEXT=true` 时，PHP 脚本中有三个 `$_SERVER` 变量可用：

| 变量 | 说明 | 示例 |
|----------|-------------|---------|
| `OXPHP_TRACE_ID` | W3C trace ID（32 个十六进制字符） | `4bf92f3577b34da6a3ce929d0e0e4736` |
| `OXPHP_SPAN_ID` | OxPHP 为当前请求生成的 span ID（16 个十六进制字符） | `00f067aa0ba902b7` |
| `OXPHP_PARENT_SPAN_ID` | 传入的父级 span ID（16 个十六进制字符，新追踪时为空） | `a3ce929d0e0e4736` |

使用这些变量向下游服务传播 trace context：

```php
<?php
$traceId  = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId   = $_SERVER['OXPHP_SPAN_ID'] ?? '';

if ($traceId) {
    // 为下游调用构建 traceparent 请求头
    $traceparent = "00-{$traceId}-{$spanId}-01";

    $response = file_get_contents('https://api.example.com/data', false,
        stream_context_create([
            'http' => [
                'header' => "traceparent: {$traceparent}\r\n",
            ],
        ])
    );
}
```

### 使用 Guzzle

```php
<?php
$traceId = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId  = $_SERVER['OXPHP_SPAN_ID'] ?? '';

$client = new \GuzzleHttp\Client();
$response = $client->get('https://api.example.com/users', [
    'headers' => [
        'traceparent' => "00-{$traceId}-{$spanId}-01",
    ],
]);
```

## 访问日志关联

启用 trace context 时，结构化 JSON 访问日志包含 `trace_id` 和 `span_id` 字段：

```json
{
  "timestamp": "2026-03-23T10:15:30.123Z",
  "level": "INFO",
  "target": "access_log",
  "request_id": "4bf92f35-00f067aa",
  "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
  "span_id": "00f067aa0ba902b7",
  "method": "GET",
  "path": "/api/users",
  "status": 200,
  "duration_us": 1523
}
```

这使得可以在日志聚合系统（Loki、Elasticsearch、Splunk、CloudWatch）中按 trace ID 搜索，找到分布式追踪的所有日志条目。

## 响应头

OxPHP 在每个响应中添加 `traceparent` 请求头，包含 OxPHP 自身的 span ID：

```http
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
```

如果传入请求包含 `tracestate` 请求头，它也会在响应中转发。

## OpenTelemetry 集成

启用 OTel 插件时，每个 HTTP 请求成为一个通过 OTLP 导出到追踪后端的 span。

### Span 属性

导出的 span 包含标准 HTTP 语义约定属性：

| 属性 | 说明 |
|-----------|-------------|
| `http.request.method` | HTTP 方法（GET、POST 等） |
| `url.path` | 请求路径 |
| `http.response.status_code` | 响应状态码 |
| `client.address` | 客户端 IP 地址 |
| `server.address` | 服务器监听地址 |
| `oxphp.request_id` | OxPHP 请求 ID |
| `http.request.body.size` | 请求体大小（字节，非零时存在） |
| `http.response.body.size` | 响应体大小（字节，非零时存在） |

5xx 响应会被标记为错误 span。

### OTel 与请求 ID

OTel 插件激活时，请求 ID 从 trace context 派生：trace ID 的前 16 个字符和 span ID 的前 8 个字符，以连字符分隔。这会出现在日志、`X-Request-ID` 响应头以及 PHP 中的 `oxphp_request_id()` 中。

## Docker 示例

### 仅 Trace Context

在无外部后端的情况下启用 W3C trace 传播：

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "80:80"
    environment:
      - TRACE_CONTEXT=true
      - INTERNAL_ADDR=0.0.0.0:9090
```

### 使用 Jaeger

以 Jaeger 为追踪后端的完整可观测性栈：

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "80:80"
    environment:
      - OTEL_ENABLED=true
      - OTEL_EXPORTER_OTLP_ENDPOINT=http://jaeger:4317
      - OTEL_SERVICE_NAME=my-app
      - OTEL_SERVICE_VERSION=1.0.0
      - OTEL_RESOURCE_ATTRIBUTES=env=production
      - INTERNAL_ADDR=0.0.0.0:9090

  jaeger:
    image: jaegertracing/all-in-one:latest
    ports:
      - "16686:16686"   # Jaeger UI
      - "4317:4317"     # OTLP gRPC
```

### 使用 Grafana Tempo

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "80:80"
    environment:
      - OTEL_ENABLED=true
      - OTEL_EXPORTER_OTLP_ENDPOINT=http://tempo:4317
      - OTEL_SERVICE_NAME=my-app

  tempo:
    image: grafana/tempo:latest
    ports:
      - "4317:4317"

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
```

## 可观测性三支柱

OxPHP 提供协同工作的三个可观测性支柱：

| 支柱 | 特性 | 关联方式 |
|--------|---------|-------------|
| **指标** | `/metrics` 处的 Prometheus 计数器和直方图 | 聚合性能数据 |
| **日志** | 通过 `ACCESS_LOG` 输出的结构化 JSON 访问日志 | 每个请求的详情，可按 `trace_id` 搜索 |
| **追踪** | W3C Trace Context + OTLP 导出 | 端到端分布式请求流 |

三者共享相同的 `trace_id` 和 `request_id`，实现从 Grafana 仪表板告警 → Tempo 追踪 → 单个请求的 Loki 日志行的无缝下钻。

## 故障排除

### 响应中没有 trace 请求头

`TRACE_CONTEXT` 未启用。

**修复：** 设置 `TRACE_CONTEXT=true`，或通过 `OTEL_ENABLED=true` 启用 OTel 插件（会自动启用 trace context）。

### `$_SERVER` 中的 trace 变量为空

trace context 已禁用，或变量在 OxPHP 之外被检查。

**检查：** `OXPHP_TRACE_ID`、`OXPHP_SPAN_ID` 和 `OXPHP_PARENT_SPAN_ID` 变量仅在 `TRACE_CONTEXT=true` 且请求由 OxPHP 提供服务时存在。使用以下方式测试：

```php
<?php
echo $_SERVER['OXPHP_TRACE_ID'] ?? 'trace context not enabled';
```

### Jaeger/Tempo 中没有 span 出现

**检查：** 验证 OTLP 端点是否可从 OxPHP 容器访问：

```bash
docker compose exec app curl -v http://jaeger:4317
```

**检查：** 验证插件是否已启用：

```bash
curl -s http://localhost:9090/config | jq '.plugins'
```

**修复：** 确保 `OTEL_ENABLED=true`，且 `OTEL_EXPORTER_OTLP_ENDPOINT` 指向正确的收集器地址。

### 生产环境中采样量过高

在高流量环境中导出每个 span 代价高昂。

**修复：** 降低采样率：

```bash
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1   # 采样 10% 的追踪
```

基于父级的采样意味着：如果传入请求携带已采样的追踪，则无论采样率如何都会被采样。在 OxPHP 发起的新追踪按配置的采样率进行采样。

## 参见

- [访问日志](access-logging.md) -- 包含 trace 字段的结构化 JSON 日志
- [请求 ID](request-ids.md) -- 请求 ID 与 trace context 的交互方式
- [指标](../operations/metrics.md) -- Prometheus 指标参考
- [健康检查](../operations/health-checks.md) -- 显示 trace context 状态的 `/config` 端点
- [配置参考](../operations/configuration.md) -- 所有环境变量
