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
| `oxphp_accept_stalled` | gauge | `1` while the accept loop is parked waiting for a free `MAX_CONNECTIONS` permit — connections are accepted by the kernel but nothing new is served, so from a client's point of view the server has stopped answering. Alert on `oxphp_accept_stalled == 1` directly; it needs no comparison against the configured budget |
| `oxphp_accept_stalls_total` | counter | Connections that had to wait for a `MAX_CONNECTIONS` permit after being accepted. Moves even when a stall starts and ends between two scrapes, which the gauge alone would miss — `rate(oxphp_accept_stalls_total[5m]) > 0` means the budget is being exhausted, however briefly. Raise `MAX_CONNECTIONS` or shrink the PHP backlog (`QUEUE_CAPACITY` + `QUEUE_MAX_WAITING`), see the [configuration reference](configuration.md) |
| `oxphp_pending_requests` | gauge | PHP requests accepted but not yet answered — waiting for a queue slot, queued, or executing. Only requests routed to PHP: a static file, a 404 or a denied path is answered without the queue and never appears here |
| `oxphp_dropped_requests_total` | counter | Requests where the PHP worker failed after accepting the request |
| `oxphp_admission_refused_total` | counter | Requests answered without reaching a worker. Label: `reason` — `wait_timeout` (waited the full `QUEUE_WAIT_TIMEOUT_MS`, give the pool more headroom), `waiting_full` (already `QUEUE_MAX_WAITING` requests waiting, raise that or `MAX_CONNECTIONS`), `waiting_bytes` (the bodies already parked fill `QUEUE_MAX_WAITING_BYTES`, so this request's body had nowhere to sit — raise that, or lower how much a client may upload), `queue_full` (`QUEUE_WAIT_TIMEOUT_MS=0`, waiting is off), `shutting_down` (the drain deadline passed while the request was still waiting for admission), `pool_unavailable` (no worker thread is left to hand the request to — the pool is gone, not busy). Only the first four are overload and answer 529; `shutting_down` answers 503 like the rest of graceful drain, and `pool_unavailable` answers 500. Alert on overload with those four specifically: the metric as a whole also moves on a restart. Excluded from `oxphp_queue_wait_us` |
| `oxphp_queue_depth` | gauge | Requests sitting in the worker queue right now: admitted, not yet picked up by a worker. Present only for the SAPI executor, which is the only one with a queue |
| `oxphp_queue_capacity` | gauge | Queue slots in total — `QUEUE_CAPACITY`, the bound `oxphp_queue_depth` is read against |
| `oxphp_admission_slots_available` | gauge | Admission permits nobody is holding. A request takes one before it enters the queue and gives it back when a worker picks it up, so this is the free capacity a new arrival can claim without waiting. During a graceful drain the gate is closed and admits nothing regardless of what this reads |

### Reading queue depth against admission slots

The two gauges answer different halves of one question and are only meaningful together, and the question is not "is the queue deep" but "is anything moving". `oxphp_queue_depth` says what is waiting; `oxphp_admission_slots_available` says whether anything more can be let in. Read both against `oxphp_busy_workers`.

| slots available | queue depth | reading |
|---|---|---|
| `> 0` | `0` | nothing waiting — normal |
| `> 0` | `> 0` and falling between scrapes | ordinary backlog: the pool is behind but working through it |
| `> 0` | `> 0` and **not** falling, with `oxphp_busy_workers` at `0` | the pool has stopped taking work off the queue while admission still has room. This is a wedge in its first phase: arrivals are still being admitted, so no refusal counter moves and every other series reads as healthy |
| `0` | at `oxphp_queue_capacity` | the same wedge once the queue has filled, or an ordinary overload the pool is far behind on. `oxphp_busy_workers` at `0` sustained across scrapes separates them — a single sample can read that way under overload too, because a worker refusing a request whose budget expired at pickup is never marked busy |
| `0` | at or near `0` | the permits are held outside the queue. For a single scrape that can be requests in flight between admission and dispatch, a window of microseconds; **sustained across scrapes it is permits taken and never given back** — nothing is queued, nothing can be admitted, and the server refuses every PHP request until it is restarted |

The last three rows are the shapes a stalled pool takes, and they are the reason these gauges exist. In each of them every other series reads as healthy — no busy worker, both health probes `200`, static files served normally — while no PHP request completes. `oxphp_pending_requests` is no help either way: it counts requests from the moment they are routed to PHP, so it rises with the stuck queue while clients are still connected and falls back to zero as they time out and give up, without either movement meaning anything about the pool. Only the fourth row is also reachable under ordinary overload, and its own text says how to tell the two apart. The third row is the shape the fault takes first, and the one that had no signal at all before: the queue is nowhere near full, so nothing is refused and nothing is slow, there is simply a growing set of requests that will never be answered.

The server notices it too. Whenever two consecutive scans see work waiting — requests in the queue, or refusals climbing — with at least one worker idle and the pool getting nothing done, it logs

```json
{"timestamp":"2026-08-28T10:56:59.015691Z","level":"WARN","fields":{"message":"PHP requests are waiting while the pool has idle workers and got nothing done since the last scan","queue_depth":22,"queue_capacity":512,"admission_slots_available":490,"workers_idle":4}}
```

The fields sit under `fields`, which is where this server's JSON formatter puts them — an alert rule wants `.fields.queue_depth`, not `.queue_depth`.

once on entry and about once a minute for as long as it lasts, and an `INFO PHP pool is reaching workers again` once work starts moving. Three things keep it from crying wolf. Two scans rather than one, because a request occupies the queue for the microseconds between admission and pickup and a single sample of that is not a fault. Progress measured as what the workers finished rather than what clients received, so a storm of client aborts — where the client is gone before any completion can be recorded, while the workers' own count still sees every one of those requests end — reads as the busy pool it is. And nothing at all reported until the pool has finished its first request, so the application bootstrap at startup is not mistaken for a wedge.

That last guard covers the process's first bootstrap only. The counter behind it is pool-wide and monotonic, so once any request has been served it never reads zero again — while a worker recycled later, on the memory ceiling or on `Worker::scheduleExit()`, boots its application again from scratch. The pool counts that worker as present the moment its thread is spawned, and it reads as idle until it takes its first request, so a bootstrap that runs into seconds looks exactly like a wedge. On a pool of one worker there is nothing else that can move the counter meanwhile, and each such recycle under traffic produces one spurious warning followed by its recovery notice; on a larger pool it takes every other worker being inside a request for the whole two-scan window. Treat a warning that clears itself within seconds of a recycle as this, not as a wedge — `oxphp_workers_spawned_total` moving at the same moment is what distinguishes them.

The warning is emitted in worker mode only. That second guard needs a count of what the workers finished, and worker mode is the only pool that keeps one; without it the rule would be left judging progress by completions, which is the misreading it exists to avoid. Nothing is lost: the state is a worker parked with a request it will never run, which cannot arise where a worker blocks on the queue itself. The three gauges are exported in every mode regardless.

Note the queue does not have to be full for this: the fault starts as a queue nobody is draining, where arrivals are still admitted and nothing is refused at all, and only becomes 529s once the queue fills. The warning fires in the first phase, which is why it does not wait for a refusal counter to move.


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
| `oxphp_worker_long_running_total` | counter | Supervisor scans that observed a request older than the stuck threshold. Label: `worker_id` |
| `oxphp_worker_stuck_total` | counter | Stuck-classification counter per worker. Labels: `worker_id`, `kind` (`io`, `c_call`, `cpu`) |

## Queue Wait Histogram

| Metric | Type | Description |
|--------|------|-------------|
| `oxphp_queue_wait_us` | histogram | Time a request waits in the queue before a worker picks it up, in microseconds |

Bucket boundaries (microseconds): `50`, `100`, `250`, `500`, `1000`, `2500`, `5000`, `10000`, `50000`, `100000`, `250000`, `500000`, `1000000`, `+Inf`.

This measures time spent waiting — for admission and then in the queue — with the script's own execution time subtracted, so it answers "how long before a worker picked this up" rather than "how long did the request take". Requests refused with 529 never queued and are not recorded here; count those with `oxphp_admission_refused_total`.

High queue wait times indicate that all workers are busy and you should increase `PHP_WORKERS`. The range reaches one second, matching the default `QUEUE_WAIT_TIMEOUT_MS`, so a request that spent most of its wait budget before running is quantified rather than lumped into `+Inf`. Nothing that gets served waits longer than the budget — past it the request is refused instead — so raising `QUEUE_WAIT_TIMEOUT_MS` is the one setting that puts waits back into `+Inf`.

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
| `oxphp_compressed_responses_total` | counter | Responses sent under a content coding (Brotli, zstd, or gzip) |
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
| `oxphp_worker_memory_bytes` | gauge | PHP heap held by each worker at the end of its last request — the quantity `memory_get_usage()` returns inside a handler, and the one `WORKER_MAX_MEMORY_MIB` is measured against. It is read a little earlier in the loop than that ceiling check, while the finished request's superglobals are still on the heap, so it sits a few KB above the figure that actually trips a recycle: set an alert below the ceiling rather than at it. Written when a request finishes, so a worker that has never served one reads zero. Label: `worker` (slot index, e.g., `"0"`, `"1"` — a recycled worker reuses its predecessor's slot and its last value with it, until the replacement finishes a request of its own) |
| `oxphp_worker_uptime_seconds` | gauge | Seconds since each worker was spawned. Label: `worker` |
| `oxphp_worker_requests_count` | gauge | Requests handled by each worker instance. Label: `worker` |
| `oxphp_worker_request_fibers_active` | gauge | Request fibers the worker is carrying right now — one per request it has taken and not yet finished, so it reads `0` on an idle worker, `1` on one serving a request start to finish, and higher where requests suspend and are multiplexed on the same thread. Written by the worker’s own loop on every turn, and again on each of the two paths before the loop enters a handler, rather than when a request completes: a worker that has stopped completing requests is the state this number is for, and a figure refreshed at completion would stand frozen at its last healthy reading exactly then. A worker admits at most 256 request fibers, and one sitting at that number has stopped taking work off the queue while continuing to run. What the rest of `/metrics` says about such a worker depends on how it got there, which is why the two are worth reading together: `oxphp_busy_workers` counts worker threads with a request in flight, so 256 fibers on a worker the pool counts as idle are fibers that outlived the requests that made them — those requests were answered and their fibers were never reclaimed — while 256 on a busy worker is that many requests genuinely in flight. Neither reading on its own separates a worker that has stopped accepting work from a quiet or a loaded one; this gauge does. Label: `worker` (slot index, the same slots as the gauges above) |

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
- [Graceful Shutdown](graceful-shutdown.md) — how connection draining affects `oxphp_active_connections`
- [Worker Mode](../features/worker-mode.md) — persistent workers and the metrics they emit
