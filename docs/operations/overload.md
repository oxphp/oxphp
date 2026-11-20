---
title: Diagnosing Overload
description: How to read the queue, admission and wait-budget metrics together, and what OxPHP logs while it sheds requests or the worker pool stops taking work.
---

# Diagnosing Overload

A server answering `529`, or holding PHP requests that never complete, is either overloaded or has a pool that stopped taking work off its queue, and the two call for different fixes. This page shows how to tell them apart from the [Connection Metrics](metrics.md#connection-metrics) gauges and from what the server logs while it sheds requests, and how the wait budget behind `QUEUE_WAIT_TIMEOUT_MS` moves.

## Reading queue depth against admission slots

The two gauges answer different halves of one question and are only meaningful together, and the question is not "is the queue deep" but "is anything moving". `oxphp_queue_depth` says what is waiting; `oxphp_admission_slots_available` says whether anything more can be let in. Read both against `oxphp_busy_workers`.

| slots available | queue depth | reading |
|---|---|---|
| `> 0` | `0` | nothing waiting — normal |
| `> 0` | `> 0` and falling between scrapes | ordinary backlog: the pool is behind but working through it |
| `> 0` | `> 0` and **not** falling, with `oxphp_busy_workers` at `0` | the pool has stopped taking work off the queue while admission still has room. This is a wedge in its first phase: arrivals are still being admitted and each is answered when its own budget runs out, wherever one is configured — a `529` where the client is still waiting for it, and `oxphp_admission_wait_wasted_total` climbs with every one of those while the budget is still at its ceiling, since no request began running while any of them waited. A client that gave up first gets a `499` and moves neither series, so under a wedge, where nothing completes and patience runs out quickly, the count can be well below the arrivals; `oxphp_queue_depth` is the part of this row that does not depend on who is still waiting. Nothing else gives it away: no worker is busy, and the refusals on their own read like overload |
| `0` | at `oxphp_queue_capacity` | the same wedge once the queue has filled, or an ordinary overload the pool is far behind on. `oxphp_busy_workers` at `0` sustained across scrapes separates them — a single sample can read that way under overload too, because a worker refusing a request whose budget expired at pickup is never marked busy |
| `0` | at or near `0` | the permits are held outside the queue. For a single scrape that can be requests in flight between admission and dispatch, a window of microseconds; **sustained across scrapes it is permits taken and never given back** — nothing is queued, nothing can be admitted, and the server refuses every PHP request until it is restarted |

The last three rows are the shapes a stalled pool takes, and they are the reason these gauges exist. In each of them the rest of the picture reads as healthy — no busy worker, `200` from liveness throughout and from readiness for the first minute, static files served normally — while no PHP request completes.

The refusal counters do move, from the first arrival after the pool stops, but they move under ordinary overload too and so cannot carry the state by themselves. `oxphp_pending_requests` is no help either way: it counts requests from the moment they are routed to PHP, so it rises with the stuck queue and then falls back towards zero as each arrival is answered on its own budget, without either movement meaning anything about the pool. Read against `oxphp_queue_depth` it does say something: the refused requests leave the count but not the queue, so `oxphp_pending_requests` near zero while `oxphp_queue_depth` sits at capacity is the wedge holding everything it was ever given.

Only the fourth row is also reachable under ordinary overload, and its own text says how to tell the two apart.

The third row is the shape the fault takes first: the queue is nowhere near full and nothing is slow. Clients are not left hanging, as long as `QUEUE_WAIT_TIMEOUT_MS` is not `0` — each gets a `529` on its own budget — but the requests behind those refusals are never run and never leave the queue, because only a pickup releases an entry.

`oxphp_admission_wait_wasted_total` moves once per arrival that waited the budget out, which is what separates this from a pool that is merely behind — with the caveats the third row carries:

- it counts only while the budget is at its ceiling;
- an arrival whose client had already gone when the budget ran out is a `499` and moves neither refusal series, and a wedge is exactly the condition that makes clients leave, so behind a load balancer whose read timeout is shorter than `QUEUE_WAIT_TIMEOUT_MS` this counter can stay quiet through the whole incident.

`oxphp_request_cancelled_total{reason="client_abort"}` is where those arrivals go, and `oxphp_queue_depth` holds either way.

The server notices it too. Whenever two consecutive scans see work waiting — requests in the queue, or refusals climbing — with at least one worker idle and the pool getting nothing done, it logs

```json
{"timestamp":"2026-08-28T10:56:59.015691Z","level":"WARN","fields":{"message":"PHP requests are waiting while the pool has idle workers and got nothing done since the last scan","queue_depth":22,"queue_capacity":512,"admission_slots_available":490,"workers_idle":4}}
```

The fields sit under `fields`, which is where this server's JSON formatter puts them — an alert rule wants `.fields.queue_depth`, not `.queue_depth`.

once on entry and about once a minute for as long as it lasts, and an `INFO PHP pool is reaching workers again` once work starts moving. Three things keep it from crying wolf. Two scans rather than one, because a request occupies the queue for the microseconds between admission and pickup and a single sample of that is not a fault. Progress measured as what the workers finished rather than what clients received, so a storm of client aborts — where the client is gone before any completion can be recorded, while the workers' own count still sees every request they began end — reads as the busy pool it is. And nothing at all reported until the pool has finished its first request, so the application bootstrap at startup is not mistaken for a wedge.

That last guard covers the process's first bootstrap only. The counter behind it is pool-wide and monotonic, so once any request has been served it never reads zero again — while a worker recycled later, on the memory ceiling or on `Worker::scheduleExit()`, boots its application again from scratch. The pool counts that worker as present the moment its thread is spawned, and it reads as idle until it takes its first request, so a bootstrap that runs into seconds looks exactly like a wedge. On a pool of one worker there is nothing else that can move the counter meanwhile, and each such recycle under traffic produces one spurious warning followed by its recovery notice; on a larger pool it takes every other worker being inside a request for the whole two-scan window. Treat a warning that clears itself within seconds of a recycle as this, not as a wedge — `oxphp_workers_spawned_total` moving at the same moment is what distinguishes them.

The warning is emitted in worker mode only. That second guard needs a count of what the workers finished, and worker mode is the only pool that keeps one; without it the rule would be left judging progress by completions, which is the misreading it exists to avoid. Nothing is lost: the state is a worker parked with a request it will never run, which cannot arise where a worker blocks on the queue itself. The three gauges are exported in every mode regardless.

Note the queue does not have to be full for this: the fault starts as a queue nobody is draining while admission still has room. Arrivals are refused from the first one after the wedge, each on its own budget, so a refusal counter by itself cannot tell the state from ordinary overload — which is why the rule watches the queue too. A queue that is not falling while a worker sits idle is the fault itself rather than a symptom it shares with a pool that is simply behind.

## What the log says while requests are being shed

The refusal counter is what an alert reads, and for a while it was the only thing that knew: an instance turning away a fifth of its arrivals for overload wrote not one line about it, so the log of an overloaded server and the log of an idle one were the same log. Whoever went looking there during the incident — which is when a log is read — found no mention of it.

The supervisor watches the four overload reasons of `oxphp_admission_refused_total` on the same one-second scan as the gauges above, and reports the episode rather than the requests: at the rates this happens at, a line per refusal would itself be the outage. The first scan that sees any of those four move logs

```json
{"timestamp":"2026-09-16T11:02:41.337104Z","level":"WARN","fields":{"message":"PHP admission has started shedding requests under overload","refused":4812,"refused_queue_full":0,"refused_wait_timeout":4812,"refused_waiting_full":0,"refused_waiting_bytes":0,"queue_depth":896,"queue_capacity":896,"admission_slots_available":0,"wait_budget_us":15625,"wait_budget_ceiling_us":1000000}}
```

`refused` is what that one scan turned away, not a running total — with one exception, the first scan that has a queue to look at, which reports everything shed before it: a pool still loading its application while arrivals pile up in front of it is a real episode, and seeding the window on that scan would have thrown it away. The queue numbers are what make the line diagnostic rather than merely alarming: a depth at capacity with no admission slot free is a pool behind on its work, and a `wait_budget_us` well under `wait_budget_ceiling_us` says the server has already shortened the wait on its own. Both numbers are on the line because the reading needs both: a one-second budget driven to its floor prints `15625`, and a server configured with a budget of that order prints a figure indistinguishable from it at a glance, so only the ceiling beside it separates "already reacting" from "this is what you asked for". Read the queue figures as a snapshot taken at the scan, not as a picture of the refusals beside them: those cover the second that has just passed, so a burst already drained prints a large `refused` next to an empty queue — the shape of a short episode, not a contradiction.

The opening line's count and the episode's are broken down by reason, into the four overload labels of `oxphp_admission_refused_total` named after the count they split: `refused_queue_full`, `refused_wait_timeout`, `refused_waiting_full` and `refused_waiting_bytes` add up to `refused`, and the repeat and the closing line below carry `episode_refused_queue_full` and its three siblings, which add up to `episode_refused`. The reason is what decides the knob — the counter's row in [Connection Metrics](metrics.md#connection-metrics) says what each one points at — and that counter is served only on `INTERNAL_ADDR`, so on an instance without an internal listener these fields, and the `oxphp.shed_reason` span attribute where tracing is on, are where the reason can still be read. All four are written even at zero: an explicit `0` is what rules a reason out.

While the episode lasts the warning repeats, as `PHP admission is still shedding requests under overload`, no more often than once a minute and only on a scan that is itself refusing — so a repeat that falls due during a quiet stretch waits for the next scan that refuses something, and the gap between two of them can run to almost two minutes. Refusals spaced further apart than a minute do not stretch that gap; they end the episode and open the next one. The repeat carries `episode_refused` — the running total for the episode — beside the same per-scan `refused`, and it is the episode total it breaks down by reason, not the scan's: the closing line is written at `INFO`, below the `warn` the production configurations in [Configuration](configuration.md) log at, so on those the repeat is the only line that says what the episode has been made of so far — up to the scan it was written on. An episode that ends inside its first minute has no repeat at all, and what is refused after the last repeat reaches only the closing line. An entry line on its own would not survive the incident it describes: by the time an operator opens the log, an hour of ordinary traffic has scrolled it away.

A minute with no refusal at all ends the episode, at `INFO`:

```json
{"timestamp":"2026-09-16T11:05:52.812440Z","level":"INFO","fields":{"message":"PHP admission has stopped shedding requests","episode_refused":18374,"episode_refused_queue_full":0,"episode_refused_wait_timeout":17950,"episode_refused_waiting_full":424,"episode_refused_waiting_bytes":0,"duration_ms":131000,"queue_depth":0,"admission_slots_available":896}}
```

`duration_ms` runs to the last refusal rather than to the line, so the quiet minute that ends the episode is not counted into it — which is why the two samples sit 191 seconds apart while the second reports 131 of them. The fields sit under `fields`, as in the sample above — an alert rule wants `.fields.episode_refused`, not `.episode_refused`.

A whole minute of quiet rather than the first quiet scan, because shedding at the edge of capacity is bursty: a queue that clears for two seconds and fills again is one episode, and a pair of lines per burst would bury what they report. The price is paid in the other direction — an episode reads as lasting up to a minute longer than it did, and a genuine gap shorter than a minute is folded into the episode around it. What that buys is a bound: two lines a minute at the very worst, whatever the traffic. Entry has no such delay, and deliberately not: a refusal is a request that was answered `529`, a fact rather than a sample that might be an artifact of when the scan happened to land.

The report is emitted in every routing mode — unlike the stalled-pool warning above, it needs only the queue, which every mode has. It moves on the four overload reasons alone. `shutting_down` and `pool_unavailable` are left out, so an ordinary restart and a pool that is gone do not announce themselves as load.

The state goes nowhere else. There is no gauge for it — the refusal counters in [Connection Metrics](metrics.md#connection-metrics) are what an alert rule reads — and no probe is told about it: shedding on its own never changes what `/health/readiness` answers, deliberately, because an instance shedding `529`s is answering quickly on a working pool, and an overload hitting every replica at once would otherwise take them out of rotation one after another until the service had no endpoints left. Overload is answered with capacity, not with rotation. That is a statement about load and not a promise that an episode always comes with a `200`: a wedged pool sheds too, and in worker mode that state answers `503` once it has held for a minute, as the section above describes. What separates them is the queue — a pool that is merely behind drains it, a wedged one does not.

## The wait budget

### How the wait budget comes down and goes back up

The server shortens the wait budget below the `QUEUE_WAIT_TIMEOUT_MS` ceiling on its own while the work the waiting admits is completing for clients who have already left, and lengthens it back once the queue drains. `oxphp_admission_wait_budget_us` publishes what is in force and `oxphp_admission_wait_budget_ceiling_us` what was configured.

The shortening is not gentle and is worth knowing the size of before choosing a ceiling:

- the budget is **halved** each time a quarter or more of what the pool started went to clients who had already gone, measured over a window of at least eight started requests rather than a fixed interval;
- it can go down to a floor of a sixty-fourth of the ceiling — 15.6 ms at the default — or 10 ms, whichever is larger, though never above the ceiling itself;
- it walks back up in eighths of the ceiling, and only while the queue is observed empty and less than a quarter of what the pool started went to departed clients — so the ordinary trickle of clients closing a tab or timing out mid-request does not hold it down, and a server whose queue never empties stays on the shortened budget until the load lets up.

An operator who has set `QUEUE_WAIT_TIMEOUT_MS=1000` should expect the 15.6 ms floor to be in force during an episode, and an alert on the wait should be written against the pair of series rather than against `QUEUE_WAIT_TIMEOUT_MS`.

`oxphp_admission_wait_budget_us` below the ceiling means the pool is being offered more than it can serve to clients less patient than the ceiling — the waiting is handing workers requests nobody is there to receive, and the shorter budget is what stops that. Sitting at the floor (a sixty-fourth of the ceiling, raised to 10 ms unless the ceiling itself is shorter than that) through an episode is normal; sitting there with `oxphp_queue_depth` at zero is not, and means a quarter or more of what the pool is still finishing on an idle queue is for departed clients.

### The deadline is enforced from both ends

The deadline is enforced from both ends: the request is answered with a 529 when it passes, and a worker that reaches a request past it refuses rather than executes it. Enforcing it only at pickup would make the budget a rule about which requests run rather than about how long anyone waits — a queued request nobody picks up would wait indefinitely, which is what [an application calling back into this same server](#when-the-application-calls-itself) produces. It does not bound how long the handler then runs: a request a worker started inside the budget is left alone.

### Clients that leave while they wait

A client that closes the connection mid-wait gives its place back immediately, on HTTP/1.1 as well as HTTP/2, so a balancer that times out and closes does not fill the waiting set with attempts it has already abandoned. A request already in the queue when its client closes keeps its place until a worker reaches it or the budget runs out, and is then answered `499` rather than counted as an overload refusal.

What the server cannot see is a client that stops waiting *without* closing — it holds its place until it is admitted or the budget runs out, and if a worker frees up first its script runs for nobody — so keeping the budget below the timeout of anything in front of it is still worth doing. The server's own shortening does not cover that case either: it is driven by clients who close, so a population that stops waiting without closing produces no signal and leaves the ceiling in force.

## Reading the wasted-wait and abandoned-work counters

### Reading `oxphp_admission_wait_wasted_total`

What counts is work started, not entries leaving the queue — a worker clearing requests their deadlines already answered moves the queue without serving anybody, and reading that as progress would silence this counter exactly where it is needed.

Read only while `oxphp_admission_wait_budget_us` is at its ceiling. Over a window the server has already shortened, "the pool began nothing" says more about the window than about the pool. A healthy pool of four workers on quarter-second handlers starts one request every 62 ms, so once the budget is down at its floor most waits end between two starts and this counter would climb on a pool that is serving perfectly well.

What the gating costs depends on whether the budget moves at all. The server reads the abandonment ratio only once the pool has started enough requests for that ratio to mean anything. A pool that starts nothing never reaches that count behind a queue, so it never reaches a decision that could shorten it and holds its ceiling on its own: the gating costs nothing there and this counter stays armed. That holds whether or not its workers are still answering requests whose clients had already gone, which moves `oxphp_abandoned_work_total` without starting a thing. A self-calling pool is a different case, covered under [When the application calls itself](#when-the-application-calls-itself).

Sustained non-zero means waiting is buying nothing on those requests at full length: shorten `QUEUE_WAIT_TIMEOUT_MS`, so they are refused sooner instead of holding a connection for most of the budget.

The classic cause is an application calling back into this same server over HTTP — the inner request needs a worker the outer one is holding while it waits for the inner one, so no amount of waiting can succeed — but it is not a self-call detector: a pool whose handlers all run longer than the budget frees nothing during a wait either, and reads the same. What separates them is how long the handlers take, and in worker mode `oxphp_worker_request_duration_us` measures exactly that — handler time, with the queue wait excluded — so handlers sitting below the budget while this counter climbs means the waits are not merely queued behind slow work. Do not read `oxphp_request_duration_us` for it: that one is end-to-end over every request including static files, and it counts these refusals themselves, each of which contributes very nearly a whole budget, so it rises as this counter does. Outside worker mode there is no handler-time series, and the question has to be answered from the application's own timings.

### Reading `oxphp_abandoned_work_total`

This is what the wait budget (`oxphp_admission_wait_budget_us`) is adjusted against, and the two read together say whether the shortening is working:

- the counter climbing while the gauge falls is the controller doing its job;
- the counter climbing with the gauge still at its ceiling means the clients are leaving during their handler rather than during their wait, so the wait is not what to shorten.

It is a count of requests, not a measure of wasted CPU, and deliberately so: a request whose client left while it was queued is answered at the pickup before any PHP starts, on either pool model, while one whose client left during its handler has cost a whole handler — very different prices for the same statement about the wait, and the wait is what the budget controls.

It does not count a request whose response went out before its handler finished — a stream whose headers were sent, or an early answer from `oxphp_finish_request()`. A client closing a response it already has is done with it rather than tired of waiting, which is what the end of every SSE session looks like, so such a request is left out even when its client was gone before the response went out.

## When the application calls itself

Lower `QUEUE_WAIT_TIMEOUT_MS` when the application calls back into this same server over HTTP — the inner call needs a worker the outer one is holding while it waits for the inner one, so the wait is spent for nothing however long it is given.

`oxphp_admission_wait_wasted_total` counts those waits, but only while the budget is at its ceiling. Whether it stays there is not a property of the self-call: the budget comes down on work finishing for clients who have already left, which the server can only read from a queue it finds non-empty when it looks and from enough started requests behind it. So the same self-caller keeps its ceiling wherever its own callers wait for it, an internal cron or a sidecar on a generous timeout being the ordinary case, and comes down only where they do not *and* those two conditions hold.

Where the budget comes down on its own it is not because the server recognised a self-call — it cannot see where a request came from — so where the callers are patient, as an internal cron or a sidecar on a generous timeout is, nothing abandons, the ceiling stands and `oxphp_admission_wait_wasted_total` is what names it.

Where the budget does come down, that counter goes quiet — and so it does when a load balancer in front has a shorter timeout of its own — so a `0` is not by itself the absence of this pattern.

### The counter on a self-calling pool

A self-calling pool is not the pool that starts nothing, on which [the gating costs nothing](#reading-oxphp_admission_wait_wasted_total). It starts a request every time an outer one gives up on its inner call, and once its own callers are less patient than its handlers, a pool still holding work behind them gives the controller the evidence it decides on and the budget comes down. Measured, that was two halvings within seconds of the load beginning, after which `oxphp_admission_wait_wasted_total` stopped moving: the waits ended by a departed client are excluded, and what is left is armed by the budget it *arrived* on, so the arrivals from before the drop go on being counted for up to a whole ceiling after it and then nothing is. In that measurement it moved a handful of times while the ceiling still stood at the start of the load, once more for an arrival that had preceded the drop, and then not again through the episode. Nothing in `/metrics` takes over from it there.

A budget under its ceiling with `oxphp_abandoned_work_total` climbing is the controller's own input and its own output, and an ordinary overload of impatient clients reads exactly that. Measured on a pool with no self-call in it at all: the budget down to an eighth of its ceiling, abandoned work climbing, and `oxphp_admission_wait_wasted_total` all but still — `0` in that run.

The wedge rows of the [queue-depth table](#reading-queue-depth-against-admission-slots) do not apply either, because they read `oxphp_busy_workers` at `0` and a self-caller's workers are busy throughout, inside the handler that is doing the waiting. Where the budget has come down, the pattern is found in the application's own outbound calls rather than in this counter. Those rows do still name one case the counter misses on its own: a pool that stops taking work off its queue while an overload already had the budget down, where `oxphp_queue_depth` does not fall and no worker is busy.

### `QUEUE_WAIT_TIMEOUT_MS=0` is not a shorter wait

`0` is not the shortest version of lowering `QUEUE_WAIT_TIMEOUT_MS` but a different setting: it drops the wait for a queue *slot* and leaves the wait *inside* the queue unbounded, so a request that finds a slot free is queued with no deadline at all. For a self-call it shortens nothing: unless `QUEUE_CAPACITY` is small enough for the queue to be full, the inner call is admitted to a queue nothing is consuming and is never answered at all. On a pool picking nothing up — the one `oxphp_admission_wait_wasted_total` names — that is the one setting under which such a request is never answered, so `0` belongs with a `QUEUE_CAPACITY` small enough that the queue is full by the time it matters.

## See Also

- [Prometheus Metrics](metrics.md) — the admission, queue and refusal metrics these readings use
- [Configuration Reference](configuration.md) — `QUEUE_CAPACITY`, `QUEUE_WAIT_TIMEOUT_MS` and the other admission limits
- [Sizing PHP Workers](sizing-workers.md) — choosing the pool, queue and waiting-set sizes the gauges are read against
- [Health Checks](health-checks.md) — what `/health/readiness` answers while the pool is stalled
