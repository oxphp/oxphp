---
title: 健康检查
description: 用于健康监控和容器编排的内部服务器端点
---

OxPHP 在独立端口上运行一个内部 HTTP 服务器，用于健康检查、指标和配置查看。该服务器与主流量端口隔离，监控流量不会与应用请求竞争。

## 启用内部服务器

设置 `INTERNAL_ADDR` 环境变量以启动内部服务器：

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

未设置此变量时，内部服务器不会启动。

## 端点

### `GET /health`

以 JSON 格式返回服务器健康状态。

```bash
curl http://localhost:9090/health
```

**响应（健康）：**

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

**响应（降级）：**

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": false,
  "plugins": {
    "example_plugin": "failed"
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `status` | `string` | 所有子系统健康时为 `"ok"`，否则为 `"degraded"` |
| `uptime_secs` | `integer` | 服务器启动后经过的秒数 |
| `total_requests` | `integer` | 主端口处理的 HTTP 请求总数 |
| `active_connections` | `integer` | 主端口当前打开的连接数 |
| `executor_healthy` | `boolean` | PHP 工作池是否正在接受请求 |
| `plugins` | `object` | 每个已加载插件的健康状态。值为 `"healthy"` 或 `"failed"` |

**HTTP 状态码：**

| 状态码 | 含义 |
|--------|------|
| `200 OK` | 执行器和所有插件均健康 |
| `503 Service Unavailable` | 执行器或任何插件报告失败状态 |

`executor_healthy` 检查调用 PHP 执行器的 `is_healthy()` 方法。如果工作池已关闭或无法处理请求，此值返回 `false`。此外，如果任何插件报告 `Failed` 健康状态，整体状态为 `"degraded"` 且端点返回 503。

### `GET /metrics`

以 Prometheus 文本格式返回兼容的指标。完整指标参考请参阅[指标](metrics.md)页面。插件可以向此输出贡献额外的指标。

```bash
curl http://localhost:9090/metrics
```

### `GET /config`

以 JSON 格式返回当前服务器配置。敏感值（TLS 密钥路径）已脱敏。插件配置包含在 `plugins` 键下。

```bash
curl http://localhost:9090/config
```

```json
{
  "listen_addr": "0.0.0.0:8080",
  "document_root": "/var/www/html",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_secs": 30,
  "header_timeout_secs": 5,
  "idle_timeout_secs": 60,
  "request_timeout_secs": 120,
  "rate_limit": 100,
  "rate_window": 60,
  "tls_enabled": true,
  "error_pages_dir": "/etc/oxphp/error-pages",
  "compression": true,
  "access_log": true,
  "plugins": {}
}
```

### 插件内部路由

以 `/__` 开头的路径保留给插件定义的内部端点。如果没有插件处理该路径，将返回 `404 Not Found` 响应。

其他任何路径均返回 `404 Not Found`。

## 容器健康检查

### Docker

```yaml
# docker-compose.yml
services:
  oxphp:
    image: oxphp:latest
    environment:
      INTERNAL_ADDR: "127.0.0.1:9090"
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

### Dockerfile HEALTHCHECK

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=5s \
  CMD wget -qO- http://127.0.0.1:9090/health || exit 1
```

### Kubernetes

```yaml
# 存活探针 — 如果服务器无响应则重启 Pod
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3

# 就绪探针 — 如果降级则从 Service 中移除 Pod
readinessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 2
  periodSeconds: 5
  failureThreshold: 2
```

对于 Kubernetes，使用 `executor_healthy` 字段和 HTTP 状态码驱动就绪状态。`503` 响应表示 PHP 工作池或某个插件处于降级状态，应将 Pod 从 Service 的端点列表中移除直到恢复。

## 负载均衡器集成

大多数负载均衡器支持 HTTP 健康检查。将它们指向内部端口：

| 负载均衡器 | 健康检查目标 |
|------------|-------------|
| AWS ALB/NLB | `http://instance:9090/health` |
| HAProxy | `option httpchk GET /health` on port 9090 |
| nginx upstream | `proxy_pass http://backend:9090/health` |
| Traefik | `traefik.http.services.oxphp.loadbalancer.healthcheck.path=/health` |

`/health` 端点非常轻量 --- 它读取原子计数器并调用执行器的 `is_healthy()`。不涉及磁盘 I/O、数据库访问或 PHP 执行。

## 安全注意事项

内部服务器默认绑定到 `127.0.0.1`，仅可从本机访问。如果需要从监控网络访问，请绑定到特定接口：

```bash
# 从监控网络可访问
INTERNAL_ADDR=10.0.1.5:9090
```

**不要**在生产环境中将内部服务器绑定到 `0.0.0.0`，除非它位于防火墙或网络策略后面以限制访问。`/config` 端点会暴露不应公开的运维细节。

## 另请参阅

- [指标](metrics.md) --- Prometheus 兼容指标完整参考
- [配置](configuration.md) --- 所有环境变量及其默认值
- [优雅关闭](graceful-shutdown.md) --- 健康检查如何与关闭排空交互
