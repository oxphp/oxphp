---
title: Metrics
description: Prometheus-compatible metrics exposed by the internal server
---

OxPHP exposes metrics in Prometheus text exposition format at `GET /metrics` on the internal server. All counters use lock-free atomics with `Relaxed` ordering for minimal performance impact on the request path.

## Enabling Metrics

Metrics are available when the internal server is running. Set the `INTERNAL_ADDR` environment variable:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Then scrape from Prometheus or any compatible collector:

```bash
curl http://localhost:9090/metrics
```

## Metric Reference

### Server

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_uptime_seconds` | gauge | Seconds since the server process started |

### Requests

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `oxphp_requests_total` | counter | --- | Total HTTP requests received on the main port |
| `oxphp_requests_by_method_total` | counter | `method` | Requests broken down by HTTP method |
| `oxphp_responses_by_status_total` | counter | `status` | Responses broken down by status class |
| `oxphp_response_time_us_total` | counter | --- | Cumulative response time in microseconds |
| `oxphp_request_duration_us` | histogram | --- | Request duration in microseconds (all requests) |
| `oxphp_request_bytes_total` | counter | --- | Total request body bytes received |
| `oxphp_response_bytes_total` | counter | --- | Total response body bytes sent |

**Method labels:** `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `OTHER`.

**Status labels:** `1xx`, `2xx`, `3xx`, `4xx`, `5xx`.

Only methods and status classes with at least one recorded event are emitted. Zero-count labels are omitted to keep the output compact.

#### Request Duration Histogram

Bucket boundaries (microseconds): 100, 500, 1000, 2500, 5000, 10000, 25000, 50000, 100000, 250000, 500000, 1000000, +Inf.

This histogram covers all requests (static files and PHP), unlike `oxphp_worker_request_duration_us` which only measures PHP handler execution time. The `_sum` reuses `oxphp_response_time_us_total` (same value, one atomic) and `_count` is derived from the sum of all `oxphp_responses_by_status_total` counters.

### Connections

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_active_connections` | gauge | Currently open TCP connections on the main port |

### PHP Worker Queue

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_pending_requests` | gauge | Requests currently waiting in the PHP worker queue |
| `oxphp_dropped_requests_total` | counter | Requests rejected with 503 because the queue was full |
| `oxphp_busy_workers` | gauge | Worker threads currently processing a request |
| `oxphp_queue_wait_us` | histogram | Time waiting in queue before worker pickup (microseconds) |

#### Queue Wait Histogram

Bucket boundaries (microseconds): 50, 100, 250, 500, 1000, 2500, 5000, 10000, 50000, +Inf.

Measures the time between submitting a request to the worker queue and a worker picking it up. Helps identify queue pressure and worker starvation.

### PHP Worker Pool

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_workers_current` | gauge | Current number of worker threads in the pool |
| `oxphp_workers_min` | gauge | Minimum worker thread count (equals current for static mode) |
| `oxphp_workers_max` | gauge | Maximum worker thread count (equals current for static mode) |
| `oxphp_workers_idle` | gauge | Worker threads not currently processing a request (dynamic mode only, 0 in static mode) |
| `oxphp_workers_spawned_total` | counter | Total workers spawned since startup (includes initial workers) |
| `oxphp_workers_retired_total` | counter | Total workers retired by the ScaleManager (dynamic mode only) |

### Rate Limiting

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_rate_limited_total` | counter | Requests rejected by the rate limiter (429) |

This counter increments each time a request is rejected with a 429 response. Only emitted when rate limiting is enabled (`RATE_LIMIT` > 0).

### Static File Cache

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_static_cache_hits_total` | counter | Static file requests served from the content cache |
| `oxphp_static_cache_misses_total` | counter | Static file requests that required disk I/O |

Cache hit ratio can be computed as `hits / (hits + misses)`. A low hit ratio may indicate the content cache budget (64 MB) is too small for the working set.

### Compression

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_compressed_responses_total` | counter | Responses compressed with Brotli |
| `oxphp_compression_bytes_saved_total` | counter | Total bytes saved by compression (original - compressed) |

These counters only increment when compression is enabled (`COMPRESSION_LEVEL` > 0) and a response is actually compressed (the compressed output is smaller than the original).

### Worker Mode

These metrics are only emitted when worker mode is active (`WORKER_FILE` is set). They provide visibility into persistent PHP worker lifecycle, recycling, and per-request execution times.

#### Global Counters

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_mode_enabled` | gauge | Always `1` when worker mode is active |
| `oxphp_worker_requests_handled_total` | counter | Total requests processed by persistent workers |
| `oxphp_worker_recycles_total` | counter | Total worker recycles (worker exited and was respawned) |
| `oxphp_worker_recycles_by_reason_total` | counter | Recycles broken down by reason. Labels: `reason="max_requests"`, `reason="max_memory"`, `reason="error"` |
| `oxphp_worker_soft_resets_total` | counter | Total soft resets between requests (should equal `requests_handled_total`) |

#### Per-Worker Gauges

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `oxphp_worker_memory_bytes` | gauge | `worker` | Current PHP heap usage in bytes for each worker |
| `oxphp_worker_uptime_seconds` | gauge | `worker` | Seconds since the worker thread was spawned |
| `oxphp_worker_requests_count` | gauge | `worker` | Requests handled by this specific worker instance |

Per-worker metrics are indexed by worker slot (e.g., `worker="0"`, `worker="1"`). Only active workers emit values.

#### Request Duration Histogram

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_request_duration_us` | histogram | PHP handler execution time per request in microseconds |

Bucket boundaries (microseconds): 100, 250, 500, 1000, 2500, 5000, 10000, 25000, 50000, +Inf.

This histogram measures the time spent executing the PHP handler callback, excluding queue wait time. It helps identify slow requests and tail latency.

## Sample Output

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

# HELP oxphp_request_duration_us Request duration in microseconds.
# TYPE oxphp_request_duration_us histogram
oxphp_request_duration_us_bucket{le="100"} 5200
oxphp_request_duration_us_bucket{le="500"} 18400
oxphp_request_duration_us_bucket{le="1000"} 31000
oxphp_request_duration_us_bucket{le="2500"} 40200
oxphp_request_duration_us_bucket{le="5000"} 44800
oxphp_request_duration_us_bucket{le="10000"} 46900
oxphp_request_duration_us_bucket{le="25000"} 47600
oxphp_request_duration_us_bucket{le="50000"} 47900
oxphp_request_duration_us_bucket{le="100000"} 48100
oxphp_request_duration_us_bucket{le="250000"} 48180
oxphp_request_duration_us_bucket{le="500000"} 48200
oxphp_request_duration_us_bucket{le="1000000"} 48203
oxphp_request_duration_us_bucket{le="+Inf"} 48203
oxphp_request_duration_us_sum 192000000
oxphp_request_duration_us_count 48203

# HELP oxphp_request_bytes_total Total request body bytes received.
# TYPE oxphp_request_bytes_total counter
oxphp_request_bytes_total 15360000

# HELP oxphp_response_bytes_total Total response body bytes sent.
# TYPE oxphp_response_bytes_total counter
oxphp_response_bytes_total 482030000

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

# HELP oxphp_queue_wait_us Time waiting in queue before worker pickup.
# TYPE oxphp_queue_wait_us histogram
oxphp_queue_wait_us_bucket{le="50"} 20100
oxphp_queue_wait_us_bucket{le="100"} 35400
oxphp_queue_wait_us_bucket{le="250"} 42000
oxphp_queue_wait_us_bucket{le="500"} 45600
oxphp_queue_wait_us_bucket{le="1000"} 47200
oxphp_queue_wait_us_bucket{le="2500"} 47900
oxphp_queue_wait_us_bucket{le="5000"} 48100
oxphp_queue_wait_us_bucket{le="10000"} 48180
oxphp_queue_wait_us_bucket{le="50000"} 48203
oxphp_queue_wait_us_bucket{le="+Inf"} 48203
oxphp_queue_wait_us_sum 4820300
oxphp_queue_wait_us_count 48203

# HELP oxphp_rate_limited_total Requests rejected by rate limiter.
# TYPE oxphp_rate_limited_total counter
oxphp_rate_limited_total 23

# HELP oxphp_static_cache_hits_total Static file cache hits.
# TYPE oxphp_static_cache_hits_total counter
oxphp_static_cache_hits_total 12400

# HELP oxphp_static_cache_misses_total Static file cache misses.
# TYPE oxphp_static_cache_misses_total counter
oxphp_static_cache_misses_total 350

# HELP oxphp_compressed_responses_total Responses compressed with brotli.
# TYPE oxphp_compressed_responses_total counter
oxphp_compressed_responses_total 38500

# HELP oxphp_compression_bytes_saved_total Bytes saved by compression.
# TYPE oxphp_compression_bytes_saved_total counter
oxphp_compression_bytes_saved_total 96250000

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

## Prometheus Configuration

Add a scrape job to `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: "oxphp"
    scrape_interval: 15s
    static_configs:
      - targets: ["oxphp:9090"]
```

For Kubernetes service discovery:

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

## Useful PromQL Queries

**Request rate (requests per second):**

```promql
rate(oxphp_requests_total[5m])
```

**Error rate (5xx responses as a percentage):**

```promql
rate(oxphp_responses_by_status_total{status="5xx"}[5m])
/ rate(oxphp_requests_total[5m]) * 100
```

**Average response time (milliseconds):**

```promql
rate(oxphp_response_time_us_total[5m])
/ rate(oxphp_requests_total[5m]) / 1000
```

**Drop rate (503 rejections per second):**

```promql
rate(oxphp_dropped_requests_total[5m])
```

**Worker pool utilization (dynamic mode):**

```promql
1 - (oxphp_workers_idle / oxphp_workers_current)
```

**Worker scaling rate (spawns per minute):**

```promql
rate(oxphp_workers_spawned_total[5m]) * 60
```

**Request p99 latency (microseconds):**

```promql
histogram_quantile(0.99, rate(oxphp_request_duration_us_bucket[5m]))
```

**Request throughput (bytes per second, in/out):**

```promql
rate(oxphp_request_bytes_total[5m])
rate(oxphp_response_bytes_total[5m])
```

**Queue wait p95 (microseconds):**

```promql
histogram_quantile(0.95, rate(oxphp_queue_wait_us_bucket[5m]))
```

**Rate-limited requests per second:**

```promql
rate(oxphp_rate_limited_total[5m])
```

**Static file cache hit ratio:**

```promql
rate(oxphp_static_cache_hits_total[5m])
/ (rate(oxphp_static_cache_hits_total[5m]) + rate(oxphp_static_cache_misses_total[5m]))
```

**Compression savings ratio:**

```promql
rate(oxphp_compression_bytes_saved_total[5m])
/ rate(oxphp_response_bytes_total[5m])
```

**Worker mode request rate:**

```promql
rate(oxphp_worker_requests_handled_total[5m])
```

**Worker recycle rate (recycles per minute):**

```promql
rate(oxphp_worker_recycles_total[5m]) * 60
```

**Worker mode p99 request duration (microseconds):**

```promql
histogram_quantile(0.99, rate(oxphp_worker_request_duration_us_bucket[5m]))
```

**Average worker memory usage (bytes):**

```promql
avg(oxphp_worker_memory_bytes)
```

**Worker error recycle rate:**

```promql
rate(oxphp_worker_recycles_by_reason_total{reason="error"}[5m])
```

## Alerting Examples

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

      - alert: OxPHPHighRequestLatency
        expr: >
          histogram_quantile(0.99,
            rate(oxphp_request_duration_us_bucket[5m])
          ) > 500000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP request p99 latency exceeds 500ms"

      - alert: OxPHPHighQueueWait
        expr: >
          histogram_quantile(0.95,
            rate(oxphp_queue_wait_us_bucket[5m])
          ) > 10000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP queue wait p95 exceeds 10ms — consider adding workers"

      - alert: OxPHPRateLimiting
        expr: rate(oxphp_rate_limited_total[5m]) > 1
        for: 5m
        labels:
          severity: info
        annotations:
          summary: "OxPHP is actively rate-limiting requests"

      - alert: OxPHPLowCacheHitRate
        expr: >
          rate(oxphp_static_cache_hits_total[5m])
          / (rate(oxphp_static_cache_hits_total[5m])
             + rate(oxphp_static_cache_misses_total[5m])) < 0.5
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP static file cache hit rate below 50%"

```

## Implementation Notes

All metric counters use `std::sync::atomic` types with `Ordering::Relaxed`. This means:

- Counter reads may be slightly stale (by microseconds) relative to the actual state.
- There is no locking or memory barrier overhead on the request path.
- Prometheus scrapes at 15-second intervals, so sub-millisecond staleness is irrelevant.

### Plugin Metrics

Plugins can contribute additional metrics to the `/metrics` output. Plugin metrics are appended after the core metrics listed above and follow the same Prometheus text exposition format.

## See Also

- [Health Checks](health-checks.md) --- the `/health` and `/config` endpoints on the internal server
- [Configuration](configuration.md) --- `INTERNAL_ADDR` and other environment variables
- [Worker Pool](/architecture/worker-pool.md) --- static and dynamic scaling, worker mode, and recycling behavior
- [Graceful Shutdown](graceful-shutdown.md) --- how connection draining affects `oxphp_active_connections`

