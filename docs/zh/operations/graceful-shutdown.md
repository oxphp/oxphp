---
title: 优雅关闭
description: OxPHP 如何处理关闭信号并排空连接
---

OxPHP 实现了优雅关闭机制，确保进行中的请求在进程退出前完成处理。这对于零停机部署、滚动更新和容器编排至关重要。

## 信号处理

OxPHP 监听两个关闭信号：

| 信号 | 来源 | 行为 |
|------|------|------|
| `SIGTERM` | 容器编排器、`kill`、`docker stop` | 启动优雅关闭 |
| `SIGINT` | 终端 Ctrl+C | 启动优雅关闭 |

两个信号触发相同的关闭流程。收到第一个信号后开始排空过程。服务器不需要第二个信号来强制退出。

## 关闭流程

收到关闭信号后，信号处理器会派生一个任务，与仍在运行的接受循环并发执行关闭流程。接受循环继续处理新连接，直到检测到关闭标志：

1. **派生关闭任务** --- 派生一个 Tokio 任务，依次调用 `plugin_manager.shutdown_all()` 和 `server.shutdown()`。这两个调用在任务内顺序执行，但任务本身与接受循环并发运行。

2. **关闭插件** --- 在派生的任务中，首先调用 `PluginManager::shutdown_all()`，按初始化时的反向优先级顺序关闭插件。

3. **停止接受连接** --- 同样在派生的任务中，`server.shutdown()` 通过 `AtomicBool` 设置 `shutdown` 标志。接受循环在下一次迭代中检测到此标志后退出。

4. **排空进行中的连接** --- 服务器等待所有活跃连接完成，每 100ms 检查一次。

5. **强制执行排空超时** --- 如果在 `DRAIN_TIMEOUT_SECS` 之后仍有连接活跃，服务器记录警告并继续关闭。剩余连接将被丢弃。

6. **中止内部服务器** --- 健康检查/指标服务器任务被取消。

7. **关闭 PHP 执行器** --- 当 `SapiExecutor` 被销毁时，它关闭请求通道，等待所有工作线程结束，并调用 `php_module_shutdown()`。

8. **退出** --- 进程以状态码 0 退出。

```
收到 SIGTERM
  └── 派生关闭任务（与接受循环并发运行）
        ├── plugin_manager.shutdown_all()（反向优先级顺序）
        └── server.shutdown()（设置 AtomicBool；接受循环在下次检查时退出）
接受循环检测到关闭标志后退出
  ├── 排空循环（等待活跃连接，100ms 轮询）
  │   ├── 全部排空 → "All connections drained"
  │   └── 超时到达 → "Drain timeout reached, forcing shutdown"
  ├── 中止内部服务器
  ├── 销毁执行器（关闭通道，等待工作线程，PHP 关闭）
  └── 退出
```

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `DRAIN_TIMEOUT_SECS` | `30` | 等待进行中连接完成的最大秒数 |
| `MAX_CONNECTIONS` | `10000` | 最大并发连接数（由 Tokio 信号量强制执行） |

### 选择排空超时

将 `DRAIN_TIMEOUT_SECS` 设置为可以容纳最慢预期请求的值：

- **API 服务器**（快速响应）：`10`--`15` 秒
- **应用程序**（文件上传或长查询）：`30`--`60` 秒
- **批处理**端点：匹配最长预期操作时间

在 Kubernetes 中，将 `DRAIN_TIMEOUT_SECS` 设置为小于 Pod 的 `terminationGracePeriodSeconds`，以确保排空在 kubelet 发送 `SIGKILL` 之前完成：

```yaml
spec:
  terminationGracePeriodSeconds: 45
  containers:
    - name: oxphp
      env:
        - name: DRAIN_TIMEOUT_SECS
          value: "30"
```

## 连接限制

OxPHP 使用 Tokio `Semaphore` 强制执行 `MAX_CONNECTIONS` 限制。每个被接受的连接获取一个许可。当所有许可被占用时，新连接在 TCP 积压队列中等待，直到有许可被释放。

### ConnectionGuard

活跃连接通过 RAII 守卫模式跟踪。当连接被接受时，`Metrics::connection_opened()` 递增活跃连接计数器。当 `ConnectionGuard` 被销毁时（连接处理器返回或任务被取消），`Metrics::connection_closed()` 自动递减计数器。

```
TCP accept
  ├── 获取信号量许可
  ├── connection_opened()（递增计数器）
  ├── 处理 HTTP 请求（可能通过 keep-alive 处理多个请求）
  └── 销毁 ConnectionGuard → connection_closed()（递减计数器）
      └── 释放许可（释放信号量槽位）
```

这确保计数器始终准确，即使连接因错误或超时而断开。

## PHP 工作线程关闭

当 `SapiExecutor` 被销毁时：

1. 设置全局关闭标志，通知 ScaleManager（如果运行中）停止。
2. 请求通道的发送端被销毁，导致有界通道关闭。
3. 设置每个线程的关闭标志，主线程等待每个工作线程结束。
4. 静态模式的工作线程在下一次 `recv()` 时检测到通道关闭并退出。动态模式的工作线程在下一次 `recv_timeout()` 时检测到通道关闭或关闭标志。
5. 所有工作线程结束后，调用 `php_module_shutdown()`、`sapi_shutdown()` 和 `tsrm_shutdown()` 清理 PHP 引擎。

这意味着正在执行的 PHP 脚本可以完成运行。不会有请求在执行中途被中断。

## Docker

Docker 在执行 `docker stop` 时发送 `SIGTERM`。Docker 的默认停止超时为 10 秒，之后 Docker 会发送 `SIGKILL`。

为了给 OxPHP 足够的排空时间，请增加 Docker 停止超时：

```bash
docker stop --time 45 oxphp
```

或在 `docker-compose.yml` 中设置：

```yaml
services:
  oxphp:
    stop_grace_period: 45s
    environment:
      DRAIN_TIMEOUT_SECS: "30"
```

## Kubernetes

在 Kubernetes 中进行滚动更新时，关闭流程如下：

1. Kubernetes 向 Pod 发送 `SIGTERM`。
2. Pod 从 Service 的端点列表中移除（就绪探针开始失败）。
3. OxPHP 排空进行中的连接。
4. 如果 Pod 在 `terminationGracePeriodSeconds` 之后仍在运行，Kubernetes 发送 `SIGKILL`。

### Pre-Stop Hook

如果你的服务从外部负载均衡器接收流量，而端点变更传播较慢，可以添加 pre-stop hook 来延迟关闭：

```yaml
lifecycle:
  preStop:
    exec:
      command: ["sleep", "5"]
```

这给负载均衡器足够的时间在 OxPHP 停止接受连接之前将 Pod 从目标列表中移除。

## 监控关闭过程

服务器在关闭过程中记录结构化 JSON 日志消息：

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"INFO","message":"Draining in-flight connections","active_connections":3}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

如果排空超时在所有连接完成之前到达：

```json
{"level":"WARN","message":"Drain timeout reached, forcing shutdown","remaining_connections":1}
```

你可以利用这些日志消息设置告警，如果服务器经常触发排空超时，可能表明需要增大 `DRAIN_TIMEOUT_SECS` 或调查长时间运行的请求。

## 另请参阅

- [配置](configuration.md) --- `DRAIN_TIMEOUT_SECS`、`MAX_CONNECTIONS` 及其他环境变量
- [健康检查](health-checks.md) --- 就绪探针如何与优雅关闭交互
- [指标](metrics.md) --- `oxphp_active_connections` 在排空期间跟踪连接数
- [工作池](/architecture/worker-pool.md) --- PHP 工作线程如何关闭和等待结束
