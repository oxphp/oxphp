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
| `oxphp_admission_refused_total` | counter | Requests answered without reaching a worker. Label: `reason` — `wait_timeout` (waited out its whole wait budget — `QUEUE_WAIT_TIMEOUT_MS`, or less where `oxphp_admission_wait_budget_us` shows the server has shortened it; give the pool more headroom — but read `oxphp_admission_wait_wasted_total` first, because where the pool began nothing at all during those waits the remedy is the opposite one; a request whose client had already gone when that budget ran out is **not** counted here — it is answered `499` and counted in `oxphp_request_cancelled_total{reason="client_abort"}` instead, so this series stays a statement about the pool rather than about how long clients were prepared to wait), `waiting_full` (already `QUEUE_MAX_WAITING` requests waiting, raise that or `MAX_CONNECTIONS`), `waiting_bytes` (the bodies already parked fill `QUEUE_MAX_WAITING_BYTES`, so this request's body had nowhere to sit — raise that, or lower how much a client may upload), `queue_full` (`QUEUE_WAIT_TIMEOUT_MS=0`, waiting is off), `shutting_down` (the drain deadline passed while the request was still waiting for admission), `pool_unavailable` (no worker thread is left to hand the request to — the pool is gone, not busy). Only the first four are overload and answer 529; `shutting_down` answers 503 like the rest of graceful drain, and `pool_unavailable` answers 500. Alert on overload with those four specifically: the metric as a whole also moves on a restart. Excluded from `oxphp_queue_wait_us` |
| `oxphp_admission_wait_wasted_total` | counter | Requests that waited out their whole wait budget while the pool began no request at all. A request whose client had already gone when that budget ran out is excluded, on the same grounds as above. What counts is work started, not entries leaving the queue — a worker clearing requests their deadlines already answered moves the queue without serving anybody, and reading that as progress would silence this counter exactly where it is needed. A subset of `oxphp_admission_refused_total{reason="wait_timeout"}`, separating a wait that lost a race for a worker from one that never had a race to lose. Read only while `oxphp_admission_wait_budget_us` is at its ceiling. Over a window the server has already shortened, "the pool began nothing" says more about the window than about the pool — a healthy pool of four workers on quarter-second handlers starts one request every 62 ms, so once the budget is down at its floor most waits end between two starts and this counter would climb on a pool that is serving perfectly well. What the gating costs depends on whether the budget moves at all. The server reads the abandonment ratio only once the pool has started enough requests for that ratio to mean anything, and a pool that starts nothing never reaches that count behind a queue, so it never reaches a decision that could shorten it and holds its ceiling on its own — the gating costs nothing there and this counter stays armed. That holds whether or not its workers are still answering requests whose clients had already gone, which moves `oxphp_abandoned_work_total` without starting a thing. A self-calling pool is not that pool. It starts a request every time an outer one gives up on its inner call, and once its own callers are less patient than its handlers, a pool still holding work behind them gives the controller the evidence it decides on and the budget comes down: measured at two halvings within seconds of the load beginning, after which this counter stopped moving — the waits ended by a departed client are excluded, and what is left is armed by the budget it *arrived* on, so the arrivals from before the drop go on being counted for up to a whole ceiling after it and then nothing is. In that measurement it moved a handful of times while the ceiling still stood at the start of the load, once more for an arrival that had preceded the drop, and then not again through the episode. Nothing on this page takes over from it there. A budget under its ceiling with `oxphp_abandoned_work_total` climbing is the controller's own input and its own output, and an ordinary overload of impatient clients reads exactly that — measured on a pool with no self-call in it at all: the budget down to an eighth of its ceiling, abandoned work climbing, and this counter all but still — `0` in that run. The wedge rows of the table below do not apply either, because they read `oxphp_busy_workers` at `0` and a self-caller's workers are busy throughout, inside the handler that is doing the waiting. Where the budget has come down, the pattern is found in the application's own outbound calls rather than here. Those rows do still name one case this counter misses on its own: a pool that stops taking work off its queue while an overload already had the budget down, where `oxphp_queue_depth` does not fall and no worker is busy. Sustained non-zero means waiting is buying nothing on those requests at full length: shorten `QUEUE_WAIT_TIMEOUT_MS`, so they are refused sooner instead of holding a connection for most of the budget. Where the budget comes down on its own it is not because the server recognised a self-call — it cannot see where a request came from — so where the callers are patient, as an internal cron or a sidecar on a generous timeout is, nothing abandons, the ceiling stands and this counter is what names it. `0` is not the shortest version of that but a different setting: it drops the wait for a queue *slot* and leaves the wait *inside* the queue unbounded, so a request that finds a slot free is queued with no deadline at all. On the pool this counter is naming — one picking nothing up — that is the one setting under which such a request is never answered, so `0` belongs with a `QUEUE_CAPACITY` small enough that the queue is full by the time it matters. The classic cause is an application calling back into this same server over HTTP — the inner request needs a worker the outer one is holding while it waits for the inner one, so no amount of waiting can succeed — but it is not a self-call detector: a pool whose handlers all run longer than the budget frees nothing during a wait either, and reads the same. What separates them is how long the handlers take, and in worker mode `oxphp_worker_request_duration_us` measures exactly that — handler time, with the queue wait excluded — so handlers sitting below the budget while this counter climbs means the waits are not merely queued behind slow work. Do not read `oxphp_request_duration_us` for it: that one is end-to-end over every request including static files, and it counts these refusals themselves, each of which contributes very nearly a whole budget, so it rises as this counter does. Outside worker mode there is no handler-time series, and the question has to be answered from the application's own timings |
| `oxphp_queue_depth` | gauge | Requests sitting in the worker queue right now: admitted, not yet picked up by a worker. An entry leaves only on a pickup, so this counts requests already answered on their deadline — `529`, or `499` where the client had gone — alongside ones still waiting for a worker — a depth above `oxphp_pending_requests` is that difference. Present only for the SAPI executor, which is the only one with a queue |
| `oxphp_queue_capacity` | gauge | Queue slots in total — `QUEUE_CAPACITY`, the bound `oxphp_queue_depth` is read against |
| `oxphp_admission_slots_available` | gauge | Admission permits nobody is holding. A request takes one before it enters the queue and gives it back when a worker picks it up, so this is the free capacity a new arrival can claim without waiting. During a graceful drain the gate is closed and admits nothing regardless of what this reads |
| `oxphp_admission_wait_budget_us` | gauge | How long a request arriving now may wait for a worker, in microseconds — the same unit as `oxphp_queue_wait_us`, which is what it bounds. `QUEUE_WAIT_TIMEOUT_MS` is the ceiling, not the value, and is published beside it as `oxphp_admission_wait_budget_ceiling_us` so the comparison can be made from this endpoint alone: the server halves this while the work the waiting admits is completing for clients who have already left, and walks it back up once the queue drains and less than a quarter of what the pool starts is going to departed clients. Below the ceiling means the pool is being offered more than it can serve to clients less patient than the ceiling — the waiting is handing workers requests nobody is there to receive, and the shorter budget is what stops that. Sitting at the floor (a sixty-fourth of the ceiling, raised to 10 ms unless the ceiling itself is shorter than that) through an episode is normal; sitting there with `oxphp_queue_depth` at zero is not, and means a quarter or more of what the pool is still finishing on an idle queue is for departed clients. `0` in fail-fast mode (`QUEUE_WAIT_TIMEOUT_MS=0`), where there is no wait to adjust. Absent for an executor with no queue |
| `oxphp_admission_wait_budget_ceiling_us` | gauge | `QUEUE_WAIT_TIMEOUT_MS` in microseconds — the longest the gauge above is ever allowed to be. Constant for the life of the process. It exists so that the reading that matters, how far under its ceiling the budget has been driven, does not have to be made against a number hardcoded into an alert and silently wrong after the next config change — the same pairing as `oxphp_queue_depth` with `oxphp_queue_capacity`. `0` in fail-fast mode. Absent for an executor with no queue |
| `oxphp_abandoned_work_total` | counter | Requests a worker took off the queue whose client had already gone. This is what the wait budget above is adjusted against, and the two read together say whether the shortening is working: the counter climbing while the gauge falls is the controller doing its job, and the counter climbing with the gauge still at its ceiling means the clients are leaving during their handler rather than during their wait, so the wait is not what to shorten. It is a count of requests, not a measure of wasted CPU, and deliberately so: a request whose client left while it was queued is answered at the pickup before any PHP starts, on either pool model, while one whose client left during its handler has cost a whole handler — very different prices for the same statement about the wait, and the wait is what the budget controls. Requests refused on their deadline are not counted; they never ran and are reported as `oxphp_admission_refused_total` instead. Only a client that closes its connection is visible here; one that stops waiting without closing looks like any other completion. Nor does it count a request whose response went out before its handler finished — a stream whose headers were sent, or an early answer from `oxphp_finish_request()`. A client closing a response it already has is done with it rather than tired of waiting, which is what the end of every SSE session looks like, so such a request is left out even when its client was gone before the response went out |
| `oxphp_pool_stalled` | gauge | `1` while requests are waiting for workers that are idle and getting nothing done — a pool that has stopped taking work off its queue. It is set once the state has held for a minute — long enough that a worker re-loading the application after a recycle does not raise it — It is cleared by a worker finishing a request, and also by a minute in which the pool has not looked wedged at all — without that second exit the readiness `503` would remove the traffic whose completion is the first one. Alert on `oxphp_pool_stalled == 1` directly; the readiness probe answers `503` on the same state, so an orchestrated deployment sees it without an alert rule. Exported in worker mode only, because it needs the count of requests the workers get through — absent rather than `0` where the state is not watched |

### Reading queue depth against admission slots

The two gauges answer different halves of one question and are only meaningful together, and the question is not "is the queue deep" but "is anything moving". `oxphp_queue_depth` says what is waiting; `oxphp_admission_slots_available` says whether anything more can be let in. Read both against `oxphp_busy_workers`.

| slots available | queue depth | reading |
|---|---|---|
| `> 0` | `0` | nothing waiting — normal |
| `> 0` | `> 0` and falling between scrapes | ordinary backlog: the pool is behind but working through it |
| `> 0` | `> 0` and **not** falling, with `oxphp_busy_workers` at `0` | the pool has stopped taking work off the queue while admission still has room. This is a wedge in its first phase: arrivals are still being admitted and each is answered when its own budget runs out, wherever one is configured — a `529` where the client is still waiting for it, and `oxphp_admission_wait_wasted_total` climbs with every one of those while the budget is still at its ceiling, since no request began running while any of them waited. A client that gave up first gets a `499` and moves neither series, so under a wedge, where nothing completes and patience runs out quickly, the count can be well below the arrivals; `oxphp_queue_depth` is the part of this row that does not depend on who is still waiting. Nothing else gives it away: no worker is busy, and the refusals on their own read like overload |
| `0` | at `oxphp_queue_capacity` | the same wedge once the queue has filled, or an ordinary overload the pool is far behind on. `oxphp_busy_workers` at `0` sustained across scrapes separates them — a single sample can read that way under overload too, because a worker refusing a request whose budget expired at pickup is never marked busy |
| `0` | at or near `0` | the permits are held outside the queue. For a single scrape that can be requests in flight between admission and dispatch, a window of microseconds; **sustained across scrapes it is permits taken and never given back** — nothing is queued, nothing can be admitted, and the server refuses every PHP request until it is restarted |

The last three rows are the shapes a stalled pool takes, and they are the reason these gauges exist. In each of them the rest of the picture reads as healthy — no busy worker, `200` from liveness throughout and from readiness for the first minute, static files served normally — while no PHP request completes. The refusal counters do move, from the first arrival after the pool stops, but they move under ordinary overload too and so cannot carry the state by themselves. `oxphp_pending_requests` is no help either way: it counts requests from the moment they are routed to PHP, so it rises with the stuck queue and then falls back towards zero as each arrival is answered on its own budget, without either movement meaning anything about the pool. Read against `oxphp_queue_depth` it does say something: the refused requests leave the count but not the queue, so `oxphp_pending_requests` near zero while `oxphp_queue_depth` sits at capacity is the wedge holding everything it was ever given. Only the fourth row is also reachable under ordinary overload, and its own text says how to tell the two apart. The third row is the shape the fault takes first: the queue is nowhere near full and nothing is slow. Clients are not left hanging, as long as `QUEUE_WAIT_TIMEOUT_MS` is not `0` — each gets a `529` on its own budget — but the requests behind those refusals are never run and never leave the queue, because only a pickup releases an entry. `oxphp_admission_wait_wasted_total` moves once per arrival that waited the budget out, which is what separates this from a pool that is merely behind — with the caveats the row carries: it counts only while the budget is at its ceiling, and an arrival whose client had already gone when the budget ran out is a `499` and moves neither refusal series, and a wedge is exactly the condition that makes clients leave, so behind a load balancer whose read timeout is shorter than `QUEUE_WAIT_TIMEOUT_MS` this counter can stay quiet through the whole incident. `oxphp_request_cancelled_total{reason="client_abort"}` is where those arrivals go, and `oxphp_queue_depth` holds either way.

The server notices it too. Whenever two consecutive scans see work waiting — requests in the queue, or refusals climbing — with at least one worker idle and the pool getting nothing done, it logs

```json
{"timestamp":"2026-08-28T10:56:59.015691Z","level":"WARN","fields":{"message":"PHP requests are waiting while the pool has idle workers and got nothing done since the last scan","queue_depth":22,"queue_capacity":512,"admission_slots_available":490,"workers_idle":4}}
```

The fields sit under `fields`, which is where this server's JSON formatter puts them — an alert rule wants `.fields.queue_depth`, not `.queue_depth`.

once on entry and about once a minute for as long as it lasts, and an `INFO PHP pool is reaching workers again` once work starts moving. Three things keep it from crying wolf. Two scans rather than one, because a request occupies the queue for the microseconds between admission and pickup and a single sample of that is not a fault. Progress measured as what the workers finished rather than what clients received, so a storm of client aborts — where the client is gone before any completion can be recorded, while the workers' own count still sees every request they began end — reads as the busy pool it is. And nothing at all reported until the pool has finished its first request, so the application bootstrap at startup is not mistaken for a wedge.

That last guard covers the process's first bootstrap only. The counter behind it is pool-wide and monotonic, so once any request has been served it never reads zero again — while a worker recycled later, on the memory ceiling or on `Worker::scheduleExit()`, boots its application again from scratch. The pool counts that worker as present the moment its thread is spawned, and it reads as idle until it takes its first request, so a bootstrap that runs into seconds looks exactly like a wedge. On a pool of one worker there is nothing else that can move the counter meanwhile, and each such recycle under traffic produces one spurious warning followed by its recovery notice; on a larger pool it takes every other worker being inside a request for the whole two-scan window. Treat a warning that clears itself within seconds of a recycle as this, not as a wedge — `oxphp_workers_spawned_total` moving at the same moment is what distinguishes them.

The warning is emitted in worker mode only. That second guard needs a count of what the workers finished, and worker mode is the only pool that keeps one; without it the rule would be left judging progress by completions, which is the misreading it exists to avoid. Nothing is lost: the state is a worker parked with a request it will never run, which cannot arise where a worker blocks on the queue itself. The three gauges are exported in every mode regardless.

Note the queue does not have to be full for this: the fault starts as a queue nobody is draining while admission still has room. Arrivals are refused from the first one after the wedge, each on its own budget, so a refusal counter by itself cannot tell the state from ordinary overload — which is why the rule watches the queue too. A queue that is not falling while a worker sits idle is the fault itself rather than a symptom it shares with a pool that is simply behind.

### What the log says while requests are being shed

The refusal counter is what an alert reads, and for a while it was the only thing that knew: an instance turning away a fifth of its arrivals for overload wrote not one line about it, so the log of an overloaded server and the log of an idle one were the same log. Whoever went looking there during the incident — which is when a log is read — found no mention of it.

The supervisor watches the four overload reasons of `oxphp_admission_refused_total` on the same one-second scan as the gauges above, and reports the episode rather than the requests: at the rates this happens at, a line per refusal would itself be the outage. The first scan that sees any of those four move logs

```json
{"timestamp":"2026-09-16T11:02:41.337104Z","level":"WARN","fields":{"message":"PHP admission has started shedding requests under overload","refused":4812,"queue_depth":896,"queue_capacity":896,"admission_slots_available":0,"wait_budget_us":15625,"wait_budget_ceiling_us":1000000}}
```

`refused` is what that one scan turned away, not a running total — with one exception, the first scan that has a queue to look at, which reports everything shed before it: a pool still loading its application while arrivals pile up in front of it is a real episode, and seeding the window on that scan would have thrown it away. The queue numbers are what make the line diagnostic rather than merely alarming: a depth at capacity with no admission slot free is a pool behind on its work, and a `wait_budget_us` well under `wait_budget_ceiling_us` says the server has already shortened the wait on its own. Both numbers are on the line because the reading needs both: a one-second budget driven to its floor prints `15625`, and a server configured with a budget of that order prints a figure indistinguishable from it at a glance, so only the ceiling beside it separates "already reacting" from "this is what you asked for". Read the queue figures as a snapshot taken at the scan, not as a picture of the refusals beside them: those cover the second that has just passed, so a burst already drained prints a large `refused` next to an empty queue — the shape of a short episode, not a contradiction.

While the episode lasts the warning repeats, as `PHP admission is still shedding requests under overload`, no more often than once a minute and only on a scan that is itself refusing — so a repeat that falls due during a quiet stretch waits for the next scan that refuses something, and the gap between two of them can run to almost two minutes. Refusals spaced further apart than a minute do not stretch that gap; they end the episode and open the next one. The repeat carries `episode_refused` — the running total for the episode — beside the same per-scan `refused`. An entry line on its own would not survive the incident it describes: by the time an operator opens the log, an hour of ordinary traffic has scrolled it away.

A minute with no refusal at all ends the episode, at `INFO`:

```json
{"timestamp":"2026-09-16T11:05:52.812440Z","level":"INFO","fields":{"message":"PHP admission has stopped shedding requests","episode_refused":18374,"duration_ms":131000,"queue_depth":0,"admission_slots_available":896}}
```

`duration_ms` runs to the last refusal rather than to the line, so the quiet minute that ends the episode is not counted into it — which is why the two samples sit 191 seconds apart while the second reports 131 of them. The fields sit under `fields`, as in the sample above — an alert rule wants `.fields.episode_refused`, not `.episode_refused`.

A whole minute of quiet rather than the first quiet scan, because shedding at the edge of capacity is bursty: a queue that clears for two seconds and fills again is one episode, and a pair of lines per burst would bury what they report. The price is paid in the other direction — an episode reads as lasting up to a minute longer than it did, and a genuine gap shorter than a minute is folded into the episode around it. What that buys is a bound: two lines a minute at the very worst, whatever the traffic. Entry has no such delay, and deliberately not: a refusal is a request that was answered `529`, a fact rather than a sample that might be an artifact of when the scan happened to land.

The report is emitted in every routing mode — unlike the stalled-pool warning above, it needs only the queue, which every mode has. It moves on the four overload reasons alone. `shutting_down` and `pool_unavailable` are left out, so an ordinary restart and a pool that is gone do not announce themselves as load.

The state goes nowhere else. There is no gauge for it — the refusal counters above are what an alert rule reads — and no probe is told about it: shedding on its own never changes what `/health/readiness` answers, deliberately, because an instance shedding `529`s is answering quickly on a working pool, and an overload hitting every replica at once would otherwise take them out of rotation one after another until the service had no endpoints left. Overload is answered with capacity, not with rotation. That is a statement about load and not a promise that an episode always comes with a `200`: a wedged pool sheds too, and in worker mode that state answers `503` once it has held for a minute, as the section above describes. What separates them is the queue — a pool that is merely behind drains it, a wedged one does not.


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

This measures time spent waiting — for admission and then in the queue — up to the moment a worker takes the request off the queue, so it answers "how long before a worker picked this up" rather than "how long did the request take". Nothing after that moment is in it: the worker preparing the request, the script (up to the response head, for a streamed response), and the handoff of the response back to the server's I/O threads count towards `oxphp_request_duration_us` and not here. The handoff is not free — the response waits for one of those threads to be woken for it or, on a busy server, to get through the work queued ahead of it — so the time a request spends past this wait is more than its script's execution time, and on a cheap script it can be several times that. Only a request a worker took off the queue inside its budget is recorded here. Nothing else is, whichever way it was answered: a `529` refused for overload, or a `503` or `500` when the server is shutting down or the pool is gone, all counted in `oxphp_admission_refused_total`; and a `499` for a client that had gone without a worker taking its request in time, counted in `oxphp_request_cancelled_total{reason="client_abort"}` and in no refusal series at all.

A high queue wait means requests took long to reach a worker, which is not by itself a shortage of workers: every reading includes the time a worker thread takes to get to a request handed to it, and that is there on an unloaded server too. Before raising `PHP_WORKERS`, check that requests are finding every worker busy, and that the machine has cores to spare for more — the server's own I/O threads, other processes and a container CPU quota compete with the workers for them. `oxphp_busy_workers` against `oxphp_workers_current` answers the first only in part: the gauge is read when the metrics are scraped, so a burst that fills the pool between two scrapes shows up here and not there, and in worker mode a thread counts as busy while it carries any request at all, although, while that request's handler is suspended waiting on I/O, a timer or an async task, the thread goes on taking more. The range reaches one second, matching the default `QUEUE_WAIT_TIMEOUT_MS`, so a request that spent most of its wait budget before running is quantified rather than lumped into `+Inf`. Nothing that gets served waits longer than the budget — past it the request is refused instead — so raising `QUEUE_WAIT_TIMEOUT_MS` is the one setting that puts waits back into `+Inf`.

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
