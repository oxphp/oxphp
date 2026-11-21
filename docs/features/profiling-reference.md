---
title: Profiler Reference
description: Profiler configuration, captured span contents, collapsed and xhprof exports, storage, internal routes, HTTP push, metrics and a command cheatsheet.
---

# Profiler Reference

Lookup material for the built-in profiler: every setting, what a captured
span holds, the export formats and where runs are stored and served. The
[profiling guide](profiling.md) covers how to turn it on and use it.

## Configuration — full reference

| Variable | Default | Description |
|---|---|---|
| `PROFILER_ENABLED` | `false` | Master switch. `true` → plugin is loaded. |
| `PROFILER_AUTH_TOKEN` | *(unset)* | Secret for the header trigger and bearer token for `/__profiler/*` routes. Empty string = "no token required" (any non-empty header value passes). **Never commit the token to the repo.** |
| `PROFILER_SAMPLE_RATE` | `0.0` | `[0.0; 1.0]`. Random sampling rate. |
| `PROFILER_EXCLUDE_PATHS` | *(unset)* | CSV glob patterns (`PHP_DENY_PATHS` syntax) excluded from `PROFILER_SAMPLE_RATE`. The header trigger still profiles them. Example: `/_profiler,/_profiler/**,/_wdt/**`. |
| `PROFILER_MAX_SPANS` | `50000` | Hard cap on the calls observed per request. Once it is reached no further spans are recorded, and the run is flagged `truncated`. With no depth limit set this is also the size of the tree; under `PROFILER_MAX_DEPTH` it is not, because a call the depth limit discards is counted here before the limit is applied — a truncated run then reaches disk with a fraction of this many spans. |
| `PROFILER_MAX_DEPTH` | `0` (no cap) | Maximum nesting depth of recorded calls. A call nested deeper is not recorded, and normally neither is anything it calls — that subtree is absent from the profile rather than flattened into it. The effective maximum is `32`, the depth to which the engine observer tracks open calls; a larger value is accepted and clamped, and — with `PROFILER_ENABLED=true` — reported as clamped at startup. How a cut subtree's time is reported, the deep-recursion exception and what the limit does not save: [The depth limit](#the-depth-limit). |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | Absolute path. Must be writable by `www-data`. |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,collapsed` | CSV subset of `xhprof`, `collapsed`. An unknown name is skipped with a startup warning; a list with no known name writes both. |
| `PROFILER_RETENTION_COUNT` | `100` | How many runs to keep (both on disk and in LRU). Background trim every 5 seconds. |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | Token bucket protecting the disk. Overflow is dropped and increments `oxphp_profiler_disk_drops_total`. |
| `PROFILER_EXPORT_URL` | *(unset)* | POST URL for each captured run. The body is exactly the `PROFILER_EXPORT_FORMAT` output. |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | `xhprof` or `collapsed`. An unknown value falls back to `xhprof` with a startup warning. |
| `PROFILER_EXPORT_AUTH_TOKEN` | *(unset)* | Bearer token for the push target. |

### The depth limit

A subtree `PROFILER_MAX_DEPTH` cuts off is absent from the profile, but its
time is not: the call at the limit still spans everything it did, and with
nothing recorded underneath it that duration reads as its own exclusive time,
so a frame at the boundary looks hot in itself rather than hot in what it
called.

The exception to the depth rule is a function that recurses into itself more
than 32 frames deep: past that the engine observer loses track of which call a
return belongs to and attributes it to a shallower frame of the same function,
so a call deeper than the limit can still be recorded and the durations around
it should not be trusted.

The limit bounds the profile, not the cost of producing it: the engine hook
still runs on the deeper frames, and the calls it drops are still counted
against `PROFILER_MAX_SPANS`. Spans an application pushes through the APM
helpers are not subject to it.

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

## What a captured span contains

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

## Export formats — when to use which

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

### collapsed (flamegraphs)

Extension: `.collapsed`

- Text format `main();func;child <microseconds>`, one line per distinct call
  stack. The number is the wall time of every call on that stack, without the
  calls they made, so the widths in a flamegraph add up.
- Opens in [speedscope.app](https://www.speedscope.app/): drop the file on the
  page and pick **Left Heavy** to put the heaviest stacks first.
- De facto input for SVG flamegraphs with `flamegraph.pl`.

```bash
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.collapsed \
  | flamegraph.pl --title "Checkout" > flame.svg
```

### xhprof (call-graph numbers)

Extension: `.xhprof.json`

- The pair map `xhprof_disable()` returns:
  `{"caller==>callee": {"ct", "wt", "cpu", "mu", "pmu"}}`.
- Suits scripts: see [Hunting a memory leak](profiling.md#hunting-a-memory-leak) for a
  `jq` one-liner over it.
- A viewer with its own import format needs an adapter in front of it.

---

## Storage and cleanup

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
  "oxphp_version": "0.16.0",
  "formats": ["xhprof.json", "collapsed"]
}
```

`source` names what admitted the run. `Header` is the header trigger of
[Activation](profiling.md#activation--2-trigger-types) and `SampleRate` is the
sampling draw; `Sdk` covers
every run no trigger admitted — a request that turned profiling on for itself
with `OxPHP\Profile\start()`, and equally a plugin that selected full
profiling on its own.

`user_agent` is the request's `User-Agent` header, cut to 512 bytes, a `…(truncated)` marker included, when longer, with bytes that are not valid UTF-8 replaced with U+FFFD; it is `null` when the request carried none.

`index.json` is parsed by `/__profiler/runs`, sorted newest-first and
paginated via `?limit=N&offset=M`.

### Retention

- A background task every 5 seconds deletes entries past
  `PROFILER_RETENTION_COUNT` (atomic `rename` → `index.json`).
- The `PROFILER_DISK_MAX_PER_SEC` token bucket protects the disk: if the
  rate is higher, runs are not written and
  `oxphp_profiler_disk_drops_total` increments.

---

## Internal HTTP routes

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

## HTTP push

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

## Prometheus metrics

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

## Command cheatsheet

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

## See Also

- [Profiling PHP code](profiling.md) — the practical guide
- [Profiling Examples](profiling-examples.md) — complete PHP scenarios
- [Prometheus Metrics](../operations/metrics.md) — the server's other metrics
