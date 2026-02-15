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

**Method labels:** `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `OTHER`.

**Status labels:** `1xx`, `2xx`, `3xx`, `4xx`, `5xx`.

Only methods and status classes with at least one recorded event are emitted. Zero-count labels are omitted to keep the output compact.

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

### PHP Worker Pool

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_workers_current` | gauge | Current number of worker threads in the pool |
| `oxphp_workers_min` | gauge | Minimum worker thread count (equals current for static mode) |
| `oxphp_workers_max` | gauge | Maximum worker thread count (equals current for static mode) |
| `oxphp_workers_idle` | gauge | Worker threads not currently processing a request (dynamic mode only, 0 in static mode) |
| `oxphp_workers_spawned_total` | counter | Total workers spawned since startup (includes initial workers) |
| `oxphp_workers_retired_total` | counter | Total workers retired by the ScaleManager (dynamic mode only) |

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
- [Worker Pool](/architecture/worker-pool.md) --- static and dynamic scaling that drives worker metrics
- [Graceful Shutdown](graceful-shutdown.md) --- how connection draining affects `oxphp_active_connections`

