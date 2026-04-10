---
title: 访问日志
description: OxPHP 为每个 HTTP 请求输出结构化 JSON 访问日志，支持记录所有请求或仅记录错误请求两种模式。
---

# 访问日志

OxPHP 为每个 HTTP 请求输出结构化 JSON 访问日志，写入标准输出（stdout）。日志写入是异步的，不会阻塞请求处理。

## 工作原理

1. 设置 `ACCESS_LOG` 后，OxPHP 在每个请求完成后向 stdout 写入一行 JSON 日志。
2. 日志写入通过后台写入线程进行缓冲，不会阻塞请求处理流水线。
3. `ACCESS_LOG=all` 记录所有请求；`ACCESS_LOG=error` 仅记录状态码为 400 及以上的响应。
4. 每行日志包含 `request_id` 字段，可将访问日志条目与应用日志关联起来。
5. 启用 W3C Trace Context 传播时，日志条目还会包含 `trace_id` 和 `span_id` 字段。

## 配置

| 变量 | 默认值 | 说明 |
|----------|---------|-------------|
| `ACCESS_LOG` | *（未设置）* | 访问日志模式。`all` 记录所有请求；`error` 仅记录 4xx 和 5xx 响应。留空或不设置则禁用 |

> **注意：** 仅接受 `all` 和 `error` 两个值。设置无效值时会记录警告并禁用访问日志。

## 日志格式

每条访问日志条目是写入 stdout 的单行 JSON：

```json
{
  "timestamp": "2026-02-11T12:34:56.789012Z",
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

启用 W3C Trace Context 时，`trace_id` 和 `span_id` 会与标准字段一起出现：

```json
{
  "timestamp": "2026-02-11T12:34:56.789012Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc12341a2b0042",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "span_id": "00f067aa0ba902b7",
    "method": "POST",
    "path": "/api/orders",
    "status": 201,
    "duration_us": 8421,
    "remote_addr": "10.0.0.1:54322",
    "message": "request completed"
  }
}
```

## 字段说明

| 字段 | 类型 | 说明 |
|-------|------|-------------|
| `request_id` | string | 唯一请求标识符。参见[请求 ID](request-ids.md) |
| `method` | string | HTTP 方法（`GET`、`POST` 等） |
| `path` | string | 请求 URI 路径 |
| `status` | number | HTTP 响应状态码 |
| `duration_us` | number | 请求总处理时间（微秒） |
| `remote_addr` | string | 客户端 IP 地址和端口 |

> 配置 `TRUSTED_PROXIES` 后，`remote_addr` 显示从转发头中提取的真实客户端 IP，而非代理 IP。
| `trace_id` | string | W3C trace ID（仅在 `TRACE_CONTEXT=true` 时存在） |
| `span_id` | string | W3C span ID（仅在 `TRACE_CONTEXT=true` 时存在） |

## 精细化过滤

OxPHP 使用内部日志目标 `access_log` 输出访问日志条目。使用 `RUST_LOG` 变量可独立于其他日志输出过滤访问日志：

```bash
# 抑制通用 info 消息，保留访问日志
RUST_LOG=warn,access_log=info
```

> **注意：** `access_log` 目标仅用于 `RUST_LOG` 过滤，不会出现在 JSON 输出中。要在下游系统中识别访问日志条目，请使用 `"message": "request completed"` 字段及特征字段集（`method`、`path`、`status`、`duration_us`）。

## 故障排除

### 没有访问日志条目出现

`ACCESS_LOG` 未设置或为空时，访问日志处于禁用状态。

**修复：** 将该变量设置为 `all` 或 `error`：

```bash
ACCESS_LOG=all
```

### 访问日志为 `error` 模式，但成功请求缺失

`ACCESS_LOG=error` 仅记录状态码为 400 及以上的响应。这是预期行为——确认该值后，如需记录所有请求，请切换至 `all`。

**检查：** 确认当前生效的设置：

```bash
curl -s http://localhost:9090/config | jq '.access_log'
```

### 日志条目中没有 `trace_id` 和 `span_id`

Trace context 字段仅在启用 W3C Trace Context 传播时存在。

**修复：** 通过以下方式启用：

```bash
TRACE_CONTEXT=true
```

并确保上游客户端或负载均衡器在请求中发送 `traceparent` 请求头。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.2.0
    ports:
      - "80:80"
      - "9090:9090"
    volumes:
      - ./src:/var/www/html:ro
    environment:
      ACCESS_LOG: "all"
      INDEX_FILE: "index.php"
      INTERNAL_ADDR: "0.0.0.0:9090"
```

## 最佳实践

- 在生产环境中使用 `ACCESS_LOG=error`，可在仍然捕获所有失败请求的同时减少日志量。成功请求不会被记录，但状态码 400 及以上的错误始终会被捕获。
- 在应用日志中通过 `oxphp_request_id()` 包含请求 ID，以便将 PHP 层的日志条目与访问日志条目关联起来。
- 使用 Elasticsearch、Loki 或 Datadog 等结构化日志聚合器高效查询和过滤 JSON 日志行。

## 集成

由于日志是 stdout 上的 JSON 行，它们可直接与容器日志驱动和聚合工具集成：

- **Docker** — 通过容器日志驱动自动收集（json-file、fluentd 等）
- **Kubernetes** — 由节点日志代理（Fluentd、Fluent Bit、Filebeat 等）采集
- **systemd** — 作为 systemd 服务运行时，通过 journald 的 stdout 日志捕获

无需 sidecar 或基于文件的日志传输。

## 参见

- [请求 ID](request-ids.md) -- 每条日志条目均包含用于追踪的 `request_id`
- [配置参考](../operations/configuration.md) -- 完整的环境变量参考
