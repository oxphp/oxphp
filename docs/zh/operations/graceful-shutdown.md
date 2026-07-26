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

1. **停止接受新连接** — 服务器停止在主端口接受新的 TCP 连接。PHP 工作进程继续运行以处理进行中的请求。
2. **收拢存活连接** — HTTP/2 客户端收到 `GOAWAY` 帧，空闲的 HTTP/1.1 keep-alive 连接被关闭，客户端会转向健康实例，而不是继续向正在关闭的实例复用新请求。打开的流会被立即干净地结束——任何已经开始 flush 输出 chunked 内容的响应都算在内，有限下载与 SSE 同样处理——参见 [Server-Sent Events](../features/sse.md#关闭时的行为)。
3. **排空进行中的请求** — 普通活跃请求不受打扰，正常完成并返回完整响应。服务器每 100ms 检查一次完成状态。内部健康/指标服务器在整个排空期间保持可用，就绪探针可继续正常工作。 请求在 `oxphp_finish_request()` 之后继续执行的工作同样计为进行中，尽管其连接已经关闭。
4. **执行排空截止时间** — 在 `DRAIN_TIMEOUT_SECONDS` 到期时仍在运行的请求会被取消（其 `register_shutdown_function()` 回调仍会执行），并获得约 2 秒时间收尾，之后服务器继续关闭流程。
5. **刷新插件** — 排空窗口期间累积的访问日志条目和 APM span 被刷出。
6. **关闭异步进程池** — 后台异步任务进程池被停止。
7. **终止内部服务器** — 排空完成后停止健康/指标服务器。
8. **退出** — 进程以状态码 0 退出。

> **注意：** PHP 工作线程不会在流程开始时停止——它们在最后阶段才收到退出信号并被 join，此时排空已经结束、进程正在退出。正因如此，即使已经没有任何客户端连接，排空也必须把仍在工作线程上执行的工作计算在内。

## 配置

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `DRAIN_TIMEOUT_SECONDS` | `25` | 进行中请求在被取消前可用于完成的最大秒数；截止时间过后进程在约 2 秒内退出。该默认值在 Kubernetes 默认的 30 秒终止宽限期内为截止后的收尾和遥测刷新留出了余量 |

根据预期的最慢请求设置 `DRAIN_TIMEOUT_SECONDS`：

- **响应快速的 API 服务器**：`10`–`15` 秒
- **包含文件上传或长查询的应用**：`30`–`60` 秒
- **包含后台处理的工作进程模式**：与最长预期操作时间匹配

## Kubernetes

在 Kubernetes 中，滚动更新期间的关闭流程为：

1. Kubernetes 向 Pod 发送 `SIGTERM`。
2. Pod 从 Service 端点列表中移除。
3. OxPHP 在 `DRAIN_TIMEOUT_SECONDS` 内排空进行中的连接，随后取消掉队的请求，并在约 2 秒内退出。
4. 如果 Pod 在 `terminationGracePeriodSeconds` 后仍在运行，Kubernetes 发送 `SIGKILL`。

将 `terminationGracePeriodSeconds` 设置为高于 `DRAIN_TIMEOUT_SECONDS` + 2 的值，确保排空——包括截止时间后的收尾与遥测刷新——在强制终止前完成：

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      terminationGracePeriodSeconds: 45
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:0.10.0
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
    image: ghcr.io/oxphp/oxphp:0.10.0
    stop_grace_period: 45s
    environment:
      DRAIN_TIMEOUT_SECONDS: "30"
```

## 日志消息

优雅关闭期间，OxPHP 会输出结构化日志消息供监控：

**排空成功：**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"INFO","message":"Draining in-flight connections","active_connections":3,"in_flight_requests":4}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

这两个计数是重叠的，而非互斥：`active_connections` 是存活的客户端连接，`in_flight_requests` 是仍在 PHP 工作线程上执行的请求，持有连接的请求会同时计入两者。通过 `oxphp_finish_request()` 提前结束响应的请求只出现在 `in_flight_requests` 中——它的连接已经关闭，而后台工作仍在继续。

**排空超时：**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"WARN","message":"Drain timeout reached, cancelling in-flight requests","remaining_connections":1,"in_flight_requests":1}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

如果您频繁看到"Drain timeout reached"警告，请增大 `DRAIN_TIMEOUT_SECONDS` 或使用 `oxphp_request_duration_us` 直方图排查长耗时请求。

## 参见

- [健康检查](health-checks.md) — 就绪探针与关闭排空的交互
- [配置参考](configuration.md) — 所有环境变量，包括 `DRAIN_TIMEOUT_SECONDS`
- [指标](metrics.md) — `oxphp_active_connections` 跟踪排空期间的连接数
