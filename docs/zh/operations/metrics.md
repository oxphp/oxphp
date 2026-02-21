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
- [工作池](/architecture/worker-pool.md) --- 驱动工作线程指标的静态和动态伸缩
- [优雅关闭](graceful-shutdown.md) --- 连接排空如何影响 `oxphp_active_connections`
