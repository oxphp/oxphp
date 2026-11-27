---
title: Configuration Reference
description: Complete environment variable reference for OxPHP. Every setting, its default, and what it controls — all in one place.
---

# Configuration Reference

OxPHP is configured through environment variables, plus one optional file: [`.oxphpdeny`](../security/oxphpdeny.md), the path deny rules read from the top of `DOCUMENT_ROOT`. Every setting has a sensible default, so a zero-configuration deployment works out of the box.

### Boolean values

Variables marked as boolean accept a fixed canonical set, case-insensitive and trimmed:

- truthy: `on`, `true`, `1`, `yes`
- falsy: `off`, `false`, `0`, `no`

Any non-empty value outside that set — typos like `ture` — fails fast at startup with an error naming the variable. This catches misconfiguration before traffic instead of silently flipping a flag the wrong way.

An unset variable or empty assignment (`FOO=`) falls back to the documented default. Empty is treated as unset on purpose: Docker Compose / Kubernetes substitution like `FOO=${FOO}` produces `FOO=` when the host variable is missing, and that should not refuse to start the server.

## Server

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:80` | Address and port for the main HTTP server |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Root directory for serving files and PHP scripts. `false` (or `off`, `0`, `no`) switches it off: every request goes to `ENTRY_FILE`, with no static files served. See [No Document Root](../features/routing.md#no-document-root) |
| `ENTRY_FILE` | *(unset)* | Single canonical entry script. Unset, or a file name without an extension = direct file mapping. `*.php` = front controller. Any other extension = static fallback (SPA). With `WORKER_MODE_ENABLED=true` = worker bootstrap. Resolved against `DOCUMENT_ROOT` (relative paths and `..` allowed; absolute paths used as-is, and required with `DOCUMENT_ROOT` switched off). See [Routing](../features/routing.md) |
| `WORKER_MODE_ENABLED` | `false` | Enable persistent worker mode. Requires `ENTRY_FILE` to point at a `.php` script. Boolean — see [Boolean values](#boolean-values) |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent TCP connections. Also the ceiling for the default `QUEUE_MAX_WAITING` (half of it), so a malformed value is a startup error rather than a silent fallback. Lowering it does not move the `QUEUE_CAPACITY` default, which is sized from the worker count alone — keep it above `PHP_WORKERS` + `QUEUE_CAPACITY` + `QUEUE_MAX_WAITING`, see [Keeping Headroom for the Accept Loop](sizing-workers.md#keeping-headroom-for-the-accept-loop) |
| `TOKIO_WORKERS` | [CPU](sizing-workers.md#how-the-cpu-count-is-detected) / 2 (min 1) | Async I/O threads. `1` = single-threaded, `N > 1` = fixed thread count, unset = auto (CPU / 2, min 1) |

### Sizing the async runtime

`TOKIO_WORKERS` defaults to half the available cores, and the half it leaves is not idle — it belongs to the PHP worker pool. Those threads are not the runtime's, and the two sets compete for the same cores, so the runtime cannot be widened without taking cycles from the application.

Raising it to the full core count is a loss wherever the pool is already sized near that count. Measured on a 4-core machine with a 4-worker pool, serving compressed responses: going from 2 runtime threads to 4 cost 13% of the rate on a 1.6 KB JSON body and 26% on a 5 KB one, and burned more CPU doing it. The threads were oversubscribed, and the scheduler paid for it.

It is worth raising where the balance genuinely differs: a pool small relative to the machine, or work that is I/O-bound rather than PHP-bound — proxied upstreams, static files, streaming responses — where the runtime does the work and the pool mostly waits. Whenever you change it, watch the pool as well as the rate: `oxphp_busy_workers` against `oxphp_workers_current` tells you which side the cores went to. A rate that rises while the pool gets busier is a real gain; a rate that rises while the pool empties usually means the work moved, not that more of it got done.

Both this and `PHP_WORKERS` default to half the machine, so their sum lands on the core count. Change one and the sum moves — which is what makes either one worth measuring rather than guessing.

## PHP Workers

| Variable | Default | Description |
|----------|---------|-------------|
| `EXECUTOR` | `sapi` | PHP executor backend. `sapi` for PHP execution, `stub` for benchmarking without PHP |
| `PHP_WORKERS` | [CPU](sizing-workers.md#how-the-cpu-count-is-detected) / 2 (min 1) | Worker pool size. `N` = fixed pool, `MIN:MAX` = dynamic scaling, `0` = auto |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Seconds a dynamic worker stays idle before being retired (dynamic mode only). A worker is retired at a moment when it has nothing in flight, so nothing it is serving is cut short — and a worker holding a request that never ends, such as an open stream, never reaches such a moment and is not retired at all |
| `QUEUE_CAPACITY` | Initial workers × 128 | Maximum pending requests in the PHP queue. Requests arriving at a full queue wait for a slot (see `QUEUE_WAIT_TIMEOUT_MS`). For dynamic pools (`MIN:MAX`), initial workers = minimum count. `0` = auto |
| `QUEUE_WAIT_TIMEOUT_MS` | `1000` | The longest a request may spend waiting for a PHP worker before it is rejected with 529 — a ceiling rather than a fixed value. One deadline, stamped on arrival, covering both waits a request can face: for a slot in the queue, and inside the queue for a worker. At most `QUEUE_MAX_WAITING` requests wait at once; past that, requests are rejected immediately. `0` = reject immediately whenever the queue is full — it removes the wait for a queue slot and leaves the wait inside the queue unbounded, so a request that finds a slot free is queued with no deadline at all ([`QUEUE_WAIT_TIMEOUT_MS=0` is not a shorter wait](overload.md#queue_wait_timeout_ms0-is-not-a-shorter-wait)). How the server shortens the budget and how the deadline is enforced: [The wait budget](overload.md#the-wait-budget); when to lower it: [When to lower `QUEUE_WAIT_TIMEOUT_MS`](overload.md#when-to-lower-queue_wait_timeout_ms) |
| `QUEUE_MAX_WAITING` | Initial workers × 128, capped at `MAX_CONNECTIONS` / 2 | Maximum requests parked waiting for a queue slot at once. Past it, requests are rejected immediately instead of waiting. Each waiter holds a connection and a fully buffered request body for as long as it waits, so this is a bound on resources held, not on waits that will pay off. The `MAX_CONNECTIONS` / 2 cap on the default bounds this part of the backlog only; queued and running requests hold a connection the same way, so the headroom the server actually keeps for accepting and refusing follows from `PHP_WORKERS` + `QUEUE_CAPACITY` + `QUEUE_MAX_WAITING` against `MAX_CONNECTIONS` — see [Keeping Headroom for the Accept Loop](sizing-workers.md#keeping-headroom-for-the-accept-loop). Size it from your own handler latency — see [Sizing the Waiting Set](sizing-workers.md#sizing-the-waiting-set). `0` = auto, never below 1 |
| `QUEUE_MAX_WAITING_BYTES` | `67108864` (64 MiB) | Maximum request-body bytes the parked requests may hold between them, in bytes. A request whose body would push the total past it is rejected with 529 immediately instead of waiting; a request carrying no body is never rejected for this. `QUEUE_MAX_WAITING` bounds the same set in requests, which says nothing about their size — bodies are buffered in full before a request reaches the queue, so with only a count to stop them the parked set can hold `QUEUE_MAX_WAITING` × 10 MiB. Raise it for an upload-heavy application that should absorb bursts of large bodies rather than shed them; lower it on a memory-capped container. `0` = auto |

How to size the pool, the queue and the waiting set against each other and against `MAX_CONNECTIONS`, static against dynamic pools, and how the CPU count behind these defaults is detected, is in [Sizing PHP Workers](sizing-workers.md).

## Worker Mode

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_MAX_MEMORY_MIB` | `0` | Maximum memory in MiB per worker before recycling. `0` = unlimited |

Set `WORKER_MODE_ENABLED=true` and point `ENTRY_FILE` at your worker bootstrap script (e.g. `ENTRY_FILE=worker.php` or `ENTRY_FILE=../worker.php`). PHP workers then stay alive across requests, keeping bootstrap state (autoloaders, database connections) in memory. Workers are recycled automatically when they exceed `WORKER_MAX_MEMORY_MIB` or take three consecutive requests that came apart, and on demand when the application calls [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit); see [Recycling](../features/worker-recycling.md#recycling) for what counts towards the third.

## SAPI / PHP

| Variable | Default | Description |
|----------|---------|-------------|
| `SUPERGLOBALS_ENABLED` | `true` | Describe the request to PHP so it builds `$_SERVER` and `$_GET` before script execution. Set to a [falsy value](#boolean-values) to skip that work in an application that reads neither. It reaches those two only: `$_POST`, `$_FILES` and `$_COOKIE` are built from the request body and the `Cookie` header, which are handed to PHP either way, and `php://input` is a stream rather than a superglobal. See [SUPERGLOBALS_ENABLED](../php/request-api.md#superglobals_enabled) for the full table |

## Response Buffering

| Variable | Default | Description |
|----------|---------|-------------|
| `RESPONSE_BUFFER_BYTES` | `1048576` (1 MiB) | How much of a buffered response's output is held in memory. Past it, the response moves to a spool file on disk — see below. `0` = default |
| `RESPONSE_SPOOL_MAX_BYTES` | `1073741824` (1 GiB) | Maximum bytes the spool files of all responses may hold together. A response that would push the total past it is answered with `503`. `0` = default |

A response is buffered unless it streams (`Content-Type: text/event-stream`, `oxphp_stream_start()` or `oxphp_stream_flush()`): it is sent when the script ends or calls `oxphp_finish_request()`, so everything the script writes is held until then, and afterwards for as long as the client takes to read it. Up to `RESPONSE_BUFFER_BYTES` that output stays in memory. Past it, the output moves to a temporary file, the rest of it follows, and the response is sent from that file, so a script that `readfile()`s a 500 MiB download no longer keeps 500 MiB of server memory for as long as a slow client takes to read it. The exception is a response eligible for [compression](../features/compression.md), which only a body of at most 3 MiB is: it is read back into memory to be compressed. Output written in streaming mode is never spooled: a streamed response leaves at every flush. That includes what a request writes between `oxphp_stream_start()` and its first flush, which stays in memory until then — all of it, if the request never flushes.

Spool files are created in the system temporary directory — `TMPDIR`, or `/tmp` when it is unset or empty — and removed from it right after they are created, so they do not stay in a directory listing, and the space each one holds is given back when its response has been sent or its client has gone, and also if the server is killed. Where that directory lives decides what the spool costs:

- **Container writable layer** (the default `/tmp` of an image): disk. On Kubernetes this counts toward the pod's ephemeral-storage limit, so size that limit for `RESPONSE_SPOOL_MAX_BYTES`.
- **tmpfs** (`docker run --tmpfs /tmp`, a Kubernetes `emptyDir` with `medium: Memory`): memory, charged to the container like any other. Point `TMPDIR` at a disk-backed volume instead.
- **Read-only root filesystem**: mount a writable volume and point `TMPDIR` at it. The server logs a warning at startup when it cannot create a file in the spool directory.

A response that cannot be spooled — `RESPONSE_SPOOL_MAX_BYTES` is reached, or the file cannot be created or written, for example because the disk is full — is answered with `503 Service Unavailable` and `Retry-After: 5` instead of the script's status, headers and output, and a warning naming the cause is logged. The script itself runs to completion; whatever it writes after that point is discarded.

## Timeouts

| Variable | Default | Description |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | Maximum seconds to receive HTTP headers after connection (Slowloris protection) |
| `DRAIN_TIMEOUT_SECONDS` | `25` | Maximum seconds to wait for in-flight connections during graceful shutdown |

PHP execution time is bounded by PHP's own `max_execution_time` ini directive (and `set_time_limit()` at runtime), not an OxPHP env var. That bound does not reach an async task: a thread of the `ASYNC_WORKERS` pool holds no execution deadline, so neither the directive nor `set_time_limit()` limits what a task dispatched with `oxphp_async()` runs. Bound such a task with the timeout argument of `oxphp_async_await()` instead — see [Async promises](../features/async-promises.md#limitations).

## Rate Limiting

| Variable | Default | Description |
|----------|---------|-------------|
| `RATE_LIMIT` | `0` (off) | Maximum requests per IP per time window. `0` disables rate limiting |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window duration in seconds |

## Security

| Variable | Default | Description |
|----------|---------|-------------|
| `FRAME_OPTIONS` | `SAMEORIGIN` | Clickjacking protection. `SAMEORIGIN` allows framing only by pages on the same origin, `DENY` blocks all framing, `off` disables (use when managing framing via your own CSP). Any other value falls back to the `SAMEORIGIN` default with a startup warning. Sets both `X-Frame-Options` and `Content-Security-Policy: frame-ancestors` on every response. See [Clickjacking protection](#clickjacking-protection) below for the emitted header values, how server headers defer to application-set ones, and guidance on choosing a value |
| `TRUSTED_PROXIES` | *(unset)* | Trusted reverse proxy networks: a comma-separated mix of CIDRs, bare IPs and the keywords `private` and `cloudflare` (e.g. `private,cloudflare`). When set, OxPHP extracts the real client IP from `Forwarded` ([RFC 7239](https://www.rfc-editor.org/rfc/rfc7239)) or `X-Forwarded-For` headers using the rightmost-non-trusted algorithm; with `cloudflare`, a request whose forwarding chain shows it arrived from a Cloudflare edge takes it from `CF-Connecting-IP` instead. Also processes `X-Forwarded-Proto` and `X-Forwarded-Host` for `$_SERVER['HTTPS']`, `REQUEST_SCHEME`, `SERVER_NAME`, and `SERVER_PORT`. Unset = feature disabled |
| `PHP_DENY_PATHS` | *(unset)* | Comma-separated glob patterns whose `.php` files must never execute via direct URI (e.g. `/uploads/**,/cache/**,/admin/legacy.php`). Patterns may target whole directories or single files. Applies in the direct-mapping modes — Traditional and SPA; ignored with a startup warning in Framework and Worker modes, which never execute arbitrary `.php` files directly. Also covers scripts reached through directory-index resolution (`/uploads/` → `uploads/index.php`). For direct `.php` URIs, matching happens before disk I/O, so denied paths produce the same response whether the file exists or not (no existence oracle). See [PHP Execution Deny-List](../security/php-deny.md) |
| `PHP_DENY_FALLBACK` | `404` | What to return on a `PHP_DENY_PATHS` match or a `.oxphpdeny` deny rule. Either an HTTP status `400`–`599` (pairs with `ERROR_PAGES_DIR` for custom HTML) or a `/`-prefixed URI path to a PHP fallback script inside `DOCUMENT_ROOT`. The fallback script receives `OXPHP_DENIED_PATH` and `OXPHP_DENIED_PATTERN` in `$_SERVER`. Validated at startup: the script must exist, canonicalize inside `DOCUMENT_ROOT`, and must not itself match `PHP_DENY_PATHS` (loop prevention). A `.oxphpdeny` rule may cover the script; in worker mode `.oxphpdeny` deny rules take a status code only. See [Path Deny Rules](../security/oxphpdeny.md) |
| `SYMLINK_ALLOW_PATHS` | *(unset)* | Comma-separated list of absolute paths under which symlinks are permitted to escape `DOCUMENT_ROOT`. Each entry must already exist on disk; relative paths and missing paths abort startup. Unset = no symlink escapes allowed. See [Symlink Allow-Paths](../security/symlink-allow-paths.md) |

The special value `private` expands to all RFC-1918 private networks, loopback, and link-local addresses (IPv4 and IPv6): `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `127.0.0.0/8`, `169.254.0.0/16`, `::1/128`, `fc00::/7`, `fe80::/10`.

### Clickjacking protection

Clickjacking is an attack where a hostile page embeds your site in an invisible `<iframe>` and tricks the user into clicking something they can't see — a "yes, delete my account" button, a one-click purchase, an OAuth "authorize" prompt. The defense is to tell the browser who, if anyone, is allowed to frame your pages. `FRAME_OPTIONS` controls this.

Because two headers govern framing — the legacy `X-Frame-Options` (understood by all browsers) and the modern `Content-Security-Policy: frame-ancestors` (which supersedes it where both are present) — OxPHP emits **both** so the policy holds on old and new browsers alike. A single `FRAME_OPTIONS` value maps to a matching pair:

| `FRAME_OPTIONS` | `X-Frame-Options` | `Content-Security-Policy` | Who may frame your pages |
|-----------------|-------------------|---------------------------|--------------------------|
| `SAMEORIGIN` (default) | `SAMEORIGIN` | `frame-ancestors 'self'` | Only pages on the same origin |
| `DENY` | `DENY` | `frame-ancestors 'none'` | No one, not even your own pages |
| `off` | *(not sent)* | *(not sent)* | Anyone — no framing restriction from the server |

**Choosing a value.** `SAMEORIGIN` is the default: it blocks the cross-origin framing that clickjacking actually relies on, while still letting your own pages embed each other — which many apps legitimately do (admin previews, dashboard widgets, payment components hosted on the same origin). Choose `DENY` when nothing on your site is ever meant to be framed, not even by itself, for the strictest posture. Choose `off` only when you manage framing yourself through a full `Content-Security-Policy` your application sets — see below.

**Framing external origins.** Neither `X-Frame-Options` value can name a specific allowed origin (`ALLOW-FROM` was removed from the standard). To permit a named third party to frame your pages, set `FRAME_OPTIONS=off` and have your application emit its own `Content-Security-Policy` with an explicit `frame-ancestors` list, e.g. `header("Content-Security-Policy: frame-ancestors 'self' https://partner.example.com");`.

**Application headers win.** The server headers are fallbacks, applied only when the response carries no such header — an application that sets its own `X-Frame-Options` or `Content-Security-Policy` via PHP `header()` keeps it untouched. The two framing headers are treated as one policy, so the server never contradicts the application:

- If your app sets `X-Frame-Options`, OxPHP skips its `frame-ancestors` fallback (a server CSP would override your app's choice in modern browsers).
- If your app sets a `Content-Security-Policy` that contains a `frame-ancestors` directive, OxPHP skips its `X-Frame-Options` fallback (a stricter server `X-Frame-Options` would over-block in legacy browsers that ignore CSP).

The same precedence applies to `X-Content-Type-Options`, which OxPHP sets to `nosniff` on every response: an application-set value is kept verbatim. Note that `nosniff` is the only value that does anything — an application overriding it with anything else silently disables MIME-sniffing protection.

## TLS

| Variable | Default | Description |
|----------|---------|-------------|
| `TLS_CERT` | *(unset)* | Path to PEM-encoded TLS certificate. Both `TLS_CERT` and `TLS_KEY` must be set to enable TLS |
| `TLS_KEY` | *(unset)* | Path to PEM-encoded TLS private key |
| `TLS_MIN_VERSION` | `1.2` | Minimum accepted TLS protocol version: `1.2` or `1.3`. Validated at startup (and by `oxphp config --check`) even when TLS is not enabled — any other value, including non-UTF-8 bytes, is a hard startup error. An empty value is treated as unset |

## HTTP/2

| Variable | Default | Description |
|----------|---------|-------------|
| `H2_MAX_CONCURRENT_STREAMS` | `PHP_WORKERS_MAX × 4` (min 32) | Maximum simultaneous open streams per HTTP/2 connection |
| `H2_MAX_PENDING_RESET` | `20` | Maximum `RST_STREAM` frames queued before a connection is closed (Rapid Reset protection) |
| `H2_MAX_HEADER_LIST_BYTES` | `65536` | Maximum total decoded header bytes per request |
| `H2_KEEPALIVE_INTERVAL_SECS` | `20` | Seconds between HTTP/2 PING frames; `0` disables |
| `H2_KEEPALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a PING reply before closing the connection |

## Static Files

| Variable | Default | Description |
|----------|---------|-------------|
| `STATIC_MAX_AGE` | `30d` | `Cache-Control: max-age` for static files. Accepts: `30s`, `5m`, `2h`, `30d`, `1w`, `1y`, bare seconds (`3600`), or `off` to disable the header. |
| `STATIC_REVALIDATE` | `off` | Boolean — see [Boolean values](#boolean-values). Set truthy to enable mtime revalidation on the in-memory content cache: the file's modification time is re-checked at most once every 3 seconds per file (not per request) and stale entries are evicted automatically, so changes become visible within 3 seconds. |
| `COMPRESSION_ENCODINGS` | `br,zstd,gzip` | Content codings the server offers, comma-separated: `br` (or `brotli`), `zstd`, `gzip`. An unknown name is a startup error. Set the whole value to `off` (or `none`) to switch compression off entirely — it is not a list token, so `br,off` is an error rather than a switch. An empty assignment is treated as unset and restores the default. The order written here is ignored — cached static files prefer Brotli, whose cost is paid once, and everything else prefers Zstandard. See [Compression](../features/compression.md#choosing-a-coding) |
| `COMPRESSION_BROTLI_LEVEL` | `5` | Brotli quality (0–11) |
| `COMPRESSION_ZSTD_LEVEL` | `6` | Zstandard compression level (0–19) |
| `COMPRESSION_GZIP_LEVEL` | `6` | Gzip compression level (0–9) |

A coding is offered when `COMPRESSION_ENCODINGS` lists it **and** its level is not `0`; either one alone withdraws it. A level outside its range, or one that is not a number, aborts startup with an error naming the variable — nothing is clamped.

## Logging

| Variable | Default | Description |
|----------|---------|-------------|
| `LOG_LEVEL` | `info` | Log verbosity: `trace`, `debug`, `info`, `warn`, `error` |
| `ACCESS_LOG` | *(unset)* | Per-request access log: `all` = every request, `error` = 4xx/5xx only, unset = off |

> **Note:** `ACCESS_LOG` accepts `all` or `error`. Leave it unset to disable access logging entirely.

> **Note:** A client that closes its connection before the server has answered is logged at `debug`, not `error`. It is the commonest thing a public listener sees — a cancelled fetch, a closed tab, a proxy whose read timeout fired — and it is not a failure of the server's, so at the default `LOG_LEVEL` it is invisible. Count them with `oxphp_request_cancelled_total{reason="client_abort"}`; raise `LOG_LEVEL=debug` to see the individual connections. Timeouts the server itself enforces (`HEADER_TIMEOUT_SECONDS`) stay at `warn`, and every other connection failure stays at `error`.

> **Note:** The two are independent. `LOG_LEVEL` sets the verbosity of the server's own diagnostics; it does not filter the access log, so `LOG_LEVEL=warn` with `ACCESS_LOG=error` reports warnings and errors *and* logs every 4xx/5xx request. Silencing the access log on its own takes a `RUST_LOG` directive naming its target — `RUST_LOG=warn,access_log=off`, with `warn` standing in for whatever `LOG_LEVEL` you were running: `RUST_LOG`, when set and parsable, replaces `LOG_LEVEL` rather than layering on it. See [Access logging](../features/access-logging.md#fine-grained-filtering).

## Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `INTERNAL_ADDR` | *(unset)* | Address for the internal server (`/health`, `/metrics`, `/config`). Internal server is not started when unset. A port-only value (`:9090` or `9090`) binds `127.0.0.1`; bind an explicit `0.0.0.0:9090` to expose it off-host |
| `INTERNAL_ALLOW_IPS` | *(unset)* | Comma-separated CIDR/IP allow-list for the internal server. A peer outside the list receives `403` on `/metrics`, `/config`, and plugin paths; health probes (`/health`, `/healthz`, `/readyz`, `/startupz` and their long forms) stay reachable. Unset/empty = allow all. Loopback is **not** implicit — list `127.0.0.1/32` to keep localhost access. A malformed list aborts startup |
| `ERROR_PAGES_DIR` | *(unset)* | Directory containing custom error pages named `{status}.html` (e.g., `404.html`, `503.html`) |
| `MAX_QUERY_BODY` | `524288` | Maximum request body size in bytes for internal query endpoints (512 KiB) |
| `TRACE_CONTEXT` | `false` | Boolean — see [Boolean values](#boolean-values). When truthy, enables W3C Trace Context propagation: reads `traceparent`/`tracestate` headers and forwards them to PHP via `$_SERVER` |

## OpenTelemetry

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | Enable OpenTelemetry span export. Automatically sets `TRACE_CONTEXT=true`. Boolean — see [Boolean values](#boolean-values) |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Export protocol: `grpc` or `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` (gRPC) or `http://localhost:4318` (HTTP) | OTLP collector endpoint. With `http/protobuf`, a base URL: spans are sent to `/v1/traces` under it |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Export timeout in milliseconds. Over gRPC it also bounds connecting to the collector, TLS handshake included, so keep it above the time that takes; an attempt that has to connect first can take up to twice the timeout. Over gRPC on Linux it also sets how soon a connection that stopped taking data is closed — see [Span export errors in the log](../features/distributed-tracing.md#span-export-errors-in-the-log) |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(unset)* | Authentication headers: `key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | Service name in exported spans |
| `OTEL_SERVICE_VERSION` | *(unset)* | Service version attribute |
| `OTEL_RESOURCE_ATTRIBUTES` | *(unset)* | Additional resource attributes: `env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Sampling strategy: `always_on`, `always_off`, `traceidratio`, `parentbased_always_on`, `parentbased_always_off`, `parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Sampling ratio (0.0–1.0) for ratio-based samplers |

> **Note:** Invalid or out-of-range `OTEL_TRACES_SAMPLER_ARG` values are clamped to `[0.0, 1.0]` and logged at warn level. Unknown `OTEL_TRACES_SAMPLER` values fall back to `parentbased_traceidratio` and are logged.

## APM

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_APM_ENABLED` | `false` | Enable APM: automatic instrumentation, error capture, and the PHP tracing SDK. Requires `OTEL_ENABLED=true`. Boolean — see [Boolean values](#boolean-values) |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | Slow query threshold in milliseconds. Database queries exceeding this get an `oxphp.db.slow=true` span attribute |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | Record bind parameters in the `db.params` span attribute. Disable in production if parameters may contain sensitive data. Boolean — see [Boolean values](#boolean-values) |
| `OTEL_APM_STACKTRACE_MAX_BYTES` | `8192` | Maximum size in bytes of the `exception.stacktrace` attribute. Over the cap the stacktrace is truncated from the tail with a `…(truncated)` marker. `0` disables truncation |
| `OTEL_APM_MESSAGE_MAX_BYTES` | `4096` | Maximum size in bytes of the `exception.message` attribute (default matches New Relic's per-attribute value limit). Over the cap the message is truncated from the tail with a `…(truncated)` marker. `0` disables truncation |

When APM is enabled, OxPHP automatically hooks 33 internal PHP functions (PDO, mysqli, cURL, Redis, Memcached, file I/O) to create child spans. The `oxphp_apm_*()` PHP functions are registered regardless of whether APM is enabled — when disabled, the span functions are no-ops returning sentinel values. `oxphp_apm_trace()` is the exception: it still runs its callback and returns the callback's value, so disabling APM never changes what your application computes.

## Async Workers

| Variable | Default | Description |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0` (disabled) | Number of dedicated async worker threads. When `0`, the async functions (`oxphp_async`, etc.) are registered but throw `OxPHP\Async\AsyncException` on call. Set to a positive value to enable background task execution |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS × 64` | Maximum pending tasks in the async queue. `0` = auto (workers × 64) |
| `ASYNC_MAX_FIBERS` | `256` | Per-worker cap on concurrent async task fibers. The process-global in-flight limit (queued + running) is `ASYNC_MAX_FIBERS × ASYNC_WORKERS`; a dispatch past it is rejected immediately with `OxPHP\Async\AsyncException` so fan-out composition cannot deadlock |

The async worker pool runs the closures PHP dispatches with `oxphp_async()`. A task the request ends without awaiting is cancelled then, and so is a task another task dispatched and ended without awaiting (see [Async Promises](../features/async-promises.md#how-it-works)), so work that has to finish must be awaited. The pool is separate from the PHP worker pool and is not required for standard request handling.

A malformed value in any of these three variables (e.g. `ASYNC_WORKERS=8x`) is a startup error — falling back to a default would silently disable or misconfigure the pool. An exactly-empty value is treated as unset.

## Runtime Hooks

| Variable | Default | Description |
|----------|---------|-------------|
| `RUNTIME_HOOKS` | _(off)_ | Opt-in replacement of blocking PHP builtins with fiber-suspending implementations. `1`/`true`/`all` enables every hook category; a comma-separated list enables specific categories (e.g. `RUNTIME_HOOKS=sleep,streams`) |

What each category hooks, how to check which ones are in effect, where they do not suspend and what they cost is in [Runtime Hooks](../features/runtime-hooks.md). What the overlap they add does to a framework that keeps request state in shared objects is in [Overlapping Requests and Framework State](../features/overlapping-requests.md).

## Shared State

In-process concurrency primitives (`OxPHP\Shared\Counter`, `Map`, `Channel`, `Mutex`, `Once`, `Pool`, `Atomic`, `Flag`, `Registry`). See [Shared State](../shared-state/shared-state.md) for the API tour.

| Variable | Default | Description |
|----------|---------|-------------|
| `SHARED_ENABLED` | `true` | Boolean — see [Boolean values](#boolean-values). Master switch for the entire `OxPHP\Shared\*` subsystem. Set to `false` and none of its classes are registered: `new OxPHP\Shared\Counter()` throws `Error: Class "OxPHP\Shared\Counter" not found`, and `/__ox_shared/*` and the `oxphp_shared_*` metrics are absent. Guard with `class_exists('OxPHP\Shared\Counter')` in code that must run either way — not with `interface_exists('OxPHP\Shared\Shareable')`, which stays true because the extension registers that interface either way |
| `SHARED_MAX_ENTRIES` | `100000` | Global cap on the number of live Shared objects (each `Counter`, `Map`, `Channel`, … counts as one). Creating an object past it fails with `CapacityException`. Keys inside a `Map` do not count toward it — bound a `Map` with `maxEntries` |
| `SHARED_MAX_BYTES` | `1073741824` (1 GiB) | Global cap on estimated memory across all Shared objects, checked only when an object is created (`CapacityException`). Objects that already exist can keep growing past it, so the total can read above the cap |
| `SHARED_METRICS_ENABLED` | `true` | Boolean. Toggles the `oxphp_shared_*` Prometheus exposition |
| `SHARED_INTROSPECTION_ENABLED` | `true` | Boolean. Toggles the `/__ox_shared/*` introspection API on the internal server |
| `SHARED_INTROSPECTION_PREVIEW_ENABLED` | `true` | Boolean. Toggles value previews in introspection responses (disable when previews could leak sensitive data) |
| `SHARED_CYCLE_DETECT_DEPTH` | `16` | BFS depth during cycle check. Raise for deep legitimate graphs |
| `SHARED_CYCLE_DETECT_EDGES` | `10000` | Edges walked during cycle check. Raise for dense legitimate graphs |
| `SHARED_MAX_VALUE_SIZE` | `1048576` (1 MiB) | Per-value size cap. Inserting a larger value fails fast |
| `SHARED_MAX_CHANNEL_BYTES` | `67108864` (64 MiB) | Per-channel total payload cap |
| `SHARED_LOCK_DIAGNOSTICS` | `warn` (`strict` in a debug build of the binary) | Cross-thread deadlock detector for `Shared\Mutex`: `off`, `warn` (log and count cycles) or `strict` (also break them with `DeadlockException`). Lowercase only; any other value silently gives the default. See [Deadlock detector](../shared-state/shared-observability.md#deadlock-detector) |
| `SHARED_LOCK_POLL_INTERVAL_MS` | `100` | How often the deadlock detector scans, in milliseconds. Unused when `SHARED_LOCK_DIAGNOSTICS=off` |
| `SHARED_PREVIEW_STRING_LIMIT` | `256` | Per-string truncation in `/__ox_shared/preview` previews, in bytes (cut at a char boundary) |
| `SHARED_PREVIEW_ARRAY_LIMIT` | `20` | Entries sampled in `/entry?id=…` previews |

## Profiling

Per-request profiler that writes xhprof and collapsed-stack profiles. See [Profiling](../features/profiling.md) for output formats and viewer integration.

| Variable | Default | Description |
|----------|---------|-------------|
| `PROFILER_ENABLED` | `false` | Boolean — see [Boolean values](#boolean-values). Master switch. Only this variable and `PROFILER_EXCLUDE_PATHS` stop the server from starting on a bad value, and they do so with the profiler off as well. A number in another `PROFILER_*` variable that does not parse silently takes its default |
| `PROFILER_SAMPLE_RATE` | `0.0` | Probability (0.0–1.0) that a request is sampled. Values outside the range are clamped |
| `PROFILER_AUTH_TOKEN` | *(unset)* | Secret the `X-OxPHP-Profile` header must carry, and the bearer token for the `/__profiler/*` routes on the internal server. Unset or empty = no token required: any non-empty header value passes, and the `/__profiler/*` routes answer without a bearer. `PROFILER_SAMPLE_RATE` sampling and the `OxPHP\Profile\*` PHP functions never consult it — under `PROFILER_ENABLED=true` a script that calls `OxPHP\Profile\start()` profiles its own request with no token involved |
| `PROFILER_MAX_SPANS` | `50000` | Per-request cap on profile spans. Profiles exceeding the cap are truncated |
| `PROFILER_MAX_DEPTH` | `0` (no cap) | Maximum nesting depth of recorded calls; a deeper call and, except under self-recursion past 32 frames, everything it calls are absent from the profile — their time is not, and reads as exclusive time of the call at the limit. Dropped calls still count against `PROFILER_MAX_SPANS`. Clamped to `32`, the depth the engine observer tracks; with `PROFILER_ENABLED=true` a larger value is reported as clamped at startup |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | Directory for on-disk profile files |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,collapsed` | Comma-separated list of output formats to write to disk: `xhprof`, `collapsed`. An unknown name is skipped with a startup warning; a list with no known name writes both |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | Rate limit on profile files written to disk per second |
| `PROFILER_RETENTION_COUNT` | `100` | Maximum profile files kept in `PROFILER_OUTPUT_DIR`. Older files are pruned |
| `PROFILER_EXPORT_URL` | *(unset)* | Remote endpoint to POST profiles to. When set, disk writes still happen unless `PROFILER_OUTPUT_FORMATS` is empty |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | Wire format for `PROFILER_EXPORT_URL` posts: `xhprof` or `collapsed`. The body is exactly that format. An unknown value falls back to `xhprof` with a startup warning |
| `PROFILER_EXPORT_AUTH_TOKEN` | *(unset)* | Optional bearer token sent with each export request |

## Example Configurations

### Development

```bash
LISTEN_ADDR=127.0.0.1:8080
DOCUMENT_ROOT=./public
LOG_LEVEL=debug
ACCESS_LOG=all
PHP_WORKERS=1
INTERNAL_ADDR=127.0.0.1:9090
```

### Production (Framework)

```bash
LISTEN_ADDR=0.0.0.0:80
DOCUMENT_ROOT=/var/www/html/public
ENTRY_FILE=index.php
PHP_WORKERS=8
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
ACCESS_LOG=error
MAX_CONNECTIONS=10000
INTERNAL_ADDR=127.0.0.1:9090
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
TRUSTED_PROXIES=private
HEADER_TIMEOUT_SECONDS=5
DRAIN_TIMEOUT_SECONDS=25
COMPRESSION_BROTLI_LEVEL=5
COMPRESSION_ZSTD_LEVEL=6
COMPRESSION_GZIP_LEVEL=6
STATIC_MAX_AGE=30d
```

### Production (Worker Mode)

```bash
LISTEN_ADDR=0.0.0.0:80
DOCUMENT_ROOT=/var/www/html/public
WORKER_MODE_ENABLED=true
ENTRY_FILE=../worker.php
PHP_WORKERS=8
WORKER_MAX_MEMORY_MIB=128
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
ACCESS_LOG=error
INTERNAL_ADDR=127.0.0.1:9090
```

### TLS

```bash
LISTEN_ADDR=0.0.0.0:443
TLS_CERT=/etc/ssl/oxphp/cert.pem
TLS_KEY=/etc/ssl/oxphp/key.pem
DOCUMENT_ROOT=/var/www/html/public
ENTRY_FILE=index.php
```

## Inspecting Active Configuration

When the internal server is running, query the `/config` endpoint to see the resolved configuration:

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "access_log": "all",
  "async_in_flight_cap": 0,
  "async_max_fibers": 256,
  "async_queue_capacity": 0,
  "async_workers": 0,
  "brotli_level": 5,
  "deny_file": {"loaded": false},
  "document_root": "/var/www/html/public",
  "drain_timeout_seconds": 30,
  "entry_file": "/var/www/html/public/index.php",
  "entry_only": false,
  "executor_type": "sapi",
  "gzip_level": 6,
  "header_timeout_seconds": 5,
  "listen_addr": "0.0.0.0:80",
  "log_level": "warn",
  "max_connections": 10000,
  "max_query_body": 524288,
  "php_workers": "8",
  "plugins": {
    "apm": {
      "db_capture_params": false,
      "enabled": true,
      "hooks_registered": 33,
      "slow_query_ms": 100
    },
    "otel": {
      "enabled": true,
      "protocol": "grpc",
      "service_name": "oxphp"
    }
  },
  "queue_capacity": 1024,
  "queue_max_waiting": 1024,
  "queue_max_waiting_bytes": 67108864,
  "queue_wait_timeout_ms": 1000,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "response_buffer_bytes": 1048576,
  "response_spool_max_bytes": 1073741824,
  "runtime_hooks": [],
  "static_max_age": 2592000,
  "static_revalidate": false,
  "superglobals_enabled": true,
  "tls_enabled": true,
  "tls_min_version": "1.2",
  "tokio_workers": 4,
  "trace_context": true,
  "trusted_proxies": false,
  "worker_max_memory_mib": 0,
  "worker_mode_enabled": false,
  "zstd_level": 6
}
```

> **Note:** The served `/config` response scrubs a few keys that the internal `Config` representation carries: TLS certificate and key paths are never emitted (`tls_enabled` indicates whether TLS is active), and `internal_addr` and `error_pages_dir` are removed — deployment topology and filesystem paths that aid an attacker and are not needed by metrics scrapers.

## See Also

- [Routing](../features/routing.md) — routing modes and `ENTRY_FILE` behavior
- [Health Checks](health-checks.md) — internal server endpoints
- [Metrics](metrics.md) — Prometheus-compatible metrics reference
- [Diagnosing Overload](overload.md) — reading the queue, admission and wait-budget metrics together, and what the server logs while it sheds requests
- [Sizing PHP Workers](sizing-workers.md) — sizing the pool, the queue and the waiting set against each other and against `MAX_CONNECTIONS`
- [Runtime Hooks](../features/runtime-hooks.md) — the `RUNTIME_HOOKS` categories and where hooked calls do not suspend
- [Graceful Shutdown](graceful-shutdown.md) — how `DRAIN_TIMEOUT_SECONDS` affects shutdown
- [TLS](../features/tls.md) — TLS setup and certificate requirements
- [Rate Limiting](../features/rate-limiting.md) — per-IP rate limiting details
- [Worker Mode](../features/worker-mode.md) — persistent PHP worker architecture
- [Compression](../features/compression.md) — codec selection and per-coding levels
- [Static Files](../features/static-files.md) — caching and file serving
- [Distributed Tracing & APM](../features/distributed-tracing.md) — OTel export, auto-instrumentation, and PHP tracing SDK
