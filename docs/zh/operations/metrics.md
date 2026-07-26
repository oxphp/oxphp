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
| `oxphp_request_cancelled_total` | counter | 按原因统计的被取消请求数。标签：`reason`（`client_abort`、`timeout`、`shutdown`）。始终输出 |

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
| `oxphp_pending_requests` | gauge | 已接受但尚未响应的 PHP 请求数——等待队列槽位、在队列中或正在执行。仅统计路由到 PHP 的请求：静态文件、404 与被拒绝的路径无需排队即可响应，不会出现在此处 |
| `oxphp_dropped_requests_total` | counter | PHP 工作进程接受请求后失败的请求数 |
| `oxphp_admission_refused_total` | counter | 未到达工作进程即被应答的请求数。标签 `reason`：`wait_timeout`（已等满 `QUEUE_WAIT_TIMEOUT_MS`，应为进程池增加余量）、`waiting_full`（等待数已达 `QUEUE_MAX_WAITING`，应调高该值或 `MAX_CONNECTIONS`）、`queue_full`（`QUEUE_WAIT_TIMEOUT_MS=0`，等待已关闭）、`shutting_down`（请求仍在等待接纳时优雅退出的期限已到）、`pool_unavailable`（已无任何工作线程可接收请求——进程池已消失，而非繁忙）。只有前三者属于过载并返回 529；`shutting_down` 与其余优雅退出流程一致返回 503，`pool_unavailable` 返回 500。过载告警应专门基于前三者：整体指标在重启时同样会变动。不计入 `oxphp_queue_wait_us` |

## 工作进程池指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_workers_current` | gauge | 当前 PHP 工作线程数 |
| `oxphp_workers_min` | gauge | 最小工作进程数（静态模式下等于当前数量） |
| `oxphp_workers_max` | gauge | 最大工作进程数（静态模式下等于当前数量） |
| `oxphp_workers_idle` | gauge | 当前没有请求在处理的工作线程数，按 `workers_current - busy_workers` 计算 |
| `oxphp_busy_workers` | gauge | 至少正在处理一个请求的工作线程数，绝不会超过 `oxphp_workers_current`。统计的是线程而非请求——worker 模式下一个线程复用多个请求纤程，仍只计一次。等待接纳或仍在队列中的请求不计入，它们体现在 `oxphp_pending_requests` 中 |
| `oxphp_workers_spawned_total` | counter | 启动以来创建的工作进程总数（含初始工作进程） |
| `oxphp_workers_retired_total` | counter | 因空闲超时退出的工作进程总数（仅动态模式） |

## 工作进程监督器指标

由工作进程监督器输出的单工作进程可观测性指标。每条序列携带 `worker_id` 标签（槽位索引）。这些指标在监督器开始跟踪单工作进程状态后出现。

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_worker_request_age_seconds` | gauge | 每个工作进程上在途请求的存活时长（秒）。标签：`worker_id` |
| `oxphp_worker_long_running_total` | counter | 监督器扫描中发现请求超过卡死阈值的次数。标签：`worker_id` |
| `oxphp_worker_stuck_total` | counter | 每个工作进程的卡死分类计数器。标签：`worker_id`、`kind`（`io`、`c_call`、`cpu`） |

## 队列等待直方图

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_queue_wait_us` | histogram | 请求在队列中等待工作进程拾取的时间（微秒） |

桶边界（微秒）：`50`、`100`、`250`、`500`、`1000`、`2500`、`5000`、`10000`、`50000`、`100000`、`250000`、`500000`、`1000000`、`+Inf`。

该指标度量的是等待时间——先等待接纳、再在队列中等待——并已扣除脚本自身的执行时间，因此它回答的是「工作进程多久之后才取走该请求」，而非「该请求总共耗时多久」。以 529 被拒绝的请求从未入队，不计入此处，请改用 `oxphp_admission_refused_total` 统计。

队列等待时间过高表明所有工作进程均处于繁忙状态，应增加 `PHP_WORKERS`。桶的范围可达一秒，与 `QUEUE_WAIT_TIMEOUT_MS` 的默认值一致，因此几乎耗尽等待预算后才得以执行的请求会落入可量化的桶中，而非 `+Inf`。被服务的请求不会等待超过该预算——超出即被拒绝——因此唯一会让等待重新落入 `+Inf` 的，是调高 `QUEUE_WAIT_TIMEOUT_MS`。

## 限流指标

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_rate_limited_total` | counter | 被限流器拒绝的请求数（返回 429） |
| `oxphp_php_deny_total` | counter | 被 `PHP_DENY_PATHS` 阻止的请求数（`.php` 执行被拒绝）。参见 [PHP 执行拒绝名单](../security/php-deny.md) |

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

这些指标仅在工作进程模式启用时（即 `WORKER_MODE_ENABLED=true`）才会输出。

### 全局计数器

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_worker_mode_enabled` | gauge | 工作进程模式启用时始终为 `1` |
| `oxphp_worker_requests_handled_total` | counter | 持久化工作进程处理的总请求数 |
| `oxphp_worker_recycles_total` | counter | 工作进程回收总次数（工作进程退出并重新创建） |
| `oxphp_worker_recycles_by_reason_total` | counter | 按原因统计的回收次数。标签：`reason`（`scheduled`、`max_memory`、`error`） |
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

这些指标要求 `ASYNC_WORKERS` 设置为非零值，并且每项都有各自的输出门控：计数器仅在至少有一个任务被分发或拒绝后出现，`_in_flight` / `_in_flight_limit` 仪表盘在池接入其在途计数器后出现，而 `oxphp_async_output_discarded_bytes_total` 仅在有输出被丢弃后才出现。

| 指标 | 类型 | 描述 |
|--------|------|-------------|
| `oxphp_async_tasks_dispatched_total` | counter | 分发到后台进程池的异步任务总数 |
| `oxphp_async_tasks_completed_total` | counter | 成功完成的异步任务数 |
| `oxphp_async_tasks_failed_total` | counter | 抛出异常的异步任务数 |
| `oxphp_async_tasks_cancelled_total` | counter | 被取消的异步任务数 |
| `oxphp_async_tasks_rejected_total` | counter | 在分发时被拒绝的异步任务数——因进程池队列已满，或达到在途上限（`ASYNC_MAX_FIBERS × ASYNC_WORKERS`） |
| `oxphp_async_tasks_stranded_total` | counter | 超过 `await_race` / `await_any` 超时仍在运行的工作进程。每个滞留任务最多可延长 RSHUTDOWN 5 秒。 |
| `oxphp_async_tasks_in_flight` | gauge | 当前排队或运行中的异步任务数（在池接入其在途计数器后才输出） |
| `oxphp_async_tasks_in_flight_limit` | gauge | 并发异步任务的最大值（`ASYNC_MAX_FIBERS × ASYNC_WORKERS`） |
| `oxphp_async_output_discarded_bytes_total` | counter | 在 Worker 空闲时丢弃的异步任务输出字节数（异步任务中的 `echo` 没有可接收的客户端） |

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

该值始终介于 `0` 与 `1` 之间。持续为 `1` 表示所有工作线程都已占满、新到请求开始排队；请结合 `rate(oxphp_admission_refused_total{reason=~"queue_full|wait_timeout|waiting_full"}[5m])` 判断积压是否已转化为拒绝，并结合 `oxphp_pending_requests` 观察积压深度。

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
