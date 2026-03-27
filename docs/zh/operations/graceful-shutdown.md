---
title: 优雅关闭
description: OxPHP 如何处理 SIGTERM 和 SIGINT 信号以排空连接并干净退出，包含 Kubernetes 和 Docker 配置示例。
---

# 优雅关闭

OxPHP 处理 `SIGTERM` 和 `SIGINT` 信号，确保进行中的请求在进程退出前完成。这对于容器编排中的零停机部署和滚动更新至关重要。

## 信号处理

OxPHP 响应两种关闭信号：

| 信号 | 来源 | 行为 |
|--------|--------|----------|
| `SIGTERM` | 容器编排器、`docker stop`、`kill` | 启动优雅关闭 |
| `SIGINT` | 终端 Ctrl+C | 启动优雅关闭 |

两个信号触发相同的关闭序列。只需发送第一个信号——服务器会立即开始排空连接。

## 关闭序列

收到关闭信号后，OxPHP 按以下序列执行：

1. **停止接受新连接** — 服务器停止在主端口接受新的 TCP 连接并关闭插件。PHP 工作进程继续运行以处理进行中的请求。
2. **排空进行中的请求** — 允许活跃连接完成处理。服务器每 100ms 检查一次完成状态。内部健康/指标服务器在整个排空期间保持可用，就绪探针可继续正常工作。
3. **执行排空超时** — 如果在 `DRAIN_TIMEOUT_SECONDS` 后连接仍处于活跃状态，服务器会记录警告并继续执行。剩余连接将被强制断开。
4. **关闭异步进程池** — 后台异步任务进程池被停止。
5. **终止内部服务器** — 排空完成后停止健康/指标服务器。
6. **退出** — 进程以状态码 0 退出。

> **注意：** PHP 工作进程在请求队列关闭时隐式关闭。工作线程在完成当前处理中的请求后退出。

## 配置

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `DRAIN_TIMEOUT_SECONDS` | `30` | 强制关闭前等待进行中连接完成的最大秒数 |

根据预期的最慢请求设置 `DRAIN_TIMEOUT_SECONDS`：

- **响应快速的 API 服务器**：`10`–`15` 秒
- **包含文件上传或长查询的应用**：`30`–`60` 秒
- **包含后台处理的工作进程模式**：与最长预期操作时间匹配

## Kubernetes

在 Kubernetes 中，滚动更新期间的关闭流程为：

1. Kubernetes 向 Pod 发送 `SIGTERM`。
2. Pod 从 Service 端点列表中移除。
3. OxPHP 在 `DRAIN_TIMEOUT_SECONDS` 内排空进行中的连接。
4. 如果 Pod 在 `terminationGracePeriodSeconds` 后仍在运行，Kubernetes 发送 `SIGKILL`。

将 `DRAIN_TIMEOUT_SECONDS` 设置为低于 `terminationGracePeriodSeconds` 的值，确保排空在强制终止前完成：

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      terminationGracePeriodSeconds: 45
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:0.2.0
          env:
            - name: DRAIN_TIMEOUT_SECONDS
              value: "30"
```

### 预停钩子

如果您的服务从传播端点变更较慢的外部负载均衡器接收流量，可添加预停钩子以延迟关闭序列：

```yaml
lifecycle:
  preStop:
    exec:
      command: ["sleep", "5"]
```

这为负载均衡器争取时间，使其在 OxPHP 停止接受连接之前将 Pod 从目标列表中移除。

## Docker

运行 `docker stop` 时 Docker 会发送 `SIGTERM`。Docker 默认的停止超时为 10 秒，超时后 Docker 会发送 `SIGKILL`。

为给 OxPHP 足够的排空时间，请增加停止超时：

```bash
docker stop --time 45 my-oxphp-container
```

或在 Compose 文件中设置：

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.2.0
    stop_grace_period: 45s
    environment:
      DRAIN_TIMEOUT_SECONDS: "30"
```

## 日志消息

优雅关闭期间，OxPHP 会输出结构化日志消息供监控：

**排空成功：**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"INFO","message":"Draining in-flight connections","active_connections":3}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

**排空超时：**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"WARN","message":"Drain timeout reached, forcing shutdown","remaining_connections":1}
{"level":"INFO","message":"Server stopped"}
```

如果您频繁看到"Drain timeout reached"警告，请增大 `DRAIN_TIMEOUT_SECONDS` 或使用 `oxphp_request_duration_us` 直方图排查长耗时请求。

## 参见

- [健康检查](health-checks.md) — 就绪探针与关闭排空的交互
- [配置参考](configuration.md) — 所有环境变量，包括 `DRAIN_TIMEOUT_SECONDS`
- [指标](metrics.md) — `oxphp_active_connections` 跟踪排空期间的连接数
