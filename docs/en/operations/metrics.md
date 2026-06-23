---
title: Prometheus Metrics
description: Reference for all Prometheus-compatible metrics exposed by OxPHP at the /metrics endpoint, including request, connection, worker, and compression metrics.
---

# Prometheus Metrics

OxPHP exposes Prometheus-compatible metrics in text exposition format at `GET /metrics` on the internal server. These metrics cover request throughput, response times, connection state, worker pool health, static file caching, compression efficiency, and worker mode performance.

## Enabling Metrics

Set `INTERNAL_ADDR` to start the internal server:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Then scrape from Prometheus or any compatible collector:

```bash
curl http://localhost:9090/metrics
```

## Server Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_uptime_seconds` | gauge | Seconds since the server process started |
| `oxphp_requests_total` | counter | Total HTTP requests received on the main port |

## Request Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_requests_by_method_total` | counter | Requests by HTTP method. Label: `method` (`GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `QUERY`, `OTHER`) |
| `oxphp_responses_by_status_total` | counter | Responses by status class. Label: `status` (`1xx`, `2xx`, `3xx`, `4xx`, `5xx`) |
| `oxphp_request_bytes_total` | counter | Total request body bytes received |
| `oxphp_response_bytes_total` | counter | Total response body bytes sent |
| `oxphp_request_cancelled_total` | counter | Cancelled requests by reason. Label: `reason` (`client_abort`, `timeout`, `shutdown`). Always emitted |

> **Note:** Only methods and status classes with at least one recorded event are emitted. Zero-count labels are omitted.

## Request Duration Histogram

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_request_duration_us` | histogram | End-to-end request duration in microseconds for all requests (static files and PHP) |

Bucket boundaries (microseconds): `100`, `500`, `1000`, `2500`, `5000`, `10000`, `25000`, `50000`, `100000`, `250000`, `500000`, `1000000`, `+Inf`.

Use this histogram to track overall latency, identify slow endpoints, and measure tail latency percentiles.

## Connection Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_active_connections` | gauge | Currently open TCP connections on the main port |
| `oxphp_pending_requests` | gauge | Requests currently dispatched to PHP workers (queued and in-flight) |
| `oxphp_dropped_requests_total` | counter | Requests where the PHP worker failed after accepting the request |

## Worker Pool Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_workers_current` | gauge | Current number of PHP worker threads |
| `oxphp_workers_min` | gauge | Minimum worker count (equals current count in static mode) |
| `oxphp_workers_max` | gauge | Maximum worker count (equals current count in static mode) |
| `oxphp_workers_idle` | gauge | Workers not currently processing a request |
| `oxphp_busy_workers` | gauge | Workers currently processing a request |
| `oxphp_workers_spawned_total` | counter | Total workers spawned since startup (includes initial workers) |
| `oxphp_workers_retired_total` | counter | Total workers retired due to idle timeout (dynamic mode only) |

## Worker Supervisor Metrics

Per-worker observability emitted by the worker supervisor. Each series carries a `worker_id` label (slot index). These appear once the supervisor is tracking per-worker state.

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_request_age_seconds` | gauge | Age of the in-flight request on each worker, in seconds. Label: `worker_id` |
| `oxphp_worker_long_running_total` | counter | Supervisor scans that observed a request older than the stuck threshold. Label: `worker_id` |
| `oxphp_worker_stuck_total` | counter | Stuck-classification counter per worker. Labels: `worker_id`, `kind` (`io`, `c_call`, `cpu`) |

## Queue Wait Histogram

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_queue_wait_us` | histogram | Time a request waits in the queue before a worker picks it up, in microseconds |

Bucket boundaries (microseconds): `50`, `100`, `250`, `500`, `1000`, `2500`, `5000`, `10000`, `50000`, `+Inf`.

High queue wait times indicate that all workers are busy and you should increase `PHP_WORKERS` or `QUEUE_CAPACITY`.

## Rate Limiting Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_rate_limited_total` | counter | Requests rejected by the rate limiter (returned 429) |
| `oxphp_php_deny_total` | counter | Requests blocked by `PHP_DENY_PATHS` (`.php` execution denied). See [PHP Execution Deny-List](../security/php-deny.md) |

## Static File Cache Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_static_cache_hits_total` | counter | Static file requests served from the in-memory cache |
| `oxphp_static_cache_misses_total` | counter | Static file requests that required a disk read |

## Compression Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_compressed_responses_total` | counter | Responses compressed with Brotli |
| `oxphp_compression_bytes_saved_total` | counter | Total bytes saved by compression (original size minus compressed size) |

## Worker Mode Metrics

These metrics are only emitted when worker mode is active (`WORKER_MODE_ENABLED=true`).

### Global Counters

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_mode_enabled` | gauge | Always `1` when worker mode is active |
| `oxphp_worker_requests_handled_total` | counter | Total requests processed by persistent workers |
| `oxphp_worker_recycles_total` | counter | Total worker recycles (worker exited and was respawned) |
| `oxphp_worker_recycles_by_reason_total` | counter | Recycles by reason. Label: `reason` (`scheduled`, `max_memory`, `error`) |
| `oxphp_worker_soft_resets_total` | counter | Total soft resets performed between requests |

### Per-Worker Gauges

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_memory_bytes` | gauge | Current PHP heap usage per worker. Label: `worker` (slot index, e.g., `"0"`, `"1"`) |
| `oxphp_worker_uptime_seconds` | gauge | Seconds since each worker was spawned. Label: `worker` |
| `oxphp_worker_requests_count` | gauge | Requests handled by each worker instance. Label: `worker` |

### Worker Request Duration Histogram

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_request_duration_us` | histogram | PHP handler execution time per request in microseconds (worker mode only) |

Bucket boundaries (microseconds): `100`, `250`, `500`, `1000`, `2500`, `5000`, `10000`, `25000`, `50000`, `+Inf`.

This histogram measures time spent inside the PHP handler callback, excluding queue wait time. Use it to identify slow handlers and track tail latency in worker mode.

## Async Pool Metrics

These metrics require `ASYNC_WORKERS` set to a non-zero value, and each has its own emission gate: the counters appear only after at least one task has been dispatched or rejected, the `_in_flight` / `_in_flight_limit` gauges appear once the pool has wired its in-flight counter, and `oxphp_async_output_discarded_bytes_total` appears only after some output has been discarded.

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_async_tasks_dispatched_total` | counter | Total async tasks dispatched to the background pool |
| `oxphp_async_tasks_completed_total` | counter | Async tasks that completed successfully |
| `oxphp_async_tasks_failed_total` | counter | Async tasks that threw an exception |
| `oxphp_async_tasks_cancelled_total` | counter | Async tasks that were cancelled |
| `oxphp_async_tasks_rejected_total` | counter | Async tasks rejected at dispatch — because the pool queue was full or the in-flight cap (`ASYNC_MAX_FIBERS × ASYNC_WORKERS`) was reached |
| `oxphp_async_tasks_stranded_total` | counter | Workers left running past an `await_race` / `await_any` timeout. Each stranded task can extend RSHUTDOWN by up to 5 seconds. |
| `oxphp_async_tasks_in_flight` | gauge | Async tasks currently queued or running (emitted once the pool wires its in-flight counter) |
| `oxphp_async_tasks_in_flight_limit` | gauge | Maximum concurrent async tasks (`ASYNC_MAX_FIBERS × ASYNC_WORKERS`) |
| `oxphp_async_output_discarded_bytes_total` | counter | Bytes of async-task output discarded at worker idle (an `echo` in an async task has no client to receive it) |

## Grafana Dashboard Tips

The following PromQL queries are useful for building dashboards:

**Request rate (requests per second):**

```text
rate(oxphp_requests_total[5m])
```

**Average response time (milliseconds):**

```text
rate(oxphp_request_duration_us_sum[5m])
/ rate(oxphp_requests_total[5m]) / 1000
```

**p99 request duration (milliseconds):**

```text
histogram_quantile(0.99, rate(oxphp_request_duration_us_bucket[5m])) / 1000
```

**Error rate (5xx responses as a percentage):**

```text
rate(oxphp_responses_by_status_total{status="5xx"}[5m])
/ rate(oxphp_requests_total[5m]) * 100
```

**Worker pool utilization:**

```text
oxphp_busy_workers / oxphp_workers_current
```

**Queue saturation (drop rate per second):**

```text
rate(oxphp_dropped_requests_total[5m])
```

**p99 queue wait (microseconds):**

```text
histogram_quantile(0.99, rate(oxphp_queue_wait_us_bucket[5m]))
```

**Static file cache hit rate:**

```text
rate(oxphp_static_cache_hits_total[5m])
/ (rate(oxphp_static_cache_hits_total[5m]) + rate(oxphp_static_cache_misses_total[5m]))
```

**Bytes saved by compression per second:**

```text
rate(oxphp_compression_bytes_saved_total[5m])
```

**Worker mode p99 latency (microseconds):**

```text
histogram_quantile(0.99, rate(oxphp_worker_request_duration_us_bucket[5m]))
```

**Worker recycle rate (per minute):**

```text
rate(oxphp_worker_recycles_total[5m]) * 60
```

**Average worker memory usage:**

```text
avg(oxphp_worker_memory_bytes)
```

## Prometheus Scrape Config

Add a scrape job to your `prometheus.yml`:

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

## See Also

- [Health Checks](health-checks.md) — the `/health` and `/config` endpoints on the internal server
- [Configuration Reference](configuration.md) — all environment variables including `INTERNAL_ADDR`
- [Graceful Shutdown](graceful-shutdown.md) — how connection draining affects `oxphp_active_connections`
- [Worker Mode](../features/worker-mode.md) — persistent workers and the metrics they emit
