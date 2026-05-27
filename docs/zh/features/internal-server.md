---
title: 内部服务器
description: 用于健康检查、Prometheus 指标、实时配置查看和插件端点的专用 HTTP 服务器。
---

# 内部服务器

OxPHP 在专用端口上运行独立的 HTTP 服务器，用于健康检查、Prometheus 指标和实时配置查看。该服务器与主应用监听器完全隔离——没有 TLS、没有速率限制、没有请求 ID，也没有事件处理。

## 工作原理

1. **设置 `INTERNAL_ADDR`** 为监听地址（例如 `127.0.0.1:9090`）。OxPHP 在该地址上启动第二个 HTTP 监听器。
2. 内部服务器暴露下列内置端点：
   - `/health` — 聚合 JSON 状态
   - `/health/liveness`、`/healthz`（别名）— 存活探针
   - `/health/readiness`、`/readyz`（别名）— 就绪探针
   - `/health/startup`、`/startupz`（别名）— 启动探针
   - `/metrics` — Prometheus 格式输出
   - `/config` — 当前生效的服务器配置 JSON
3. 插件可以在 `/__<plugin>/` 前缀下注册额外的端点。
4. 优雅关闭期间，内部服务器保持可用，直到主服务器完成连接排空。

> **注意：** 内部服务器仅在显式设置 `INTERNAL_ADDR` 时启动。未设置时，健康、指标和配置端点不可通过 HTTP 访问。

## 配置

| 变量 | 默认值 | 说明 |
|----------|---------|-------------|
| `INTERNAL_ADDR` | *（未设置）* | 内部服务器地址。未设置时不启动。示例：`127.0.0.1:9090` |

## 端点

### GET /health

返回 JSON 健康状态。用于 Kubernetes 就绪探针和存活探针。

**200 OK** — 所有系统正常：

```json
{
  "status": "ok",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {
    "otel": "ok"
  }
}
```

**503 Service Unavailable** — 某个插件报告了故障：

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {
    "otel": "failed"
  }
}
```

`plugins` 对象列出每个已加载插件及其健康状态：`"ok"`、`"degraded"` 或 `"failed"`。当任何插件报告 `"failed"` **或** 脚本执行器不健康（`executor_healthy: false`）时，该端点返回 503。`"degraded"` 插件会出现在 JSON 响应体中，但 HTTP 状态码仍为 200。

### GET /metrics

以文本格式（`text/plain; version=0.0.4; charset=utf-8`）返回兼容 Prometheus 的指标。始终返回 200。

```bash
curl http://localhost:9090/metrics
```

```text
# HELP oxphp_requests_total Total HTTP requests
# TYPE oxphp_requests_total counter
oxphp_requests_total 48203
# HELP oxphp_active_connections Current open connections
# TYPE oxphp_active_connections gauge
oxphp_active_connections 7
...
```

有关可用指标的完整列表，参见[指标](../operations/metrics.md)。

### GET /config

以 JSON 格式返回当前生效的服务器配置。始终返回 200。

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:80",
  "document_root": "/var/www/html/public",
  "entry_file": "/var/www/html/public/index.php",
  "log_level": "info",
  "executor_type": "sapi",
  "php_workers": "8",
  "tokio_workers": 4,
  "queue_capacity": 1024,
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "internal_addr": "127.0.0.1:9090",
  "header_timeout_seconds": 5,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode_enabled": false,
  "worker_max_memory_mib": 0,
  "static_max_age": 2592000,
  "static_revalidate": false,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": false,
  "superglobals_enabled": true,
  "trusted_proxies": false,
  "plugins": {}
}
```

出于安全考虑，TLS 证书和密钥文件路径被故意从输出中省略。只暴露 `tls_enabled` 布尔值。

### 插件端点

插件可以在 `/__<plugin_name>/` 前缀下注册自定义端点。例如，名为 `otel` 的插件可以暴露 `/__otel/status`。这些端点仅在对应插件已加载并注册了处理器时才可访问。

任何与内置或插件端点都不匹配的路径将返回 `404 Not Found`。

## Kubernetes 集成

### 就绪探针

使用 `/health` 控制 Kubernetes 是否将流量路由到该 Pod：

```yaml
readinessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 2
  periodSeconds: 5
```

当 `/health` 返回 503 时，Kubernetes 将该 Pod 从 Service 端点列表中移除。端点再次返回 200 后，流量恢复。

### 存活探针

使用同一端点重启无响应的 Pod：

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3
```

### 启动探针

对于启动缓慢的应用（大型框架、重型自动加载器）：

```yaml
startupProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 1
  periodSeconds: 2
  failureThreshold: 15
```

## 安全性

内部服务器没有内置认证或访问控制。保护措施：

- **绑定到 localhost** — 设置 `INTERNAL_ADDR=127.0.0.1:9090`，使端口只能从容器或主机内部访问
- **不作为 Kubernetes Service 暴露** — 将端口声明为 `containerPort`，但不为其创建 Service。Kubernetes 探针直接访问容器端口
- **使用网络策略** — 如果端口必须暴露，请在网络层限制访问

`/config` 端点会暴露运营细节（文档根目录、速率限制、Worker 数量、超时值）。虽然 TLS 路径已被隐藏，但请考虑这些信息是否应该可从 Pod 外部访问。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "80:80"
      - "9090:9090"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
```

在生产环境中，将内部服务器绑定到 localhost 并使用 Kubernetes 探针：

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "80:80"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=127.0.0.1:9090
```

## 故障排除

### 健康、指标和配置端点不可用

`INTERNAL_ADDR` 未设置。

**修复：** 添加该环境变量：

```bash
INTERNAL_ADDR=0.0.0.0:9090
```

### /health 返回 503，但应用正常运行

某个已加载的插件正在报告 `PluginHealth::Failed`。查看健康响应中的 `plugins` 对象，找出哪个插件正在失败：

```bash
curl -s http://localhost:9090/health | jq '.plugins'
```

### 无法从容器外部访问内部服务器

服务器绑定到 `127.0.0.1`，只能从容器内部访问。

**修复：** 改为 `0.0.0.0` 以允许外部访问，或使用直接访问容器的 Kubernetes 探针。

### 指标中未显示 Worker 模式或异步指标

Worker 模式指标仅在启用 Worker 模式时（`WORKER_MODE_ENABLED=true`）出现。异步指标仅在 `ASYNC_WORKERS > 0` 且至少有一个任务被分发或拒绝时出现。

## 参见

- [指标](../operations/metrics.md) -- 完整的 Prometheus 指标参考
- [健康检查](../operations/health-checks.md) -- 详细的健康检查行为说明
- [配置参考](../operations/configuration.md) -- 所有环境变量
- [优雅关闭](../operations/graceful-shutdown.md) -- 关闭序列和内部服务器生命周期
