---
title: Profiling PHP code
description: The definitive practical guide — from first run to production use. Triggers, PHP SDK, attributes, export formats, integration with speedscope/xhgui/pprof, metrics, troubleshooting.
---

# Profiling PHP code with OxPHP

OxPHP ships a built-in, per-request profiler. Unlike xdebug or standalone
extensions, it runs inside the server itself, requires no PHP restart, and
adds no meaningful overhead when disabled (the `mode=Off` branch bails out
before even touching the filter cache).

This document is a **practical guide**: from zero config, to hunting slow
production endpoints, to reading flamegraphs, to comparing before/after
optimization runs.

---

## 1. TL;DR — 60 seconds to first profile

```yaml
# compose.yml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    environment:
      INTERNAL_ADDR: 0.0.0.0:9090
      PROFILER_ENABLED: "true"
      PROFILER_AUTH_TOKEN: "dev-secret"
      PROFILER_OUTPUT_FORMATS: "xhprof,speedscope,collapsed"
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

# 3. Open the profile in speedscope (in-browser flamegraph).
open "http://localhost:9090/__profiler/runs/<run_id>/speedscope"
```

That's it. The rest of this guide explains what actually happens, and how
to turn that into production insight.

---

## 2. What the profiler does

- **Captures every PHP function call** via the Zend Observer API — no
  bytecode patching, no changes to application code.
- **Builds a span tree** with wall-time, CPU-time, memory at entry/exit,
  attributes, and events.
- **Exports four formats** simultaneously: `xhprof.json`, `speedscope.json`,
  `pprof` (protobuf + gzip), `collapsed` (for `flamegraph.pl`).
- **Persists runs**: in-memory LRU cache + disk files + optional HTTP push
  (xhgui or any custom collector).
- **Exposes 8 internal HTTP routes** on `INTERNAL_ADDR` to browse and
  download profiles.
- **Emits Prometheus metrics** — runs per source, spans collected,
  bytes written, drops, push failures.
- **No restart required** — activated per request by a trigger.

---

## 3. How it works under the hood

```
 ┌─ Request ─────────────────────────────────────────────────────────┐
 │  1. Tokio thread: ProfilerRequestHandler inspects the trigger      │
 │     (header/cookie/query/sample_rate, constant-time compare)       │
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
 │     Arc<SpanTree>, runs 4 exporters, puts into the LRU cache,      │
 │     spawns disk-write and HTTP-push tasks (semaphores bound        │
 │     the fan-out)                                                   │
 └────────────────────────────────────────────────────────────────────┘
```

### Three per-request modes

| Mode | When | What is captured |
|------|------|------------------|
| `Off` | Default. No plugin asked for profiling. | Nothing. Zero overhead. |
| `ApmOnly` | `plugin-apm` is enabled but no profiler trigger matched. | Only APM's explicit hooks: `#[Trace]`, PDO/cURL emitters, `oxphp_trace_*()`. |
| `ProfileAll` | A profiler trigger matched (or `OxPHP\Profile\start()` was called). | **Every** PHP function call via the Observer API plus everything APM collects. |

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

## 5. Activation — 4 trigger types

Check order (priority): **header → cookie → query → sample_rate**. Any
match activates `ProfileAll`. Tokens are compared in **constant time**
(`subtle::ConstantTimeEq`).

### 5.1. HTTP header (for development and scripts)

```bash
curl -H "X-OxPHP-Profile: dev-secret" https://app.local/checkout
```

Ideal for CI benchmarks, Postman collections, curl scripts.

### 5.2. Cookie (for browser debugging)

Set the cookie in the browser via DevTools / an extension:

```
OXPROF=dev-secret; Domain=app.local; Path=/
```

As long as the cookie is alive, every request is profiled. Delete it to
stop. Useful for walking a user scenario (open product → add to cart →
checkout) and collecting a **series** of profiles.

### 5.3. Query string (for sharing links)

```
https://app.local/admin/report?__oxprof=dev-secret
```

The bluntest method, but handy when you want to send a coworker a link
like "open this, it reproduces the bug". Careful: the parameter ends up
in access logs and in `Referer` — don't use it with a production token.

### 5.4. Random sampling (for production)

```bash
PROFILER_SAMPLE_RATE=0.001   # ≈ 1 in 1000 requests
```

Runs **without a token**. Turn it on in production to accumulate statistics
against real traffic. A good starting range is `0.0005..0.002`; higher
values produce noticeable overhead, especially with
`PROFILER_INTERNAL=true`.

---

## 6. Configuration — full reference

| Variable | Default | Description |
|---|---|---|
| `PROFILER_ENABLED` | `false` | Master switch. `true` → plugin is loaded. |
| `PROFILER_AUTH_TOKEN` | *(unset)* | Secret for triggers and bearer token for `/__profiler/*` routes. Empty string = "no token required" (any non-empty trigger value passes). **Never commit the token to the repo.** |
| `PROFILER_SAMPLE_RATE` | `0.0` | `[0.0; 1.0]`. Random sampling rate. |
| `PROFILER_INTERNAL` | `false` | Observe **internal** C functions (`strlen`, `json_encode`, …). Full coverage, but **2–5× overhead**. Use surgically. |
| `PROFILER_MAX_SPANS` | `50000` | Hard cap on tree size per request. When exceeded, further spans are marked `truncated` and not written. |
| `PROFILER_MAX_DEPTH` | `256` | Hard cap on stack depth. |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | Absolute path. Must be writable by `www-data`. |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,speedscope` | CSV subset of `xhprof`, `speedscope`, `pprof`, `collapsed`. |
| `PROFILER_RETENTION_COUNT` | `100` | How many runs to keep (both on disk and in LRU). Background trim every 5 seconds. |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | Token bucket protecting the disk. Overflow is dropped and increments `oxphp_profiler_disk_drops_total`. |
| `PROFILER_EXPORT_URL` | *(unset)* | POST URL for each captured run (xhgui, custom collector). |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | One of the four formats for HTTP push. |
| `PROFILER_EXPORT_AUTH_TOKEN` | *(unset)* | Bearer token for the push target. |
| `PROFILER_EXPORT_XHGUI` | `auto` | Force xhgui envelope mode. Auto: URL contains `xhgui` or ends with `/run/import`. |

### Example production config

```yaml
environment:
  PROFILER_ENABLED: "true"
  PROFILER_AUTH_TOKEN: "${PROFILER_TOKEN_FROM_VAULT}"
  PROFILER_SAMPLE_RATE: "0.001"             # ~0.1% of traffic
  PROFILER_INTERNAL: "false"
  PROFILER_OUTPUT_DIR: /var/lib/oxphp/profiles
  PROFILER_OUTPUT_FORMATS: "xhprof,collapsed"
  PROFILER_RETENTION_COUNT: "500"
  PROFILER_DISK_MAX_PER_SEC: "20"
  PROFILER_EXPORT_URL: "http://xhgui.monitoring.svc.cluster.local/run/import"
  PROFILER_EXPORT_FORMAT: "xhprof"
```

---

## 7. PHP SDK — seven functions

All functions live in the `OxPHP\Profile` namespace. They are always safe
to call: if profiling isn't active for the current request, mutators are
safe no-ops and `is_active()` returns `false`.

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

> ⚠️ Calling `start()` mid-request **resets** the current tree (see
> `PROFILING_CONTEXT.reset()` in `php_sdk.rs`). This matches the spec
> invariant: mode is set **once** per request — either by the trigger at
> RINIT, or by the first `start()` call.

### 7.2. Pause and resume

```php
use function OxPHP\Profile\{pause, resume};

pause();
noisy_helper_we_dont_care_about();  // will not land in the tree
resume();
```

Unlike `stop()`, pause/resume is a documentary signal for "temporarily".
Internally it is the same flag, but the distinction helps a reader of the
code.

### 7.3. Point markers — mark()

```php
use function OxPHP\Profile\mark;

mark('cache_miss');
mark('got_auth_token', ['user_id' => (string) $user->id]);
```

Attaches a `SpanEventKind::Mark` event to the **topmost open span**. No-op
when no span is open. Useful for interim timestamps in a long function
or for marking if/else branches.

### 7.4. Numeric metrics — metric()

```php
use function OxPHP\Profile\metric;

$rows = $pdo->query('SELECT ...')->fetchAll();
metric('db.rows', (float) count($rows));
metric('payload.kb', strlen($body) / 1024.0);
```

Appends `metric.<name>=<value>` to the **attributes** of the current span.
Unlike `mark()`, this is a plain key-value pair (no timestamp). It shows
up in speedscope/xhgui as a span property.

### 7.5. Status check — is_active()

```php
if (OxPHP\Profile\is_active()) {
    // can afford an expensive debug dump —
    // this request is being profiled anyway
    error_log(json_encode($debug_state));
}
```

Two TLS reads, no FFI. Safe to call in hot code.

---

## 8. Attributes (PHP 8) — declarative control

The seven attributes split into two categories: **observer filters** run
**before** span creation; **decorators** run **after** the span closes.

| Attribute | Category | Effect |
|---|---|---|
| `#[Profile]` | filter | Force-include the function in the tree (even if general rules would exclude it). |
| `#[Exclude]` | filter | Skip the function; its children re-parent to the nearest included ancestor. |
| `#[Sample(rate: 0.1)]` | filter | Keep only a fraction of calls (`rate ∈ [0.0; 1.0]`). Probabilistic — lock-free. |
| `#[Tag(key, value)]` | filter | Attach a label to the span. Repeatable — multiple `#[Tag]` accumulate. |
| `#[Mark(label?)]` | decorator | Emit a `Mark` event on function entry. |
| `#[SlowThreshold(ms)]` | decorator | Emit a `Slow` event + set status when wall-time ≥ `ms`. |
| `#[MemoryThreshold(kb)]` | decorator | Emit `MemorySpike` + status when net allocation ≥ `kb`. |

### Class vs method composition

```php
use OxPHP\Profile\{Tag, Profile, Exclude};

#[Tag(key: 'layer', value: 'domain')]
#[Profile]                                    // the whole class is always profiled
class OrderService
{
    #[Tag(key: 'op', value: 'create')]
    public function create(array $data): Order { /* ... */ }

    #[Exclude]                                 // excluded, despite class-level #[Profile]
    public function debug_dump(): void { /* ... */ }

    public function find(int $id): ?Order { /* ... */ }   // inherits #[Profile] and #[Tag(layer)]
}
```

- Class-level attributes **propagate** to every method.
- Method-level attributes **add to** class-level ones (tags accumulate).
- `#[Exclude]` on a method **overrides** class-level `#[Profile]`.

### Slow-function threshold

```php
use OxPHP\Profile\SlowThreshold;

#[SlowThreshold(ms: 250)]
function render_dashboard(User $u): string
{
    // if it runs ≥ 250 ms — a Slow event is appended to the span and
    // status_code=2 (error). Immediately visible in xhgui / speedscope.
}
```

### Memory threshold

```php
use OxPHP\Profile\MemoryThreshold;

#[MemoryThreshold(kb: 512)]
function import_csv(string $path): int
{
    // if the function net-allocates ≥ 512 KB during execution —
    // MemorySpike event + status=error
}
```

### Sampling individual functions

```php
use OxPHP\Profile\Sample;

#[Sample(rate: 0.01)]
function log_event(string $evt, array $ctx): void
{
    // ≈ 1% of calls land in the tree; the rest are skipped entirely —
    // neither the span nor its children are created. Useful for functions
    // called millions of times per request.
}
```

> **Filters vs decorators?** When a function is called **very often** and
> you want to reduce capture cost — use `#[Sample]` or `#[Exclude]`
> (they work before span creation). When you want to flag an event above
> a threshold — use `#[SlowThreshold]` / `#[MemoryThreshold]` (they look
> at the already-collected span).

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
  attributes      # Vec<(Arc<str>, Arc<str>)> — from #[Tag], metric(), APM SQL/HTTP
  events          # Vec<SpanEvent { ts, kind, label, attrs }>
  status_code     # 0 = unset, 1 = ok, 2 = error
  status_message
  leaked          # true if the span was force-closed by finalize (PHP threw past the observer)
}
```

**Event kinds** (`SpanEvent::kind`):

| Kind | Emitted by |
|---|---|
| `Mark` | `mark()`, `metric()`, `#[Mark]` |
| `Slow` | `#[SlowThreshold]` |
| `MemorySpike` | `#[MemoryThreshold]` |
| `Sql` | APM hooks (PDO, mysqli) |
| `Http` | APM hooks (cURL, HTTP streams) |
| `Exception` | APM exception handler |
| `Alloc` | (reserved for heap sampling) |
| `Other` | fallback |

---

## 10. Export formats — when to use which

Files live under `PROFILER_OUTPUT_DIR`, named `<run_id>.<ext>`, where
`run_id = <ts_ms>-<req_id_prefix>-<rand4>` (e.g.
`1713600000000-a1b2c3d4-0f5e`).

### 10.1. speedscope (🏆 default for interactive analysis)

Extension: `.speedscope.json`

- In-browser flamegraph with zoom, search, CPU / time / memory toggle.
- Zero setup — open directly at [speedscope.app](https://www.speedscope.app/).
- OxPHP returns a 302 redirect at `/__profiler/runs/{id}/speedscope` →
  speedscope.app with a `profileURL=…` parameter that fetches the profile
  straight from your server.

```bash
# Ctrl-click in macOS Terminal / xdg-open on Linux
open "http://localhost:9090/__profiler/runs/<run_id>/speedscope"
```

### 10.2. xhprof (for xhgui — timeline + historical diff)

Extension: `.xhprof.json`

- Compatible with xhgui (URL search, trends, diff between two runs).
- Perfect for **production accumulation**: run an xhgui container next
  to your app, point `PROFILER_EXPORT_URL=http://xhgui/run/import`, and
  history piles up in the UI.
- Ready docker-compose: `tests/compose.xhgui.yml`.

### 10.3. pprof (Google pprof tooling, Grafana pprof plugin, Pyroscope)

Extension: `.pprof` (protobuf + gzip, level `fast`, zlib backend)

```bash
# save and open
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.pprof > profile.pprof

go tool pprof -http=:8080 profile.pprof
# or
pyroscope-cli adhoc --input profile.pprof
```

### 10.4. collapsed (Brendan Gregg's flamegraph.pl)

Extension: `.collapsed`

- Text format `func;child;grandchild <count>`.
- De facto input for SVG flamegraphs.
- Three metric variants: wall-time, CPU, memory. OxPHP writes `.collapsed`
  (wall); internal pathways also produce `.collapsed.cpu` and
  `.collapsed.mem` (see `tests/fixtures/profiler_exports/`).

```bash
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.collapsed \
  | flamegraph.pl --title "Checkout $run_id" > flame.svg
```

---

## 11. Storage and cleanup

```
/tmp/oxphp-profiles/
├── index.json                                # NDJSON — one record per line
├── 1713600000000-a1b2c3d4-0f5e.xhprof.json
├── 1713600000000-a1b2c3d4-0f5e.speedscope.json
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
  "source": "Header",                 // Header | Cookie | Query | SampleRate
  "span_count": 4821,
  "event_count": 7,
  "error_count": 0,
  "leaked_count": 0,
  "truncated": false,                 // true — exceeded PROFILER_MAX_SPANS
  "oxphp_version": "0.5.0",
  "formats": ["xhprof.json", "speedscope.json"]
}
```

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

With `INTERNAL_ADDR=0.0.0.0:9090`, the plugin registers 8 endpoints under
the `/__profiler/` prefix. All require `Authorization: Bearer
<PROFILER_AUTH_TOKEN>` when a token is configured. Comparison is
**constant time**.

| Route | Method | Purpose |
|---|---|---|
| `/__profiler/` | GET | HTML landing page with endpoint index. |
| `/__profiler/runs` | GET | JSON array of runs. `?limit=N&offset=M`. |
| `/__profiler/runs/{id}` | GET | JSON metadata for one run. |
| `/__profiler/runs/{id}.{format}` | GET | Raw profile bytes. `format` ∈ `xhprof.json`, `speedscope.json`, `pprof`, `collapsed`. |
| `/__profiler/runs/{id}/speedscope` | GET | 302 → speedscope.app with `profileURL=…`. |
| `/__profiler/runs/{id}` | DELETE | Delete all format files + index entry (returns 204). |
| `/__profiler/config` | GET | Current plugin configuration (tokens redacted). |
| `/__profiler/stats` | GET | JSON counter snapshot. |

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

# Delete all runs older than 1 hour (independent of the plugin's retention)
NOW=$(date +%s%3N)
CUTOFF=$((NOW - 3600000))
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=1000" \
  | jq -r --arg c "$CUTOFF" '.runs[] | select(.timestamp_ms < ($c|tonumber)) | .run_id' \
  | xargs -I{} curl -X DELETE -H "Authorization: Bearer $TOK" \
       "http://localhost:9090/__profiler/runs/{}"
```

---

## 13. HTTP push + xhgui

Send each run to a remote collector:

```yaml
environment:
  PROFILER_EXPORT_URL: "http://xhgui/run/import"
  PROFILER_EXPORT_FORMAT: "xhprof"
  PROFILER_EXPORT_AUTH_TOKEN: "shared-secret"   # optional
```

- xhgui envelope auto-detection: URL contains `xhgui` **or** ends with
  `/run/import`. Force via `PROFILER_EXPORT_XHGUI=true|false`.
- Retry plan: 3 attempts with exponential backoff `100/200/400 ms`, total
  budget 5 s wall-clock. The request body is shared across attempts as
  `bytes::Bytes` (zero retry allocations).
- Errors increment `oxphp_profiler_http_push_failures_total`.

### Full demo stack

```bash
docker compose -f tests/compose.xhgui.yml up -d
# app: :80, xhgui: :8142 (UI), :27017 (mongo)
```

E2E smoke test: `tests/php/profiler/test_xhgui_import.php`.

---

## 14. Prometheus metrics

Exposed on `/metrics`:

```
oxphp_profiler_runs_total{source="header"|"cookie"|"query"|"sample"}
oxphp_profiler_spans_collected_total
oxphp_profiler_bytes_written_total{format="xhprof"|"speedscope"|"pprof"|"collapsed"}
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
    summary: "xhgui / collector is unreachable"

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
3. Open the top one in speedscope: `.../__profiler/runs/<id>/speedscope`.
4. Enable **Left Heavy** mode in speedscope — you'll see the functions
   with the most cumulative time.
5. Click the widest bar — get file:line and the list of children.

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
3. For a detailed diff, download two xhprof profiles and upload into
   xhgui — it has a built-in diff view.

### 15.3. Hunting a memory leak

1. Send the request that "grows":
   ```bash
   curl -H "X-OxPHP-Profile: dev-secret" http://localhost/import?file=big.csv
   ```
2. Open it in speedscope, switch to the **memory metric** (via
   `.collapsed.mem` or speedscope's memory view).
3. Add `#[MemoryThreshold(kb: 1024)]` on suspicious functions — you'll
   get explicit `MemorySpike` events on the next run.
4. Use `metric('mem.after', memory_get_usage())` for surgical
   instrumentation.

### 15.4. Continuous monitoring of a critical path

```php
#[Profile]
#[SlowThreshold(ms: 500)]
public function chargeCard(PaymentRequest $r): PaymentResult
{
    // always captured + an explicit Slow mark when it lags
}
```

In Grafana, add a panel for
`oxphp_profiler_runs_total{source="sample"}` and an alert on
`duration_ms` outliers from `index.json` (via a log-based metric or a
sidecar exporter).

### 15.5. Reproduce a bug via a link

A coworker says "/admin/report returns 500 for me". You reply:

```
https://app.local/admin/report?__oxprof=<one-time-token>
```

After they visit — `/__profiler/runs?limit=5`, open the profile, and
see exactly where the exception landed (`status_code=2` + `Exception`
event).

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
2. **In production — only `SAMPLE_RATE`**. Header/cookie/query are
   developer tools. If you need on-demand prod profiling — use a
   dedicated, daily-rotated token.
3. **Don't turn on `PROFILER_INTERNAL=true` globally**. 2–5× overhead
   turns production into a lab. Use surgically in isolation.
4. **Keep `PROFILER_RETENTION_COUNT` realistic** — a run can weigh from
   hundreds of KB (small request) to megabytes (large tree). 500 runs ×
   2 MB = 1 GB. Size the disk accordingly.
5. **`#[Exclude]` noisy helpers** (logging, i18n, the autoloader) — the
   tree becomes readable without losing meaning.
6. **Link profiles to traces**: `trace_id` is shared. In Grafana / Kibana,
   link `/__profiler/runs/<id>` from the trace view.
7. **Git-friendly identifiers**. In this build, `span_id` is a
   deterministic big-endian monotonic counter. Diffing two stored
   profiles is clean.
8. **APM + profiler is free**. Keep both enabled; the tree is shared,
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
   - For query — is it correctly URL-encoded?
4. Does the server see your request at all? Check the access log.

### 401 from `/__profiler/runs`

The bearer token in the header doesn't match `PROFILER_AUTH_TOKEN`.
Common trap: `echo "secret" > secret.txt` appends `\n`. Use `printf`
or pass via env.

### "xhgui doesn't show new runs"

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

- `PROFILER_INTERNAL=false` (this is the default).
- `PROFILER_SAMPLE_RATE` in a reasonable range (0.0005..0.002).
- Reasonable `PROFILER_MAX_SPANS` — when exceeded the tree is truncated
  but capture still runs. For very large requests prefer a surgical
  `start()`/`stop()` around the section of interest.

### `truncated=true` in `index.json`

A request exceeded `PROFILER_MAX_SPANS` (default 50,000). Options:
1. Raise the cap (trading memory for detail).
2. Add `#[Exclude]` / `#[Sample(rate: 0.01)]` on functions called tens
   of thousands of times.
3. Wrap only the suspect region in `start()`/`stop()`.

---

## 19. Command cheatsheet

```bash
# Activate for one request
curl -H "X-OxPHP-Profile: $TOK" http://localhost/endpoint

# List runs, top-10 by duration
curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/runs \
  | jq '.runs | sort_by(.duration_ms) | reverse | .[:10]'

# Open in speedscope
open "http://localhost:9090/__profiler/runs/$RUN_ID/speedscope"

# Download as xhprof for xhgui import
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.xhprof.json > run.xhprof.json

# Download as pprof and open
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.pprof > run.pprof
go tool pprof -http=:8080 run.pprof

# flamegraph.pl
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.collapsed \
  | flamegraph.pl > flame.svg

# Delete a run
curl -X DELETE -H "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID

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

use OxPHP\Profile\{Profile, Tag, Exclude, Sample, SlowThreshold, MemoryThreshold};

#[Profile]
#[Tag(key: 'layer', value: 'domain')]
#[Tag(key: 'svc',   value: 'orders')]
final class OrderService
{
    public function __construct(
        private readonly PDO $db,
        private readonly Mailer $mailer,
    ) {}

    #[SlowThreshold(ms: 250)]
    #[Tag(key: 'op', value: 'create')]
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
    #[Tag(key: 'op', value: 'export')]
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

    // Trivial getter — don't clutter the tree.
    #[Exclude]
    public function find(int $id): ?array
    {
        $stmt = $this->db->prepare('SELECT * FROM orders WHERE id = ?');
        $stmt->execute([$id]);
        return $stmt->fetch(PDO::FETCH_ASSOC) ?: null;
    }

    // Very frequent audit — sample to avoid inflating the tree.
    #[Sample(rate: 0.05)]
    private function audit(string $event, array $ctx): void
    {
        $this->db->prepare('INSERT INTO audit (event, ctx) VALUES (?, ?)')
                 ->execute([$event, json_encode($ctx)]);
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

use function OxPHP\Profile\{start, stop, pause, resume, mark};

$files = glob('/data/incoming/*.csv');
$i = 0;

foreach ($files as $path) {
    if ($i === 0) {
        start();                       // profile only the first file in full
        mark('batch.begin', ['path' => $path]);
    } else {
        pause();                       // the rest — no-op for capture
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

# Diff in xhgui (two latest xhprof runs)
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

// Enable the profiler manually (no trigger needed to test the SDK).
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
- xhgui demo: `tests/compose.xhgui.yml`
- speedscope: <https://www.speedscope.app/>
- xhgui: <https://github.com/perftools/xhgui>
- Google pprof: <https://github.com/google/pprof>
- flamegraph.pl: <https://github.com/brendangregg/FlameGraph>
