---
title: 指标
description: 内部服务器暴露的 Prometheus 兼容指标
---

OxPHP 在内部服务器的 `GET /metrics` 端点以 Prometheus 文本格式暴露指标。所有计数器使用无锁原子操作和 `Relaxed` 排序，对请求路径的性能影响降到最低。

## 启用指标

内部服务器运行时即可使用指标。设置 `INTERNAL_ADDR` 环境变量：

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

然后通过 Prometheus 或任何兼容的采集器抓取：

```bash
curl http://localhost:9090/metrics
```

## 指标参考

### 服务器

| 指标 | 类型 | 说明 |
|------|------|------|
| `oxphp_uptime_seconds` | gauge | 服务器进程启动后的秒数 |

### 请求

| 指标 | 类型 | 标签 | 说明 |
|------|------|------|------|
| `oxphp_requests_total` | counter | --- | 主端口接收的 HTTP 请求总数 |
| `oxphp_requests_by_method_total` | counter | `method` | 按 HTTP 方法分类的请求数 |
| `oxphp_responses_by_status_total` | counter | `status` | 按状态码分类的响应数 |
| `oxphp_response_time_us_total` | counter | --- | 累计响应时间（微秒） |

**方法标签：** `GET`、`POST`、`PUT`、`DELETE`、`PATCH`、`HEAD`、`OPTIONS`、`CONNECT`、`OTHER`。

**状态标签：** `1xx`、`2xx`、`3xx`、`4xx`、`5xx`。

只有至少有一个记录事件的方法和状态分类才会输出。零计数标签被省略以保持输出紧凑。

### 连接

| 指标 | 类型 | 说明 |
|------|------|------|
| `oxphp_active_connections` | gauge | 主端口当前打开的 TCP 连接数 |

### PHP 工作队列

| 指标 | 类型 | 说明 |
|------|------|------|
| `oxphp_pending_requests` | gauge | 当前在 PHP 工作队列中等待的请求数 |
| `oxphp_dropped_requests_total` | counter | 因队列满而被 503 拒绝的请求数 |
| `oxphp_busy_workers` | gauge | 当前正在处理请求的工作线程数 |

### PHP 工作池

| 指标 | 类型 | 说明 |
|------|------|------|
| `oxphp_workers_current` | gauge | 池中当前的工作线程数 |
| `oxphp_workers_min` | gauge | 最小工作线程数（静态模式下等于当前值） |
| `oxphp_workers_max` | gauge | 最大工作线程数（静态模式下等于当前值） |
| `oxphp_workers_idle` | gauge | 当前未处理请求的工作线程数（仅动态模式，静态模式为 0） |
| `oxphp_workers_spawned_total` | counter | 自启动以来创建的工作线程总数（包括初始工作线程） |
| `oxphp_workers_retired_total` | counter | 被 ScaleManager 回收的工作线程总数（仅动态模式） |

### 工作进程模式

以下指标仅在工作进程模式激活时（设置了 `WORKER_FILE`）输出。它们提供对持久化 PHP 工作进程生命周期、回收和每请求执行时间的可见性。

#### 全局计数器

| 指标 | 类型 | 说明 |
|------|------|------|
| `oxphp_worker_mode_enabled` | gauge | 工作进程模式激活时始终为 `1` |
| `oxphp_worker_requests_handled_total` | counter | 持久化工作进程处理的请求总数 |
| `oxphp_worker_recycles_total` | counter | 工作进程回收总数（工作进程退出并被重新生成） |
| `oxphp_worker_recycles_by_reason_total` | counter | 按原因分类的回收数。标签：`reason="max_requests"`、`reason="max_memory"`、`reason="error"` |
| `oxphp_worker_soft_resets_total` | counter | 请求之间的软重置总数（应等于 `requests_handled_total`） |

#### 每工作进程 Gauge

| 指标 | 类型 | 标签 | 说明 |
|------|------|------|------|
| `oxphp_worker_memory_bytes` | gauge | `worker` | 每个工作进程的当前 PHP 堆使用量（字节） |
| `oxphp_worker_uptime_seconds` | gauge | `worker` | 工作线程生成后的秒数 |
| `oxphp_worker_requests_count` | gauge | `worker` | 此工作进程实例处理的请求数 |

每工作进程指标按工作槽索引（例如 `worker="0"`、`worker="1"`）。仅活跃的工作进程输出值。

#### 请求耗时直方图

| 指标 | 类型 | 说明 |
|------|------|------|
| `oxphp_worker_request_duration_us` | histogram | 每请求的 PHP 处理器执行时间（微秒） |

桶边界（微秒）：100、250、500、1000、2500、5000、10000、25000、50000、+Inf。

此直方图测量执行 PHP 处理器回调所花费的时间，不包括队列等待时间。有助于识别慢请求和尾部延迟。

## 示例输出

```
# HELP oxphp_uptime_seconds Server uptime in seconds.
# TYPE oxphp_uptime_seconds gauge
oxphp_uptime_seconds 3612

# HELP oxphp_requests_total Total HTTP requests.
# TYPE oxphp_requests_total counter
oxphp_requests_total 48203

# HELP oxphp_requests_by_method_total Requests by HTTP method.
# TYPE oxphp_requests_by_method_total counter
oxphp_requests_by_method_total{method="GET"} 42100
oxphp_requests_by_method_total{method="POST"} 6103

# HELP oxphp_responses_by_status_total Responses by status class.
# TYPE oxphp_responses_by_status_total counter
oxphp_responses_by_status_total{status="2xx"} 47500
oxphp_responses_by_status_total{status="4xx"} 650
oxphp_responses_by_status_total{status="5xx"} 53

# HELP oxphp_active_connections Current active connections.
# TYPE oxphp_active_connections gauge
oxphp_active_connections 7

# HELP oxphp_pending_requests Requests waiting in queue.
# TYPE oxphp_pending_requests gauge
oxphp_pending_requests 2

# HELP oxphp_dropped_requests_total Requests dropped (503).
# TYPE oxphp_dropped_requests_total counter
oxphp_dropped_requests_total 0

# HELP oxphp_response_time_us_total Total response time in microseconds.
# TYPE oxphp_response_time_us_total counter
oxphp_response_time_us_total 192000000

# HELP oxphp_busy_workers Currently busy worker threads.
# TYPE oxphp_busy_workers gauge
oxphp_busy_workers 2

# HELP oxphp_workers_current Current number of worker threads.
# TYPE oxphp_workers_current gauge
oxphp_workers_current 8

# HELP oxphp_workers_min Minimum worker thread count.
# TYPE oxphp_workers_min gauge
oxphp_workers_min 2

# HELP oxphp_workers_max Maximum worker thread count.
# TYPE oxphp_workers_max gauge
oxphp_workers_max 16

# HELP oxphp_workers_idle Currently idle worker threads.
# TYPE oxphp_workers_idle gauge
oxphp_workers_idle 6

# HELP oxphp_workers_spawned_total Total workers spawned.
# TYPE oxphp_workers_spawned_total counter
oxphp_workers_spawned_total 12

# HELP oxphp_workers_retired_total Total workers retired.
# TYPE oxphp_workers_retired_total counter
oxphp_workers_retired_total 4

# HELP oxphp_worker_mode_enabled Whether worker mode is active.
# TYPE oxphp_worker_mode_enabled gauge
oxphp_worker_mode_enabled 1

# HELP oxphp_worker_requests_handled_total Total requests processed by worker mode.
# TYPE oxphp_worker_requests_handled_total counter
oxphp_worker_requests_handled_total 48203

# HELP oxphp_worker_recycles_total Total worker recycles.
# TYPE oxphp_worker_recycles_total counter
oxphp_worker_recycles_total 5

# HELP oxphp_worker_recycles_by_reason_total Worker recycles by reason.
# TYPE oxphp_worker_recycles_by_reason_total counter
oxphp_worker_recycles_by_reason_total{reason="max_requests"} 4
oxphp_worker_recycles_by_reason_total{reason="max_memory"} 1
oxphp_worker_recycles_by_reason_total{reason="error"} 0

# HELP oxphp_worker_soft_resets_total Total soft resets between requests.
# TYPE oxphp_worker_soft_resets_total counter
oxphp_worker_soft_resets_total 48203

# HELP oxphp_worker_memory_bytes Current PHP heap per worker.
# TYPE oxphp_worker_memory_bytes gauge
# HELP oxphp_worker_uptime_seconds Time since worker thread spawned.
# TYPE oxphp_worker_uptime_seconds gauge
# HELP oxphp_worker_requests_count Requests handled by this worker instance.
# TYPE oxphp_worker_requests_count gauge
oxphp_worker_memory_bytes{worker="0"} 524288
oxphp_worker_uptime_seconds{worker="0"} 3600
oxphp_worker_requests_count{worker="0"} 6025
oxphp_worker_memory_bytes{worker="1"} 491520
oxphp_worker_uptime_seconds{worker="1"} 3600
oxphp_worker_requests_count{worker="1"} 6030

# HELP oxphp_worker_request_duration_us PHP execution time per request.
# TYPE oxphp_worker_request_duration_us histogram
oxphp_worker_request_duration_us_bucket{le="100"} 12050
oxphp_worker_request_duration_us_bucket{le="250"} 30100
oxphp_worker_request_duration_us_bucket{le="500"} 42000
oxphp_worker_request_duration_us_bucket{le="1000"} 46500
oxphp_worker_request_duration_us_bucket{le="2500"} 47800
oxphp_worker_request_duration_us_bucket{le="5000"} 48100
oxphp_worker_request_duration_us_bucket{le="10000"} 48180
oxphp_worker_request_duration_us_bucket{le="25000"} 48200
oxphp_worker_request_duration_us_bucket{le="50000"} 48203
oxphp_worker_request_duration_us_bucket{le="+Inf"} 48203
oxphp_worker_request_duration_us_sum 9640600
oxphp_worker_request_duration_us_count 48203
```

## Prometheus 配置

在 `prometheus.yml` 中添加抓取任务：

```yaml
scrape_configs:
  - job_name: "oxphp"
    scrape_interval: 15s
    static_configs:
      - targets: ["oxphp:9090"]
```

Kubernetes 服务发现配置：

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

## 常用 PromQL 查询

**请求速率（每秒请求数）：**

```promql
rate(oxphp_requests_total[5m])
```

**错误率（5xx 响应百分比）：**

```promql
rate(oxphp_responses_by_status_total{status="5xx"}[5m])
/ rate(oxphp_requests_total[5m]) * 100
```

**平均响应时间（毫秒）：**

```promql
rate(oxphp_response_time_us_total[5m])
/ rate(oxphp_requests_total[5m]) / 1000
```

**丢弃率（每秒 503 拒绝数）：**

```promql
rate(oxphp_dropped_requests_total[5m])
```

**工作池利用率（动态模式）：**

```promql
1 - (oxphp_workers_idle / oxphp_workers_current)
```

**工作线程伸缩速率（每分钟创建数）：**

```promql
rate(oxphp_workers_spawned_total[5m]) * 60
```

**工作进程模式请求速率：**

```promql
rate(oxphp_worker_requests_handled_total[5m])
```

**工作进程回收速率（每分钟回收数）：**

```promql
rate(oxphp_worker_recycles_total[5m]) * 60
```

**工作进程模式 p99 请求耗时（微秒）：**

```promql
histogram_quantile(0.99, rate(oxphp_worker_request_duration_us_bucket[5m]))
```

**平均工作进程内存使用量（字节）：**

```promql
avg(oxphp_worker_memory_bytes)
```

**工作进程错误回收速率：**

```promql
rate(oxphp_worker_recycles_by_reason_total{reason="error"}[5m])
```

## 告警示例

```yaml
groups:
  - name: oxphp
    rules:
      - alert: OxPHPHighErrorRate
        expr: >
          rate(oxphp_responses_by_status_total{status="5xx"}[5m])
          / rate(oxphp_requests_total[5m]) > 0.05
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP error rate above 5%"

      - alert: OxPHPQueueDropping
        expr: rate(oxphp_dropped_requests_total[5m]) > 0
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "OxPHP is dropping requests (503)"

      - alert: OxPHPWorkerErrorRecycles
        expr: rate(oxphp_worker_recycles_by_reason_total{reason="error"}[5m]) > 0
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP workers are recycling due to errors"

      - alert: OxPHPWorkerHighMemory
        expr: oxphp_worker_memory_bytes > 134217728
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP worker memory exceeds 128 MiB"

      - alert: OxPHPWorkerSlowRequests
        expr: >
          histogram_quantile(0.99,
            rate(oxphp_worker_request_duration_us_bucket[5m])
          ) > 50000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP worker p99 latency exceeds 50ms"

```

## 实现说明

所有指标计数器使用 `std::sync::atomic` 类型和 `Ordering::Relaxed`。这意味着：

- 计数器读取可能相对于实际状态有微小延迟（微秒级）。
- 请求路径上没有锁或内存屏障开销。
- Prometheus 以 15 秒间隔抓取，亚毫秒级的延迟完全无关紧要。

### 插件指标

插件可以向 `/metrics` 输出贡献额外指标。插件指标附加在上述核心指标之后，遵循相同的 Prometheus 文本格式。

## 另请参阅

- [健康检查](health-checks.md) --- 内部服务器的 `/health` 和 `/config` 端点
- [配置](configuration.md) --- `INTERNAL_ADDR` 及其他环境变量
- [工作池](/architecture/worker-pool.md) --- 静态和动态伸缩、工作进程模式和回收行为
- [优雅关闭](graceful-shutdown.md) --- 连接排空如何影响 `oxphp_active_connections`
