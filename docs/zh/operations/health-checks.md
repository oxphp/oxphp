---
title: 健康检查
description: 用于健康监控、Prometheus 指标采集和运行时配置检查的内部服务器端点。
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

## GET /health

以 JSON 格式返回服务器健康状态。此端点可用于就绪探针和存活探针。

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
|-------|------|-------------|
| `status` | string | 所有子系统健康时为 `"ok"`，否则为 `"degraded"` |
| `uptime_secs` | integer | 服务器启动后的运行秒数 |
| `total_requests` | integer | 主端口处理的 HTTP 请求总数 |
| `active_connections` | integer | 主端口当前打开的连接数 |
| `executor_healthy` | boolean | PHP 工作进程池是否正在接受请求 |

**HTTP 状态码：**

| 状态码 | 含义 |
|------|---------|
| `200 OK` | 所有子系统均健康 |
| `503 Service Unavailable` | PHP 工作进程池降级或不可用，或某个插件报告故障 |

`/health` 端点非常轻量——它仅读取内存计数器，不涉及磁盘 I/O、数据库访问或 PHP 执行。

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
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode": false,
  "worker_file": null,
  "worker_max_requests": 0,
  "worker_max_memory_mib": 0,
  "static_cache_ttl": 2592000,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": false,
  "plugins": {}
}
```

> **注意：** TLS 证书和私钥路径已省略。`tls_enabled` 布尔值表示 TLS 是否已启用。

## Kubernetes 集成

将 `/health` 端点同时用于存活探针和就绪探针：

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:0.2.0
          env:
            - name: INTERNAL_ADDR
              value: "0.0.0.0:9090"
          ports:
            - containerPort: 8080
            - containerPort: 9090
          livenessProbe:
            httpGet:
              path: /health
              port: 9090
            initialDelaySeconds: 5
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health
              port: 9090
            initialDelaySeconds: 2
            periodSeconds: 5
            failureThreshold: 2
```

`/health` 返回 `503` 时，Kubernetes 会根据探针类型将 Pod 从 Service 端点列表中移除（就绪探针）或重启它（存活探针）。

## Docker Compose 健康检查

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.2.0
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
