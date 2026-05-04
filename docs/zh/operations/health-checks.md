---
title: 健康检查
description: 用于健康监控、Kubernetes 探针、Prometheus 指标采集和运行时配置检查的内部服务器端点。
---

# 健康检查

OxPHP 在独立端口上提供内部 HTTP 服务器，用于健康监控、指标采集和配置检查。该服务器与应用流量相互隔离，确保监控操作不会与用户请求竞争资源。

## 配置

设置 `INTERNAL_ADDR` 以启动内部服务器：

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

当 `INTERNAL_ADDR` 未设置时，内部服务器不会启动，健康端点也不可用。

> **注意：** 在生产环境中请绑定到 `127.0.0.1`，除非内部服务器部署在防火墙之后。`/config` 端点会暴露不应公开的运维详情。

## Kubernetes 探针

OxPHP 为每种 Kubernetes 探针类型提供专用端点。每个端点也可通过短别名访问（`/healthz`、`/readyz`、`/startupz`）。

| 端点 | 别名 | 检查内容 | 200 | 503 |
|------|------|----------|-----|-----|
| `/health/liveness` | `/healthz` | 无（能响应即存活） | 始终 | 从不 |
| `/health/readiness` | `/readyz` | 未关闭、executor 健康、无失败插件 | 就绪 | 未就绪 |
| `/health/startup` | `/startupz` | Executor 健康 | 就绪 | 未就绪 |

**Liveness** 始终返回 `200 OK`。如果进程能响应 HTTP 请求，则表明它是存活的。不执行 executor 或插件检查——这可以防止 Kubernetes 因工作进程池的临时问题而重启 Pod。

**Readiness** 在以下情况返回 `503 Service Unavailable`：
- 服务器正在关闭（优雅关闭进行中）
- PHP 工作进程池不健康
- 任何插件报告故障

在优雅关闭期间，readiness 立即返回 `503`，使 Kubernetes 在排空完成前将 Pod 从 Service 端点中移除。

**Startup** 在 executor 尚未就绪时返回 `503 Service Unavailable`。使用此探针可防止在初始化缓慢时过早终止 Pod。

所有探针端点返回 `Content-Type: text/plain`，响应体为探针名称（如 `readiness`）。Kubernetes 只检查 HTTP 状态码。

```bash
# 快速检查
curl -s -o /dev/null -w '%{http_code}' http://localhost:9090/health/readiness
```

## GET /health

以 JSON 格式返回服务器完整健康状态。用于仪表盘和监控系统，而非 Kubernetes 探针。

```bash
curl http://localhost:9090/health
```

**健康响应（200 OK）：**

```json
{
  "status": "ok",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {}
}
```

**降级响应（503 Service Unavailable）：**

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": false,
  "plugins": {}
}
```

| 字段 | 类型 | 描述 |
|------|------|------|
| `status` | string | 所有子系统健康时为 `"ok"`，否则为 `"degraded"` |
| `uptime_secs` | integer | 服务器启动后的运行秒数 |
| `total_requests` | integer | 主端口处理的 HTTP 请求总数 |
| `active_connections` | integer | 主端口当前打开的连接数 |
| `executor_healthy` | boolean | PHP 工作进程池是否正在接受请求 |

## GET /metrics

以文本展示格式返回 Prometheus 兼容指标。完整指标参考请参见 [Prometheus 指标](metrics.md)。

```bash
curl http://localhost:9090/metrics
```

## GET /config

以 JSON 格式返回服务器当前配置。出于安全考虑，TLS 证书和私钥路径会被省略。

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
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode_enabled": false,
  "worker_max_memory_mib": 0,
  "static_cache_ttl": 2592000,
  "static_cache_enabled": true,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": false,
  "superglobals_enabled": true,
  "trusted_proxies": false,
  "plugins": {}
}
```

> **注意：** TLS 证书和私钥路径已省略。`tls_enabled` 布尔值表示 TLS 是否已启用。

## Kubernetes 集成

为每种探针类型使用专用端点：

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:latest
          env:
            - name: INTERNAL_ADDR
              value: "0.0.0.0:9090"
          ports:
            - containerPort: 8080
            - containerPort: 9090
          startupProbe:
            httpGet:
              path: /health/startup
              port: 9090
            initialDelaySeconds: 1
            periodSeconds: 2
            failureThreshold: 15
          livenessProbe:
            httpGet:
              path: /health/liveness
              port: 9090
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health/readiness
              port: 9090
            periodSeconds: 5
            failureThreshold: 2
```

| 探针 | 失败时的效果 |
|------|-------------|
| Startup | Kubernetes 等待——初始化期间不终止 Pod |
| Liveness | Kubernetes 重启 Pod |
| Readiness | Kubernetes 从 Service 端点中移除 Pod（不重启） |

短别名（`/healthz`、`/readyz`、`/startupz`）完全等效，可替代完整路径使用。

## Docker Compose 健康检查

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:latest
    ports:
      - "8080:80"
    environment:
      INTERNAL_ADDR: "127.0.0.1:9090"
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

在配置的重试次数失败后，Docker 会将容器标记为 `unhealthy`，这可能触发重启策略或负载均衡器移除。

## 参见

- [Prometheus 指标](metrics.md) — 所有暴露指标的完整参考
- [优雅关闭](graceful-shutdown.md) — 健康探针与关闭排空的交互
- [配置参考](configuration.md) — 所有环境变量，包括 `INTERNAL_ADDR`
