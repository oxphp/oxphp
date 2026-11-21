---
title: Profiling PHP code
description: A practical guide to the built-in per-request profiler: triggers, the PHP SDK, attributes, workflows for slow endpoints and leaks, and troubleshooting.
---

# Profiling PHP code with OxPHP

OxPHP ships a built-in, per-request profiler. Unlike xdebug or standalone
extensions, it runs inside the server itself, requires no PHP restart, and
adds no meaningful overhead when disabled.

This document is a **practical guide**: from zero config, to hunting slow
production endpoints, to reading flamegraphs, to comparing before/after
optimization runs.

---

## Quick Start

```yaml
# compose.yml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.16.0
    environment:
      INTERNAL_ADDR: 0.0.0.0:9090
      PROFILER_ENABLED: "true"
      PROFILER_AUTH_TOKEN: "dev-secret"
    volumes:
      - ./www:/var/www/html
      - profiles:/tmp/oxphp-profiles
    ports:
      - "80:80"
      - "9090:9090"

volumes:
  profiles:
```

```bash
# 1. Request a page with the profiling trigger.
curl -H "X-OxPHP-Profile: dev-secret" http://localhost/slow-endpoint

# 2. List captured runs.
curl -H "Authorization: Bearer dev-secret" http://localhost:9090/__profiler/runs \
  | jq '.runs[0]'

# 3. Download the newest run as collapsed stacks.
RUN_ID=$(curl -s -H "Authorization: Bearer dev-secret" http://localhost:9090/__profiler/runs \
  | jq -r '.runs[0].run_id')
curl -H "Authorization: Bearer dev-secret" \
  "http://localhost:9090/__profiler/runs/$RUN_ID.collapsed" > profile.collapsed
```

Drop `profile.collapsed` on [speedscope.app](https://www.speedscope.app/) to see
the flamegraph.

That's it. The rest of this guide explains what actually happens, and how
to turn that into production insight.

---

## What the profiler does

- **Captures every PHP function call** via the Zend Observer API — no
  bytecode patching, no changes to application code.
- **Builds a span tree** with wall-time, CPU-time, memory at entry/exit,
  attributes, and events.
- **Exports two formats** simultaneously: `xhprof.json` and `collapsed` (for
  `flamegraph.pl` and speedscope.app).
- **Persists runs**: in-memory LRU cache + disk files + optional HTTP push
  to any collector.
- **Exposes 4 internal HTTP routes** on `INTERNAL_ADDR` to browse and
  download profiles.
- **Emits Prometheus metrics** — runs per source, spans collected,
  bytes written, drops, push failures.
- **No restart required** — activated per request by a trigger.

---

## How it works under the hood

```
 ┌─ Request ─────────────────────────────────────────────────────────┐
 │  1. Tokio thread: ProfilerRequestHandler inspects the trigger      │
 │     (header/sample_rate, constant-time compare)                    │
 │                                                                    │
 │  2. Decision is written into PluginRequestActions → forwarded      │
 │     to the worker via the SAPI channel                             │
 │                                                                    │
 │  3. Before RINIT, the worker sets ProfilingMode = ProfileAll       │
 │     and registers Observer handlers on begin/end of every function │
 │                                                                    │
 │  4. Each PHP function call → C hook → bridge buffer → Rust         │
 │     SpanTree (span_id = monotonic BE counter; names interned in    │
 │     a thread-local interner — no extra allocations)                │
 │                                                                    │
 │  5. After the response: ProfilerCompleteHandler receives           │
 │     Arc<SpanTree>, puts it into the LRU cache,                     │
 │     spawns disk-write and HTTP-push tasks (semaphores bound        │
 │     the fan-out)                                                   │
 └────────────────────────────────────────────────────────────────────┘
```

### Three per-request modes

| Mode | When | What is captured |
|------|------|------------------|
| `Off` | Default. No plugin asked for profiling. | Nothing. Zero overhead. |
| `ApmOnly` | `plugin-apm` is enabled but no profiler trigger matched. | Only APM's explicit hooks: `#[Trace]`, PDO/cURL emitters, `oxphp_apm_*()`. |
| `ProfileAll` | A profiler trigger matched (or `OxPHP\Profile\start()` was called while the profiler is enabled). | **Every** PHP function call via the Observer API plus everything APM collects. |

`ProfileAll` supersedes `ApmOnly`: when both plugins are enabled and a
trigger matches, a single shared `Arc<SpanTree>` is used — no double
collection.

---

## Installation and build

The `plugin-profiler` plugin is part of the **default cargo features**.
A stock `docker compose build` already includes it.

To disable:

```dockerfile
ARG OXPHP_WITH_PROFILER=0
# or
ARG CARGO_FEATURES="plugin-apm,plugin-otel"  # no plugin-profiler
```

To verify the plugin is compiled in:

```bash
docker compose exec app cat /proc/self/maps | grep -i profiler
# or: oxphp --list-plugins (if the command is available)
```

---

## Activation — 2 trigger types

Check order (priority): **header → sample_rate**. Either one activates
`ProfileAll`. The header token is compared in **constant time**
(`subtle::ConstantTimeEq`).

### HTTP header (for development and scripts)

```bash
curl -H "X-OxPHP-Profile: dev-secret" https://app.local/checkout
```

Ideal for CI benchmarks, Postman collections, curl scripts. To walk a scenario
in a browser, set the header with a request-header extension: every request
sent while it is on is profiled.

### Random sampling (for production)

```bash
PROFILER_SAMPLE_RATE=0.001   # ≈ 1 in 1000 requests
```

Runs **without a token**. Turn it on in production to accumulate statistics
against real traffic. A good starting range is `0.0005..0.002`; higher
values produce noticeable overhead.

### Excluding paths from sampling

`PROFILER_SAMPLE_RATE` samples a random fraction of **all** requests — including
framework self-traffic that pollutes the data. Symfony's web debug toolbar polls
`/_wdt/{token}` and links to `/_profiler/{token}`; Laravel Debugbar and Telescope
behave similarly. Keep these out of sampling with `PROFILER_EXCLUDE_PATHS`:

```bash
PROFILER_EXCLUDE_PATHS=/_profiler,/_profiler/**,/_wdt/**
```

Comma-separated glob patterns, same syntax as `PHP_DENY_PATHS`: `*` does not
cross `/`, `**` does, and a leading `/` is optional. A pattern matches a bare
path **or** its subtree only if you list both — `/_profiler/**` covers
`/_profiler/x` but not the bare `/_profiler`, hence the two-pattern recipe above.
Patterns match the request path as received — no percent-decoding or `..`
normalization — so list the literal path your framework uses.

> **Exclusion affects automatic sampling only.** A request carrying the
> `x-oxphp-profile` header is **always** profiled, even on an excluded path. This
> lets you deliberately profile `/_profiler` itself while keeping it out of
> background sampling.

---

## PHP SDK — five functions

All functions live in the `OxPHP\Profile` namespace. They are always safe
to call: if profiling isn't active for the current request, mutators are
safe no-ops and `is_active()` returns `false`. Each one's full contract is
in [PHP Functions](../php/functions.md#oxphpprofileis_active).

### Explicit capture around a region

```php
use function OxPHP\Profile\{start, stop, is_active};

function heavy_report(): array
{
    start();                       // activate ProfileAll inside the request
    $result = build_report();      // this lands in the tree
    stop();                        // stop capture
    return $result;
}
```

[`start()`](../php/functions.md#oxphpprofilestart) turns capture on for the
rest of the request, and [`stop()`](../php/functions.md#oxphpprofilestop)
turns it off again. Read the `start()` entry before calling it in a request
APM may already be tracing: it can discard spans.

### Point markers — mark()

```php
use function OxPHP\Profile\mark;

mark('cache_miss');
mark('got_auth_token', ['user_id' => (string) $user->id]);
```

[`mark()`](../php/functions.md#oxphpprofilemark) attaches a timestamped
event to the topmost open span — for interim timestamps in a long function
or for marking if/else branches. See [What a captured span
contains](profiling-reference.md#what-a-captured-span-contains) for where
events can be read.

### Numeric metrics — metric()

```php
use function OxPHP\Profile\metric;

$rows = $pdo->query('SELECT ...')->fetchAll();
metric('db.rows', (float) count($rows));
metric('payload.kb', strlen($body) / 1024.0);
```

[`metric()`](../php/functions.md#oxphpprofilemetric) records a number as a
`metric.<name>` attribute of the current span — rows fetched, payload size.
See [What a captured span
contains](profiling-reference.md#what-a-captured-span-contains) for where
attributes can be read.

### Status check — is_active()

```php
if (OxPHP\Profile\is_active()) {
    // can afford an expensive debug dump —
    // this request is being profiled anyway
    error_log(json_encode($debug_state));
}
```

[`is_active()`](../php/functions.md#oxphpprofileis_active) tells whether
this request is being profiled — use it to guard work that only makes sense
while it is.

---

## Attributes (PHP 8) — decorators

Three attributes add an event to the span of a function or method, and change
nothing else about it.

| Attribute | Effect |
|---|---|
| `#[Mark(label?)]` | Emit a `Mark` event on function entry. Without a label, the event takes the function's name. |
| `#[SlowThreshold(ms)]` | Emit a `Slow` event when wall-time ≥ `ms`. |
| `#[MemoryThreshold(kb)]` | Emit a `MemorySpike` event when net allocation ≥ `kb`. |

### Slow-function threshold

```php
use OxPHP\Profile\SlowThreshold;

#[SlowThreshold(ms: 250)]
function render_dashboard(User $u): string
{
    // if it runs ≥ 250 ms, a Slow event is appended to the span
}
```

### Memory threshold

```php
use OxPHP\Profile\MemoryThreshold;

#[MemoryThreshold(kb: 512)]
function import_csv(string $path): int
{
    // if the function net-allocates ≥ 512 KB during execution,
    // a MemorySpike event is appended to the span
}
```

---

## Workflows — step by step

### Find a slow endpoint

1. Turn on `PROFILER_SAMPLE_RATE=0.001` in production. Let it accumulate.
2. Sort runs by `duration_ms`:
   ```bash
   curl -s -H "Authorization: Bearer $TOK" \
        "http://INT_ADDR/__profiler/runs?limit=500" \
     | jq '.runs | sort_by(.duration_ms) | reverse | .[:10]
            | map({run_id, url, duration_ms, span_count})'
   ```
3. Download the top one and drop it on
   [speedscope.app](https://www.speedscope.app/):
   ```bash
   curl -s -H "Authorization: Bearer $TOK" \
        "http://INT_ADDR/__profiler/runs/<id>.collapsed" > top.collapsed
   ```
4. Enable **Left Heavy** mode in speedscope — you'll see the functions
   with the most cumulative time.
5. Click the widest bar to see its time and what it calls.

### Validate a before/after hypothesis

1. Run a benchmark BEFORE your changes:
   ```bash
   for i in $(seq 1 20); do
     curl -s -H "X-OxPHP-Profile: dev-secret" http://localhost/api/report > /dev/null
   done
   curl -s -H "Authorization: Bearer dev-secret" \
        "http://localhost:9090/__profiler/runs?limit=20" \
     | jq '.runs | map(.duration_ms) | add / length' > /tmp/p50_before.txt
   ```
2. Apply changes, rebuild, repeat. Compare the medians.
3. For a detailed diff, download the `.collapsed` of one run from each side
   and compare them with the FlameGraph scripts:
   ```bash
   difffolded.pl before.collapsed after.collapsed | flamegraph.pl > diff.svg
   ```

### Hunting a memory leak

1. Send the request that "grows":
   ```bash
   curl -H "X-OxPHP-Profile: dev-secret" http://localhost/import?file=big.csv
   ```
2. Download its xhprof profile and list the calls that grew memory the most:
   ```bash
   curl -s -H "Authorization: Bearer dev-secret" \
        "http://localhost:9090/__profiler/runs/<id>.xhprof.json" \
     | jq 'to_entries | sort_by(-.value.mu) | .[:10]
           | map({call: .key, mu: .value.mu, pmu: .value.pmu})'
   ```
3. Add `#[MemoryThreshold(kb: 1024)]` on suspicious functions: a call over the
   limit adds a `MemorySpike` event to its span, counted in `event_count` of
   the run.

### Continuous monitoring of a critical path

```php
#[SlowThreshold(ms: 500)]
public function chargeCard(PaymentRequest $r): PaymentResult
{
    // an explicit Slow event on the span when it lags
}
```

In Grafana, add a panel for
`oxphp_profiler_runs_total{source="sample"}` and an alert on
`duration_ms` outliers from `index.json` (via a log-based metric or a
sidecar exporter).

---

## Interaction with APM (`plugin-apm`, OpenTelemetry)

- Both plugins share a **single** `Arc<SpanTree>`. No double collection.
- No profiler trigger + APM enabled → `mode=ApmOnly`. The tree contains
  only explicitly tagged spans (`#[Trace]`, APM SQL/HTTP hooks).
- Profiler trigger hit → `mode=ProfileAll`. The tree contains **everything**
  plus APM annotations.
- APM still ships **only** its explicit spans to OTLP (Jaeger/Tempo cap
  at ~10k spans per trace). For the full picture —
  `/__profiler/runs/<id>`.

---

## Best practices

1. **Never commit `PROFILER_AUTH_TOKEN`**. Read from Vault / Docker
   secrets / Kubernetes secrets.
2. **In production — only `SAMPLE_RATE`**. The header trigger is a
   developer tool. If you need on-demand prod profiling — use a
   dedicated, daily-rotated token.
3. **Keep `PROFILER_RETENTION_COUNT` realistic** — a run can weigh from
   hundreds of KB (small request) to megabytes (large tree). 500 runs ×
   2 MB = 1 GB. Size the disk accordingly.
4. **Mind `PROFILER_MAX_SPANS` on heavy requests**. A request that calls
   helpers tens of thousands of times hits the cap: raise it, or profile
   only the section of interest with `start()`/`stop()`.
5. **Link profiles to traces**: `trace_id` is shared. In Grafana / Kibana,
   link `/__profiler/runs/<id>` from the trace view.
6. **Git-friendly identifiers**. In this build, `span_id` is a
   deterministic big-endian monotonic counter. Diffing two stored
   profiles is clean.
7. **APM + profiler is free**. Keep both enabled; the tree is shared,
   overhead comes only from APM's accumulated coverage.

---

## Troubleshooting

### "No profiles appear"

1. Is the plugin compiled in? `docker compose build` includes it by
   default. Check you didn't pass `--build-arg OXPHP_WITH_PROFILER=0`
   or a custom `CARGO_FEATURES` without `plugin-profiler`.
2. `PROFILER_ENABLED=true`?
3. Does the trigger actually match `PROFILER_AUTH_TOKEN`?
   - Look for a stray `\n` in the env var.
4. Does the server see your request at all? Check the access log.

### 401 from `/__profiler/runs`

The bearer token in the header doesn't match `PROFILER_AUTH_TOKEN`.
Common trap: `echo "secret" > secret.txt` appends `\n`. Use `printf`
or pass via env.

### "The collector doesn't receive new runs"

1. Check reachability:
   ```bash
   docker compose exec app curl -v $PROFILER_EXPORT_URL
   ```
2. Look at `oxphp_profiler_http_push_failures_total`.
3. Check the logs: `tracing::warn!` with `run_id` and HTTP status is
   emitted on each failure.

### No files on disk

- Is `PROFILER_OUTPUT_DIR` **absolute**? Relative paths are ignored.
- Writable by `www-data`?
  ```bash
  docker compose exec app ls -la /tmp/oxphp-profiles
  ```
- Is `PROFILER_DISK_MAX_PER_SEC` too low? Look at
  `oxphp_profiler_disk_drops_total`.

### Too much overhead in production

- `PROFILER_SAMPLE_RATE` in a reasonable range (0.0005..0.002).
- Reasonable `PROFILER_MAX_SPANS` — once it is reached no further spans
  are recorded, the request itself is not interrupted, and the prefix
  collected up to that point is written with `truncated: true`. For very
  large requests prefer a surgical `start()`/`stop()` around the section
  of interest.

### `truncated=true` in `index.json`

A request exceeded `PROFILER_MAX_SPANS` (default 50,000). Options:
1. Raise the cap (trading memory for detail).
2. Profile from PHP instead of a trigger, wrapping only the suspect region
   in `start()`/`stop()`.

---

## See Also

- [Profiler Reference](profiling-reference.md) — configuration, span contents,
  export formats, storage, internal routes, HTTP push, metrics, cheatsheet
- [Profiling Examples](profiling-examples.md) — complete PHP scenarios
- [OxPHP\Profile functions](../php/functions.md) — the PHP SDK reference
- In-tree spec: `src/profiling/mod.rs`, `src/plugins/ox_profiler/`
- Bridge (C): `ext/bridge/oxphp_bridge.c`, `ext/oxphp_sapi.c`
- PHP tests: `tests/php/profiler/`
- Format fixtures: `tests/fixtures/profiler_exports/`
- speedscope: <https://www.speedscope.app/>
- flamegraph.pl: <https://github.com/brendangregg/FlameGraph>
