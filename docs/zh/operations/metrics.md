---
title: Prometheus 指标
description: OxPHP 在 /metrics 端点暴露的所有 Prometheus 兼容指标参考，涵盖请求、连接、工作进程和压缩指标。
---

# Prometheus 指标

OxPHP 在内部服务器的 `GET /metrics` 端点以文本展示格式暴露 Prometheus 兼容指标。这些指标涵盖请求吞吐量、响应时间、连接状态、工作进程池健康状况、静态文件缓存、压缩效率和工作进程模式性能。

## 启用指标

设置 `INTERNAL_ADDR` 以启动内部服务器：

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

然后通过 Prometheus 或任何兼容的采集器进行采集：

```bash
curl http://localhost:9090/metrics
```

## 服务器指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_uptime_seconds` | gauge | 服务器进程启动后的运行秒数 |
| `oxphp_requests_total` | counter | 主端口接收的 HTTP 请求总数 |

## 请求指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_requests_by_method_total` | counter | 按 HTTP 方法统计的请求数。标签：`method`（`GET`、`POST`、`PUT`、`DELETE`、`PATCH`、`HEAD`、`OPTIONS`、`CONNECT`、`QUERY`、`OTHER`） |
| `oxphp_responses_by_status_total` | counter | 按状态类别统计的响应数。标签：`status`（`1xx`、`2xx`、`3xx`、`4xx`、`5xx`） |
| `oxphp_request_bytes_total` | counter | 接收的请求体总字节数 |
| `oxphp_response_bytes_total` | counter | 发送的响应体总字节数 |

> **注意：** 仅输出至少有一次记录的方法和状态类别。零计数标签不会输出。

## 请求时长直方图

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_request_duration_us` | histogram | 所有请求（静态文件和 PHP）的端到端请求时长（微秒） |

桶边界（微秒）：`100`、`500`、`1000`、`2500`、`5000`、`10000`、`25000`、`50000`、`100000`、`250000`、`500000`、`1000000`、`+Inf`。

使用此直方图跟踪整体延迟、识别慢速端点以及测量尾部延迟百分位数。

## 连接指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_active_connections` | gauge | 主端口当前打开的 TCP 连接数 |
| `oxphp_pending_requests` | gauge | 当前已分发到 PHP 工作进程的请求数（排队中和处理中） |
| `oxphp_dropped_requests_total` | counter | PHP 工作进程接受请求后失败的请求数 |

## 工作进程池指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_workers_current` | gauge | 当前 PHP 工作线程数 |
| `oxphp_workers_min` | gauge | 最小工作进程数（静态模式下等于当前数量） |
| `oxphp_workers_max` | gauge | 最大工作进程数（静态模式下等于当前数量） |
| `oxphp_workers_idle` | gauge | 当前未处理请求的工作进程数 |
| `oxphp_busy_workers` | gauge | 当前正在处理请求的工作进程数 |
| `oxphp_workers_spawned_total` | counter | 启动以来创建的工作进程总数（含初始工作进程） |
| `oxphp_workers_retired_total` | counter | 因空闲超时退出的工作进程总数（仅动态模式） |

## 队列等待直方图

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_queue_wait_us` | histogram | 请求在队列中等待工作进程拾取的时间（微秒） |

桶边界（微秒）：`50`、`100`、`250`、`500`、`1000`、`2500`、`5000`、`10000`、`50000`、`+Inf`。

队列等待时间过高表明所有工作进程均处于繁忙状态，应增加 `PHP_WORKERS` 或 `QUEUE_CAPACITY`。

## 限流指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_rate_limited_total` | counter | 被限流器拒绝的请求数（返回 429） |

## 静态文件缓存指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_static_cache_hits_total` | counter | 从内存缓存中服务的静态文件请求数 |
| `oxphp_static_cache_misses_total` | counter | 需要磁盘读取的静态文件请求数 |

## 压缩指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_compressed_responses_total` | counter | 使用 Brotli 压缩的响应数 |
| `oxphp_compression_bytes_saved_total` | counter | 压缩节省的总字节数（原始大小减去压缩后大小） |

## 工作进程模式指标

这些指标仅在工作进程模式启用时（即设置了 `WORKER_FILE`）才会输出。

### 全局计数器

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_worker_mode_enabled` | gauge | 工作进程模式启用时始终为 `1` |
| `oxphp_worker_requests_handled_total` | counter | 持久化工作进程处理的总请求数 |
| `oxphp_worker_recycles_total` | counter | 工作进程回收总次数（工作进程退出并重新创建） |
| `oxphp_worker_recycles_by_reason_total` | counter | 按原因统计的回收次数。标签：`reason`（`max_requests`、`max_memory`、`error`） |
| `oxphp_worker_soft_resets_total` | counter | 请求间执行软重置的总次数 |

### 单工作进程指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_worker_memory_bytes` | gauge | 每个工作进程当前的 PHP 堆内存用量。标签：`worker`（槽位索引，如 `"0"`、`"1"`） |
| `oxphp_worker_uptime_seconds` | gauge | 每个工作进程自创建以来的运行秒数。标签：`worker` |
| `oxphp_worker_requests_count` | gauge | 每个工作进程实例处理的请求数。标签：`worker` |

### 工作进程请求时长直方图

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_worker_request_duration_us` | histogram | 工作进程模式下每个请求的 PHP 处理器执行时间（微秒） |

桶边界（微秒）：`100`、`250`、`500`、`1000`、`2500`、`5000`、`10000`、`25000`、`50000`、`+Inf`。

此直方图测量 PHP 处理器回调内部的耗时，不含队列等待时间。可用于识别慢速处理器并跟踪工作进程模式下的尾部延迟。

## 异步进程池指标

这些指标仅在 `ASYNC_WORKERS` 设置为非零值时才会输出。

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_async_tasks_dispatched_total` | counter | 分发到后台进程池的异步任务总数 |
| `oxphp_async_tasks_completed_total` | counter | 成功完成的异步任务数 |
| `oxphp_async_tasks_failed_total` | counter | 抛出异常的异步任务数 |
| `oxphp_async_tasks_cancelled_total` | counter | 被取消的异步任务数 |
| `oxphp_async_tasks_rejected_total` | counter | 因进程池队列已满而被拒绝的异步任务数 |

## Grafana 仪表板技巧

以下 PromQL 查询适用于构建仪表板：

**请求速率（每秒请求数）：**

```text
rate(oxphp_requests_total[5m])
```

**平均响应时间（毫秒）：**

```text
rate(oxphp_request_duration_us_sum[5m])
/ rate(oxphp_requests_total[5m]) / 1000
```

**p99 请求时长（毫秒）：**

```text
histogram_quantile(0.99, rate(oxphp_request_duration_us_bucket[5m])) / 1000
```

**错误率（5xx 响应占比）：**

```text
rate(oxphp_responses_by_status_total{status="5xx"}[5m])
/ rate(oxphp_requests_total[5m]) * 100
```

**工作进程池利用率：**

```text
oxphp_busy_workers / oxphp_workers_current
```

**队列饱和度（每秒丢弃请求数）：**

```text
rate(oxphp_dropped_requests_total[5m])
```

**p99 队列等待时间（微秒）：**

```text
histogram_quantile(0.99, rate(oxphp_queue_wait_us_bucket[5m]))
```

**静态文件缓存命中率：**

```text
rate(oxphp_static_cache_hits_total[5m])
/ (rate(oxphp_static_cache_hits_total[5m]) + rate(oxphp_static_cache_misses_total[5m]))
```

**每秒压缩节省字节数：**

```text
rate(oxphp_compression_bytes_saved_total[5m])
```

**工作进程模式 p99 延迟（微秒）：**

```text
histogram_quantile(0.99, rate(oxphp_worker_request_duration_us_bucket[5m]))
```

**工作进程回收速率（每分钟）：**

```text
rate(oxphp_worker_recycles_total[5m]) * 60
```

**工作进程平均内存用量：**

```text
avg(oxphp_worker_memory_bytes)
```

## Prometheus 采集配置

在 `prometheus.yml` 中添加采集任务：

```yaml
scrape_configs:
  - job_name: "oxphp"
    scrape_interval: 15s
    static_configs:
      - targets: ["oxphp:9090"]
```

用于 Kubernetes 服务发现：

```yaml
scrape_configs:
  - job_name: "oxphp"
    kubernetes_sd_configs:
      - role: pod
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_label_app]
        regex: oxphp
        action: keep
      - source_labels: [__meta_kubernetes_pod_ip]
        target_label: __address__
        replacement: "$1:9090"
```

## 参见

- [健康检查](health-checks.md) — 内部服务器上的 `/health` 和 `/config` 端点
- [配置参考](configuration.md) — 所有环境变量，包括 `INTERNAL_ADDR`
- [优雅关闭](graceful-shutdown.md) — 连接排空如何影响 `oxphp_active_connections`
- [工作进程模式](../features/worker-mode.md) — 持久化工作进程及其暴露的指标
