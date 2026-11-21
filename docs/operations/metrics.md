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
| `oxphp_request_cancelled_total` | counter | Cancelled requests by reason. Label: `reason` (`client_abort`, `timeout`, `shutdown`). Counted once for each request the server dispatches — not only the ones that reach PHP — at the point that request is answered. So `client_abort` covers a client that hung up while its request was still queued, which is the common case, as well as one that hung up while its handler was running; a client that hangs up once the response has started going out is past that point and is not counted. Always emitted |

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
| `oxphp_accept_stalled` | gauge | `1` while the accept loop is parked waiting for a free `MAX_CONNECTIONS` permit — connections are accepted by the kernel but nothing new is served, so from a client's point of view the server has stopped answering. Alert on `oxphp_accept_stalled == 1` directly; it needs no comparison against the configured budget |
| `oxphp_accept_stalls_total` | counter | Connections that had to wait for a `MAX_CONNECTIONS` permit after being accepted. Moves even when a stall starts and ends between two scrapes, which the gauge alone would miss — `rate(oxphp_accept_stalls_total[5m]) > 0` means the budget is being exhausted, however briefly. Raise `MAX_CONNECTIONS` or shrink the PHP backlog (`QUEUE_CAPACITY` + `QUEUE_MAX_WAITING`), see the [configuration reference](configuration.md) |
| `oxphp_pending_requests` | gauge | PHP requests accepted but not yet answered — waiting for a queue slot, queued, or executing. Only requests routed to PHP: a static file, a 404 or a denied path is answered without the queue and never appears here |
| `oxphp_dropped_requests_total` | counter | Requests where the PHP worker failed after accepting the request |
| `oxphp_admission_refused_total` | counter | Requests answered without reaching a worker. Label: `reason` — `wait_timeout`, `waiting_full`, `waiting_bytes`, `queue_full`, `shutting_down`, `pool_unavailable`, described under [Refusal reasons](#refusal-reasons). Only the first four are overload and answer 529; `shutting_down` answers 503 like the rest of graceful drain, and `pool_unavailable` answers 500. Alert on overload with those four specifically: the metric as a whole also moves on a restart. Excluded from `oxphp_queue_wait_us` |
| `oxphp_admission_wait_wasted_total` | counter | Requests that waited out their whole wait budget while the pool began no request at all. A request whose client had already gone when that budget ran out is excluded, on the same grounds as under [`wait_timeout`](#refusal-reasons). A subset of `oxphp_admission_refused_total{reason="wait_timeout"}`, separating a wait that lost a race for a worker from one that never had a race to lose. Read only while `oxphp_admission_wait_budget_us` is at its ceiling; why, and how to read it, is in [Diagnosing Overload](overload.md#reading-oxphp_admission_wait_wasted_total) |
| `oxphp_queue_depth` | gauge | Requests sitting in the worker queue right now: admitted, not yet picked up by a worker. An entry leaves only on a pickup, so this counts requests already answered on their deadline — `529`, or `499` where the client had gone — alongside ones still waiting for a worker; no metric counts the answered ones on their own. Present only for the SAPI executor, which is the only one with a queue |
| `oxphp_queue_capacity` | gauge | Queue slots in total — `QUEUE_CAPACITY`, the bound `oxphp_queue_depth` is read against |
| `oxphp_admission_slots_available` | gauge | Admission permits nobody is holding. A request takes one before it enters the queue and gives it back when a worker picks it up, so this is the free capacity a new arrival can claim without waiting. During a graceful drain the gate is closed and admits nothing regardless of what this reads |
| `oxphp_admission_wait_budget_us` | gauge | How long a request arriving now may wait for a worker, in microseconds — the same unit as `oxphp_queue_wait_us`, which is what it bounds. `QUEUE_WAIT_TIMEOUT_MS` is the ceiling, not the value, and is published beside it as `oxphp_admission_wait_budget_ceiling_us` so the comparison can be made from this endpoint alone; how and when the server moves it, and what a reading below the ceiling means, is in [Diagnosing Overload](overload.md#how-the-wait-budget-comes-down-and-goes-back-up). `0` in fail-fast mode (`QUEUE_WAIT_TIMEOUT_MS=0`), where there is no wait to adjust. Absent for an executor with no queue |
| `oxphp_admission_wait_budget_ceiling_us` | gauge | `QUEUE_WAIT_TIMEOUT_MS` in microseconds — the longest the gauge above is ever allowed to be. Constant for the life of the process. It exists so that the reading that matters, how far under its ceiling the budget has been driven, does not have to be made against a number hardcoded into an alert and silently wrong after the next config change — the same pairing as `oxphp_queue_depth` with `oxphp_queue_capacity`. `0` in fail-fast mode. Absent for an executor with no queue |
| `oxphp_abandoned_work_total` | counter | Requests a worker took off the queue whose client had already gone. Requests refused on their deadline are not counted; they never ran and are reported as `oxphp_admission_refused_total` instead. Only a client that closes its connection is visible here; one that stops waiting without closing looks like any other completion. What else it leaves out, and how to read it against the wait budget: [Diagnosing Overload](overload.md#reading-oxphp_abandoned_work_total) |
| `oxphp_pool_stalled` | gauge | `1` while requests are waiting for workers that are idle and getting nothing done — a pool that has stopped taking work off its queue. It is set once the state has held for a minute — long enough that a worker re-loading the application after a recycle does not raise it — It is cleared by a worker finishing a request, and also by a minute in which the pool has not looked wedged at all — without that second exit the readiness `503` would remove the traffic whose completion is the first one. Alert on `oxphp_pool_stalled == 1` directly; the readiness probe answers `503` on the same state, so an orchestrated deployment sees it without an alert rule. Exported in worker mode only, because it needs the count of requests the workers get through — absent rather than `0` where the state is not watched |

How to read these gauges together during an incident, and what the server logs while it sheds requests or the pool stalls, is in [Diagnosing Overload](overload.md).

### Refusal reasons

The `reason` label of `oxphp_admission_refused_total` takes these values:

- `wait_timeout` — waited out its whole wait budget: `QUEUE_WAIT_TIMEOUT_MS`, or less where `oxphp_admission_wait_budget_us` shows the server has shortened it. Give the pool more headroom — but read `oxphp_admission_wait_wasted_total` first, because where the pool began nothing at all during those waits the remedy is the opposite one. A request whose client had already gone when that budget ran out is **not** counted in `oxphp_admission_refused_total` — it is answered `499` and counted in `oxphp_request_cancelled_total{reason="client_abort"}` instead, so this series stays a statement about the pool rather than about how long clients were prepared to wait.
- `waiting_full` — already `QUEUE_MAX_WAITING` requests waiting; raise that or `MAX_CONNECTIONS`.
- `waiting_bytes` — the bodies already parked fill `QUEUE_MAX_WAITING_BYTES`, so this request's body had nowhere to sit; raise that, or lower how much a client may upload.
- `queue_full` — `QUEUE_WAIT_TIMEOUT_MS=0`, waiting is off.
- `shutting_down` — the drain deadline passed while the request was still waiting for admission.
- `pool_unavailable` — no worker thread is left to hand the request to: the pool is gone, not busy.

## Worker Pool Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_workers_current` | gauge | Current number of PHP worker threads |
| `oxphp_workers_min` | gauge | Minimum worker count (equals current count in static mode) |
| `oxphp_workers_max` | gauge | Maximum worker count (equals current count in static mode) |
| `oxphp_workers_idle` | gauge | Worker threads with no request in flight, computed as `workers_current - busy_workers` |
| `oxphp_busy_workers` | gauge | Worker threads currently executing at least one request; never exceeds `oxphp_workers_current`. Counts threads, not requests — in worker mode one thread multiplexes many request fibers and still counts once. Requests waiting for admission or sitting in the queue are not counted; those appear in `oxphp_pending_requests` |
| `oxphp_workers_spawned_total` | counter | Total workers spawned since startup (includes initial workers) |
| `oxphp_workers_retired_total` | counter | Total workers retired due to idle timeout (dynamic mode only) |

## Worker Supervisor Metrics

Per-worker observability emitted by the worker supervisor. Each series carries a `worker_id` label (slot index). These appear once the supervisor is tracking per-worker state.

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_request_age_seconds` | gauge | Age of the in-flight request on each worker, in seconds. Label: `worker_id` |
| `oxphp_worker_long_running_total` | counter | Supervisor scans that observed a request older than the stuck threshold (60 seconds, fixed). Label: `worker_id` |
| `oxphp_worker_stuck_total` | counter | Stuck-classification counter per worker. Labels: `worker_id`, `kind` (`io`, `c_call`, `cpu`). Reporting only: a request classified as stuck is not cancelled and its worker is not recycled — nothing in the server acts on it, so alert on this series |

## Queue Wait Histogram

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_queue_wait_us` | histogram | Time a request waits in the queue before a worker picks it up, in microseconds |

Bucket boundaries (microseconds): `50`, `100`, `250`, `500`, `1000`, `2500`, `5000`, `10000`, `50000`, `100000`, `250000`, `500000`, `1000000`, `+Inf`.

This measures time spent waiting — for admission and then in the queue — up to the moment a worker takes the request off the queue, so it answers "how long before a worker picked this up" rather than "how long did the request take". Nothing after that moment is in it: the worker preparing the request, the script (up to the response head, for a streamed response), and the handoff of the response back to the server's I/O threads count towards `oxphp_request_duration_us` and not here. The handoff is not free — the response waits for one of those threads to be woken for it or, on a busy server, to get through the work queued ahead of it — so the time a request spends past this wait is more than its script's execution time, and on a cheap script it can be several times that. Only a request a worker took off the queue inside its budget is recorded here. Nothing else is, whichever way it was answered: a `529` refused for overload, or a `503` or `500` when the server is shutting down or the pool is gone, all counted in `oxphp_admission_refused_total`; and a `499` for a client that had gone without a worker taking its request in time, counted in `oxphp_request_cancelled_total{reason="client_abort"}` and in no refusal series at all.

A high queue wait means requests took long to reach a worker, which is not by itself a shortage of workers: every reading includes the time a worker thread takes to get to a request handed to it, and that is there on an unloaded server too. Before raising `PHP_WORKERS`, check that requests are finding every worker busy, and that the machine has cores to spare for more — the server's own I/O threads, other processes and a container CPU quota compete with the workers for them. `oxphp_busy_workers` against `oxphp_workers_current` answers the first only in part: the gauge is read when the metrics are scraped, so a burst that fills the pool between two scrapes shows up here and not there, and in worker mode a thread counts as busy while it carries any request at all, although, while that request's handler is suspended waiting on I/O, a timer or an async task, the thread goes on taking more, unless it is holding new requests back behind a paused open session or, with `RUNTIME_HOOKS` on, behind its first request. The range reaches one second, matching the default `QUEUE_WAIT_TIMEOUT_MS`, so a request that spent most of its wait budget before running is quantified rather than lumped into `+Inf`. Nothing that gets served waits longer than the budget — past it the request is refused instead — so raising `QUEUE_WAIT_TIMEOUT_MS` is the one setting that puts waits back into `+Inf`.

## Rate Limiting Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_rate_limited_total` | counter | Requests rejected by the rate limiter (returned 429) |
| `oxphp_php_deny_total` | counter | Requests blocked by `PHP_DENY_PATHS` (`.php` execution denied). See [PHP Execution Deny-List](../security/php-deny.md) |
| `oxphp_path_deny_total` | counter | Requests denied by a `.oxphpdeny` rule; a request `PHP_DENY_PATHS` denies too is counted here only. See [Path Deny Rules](../security/oxphpdeny.md) |

## Static File Cache Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_static_cache_hits_total` | counter | Static file requests served from the in-memory cache |
| `oxphp_static_cache_misses_total` | counter | Static file requests that required a disk read |

## Compression Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_compressed_responses_total` | counter | Responses sent under a content coding (Brotli, zstd, or gzip) |
| `oxphp_compression_bytes_saved_total` | counter | Total bytes saved by compression (original size minus compressed size) |

## Worker Mode Metrics

These metrics are only emitted when worker mode is active (`WORKER_MODE_ENABLED=true`).

### Global Counters

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_mode_enabled` | gauge | Always `1` when worker mode is active |
| `oxphp_worker_requests_handled_total` | counter | Total requests processed by persistent workers |
| `oxphp_worker_recycles_total` | counter | Total worker recycles — a worker's loop ended on one of the reasons below. A dynamic pool retiring an idle worker also ends its loop but is scaling down rather than recycling: it is counted in `oxphp_workers_retired_total`, not here or in the series below |
| `oxphp_worker_recycles_by_reason_total` | counter | Recycles by reason. Label: `reason` (`scheduled`, `max_memory`, `error`) |
| `oxphp_worker_soft_resets_total` | counter | Total soft resets performed between requests |

### Per-Worker Gauges

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_worker_memory_bytes` | gauge | PHP heap held by each worker at the end of its last request — the quantity `memory_get_usage()` returns inside a handler, and the one `WORKER_MAX_MEMORY_MIB` is measured against. It is read as the response is sent, once the finished request has let go of its superglobals, and the ceiling is checked after that; with other requests multiplexed on the worker, what they allocate or free in between moves the two apart, so set an alert below the ceiling rather than at it. Written when a request finishes, so a worker that has never served one reads zero. Label: `worker` (slot index, e.g., `"0"`, `"1"` — a recycled worker reuses its predecessor's slot and its last value with it, until the replacement finishes a request of its own) |
| `oxphp_worker_uptime_seconds` | gauge | Seconds since each worker was spawned. Label: `worker` |
| `oxphp_worker_requests_count` | gauge | Requests handled by each worker instance. Label: `worker` |
| `oxphp_worker_request_fibers_active` | gauge | Request fibers the worker is carrying right now — one per request it has taken and not yet finished, so it reads `0` on an idle worker, `1` on one serving a request start to finish, and higher where requests suspend and are multiplexed on the same thread. A worker admits at most 256 request fibers, and one sitting at that number has stopped taking work off the queue while continuing to run; see [Reading the fiber count](#reading-the-fiber-count). Label: `worker` (slot index, the same slots as the gauges above) |

#### Reading the fiber count

`oxphp_worker_request_fibers_active` is written by the worker’s own loop on every turn, and again on each of the two paths before the loop enters a handler, rather than when a request completes: a worker that has stopped completing requests is the state this number is for, and a figure refreshed at completion would stand frozen at its last healthy reading exactly then.

What the rest of `/metrics` says about a worker at the 256-fiber limit depends on how it got there, which is why the two are worth reading together: `oxphp_busy_workers` counts worker threads with a request in flight, so 256 fibers on a worker the pool counts as idle are fibers that outlived the requests that made them — those requests were answered and their fibers were never reclaimed — while 256 on a busy worker is that many requests genuinely in flight. Neither reading on its own separates a worker that has stopped accepting work from a quiet or a loaded one; this gauge does.

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
| `oxphp_async_tasks_stranded_total` | counter | Workers left running past an `await_race` / `await_any` timeout. Each stranded task can extend RSHUTDOWN by up to 5 seconds — or until it stops, when its code is not in OPcache shared memory ([details](../features/async-promises.md#timeouts-cancel-the-abandoned-task)). |
| `oxphp_async_tasks_in_flight` | gauge | Async tasks currently queued or running (emitted once the pool wires its in-flight counter) |
| `oxphp_async_tasks_in_flight_limit` | gauge | Maximum concurrent async tasks (`ASYNC_MAX_FIBERS × ASYNC_WORKERS`) |
| `oxphp_async_output_discarded_bytes_total` | counter | Bytes of async-task output discarded at worker idle (an `echo` in an async task has no client to receive it) |

## OpenTelemetry Export Metrics

These metrics require `OTEL_ENABLED=true`. They count the batches of spans OxPHP sends to the OTLP collector — the request spans, and the APM spans when the APM plugin is on.

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_otel_export_failures_total` | counter | Span exports that failed — after the retry, where one was made. Their spans are not sent again |
| `oxphp_otel_export_failed_spans_total` | counter | Spans carried by those failed exports |
| `oxphp_otel_export_retries_total` | counter | Exports retried once because the first attempt failed fast, without an answer from the collector |

An export is retried once when its first attempt fails within half of `OTEL_EXPORTER_OTLP_TIMEOUT`, and at most one second, without an answer from the collector: the connection was reset or closed, or could not be made — most often because a load balancer or NAT dropped it while it sat idle between exports. An error status the collector answers with, such as a rejected API key or throttling, is not retried: the OTLP specification forbids retrying most of them, and the exporter does not pass on the delay a throttling collector asks for. A retry that delivers the batch is counted only in `oxphp_otel_export_retries_total`. An attempt that runs into the export timeout is not retried, so a slow collector is not sent the same batch twice. When the collector did receive the first attempt before the connection broke, the retry delivers those spans a second time. For the same reason, an export counted as failed may still have reached the collector.

Spans the tracer drops before export, because its queue (`OTEL_BSP_MAX_QUEUE_SIZE`, 2048 spans by default) is full, are not counted here.

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

This is a true fraction between `0` and `1`. Sustained values at `1` mean every worker is occupied and further arrivals are queueing; pair it with `rate(oxphp_admission_refused_total{reason=~"queue_full|wait_timeout|waiting_full|waiting_bytes"}[5m])` to see whether that backlog is turning into refusals, and with `oxphp_pending_requests` to see how deep it is.

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
- [Diagnosing Overload](overload.md) — reading the queue, admission and wait-budget metrics together during an overload
- [Graceful Shutdown](graceful-shutdown.md) — how connection draining affects `oxphp_active_connections`
- [Worker Mode](../features/worker-mode.md) — persistent workers and the metrics they emit
