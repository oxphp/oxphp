---
title: Sizing PHP Workers
description: How many PHP workers, queue slots and waiting requests to allow, how they share the connection budget, and how OxPHP counts the CPUs its defaults use.
---

# Sizing PHP Workers

The [PHP Workers](configuration.md#php-workers) settings bound how many requests run, queue and wait at once. This page works out values for them from the handler time and the connection budget, compares static and dynamic pools, and explains how the CPU count behind the defaults is detected.

# Sizing the Waiting Set

How many requests can *usefully* wait follows from how fast the pool retires them. With `W` workers and a handler that takes `T` milliseconds, the pool admits `W / T` requests per millisecond, so a budget of `B` milliseconds can let in about `W × B / T` waiters. Everything past that waits the whole budget and is refused anyway, holding a connection and a buffered request body the entire time.

The default cannot compute this — service rate is not known at startup — so it is deliberately generous. That fits fast handlers, where the pool drains a deep backlog well inside the budget, and is far too large for slow ones: with 8 workers, a 200 ms handler and the default 1 s ceiling — the budget in force whenever the pool is not shedding — only about 40 requests can be admitted in time while the default parks up to 1024.

```bash
QUEUE_MAX_WAITING=40
```

Setting it near `W × B / T` turns the surplus into an immediate 529 rather than a 529 a second later. Shortening `QUEUE_WAIT_TIMEOUT_MS` achieves the same by the other term — the two trade against each other, and on a slow handler the smaller budget is usually the better lever, because it also bounds how long a client waits for the refusal. `B` here is the budget in force, which under overload is the one the server has shortened to rather than the configured ceiling — so read `oxphp_admission_wait_budget_us` before sizing the waiting set from a number in a config file.

The set is bounded a second time, in bytes. Every waiter holds its request body in memory for as long as it waits, so a cap counted in requests leaves the memory they hold to the traffic: the same 1024 waiters cost nothing on bodyless `GET`s and gigabytes on uploads. `QUEUE_MAX_WAITING_BYTES` bounds that sum directly — past it a request carrying a body is refused at once rather than parked, while bodyless ones keep waiting normally. An upload-heavy application that should absorb bursts instead of shedding them wants a larger value; a container with a hard memory limit wants a smaller one. Refusals for either cap are counted separately in `oxphp_admission_refused_total` (`waiting_full` against the count, `waiting_bytes` against the memory), so the metric names the knob to reach for.

# Keeping Headroom for the Accept Loop

A request holds its connection — and one of the `MAX_CONNECTIONS` permits — from the moment it arrives until it is answered, whatever it is doing in between. The PHP path holds requests in three places, each with a bound of its own, because a queue slot is released the moment a worker picks the request up, before the script runs:

- **running** in a worker: at least `PHP_WORKERS`, and more in worker mode, where one thread multiplexes fibers;
- **queued**, up to `QUEUE_CAPACITY`;
- **parked** in admission, up to `QUEUE_MAX_WAITING`.

These count requests, not connections, and the two part in the queue: a request answered on its queue deadline — `529`, or `499` once its client has gone — gives its connection back at once and keeps its queue slot until a worker reaches it. So `QUEUE_CAPACITY` can overstate the connections the queue holds, as `PHP_WORKERS` understates the ones running in worker mode; the sum is a guide, not a count. Only the third has a ceiling derived from `MAX_CONNECTIONS`. Keep `PHP_WORKERS` + `QUEUE_CAPACITY` + `QUEUE_MAX_WAITING` below `MAX_CONNECTIONS`. Past it the failure mode changes for the worse: the accept loop takes a connection permit before it starts serving a connection, so once the PHP path holds them all it parks, and a client arriving then gets no response at all instead of a `529` — which a load balancer cannot tell apart from a dead instance, while the health probe on `INTERNAL_ADDR` stays green because it does not go through either limit.

The stall itself is no longer silent: an accept that has to wait for a permit logs a `WARN` naming the budget, and an `INFO` reporting how long the loop was parked is written the moment the permit arrives — not on the next connection, which on an overload that ended because the load went away may be minutes later or never. The reports are rate-limited so a server flapping around the ceiling cannot turn its own log into the outage — a stall that begins within seconds of a reported one is held back at first, but if it is still going when that window closes it is reported then, so no ongoing stall stays silent however it started. A stall passed over that way leaves no line at all; the metrics still see it. The state is exported as `oxphp_accept_stalled` (gauge, readable while the stall lasts because the internal listener does not go through `MAX_CONNECTIONS`) and `oxphp_accept_stalls_total` (counter) — see the [metrics reference](metrics.md). The behaviour is unchanged — arriving clients still get no response rather than an error — but the node now says so.

The server checks this at startup and warns when the sum reaches the budget, naming every value; `oxphp config --check` reports the same, without changing its verdict or exit code. Treat clearing it as necessary rather than sufficient: idle keep-alive connections, static-file requests and handshakes in progress hold permits too, and none of them can be counted at startup.

Lowering `MAX_CONNECTIONS` is the usual way into the warning, because the `QUEUE_CAPACITY` default is sized from the worker count and does not follow it down — 7 workers and `MAX_CONNECTIONS=1000` gives 7 + 896 + 500 against a budget of 1000. Note which knob to reach for: raising `MAX_CONNECTIONS` on its own does not clear it while `QUEUE_MAX_WAITING` is left at its default, because that default is half of `MAX_CONNECTIONS` and rises with it — at 7 workers the sum settles at 1799 and the condition only clears from `MAX_CONNECTIONS=1800` up. Lower `QUEUE_CAPACITY`, or set `QUEUE_MAX_WAITING` explicitly and then raise the budget.

For a dynamic pool (`MIN:MAX`) the running term is the minimum, the same count the queue defaults are sized from, so a pool grown to its maximum holds more than the sum says.

The comparison counts connections while the budget is spent by requests, so an HTTP/2-heavy deployment can legitimately sit above it: one connection carries up to `H2_MAX_CONCURRENT_STREAMS` requests, so the same backlog is held by a fraction of the connections. The warning is expected there. A large pool on the stock budget reaches it too — an auto-sized pool of 39 workers gives 39 + 4992 + 4992 against 10 000 — and that one is worth acting on rather than ignoring.

# Static vs Dynamic Workers

Set `PHP_WORKERS` to a single number for a fixed pool:

```bash
PHP_WORKERS=8      # Fixed 8 workers
PHP_WORKERS=0      # Auto-detect: CPU / 2 (min 1)
```

Set `PHP_WORKERS` to `MIN:MAX` for automatic scaling:

```bash
PHP_WORKERS=2:16   # Scale between 2 and 16 workers
PHP_WORKERS=4:0    # 4 minimum, auto-detect maximum (CPU × 2)
PHP_WORKERS=0:16   # auto-detect minimum (CPU / 4, min 1), 16 maximum
```

In dynamic mode, OxPHP scales workers up when all are busy and scales down when workers have been idle longer than `PHP_WORKERS_IDLE_SECONDS`.

# How the CPU count is detected

Every default in the [Configuration Reference](configuration.md) written in terms of CPU — `TOKIO_WORKERS`, and `PHP_WORKERS` unset, `0`, or with `0` on either side of `MIN:MAX` — counts the CPUs the process is allowed to run on: its affinity mask (`taskset`, `docker run --cpuset-cpus`), lowered to the container's CPU quota where one is set (`docker run --cpus`; `cpu.max` on cgroup v2, `cpu.cfs_quota_us` on v1). A quota is rounded down and counts as at least 1, so `--cpus=1.5` counts as 1.

Two kinds of limit are not counted, and a container held back only by one of them is sized from every CPU it can see:

- **A CPU weight** — `docker run --cpu-shares`, `cpu.weight` on cgroup v2, `cpu.shares` on v1. It decides how contended CPU time is shared with neighbours; it is not a cap, and no CPU count follows from it.
- **A limit enforced outside the container.** A platform can sell a share of a machine and enforce it where the container cannot see it. On DigitalOcean App Platform, for example, an instance sold as 1 vCPU was observed running in a gVisor sandbox with no quota (`cpu.cfs_quota_us` is `-1`) and 8 CPUs visible, so the defaults came to 4 PHP workers, 4 runtime threads and a queue of 512.

On such a plan, set `PHP_WORKERS` and `TOKIO_WORKERS` explicitly, to what the plan actually provides. In Worker mode, check `PHP_WORKERS` × `WORKER_MAX_MEMORY_MIB` against the plan's memory as well: the threshold is a recycle trigger the worker checks in its loop, not a cap on what its PHP heap can allocate, so the pool can hold that much and more before any worker is recycled for it, with OPcache and the server itself on top — and with the default `0` there is no threshold at all. The same sandbox can hide the memory limit too — the App Platform container above saw none.

At startup the server logs the count, what set it, and the resulting `PHP_WORKERS` and `TOKIO_WORKERS` — here under `docker run --cpus=1` on a 14-CPU machine:

```json
{"timestamp":"2026-10-05T18:55:39.801254Z","level":"INFO","fields":{"message":"CPU count for default sizing","cpus":1,"cpu_source":"cgroup quota","host_cpus":14,"php_workers":"1 (auto)","tokio_workers":"1 (auto)"}}
```

- `cpu_source` is `host` when neither a mask nor a quota lowered the count, `affinity` or `cgroup quota` when one of them did, and `unknown` when the server could not tell, because the mask or the online count could not be read (if it could not count CPUs at all, `cpus` is 4).
- `host_cpus` is how many CPUs the system reports online (`/sys/devices/system/cpu/online` on Linux), which a mask or quota does not lower; it is omitted when it cannot be read. A runtime that rewrites that file to list only the container's CPUs makes a narrowed mask read as `host`.
- `(auto)` marks a value left entirely to the default: the variable is unset or empty, or for `TOKIO_WORKERS` not a number.

`cpu_source` of `host` on a plan that sold you fewer CPUs than `cpus` is the case above.

## See Also

- [Configuration Reference](configuration.md) — `PHP_WORKERS`, `QUEUE_CAPACITY`, `QUEUE_MAX_WAITING`, `MAX_CONNECTIONS` and their defaults
- [Diagnosing Overload](overload.md) — reading the queue and admission gauges when the sizing is wrong
- [Worker Mode](../features/worker-mode.md) — persistent workers and how they are recycled
