---
title: Profiling PHP code
description: The definitive practical guide — from first run to production use. Triggers, PHP SDK, attributes, export formats, integration with speedscope and flamegraph.pl, metrics, troubleshooting.
---

# Profiling PHP code with OxPHP

OxPHP ships a built-in, per-request profiler. Unlike xdebug or standalone
extensions, it runs inside the server itself, requires no PHP restart, and
adds no meaningful overhead when disabled.

This document is a **practical guide**: from zero config, to hunting slow
production endpoints, to reading flamegraphs, to comparing before/after
optimization runs.

---

## 1. TL;DR — 60 seconds to first profile

```yaml
# compose.yml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.15.0
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

## 2. What the profiler does

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

## 3. How it works under the hood

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

## 4. Installation and build

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

## 5. Activation — 2 trigger types

Check order (priority): **header → sample_rate**. Either one activates
`ProfileAll`. The header token is compared in **constant time**
(`subtle::ConstantTimeEq`).

### 5.1. HTTP header (for development and scripts)

```bash
curl -H "X-OxPHP-Profile: dev-secret" https://app.local/checkout
```

Ideal for CI benchmarks, Postman collections, curl scripts. To walk a scenario
in a browser, set the header with a request-header extension: every request
sent while it is on is profiled.

### 5.2. Random sampling (for production)

```bash
PROFILER_SAMPLE_RATE=0.001   # ≈ 1 in 1000 requests
```

Runs **without a token**. Turn it on in production to accumulate statistics
against real traffic. A good starting range is `0.0005..0.002`; higher
values produce noticeable overhead.

### 5.3. Excluding paths from sampling

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

## 6. Configuration — full reference

| Variable | Default | Description |
|---|---|---|
| `PROFILER_ENABLED` | `false` | Master switch. `true` → plugin is loaded. |
| `PROFILER_AUTH_TOKEN` | *(unset)* | Secret for the header trigger and bearer token for `/__profiler/*` routes. Empty string = "no token required" (any non-empty header value passes). **Never commit the token to the repo.** |
| `PROFILER_SAMPLE_RATE` | `0.0` | `[0.0; 1.0]`. Random sampling rate. |
| `PROFILER_EXCLUDE_PATHS` | *(unset)* | CSV glob patterns (`PHP_DENY_PATHS` syntax) excluded from `PROFILER_SAMPLE_RATE`. The header trigger still profiles them. Example: `/_profiler,/_profiler/**,/_wdt/**`. |
| `PROFILER_MAX_SPANS` | `50000` | Hard cap on the calls observed per request. Once it is reached no further spans are recorded, and the run is flagged `truncated`. With no depth limit set this is also the size of the tree; under `PROFILER_MAX_DEPTH` it is not, because a call the depth limit discards is counted here before the limit is applied — a truncated run then reaches disk with a fraction of this many spans. |
| `PROFILER_MAX_DEPTH` | `0` (no cap) | Maximum nesting depth of recorded calls. A call nested deeper is not recorded, and normally neither is anything it calls — that subtree is absent from the profile rather than flattened into it. Its time is not: the call at the limit still spans everything it did, and with nothing recorded underneath it that duration reads as its own exclusive time, so a frame at the boundary looks hot in itself rather than hot in what it called. The exception to the depth rule is a function that recurses into itself more than 32 frames deep: past that the engine observer loses track of which call a return belongs to and attributes it to a shallower frame of the same function, so a call deeper than the limit can still be recorded and the durations around it should not be trusted. The effective maximum is `32`, the depth to which the engine observer tracks open calls; a larger value is accepted and clamped, and — with `PROFILER_ENABLED=true` — reported as clamped at startup. This bounds the profile, not the cost of producing it: the engine hook still runs on the deeper frames, and the calls it drops are still counted against `PROFILER_MAX_SPANS`. Spans an application pushes through the APM helpers are not subject to it. |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | Absolute path. Must be writable by `www-data`. |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,collapsed` | CSV subset of `xhprof`, `collapsed`. An unknown name is skipped with a startup warning. |
| `PROFILER_RETENTION_COUNT` | `100` | How many runs to keep (both on disk and in LRU). Background trim every 5 seconds. |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | Token bucket protecting the disk. Overflow is dropped and increments `oxphp_profiler_disk_drops_total`. |
| `PROFILER_EXPORT_URL` | *(unset)* | POST URL for each captured run. The body is exactly the `PROFILER_EXPORT_FORMAT` output. |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | `xhprof` or `collapsed`. An unknown value falls back to `xhprof` with a startup warning. |
| `PROFILER_EXPORT_AUTH_TOKEN` | *(unset)* | Bearer token for the push target. |

### Example production config

```yaml
environment:
  PROFILER_ENABLED: "true"
  PROFILER_AUTH_TOKEN: "${PROFILER_TOKEN_FROM_VAULT}"
  PROFILER_SAMPLE_RATE: "0.001"             # ~0.1% of traffic
  PROFILER_OUTPUT_DIR: /var/lib/oxphp/profiles
  PROFILER_OUTPUT_FORMATS: "xhprof,collapsed"
  PROFILER_RETENTION_COUNT: "500"
  PROFILER_DISK_MAX_PER_SEC: "20"
  PROFILER_EXPORT_URL: "http://profile-collector.monitoring.svc.cluster.local/ingest"
  PROFILER_EXPORT_FORMAT: "xhprof"
```

---

## 7. PHP SDK — five functions

All functions live in the `OxPHP\Profile` namespace. They are always safe
to call: if profiling isn't active for the current request, mutators are
safe no-ops and `is_active()` returns `false`. With the profiler disabled
(`PROFILER_ENABLED`), `start()` does nothing either.

### 7.1. Explicit capture around a region

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

`start()` is idempotent. `stop()` is too — calling it twice in a row is
safe.

> ⚠️ If the request was running in APM-only mode, `start()` **discards** the
> spans collected so far. A request that is already profiling keeps its tree,
> so `stop()` followed by `start()` leaves a gap in it, not a reset.

### 7.2. Point markers — mark()

```php
use function OxPHP\Profile\mark;

mark('cache_miss');
mark('got_auth_token', ['user_id' => (string) $user->id]);
```

Attaches a `SpanEventKind::Mark` event to the **topmost open span**. No-op
when no span is open. Useful for interim timestamps in a long function
or for marking if/else branches. See [section 9](#9-what-a-captured-span-contains)
for where events can be read.

### 7.3. Numeric metrics — metric()

```php
use function OxPHP\Profile\metric;

$rows = $pdo->query('SELECT ...')->fetchAll();
metric('db.rows', (float) count($rows));
metric('payload.kb', strlen($body) / 1024.0);
```

Appends `metric.<name>=<value>` to the **attributes** of the current span.
Unlike `mark()`, this is a plain key-value pair (no timestamp), and like
events, attributes are in neither file format. See
[section 9](#9-what-a-captured-span-contains).

### 7.4. Status check — is_active()

```php
if (OxPHP\Profile\is_active()) {
    // can afford an expensive debug dump —
    // this request is being profiled anyway
    error_log(json_encode($debug_state));
}
```

Two TLS reads. Safe to call in hot code.

---

## 8. Attributes (PHP 8) — decorators

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

## 9. What a captured span contains

```ruby
FinishedSpan {
  span_id         # Arc<str>, W3C-compatible
  parent_span_id  # Arc<str>
  trace_id        # Arc<str>, shared with APM
  name            # Fully-qualified PHP function/method name
  start_ns        # wall-clock, ns since the profiler epoch
  end_ns
  cpu_ns          # CLOCK_THREAD_CPUTIME_ID (0 when the platform doesn't provide it)
  memory_start    # zend_memory_usage(0) on entry
  memory_end      # zend_memory_usage(0) on exit
  attributes      # Vec<(Arc<str>, Arc<str>)> — from metric(), APM
  events          # Vec<SpanEvent { ts, kind, label, attrs }>
  status_code     # 0 = unset, 1 = ok, 2 = error
  status_message
  leaked          # true if the span was force-closed by finalize (PHP threw past the observer)
}
```

**Event kinds** (`SpanEvent::kind`):

| Kind | Emitted by |
|---|---|
| `Mark` | `mark()`, `#[Mark]` |
| `Slow` | `#[SlowThreshold]` |
| `MemorySpike` | `#[MemoryThreshold]` |
| `Exception` | APM exception handler |
| `Custom` | `oxphp_apm_event()` |

Events and attributes are in neither file format. The run index counts events
in `event_count`, and they leave the server only through the APM OTLP export, on
the spans APM exports, with the kind in the event attribute `oxphp.event.kind`.

---

## 10. Export formats — when to use which

Files live under `PROFILER_OUTPUT_DIR`, named `<run_id>.<ext>`, where
`run_id = <ts_ms>-<req_id_prefix>-<rand4>` (e.g.
`1713600000000-a1b2c3d4-0f5e`). The name is minted when the run is stored, so
every run is named this way whatever turned profiling on — a trigger, the
sampling draw, or `OxPHP\Profile\start()` from PHP.
`req_id_prefix` is the first eight characters of the
request id and is not a key on its own — in the id the server mints those
characters are the epoch seconds, shared by every request of that second. To
tie a profile to its access-log line, use the `request_id` field of the
`index.json` entry, which carries the id in full and is what the access log
prints.

### 10.1. collapsed (flamegraphs)

Extension: `.collapsed`

- Text format `main();func;child <microseconds>`, one line per call. The number
  is the wall time of that call alone, without the calls it made, so the widths
  in a flamegraph add up.
- Opens in [speedscope.app](https://www.speedscope.app/): drop the file on the
  page and pick **Left Heavy** to merge repeated calls.
- De facto input for SVG flamegraphs with `flamegraph.pl`.

```bash
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.collapsed \
  | flamegraph.pl --title "Checkout" > flame.svg
```

### 10.2. xhprof (call-graph numbers)

Extension: `.xhprof.json`

- The pair map `xhprof_disable()` returns:
  `{"caller==>callee": {"ct", "wt", "cpu", "mu", "pmu"}}`.
- Suits scripts: see [Hunting a memory leak](#153-hunting-a-memory-leak) for a
  `jq` one-liner over it.
- A viewer with its own import format needs an adapter in front of it.

---

## 11. Storage and cleanup

```
/tmp/oxphp-profiles/
├── index.json                                # NDJSON — one record per line
├── 1713600000000-a1b2c3d4-0f5e.xhprof.json
├── 1713600000000-a1b2c3d4-0f5e.collapsed
└── 1713600001234-b2c3d4e5-4a2b.xhprof.json
```

### `index.json` entry schema

```json
{
  "run_id": "1713600000000-a1b2c3d4-0f5e",
  "request_id": "a1b2c3d4e5f67890",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "timestamp_ms": 1713600000000,
  "duration_ms": 123,
  "method": "GET",
  "url": "/checkout",
  "status": 200,
  "user_agent": "Mozilla/5.0 …",
  "client_ip": "10.0.0.42",
  "source": "Header",                 // Header | SampleRate | Sdk
  "span_count": 4821,
  "event_count": 7,
  "error_count": 0,
  "leaked_count": 0,
  "truncated": false,                 // true — exceeded PROFILER_MAX_SPANS
  "oxphp_version": "0.15.0",
  "formats": ["xhprof.json", "collapsed"]
}
```

`source` names what admitted the run. `Header` is the header trigger of
section 5 and `SampleRate` is the sampling draw; `Sdk` covers
every run no trigger admitted — a request that turned profiling on for itself
with `OxPHP\Profile\start()`, and equally a plugin that selected full
profiling on its own.

`user_agent` is the request's `User-Agent` header, cut to 512 bytes, a `…(truncated)` marker included, when longer, with bytes that are not valid UTF-8 replaced with U+FFFD; it is `null` when the request carried none.

`index.json` is parsed by `/__profiler/runs`, sorted newest-first and
paginated via `?limit=N&offset=M`.

### Retention

- A background task every 5 seconds deletes entries past
  `PROFILER_RETENTION_COUNT` (atomic `rename` → `index.json`).
- Files without an `index.json` entry (orphaned by a crash) are swept.
- The `PROFILER_DISK_MAX_PER_SEC` token bucket protects the disk: if the
  rate is higher, runs are not written and
  `oxphp_profiler_disk_drops_total` increments.

---

## 12. Internal HTTP routes

With `INTERNAL_ADDR=0.0.0.0:9090`, the plugin registers 4 endpoints under
the `/__profiler/` prefix. All require `Authorization: Bearer
<PROFILER_AUTH_TOKEN>` when a token is configured. Comparison is
**constant time**.

| Route | Method | Purpose |
|---|---|---|
| `/__profiler/runs` | GET | JSON array of runs. `?limit=N&offset=M`. |
| `/__profiler/runs/{id}` | GET | JSON metadata for one run. |
| `/__profiler/runs/{id}.{format}` | GET | Raw profile bytes. `format` ∈ `xhprof.json`, `collapsed`. |
| `/__profiler/config` | GET | Current plugin configuration (tokens redacted). |

### Example scripts

```bash
# Top-5 slowest of the last 20 runs
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=20" \
  | jq '.runs | sort_by(.duration_ms) | reverse | .[:5]'

# All profiles for a given URL
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=500" \
  | jq '.runs[] | select(.url == "/checkout")'
```

---

## 13. HTTP push

Send each run to a remote collector:

```yaml
environment:
  PROFILER_EXPORT_URL: "http://profile-collector/ingest"
  PROFILER_EXPORT_FORMAT: "xhprof"
  PROFILER_EXPORT_AUTH_TOKEN: "shared-secret"   # optional
```

- The request body is exactly the `PROFILER_EXPORT_FORMAT` output, with no
  wrapper. A collector that expects its own envelope needs an adapter in front
  of it.
- Retry plan: 3 attempts with exponential backoff `100/200/400 ms`, total
  budget 5 s wall-clock. The request body is shared across attempts as
  `bytes::Bytes` (zero retry allocations).
- Errors increment `oxphp_profiler_http_push_failures_total`.

---

## 14. Prometheus metrics

Exposed on `/metrics`:

```
oxphp_profiler_runs_total{source="header"|"sample"|"sdk"}
oxphp_profiler_spans_collected_total
oxphp_profiler_bytes_written_total{format="xhprof"|"collapsed"}
oxphp_profiler_disk_drops_total
oxphp_profiler_http_push_failures_total
oxphp_profiler_truncated_total
oxphp_profiler_in_memory_runs
```

Starter Prometheus alerts:

```yaml
- alert: ProfilerDiskDrops
  expr: rate(oxphp_profiler_disk_drops_total[5m]) > 0
  annotations:
    summary: "Profiler is dropping runs on disk — check PROFILER_DISK_MAX_PER_SEC"

- alert: ProfilerPushFailing
  expr: rate(oxphp_profiler_http_push_failures_total[5m]) > 0
  annotations:
    summary: "Profile collector is unreachable"

- alert: ProfilerTruncatingTrees
  expr: rate(oxphp_profiler_truncated_total[5m]) > 0
  annotations:
    summary: "Requests exceed PROFILER_MAX_SPANS — raise the cap or investigate"
```

---

## 15. Workflows — step by step

### 15.1. Find a slow endpoint

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

### 15.2. Validate a before/after hypothesis

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

### 15.3. Hunting a memory leak

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

### 15.4. Continuous monitoring of a critical path

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

## 16. Interaction with APM (`plugin-apm`, OpenTelemetry)

- Both plugins share a **single** `Arc<SpanTree>`. No double collection.
- No profiler trigger + APM enabled → `mode=ApmOnly`. The tree contains
  only explicitly tagged spans (`#[Trace]`, APM SQL/HTTP hooks).
- Profiler trigger hit → `mode=ProfileAll`. The tree contains **everything**
  plus APM annotations.
- APM still ships **only** its explicit spans to OTLP (Jaeger/Tempo cap
  at ~10k spans per trace). For the full picture —
  `/__profiler/runs/<id>`.

---

## 17. Best practices

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

## 18. Troubleshooting

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

## 19. Command cheatsheet

```bash
# Activate for one request
curl -H "X-OxPHP-Profile: $TOK" http://localhost/endpoint

# List runs, top-10 by duration
curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/runs \
  | jq '.runs | sort_by(.duration_ms) | reverse | .[:10]'

# The newest run's id
RUN_ID=$(curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/runs \
  | jq -r '.runs[0].run_id')

# Download for speedscope.app (drop the file on the page)
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.collapsed > run.collapsed

# Download as xhprof
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.xhprof.json > run.xhprof.json

# flamegraph.pl
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.collapsed \
  | flamegraph.pl > flame.svg

# Metrics
curl -s http://localhost:9090/metrics | grep oxphp_profiler_

# Current plugin config (safe — tokens are redacted)
curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/config | jq
```

---

## 20. Practical examples (full code)

Below are ready-to-run PHP scenarios you can drop into `www/public/` and
hit with curl.

### 20.1. Simple controller with manual control

```php
<?php
// www/public/report.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric, is_active};

function fetch_rows(PDO $db, int $user_id): array
{
    $stmt = $db->prepare('SELECT * FROM orders WHERE user_id = ? LIMIT 1000');
    $stmt->execute([$user_id]);
    return $stmt->fetchAll(PDO::FETCH_ASSOC);
}

function render_report(array $rows): string
{
    $sum = array_sum(array_column($rows, 'amount'));
    return json_encode(['count' => count($rows), 'total' => $sum]);
}

$db = new PDO('mysql:host=db;dbname=app', 'app', 'secret');
$user_id = (int) ($_GET['user_id'] ?? 1);

// Explicitly profile only the heavy block — even if the trigger wasn't set.
start();

mark('report.begin', ['user_id' => (string) $user_id]);

$rows = fetch_rows($db, $user_id);
metric('db.rows', (float) count($rows));

$body = render_report($rows);
metric('response.bytes', (float) strlen($body));

mark('report.done');
stop();

header('Content-Type: application/json');
echo $body;

// Optional — tell the frontend that this request was profiled:
if (is_active()) {
    header('X-Profiled: 1');
}
```

Invoke:

```bash
curl -H "X-OxPHP-Profile: dev-secret" 'http://localhost/report.php?user_id=42'
```

### 20.2. Service class with attributes

```php
<?php
// www/lib/OrderService.php
declare(strict_types=1);

use OxPHP\Profile\{SlowThreshold, MemoryThreshold};

final class OrderService
{
    public function __construct(
        private readonly PDO $db,
        private readonly Mailer $mailer,
    ) {}

    #[SlowThreshold(ms: 250)]
    public function create(array $payload): int
    {
        $this->db->beginTransaction();
        try {
            $id = $this->insertOrder($payload);
            $this->insertLines($id, $payload['items']);
            $this->db->commit();
            $this->mailer->sendReceipt($id);
            return $id;
        } catch (\Throwable $e) {
            $this->db->rollBack();
            throw $e;
        }
    }

    #[MemoryThreshold(kb: 2048)]
    public function exportCsv(int $user_id): string
    {
        $stmt = $this->db->prepare('SELECT * FROM orders WHERE user_id = ?');
        $stmt->execute([$user_id]);

        $buf = fopen('php://temp', 'r+');
        fputcsv($buf, ['id', 'created_at', 'total']);
        while ($row = $stmt->fetch(PDO::FETCH_ASSOC)) {
            fputcsv($buf, [$row['id'], $row['created_at'], $row['total']]);
        }
        rewind($buf);
        return stream_get_contents($buf);
    }

    private function insertOrder(array $p): int { /* ... */ return 0; }
    private function insertLines(int $id, array $items): void { /* ... */ }
}
```

### 20.3. Batch job: profile only the first iteration of N

```php
<?php
// www/bin/import.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark};

$files = glob('/data/incoming/*.csv');
$i = 0;

foreach ($files as $path) {
    if ($i === 0) {
        start();                       // profile only the first file in full
        mark('batch.begin', ['path' => $path]);
    }

    import_one($path);

    if ($i === 0) {
        mark('batch.first_done');
        stop();
    }
    $i++;
}

function import_one(string $path): void { /* ... */ }
```

### 20.4. Compare two implementations — micro-benchmark with profiles

```php
<?php
// www/public/bench.php — naive vs streaming comparison
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric};

function naive_sum(string $path): int
{
    $rows = array_map('str_getcsv', file($path));           // whole file into memory
    return array_sum(array_column($rows, 1));
}

function streaming_sum(string $path): int
{
    $h = fopen($path, 'r');
    $total = 0;
    while (($row = fgetcsv($h)) !== false) {
        $total += (int) ($row[1] ?? 0);
    }
    fclose($h);
    return $total;
}

$path = '/data/big.csv';
$which = $_GET['impl'] ?? 'naive';

start();
mark('bench.begin', ['impl' => $which]);
$t0 = hrtime(true);

$result = $which === 'naive' ? naive_sum($path) : streaming_sum($path);

$elapsed_ms = (hrtime(true) - $t0) / 1e6;
metric('bench.elapsed_ms', $elapsed_ms);
metric('bench.result',     (float) $result);
mark('bench.done');
stop();

echo json_encode(['impl' => $which, 'elapsed_ms' => $elapsed_ms, 'result' => $result]);
```

Workflow:

```bash
# Naive
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=naive"

# Streaming
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=streaming"

# The two latest runs: download their .collapsed files and diff them (section 15.2)
curl -sH "Authorization: Bearer dev-secret" \
     "http://localhost:9090/__profiler/runs?limit=2" | jq '.runs[] | .run_id'
```

### 20.5. Conditional profiling in production code

```php
<?php
// Classic case: a suspected function is slow for certain users only.
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, is_active};

function charge(User $user, Money $amount): PaymentResult
{
    // Audit: if this request is being profiled,
    // enable extra logging inside the third-party call.
    $verbose = is_active();

    $gateway = new StripeClient(verbose: $verbose);
    return $gateway->charge($user->id, $amount);
}

function oncall_path(Order $order): void
{
    // Profile only VIP users — no external trigger needed.
    if ($order->user->tier === 'vip') {
        start();
    }
    process($order);
    if ($order->user->tier === 'vip') {
        stop();
    }
}
```

### 20.6. Integration test that profiles itself

```php
<?php
// tests/php/profile_smoke.php
declare(strict_types=1);
require __DIR__ . '/test_helper.php';

use function OxPHP\Profile\{start, stop, mark, is_active};

$t = new TestCase('profile_smoke', 'my-app');

// Turn profiling on from PHP — no trigger needed to test the SDK, but the
// profiler must be enabled (PROFILER_ENABLED=true), or start() does nothing.
$t->assertFalse('initially not active', is_active());
start();
$t->assertTrue('active after start', is_active());

mark('test.midpoint');

// some work
$sum = 0;
for ($i = 0; $i < 100_000; $i++) { $sum += $i; }

stop();
$t->assertFalse('inactive after stop', is_active());
$t->assertSame('computation OK', $sum, 4999950000);

$t->done();
```

### 20.7. Finding a hotspot from a series of Postman requests

Scenario: "/api/search is sometimes slow, not always."

```javascript
// Postman Pre-request Script
pm.request.headers.add({
    key: 'X-OxPHP-Profile',
    value: pm.environment.get('PROFILE_TOKEN')
});
```

After 100 runs, a `jq` one-liner for top anomalies:

```bash
curl -sH "Authorization: Bearer $TOK" \
     "http://int.app.local:9090/__profiler/runs?limit=200" \
  | jq -r '.runs
          | map(select(.url | startswith("/api/search")))
          | sort_by(-.duration_ms)
          | .[:5]
          | map("\(.duration_ms)ms  \(.run_id)  \(.url)")
          | .[]'
```

### 20.8. A custom decorator that feeds the profiler

Your own `#[ProfileDb]` — logs row count and automatically calls
`metric('db.rows', …)`:

```php
<?php
use OxPHP\Decorator\{AttributeInterface, Context};
use function OxPHP\Profile\metric;

#[Attribute(Attribute::TARGET_METHOD)]
class ProfileDb implements AttributeInterface
{
    public function before(Context $ctx): void {}

    public function after(Context $ctx): void
    {
        $result = $ctx->returnValue;
        if (is_array($result)) {
            metric('db.rows', (float) count($result));
        } elseif ($result instanceof PDOStatement) {
            metric('db.rows', (float) $result->rowCount());
        }
    }
}

oxphp_register_decorator(ProfileDb::class);

class UserRepository
{
    #[ProfileDb]
    public function findAll(): array { /* ... */ return []; }
}
```

The decorator + profiler pairing works out of the box: `metric()`
automatically attaches to the span of the function the Observer API is
currently watching.

---

## 21. References

- In-tree spec: `src/profiling/mod.rs`, `src/plugins/ox_profiler/`
- Bridge (C): `ext/bridge/oxphp_bridge.c`, `ext/oxphp_sapi.c`
- PHP tests: `tests/php/profiler/`
- Format fixtures: `tests/fixtures/profiler_exports/`
- speedscope: <https://www.speedscope.app/>
- flamegraph.pl: <https://github.com/brendangregg/FlameGraph>
