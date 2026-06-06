---
title: Configuration Reference
description: Complete environment variable reference for OxPHP. Every setting, its default, and what it controls — all in one place.
---

# Configuration Reference

OxPHP is configured entirely through environment variables. There are no configuration files to manage — every setting has a sensible default, so a zero-configuration deployment works out of the box.

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
| `DOCUMENT_ROOT` | `/var/www/html/public` | Root directory for serving files and PHP scripts |
| `ENTRY_FILE` | *(unset)* | Single canonical entry script. Unset = direct file mapping. `*.php` = front controller. Non-`.php` = static fallback (SPA). With `WORKER_MODE_ENABLED=true` = worker bootstrap. Resolved against `DOCUMENT_ROOT` (relative paths and `..` allowed; absolute paths used as-is). See [Routing](../features/routing.md) |
| `WORKER_MODE_ENABLED` | `false` | Enable persistent worker mode. Requires `ENTRY_FILE` to point at a `.php` script. Boolean — see [Boolean values](#boolean-values) |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent TCP connections |
| `TOKIO_WORKERS` | CPU / 2 (min 1) | Async I/O threads. `1` = single-threaded, `N > 1` = fixed thread count, unset = auto (CPU / 2, min 1) |

## PHP Workers

| Variable | Default | Description |
|----------|---------|-------------|
| `EXECUTOR` | `sapi` | PHP executor backend. `sapi` for PHP execution, `stub` for benchmarking without PHP |
| `PHP_WORKERS` | CPU / 2 (min 1) | Worker pool size. `N` = fixed pool, `MIN:MAX` = dynamic scaling, `0` = auto |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Seconds a dynamic worker stays idle before being retired (dynamic mode only) |
| `QUEUE_CAPACITY` | Initial workers × 128 | Maximum pending requests in the PHP queue. Returns 529 when full. For dynamic pools (`MIN:MAX`), initial workers = minimum count |

### Static vs Dynamic Workers

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

## Worker Mode

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_MAX_MEMORY_MIB` | `0` | Maximum memory in MiB per worker before recycling. `0` = unlimited |

Set `WORKER_MODE_ENABLED=true` and point `ENTRY_FILE` at your worker bootstrap script (e.g. `ENTRY_FILE=worker.php` or `ENTRY_FILE=../worker.php`). PHP processes then stay alive across requests, keeping bootstrap state (autoloaders, database connections) in memory. Workers are recycled automatically when they exceed `WORKER_MAX_MEMORY_MIB`, or on demand when the application calls [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit). The `WORKER_MAX_REQUESTS` knob from earlier releases is deprecated and ignored — set neither, or migrate to `Worker::scheduleExit()`.

### Deprecated: `INDEX_FILE` and `WORKER_FILE`

The legacy `INDEX_FILE` and `WORKER_FILE` variables are still parsed for backwards compatibility. When set, they emit a `WARN` log line at startup and map onto the new model:

| Legacy | Equivalent today |
|---|---|
| `INDEX_FILE=index.php` | `ENTRY_FILE=index.php` |
| `INDEX_FILE=index.html` | `ENTRY_FILE=index.html` |
| `WORKER_FILE=/path/worker.php` | `WORKER_MODE_ENABLED=true ENTRY_FILE=/path/worker.php` |

If both old and new are set, `ENTRY_FILE` / `WORKER_MODE_ENABLED` win. Migrate at your convenience; the deprecated forms will be removed in a future release.

## SAPI / PHP

| Variable | Default | Description |
|----------|---------|-------------|
| `SUPERGLOBALS_ENABLED` | `true` | Populate PHP superglobals (`$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_SERVER`, `php://input`) before script execution. Set to a [falsy value](#boolean-values) to skip population — request data is then only available through the object API (`oxphp_http_request()`). Useful for applications that consume the object API directly and want to avoid the cost of building superglobals on every request |

## Timeouts

| Variable | Default | Description |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | Maximum seconds to receive HTTP headers after connection (Slowloris protection) |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Maximum seconds to wait for in-flight connections during graceful shutdown |

PHP execution time is bounded by PHP's own `max_execution_time` ini directive (and `set_time_limit()` at runtime), not an OxPHP env var.

## Rate Limiting

| Variable | Default | Description |
|----------|---------|-------------|
| `RATE_LIMIT` | `0` (off) | Maximum requests per IP per time window. `0` disables rate limiting |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window duration in seconds |

## Security

| Variable | Default | Description |
|----------|---------|-------------|
| `FRAME_OPTIONS` | `DENY` | Clickjacking protection. `DENY` blocks all framing, `SAMEORIGIN` allows same-origin framing, `off` disables (use when managing framing via your own CSP). Sets both `X-Frame-Options` and `Content-Security-Policy: frame-ancestors` |
| `TRUSTED_PROXIES` | *(unset)* | Trusted reverse proxy networks (comma-separated CIDRs or `private`). When set, OxPHP extracts the real client IP from `Forwarded` ([RFC 7239](https://www.rfc-editor.org/rfc/rfc7239)) or `X-Forwarded-For` headers using the rightmost-non-trusted algorithm. Also processes `X-Forwarded-Proto` and `X-Forwarded-Host` for `$_SERVER['HTTPS']`, `REQUEST_SCHEME`, `SERVER_NAME`, and `SERVER_PORT`. Unset = feature disabled |
| `PHP_DENY_PATHS` | *(unset)* | Comma-separated glob patterns whose `.php` files must never execute via direct URI (e.g. `/uploads/**,/cache/**,/admin/legacy.php`). Patterns may target whole directories or single files. Direct-mapping (no `ENTRY_FILE`) only — ignored with a startup warning when `ENTRY_FILE` is set, since front-controller, SPA, and worker modes already route every request through one trusted script. Matching happens before disk I/O, so denied paths produce the same response whether the file exists or not (no existence oracle). The legacy name `PHP_DENY_DIRS` is accepted as a deprecated alias and emits a startup `WARN`. See [PHP Execution Deny-List](../security/php-deny.md) |
| `PHP_DENY_FALLBACK` | `404` | What to return on a `PHP_DENY_PATHS` match. Either an HTTP status `400`–`599` (pairs with `ERROR_PAGES_DIR` for custom HTML) or a `/`-prefixed URI path to a PHP fallback script inside `DOCUMENT_ROOT`. The fallback script receives `OXPHP_DENIED_PATH` and `OXPHP_DENIED_PATTERN` in `$_SERVER`. Validated at startup: the script must exist, canonicalize inside `DOCUMENT_ROOT`, and must not itself match `PHP_DENY_PATHS` (loop prevention) |
| `SYMLINK_ALLOW_PATHS` | *(unset)* | Comma-separated list of absolute paths under which symlinks are permitted to escape `DOCUMENT_ROOT`. Each entry must already exist on disk; relative paths and missing paths abort startup. Unset = no symlink escapes allowed. See [Symlink Allow-Paths](../security/symlink-allow-paths.md) |

The special value `private` expands to all RFC-1918 private networks, loopback, and link-local addresses (IPv4 and IPv6): `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `127.0.0.0/8`, `169.254.0.0/16`, `::1/128`, `fc00::/7`, `fe80::/10`.

## TLS

| Variable | Default | Description |
|----------|---------|-------------|
| `TLS_CERT` | *(unset)* | Path to PEM-encoded TLS certificate. Both `TLS_CERT` and `TLS_KEY` must be set to enable TLS |
| `TLS_KEY` | *(unset)* | Path to PEM-encoded TLS private key |

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
| `STATIC_MAX_AGE` | `30d` | `Cache-Control: max-age` for static files. Accepts: `30s`, `5m`, `2h`, `30d`, `1w`, `1y`, bare seconds (`3600`), or `off` to disable the header. Replaces deprecated `STATIC_CACHE_TTL`. |
| `STATIC_REVALIDATE` | `off` | Boolean — see [Boolean values](#boolean-values). Set truthy to enable mtime revalidation on the in-memory content cache: each cache hit checks the file's modification time and evicts stale entries automatically. Replaces deprecated `STATIC_CACHE` (where `off` had the inverse meaning). |
| `COMPRESSION_LEVEL` | `4` | Brotli compression quality (0–11). `0` disables compression |

## Logging

| Variable | Default | Description |
|----------|---------|-------------|
| `LOG_LEVEL` | `info` | Log verbosity: `trace`, `debug`, `info`, `warn`, `error` |
| `ACCESS_LOG` | *(unset)* | Per-request access log: `all` = every request, `error` = 4xx/5xx only, unset = off |

> **Note:** `ACCESS_LOG` accepts `all` or `error`. Leave it unset to disable access logging entirely.

## Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `INTERNAL_ADDR` | *(unset)* | Address for the internal server (`/health`, `/metrics`, `/config`). Internal server is not started when unset |
| `ERROR_PAGES_DIR` | *(unset)* | Directory containing custom error pages named `{status}.html` (e.g., `404.html`, `503.html`) |
| `MAX_QUERY_BODY` | `524288` | Maximum request body size in bytes for internal query endpoints (512 KiB) |
| `TRACE_CONTEXT` | `false` | Boolean — see [Boolean values](#boolean-values). When truthy, enables W3C Trace Context propagation: reads `traceparent`/`tracestate` headers and forwards them to PHP via `$_SERVER` |

## OpenTelemetry

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | Enable OpenTelemetry span export. Automatically sets `TRACE_CONTEXT=true`. Boolean — see [Boolean values](#boolean-values) |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Export protocol: `grpc` or `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` (gRPC) or `http://localhost:4318` (HTTP) | OTLP collector endpoint |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Export timeout in milliseconds |
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

When APM is enabled, OxPHP automatically hooks 33 internal PHP functions (PDO, mysqli, cURL, Redis, Memcached, file I/O) to create child spans. The `oxphp_apm_*()` PHP functions are registered regardless of whether APM is enabled — when disabled, they are safe no-ops.

## Async Workers

| Variable | Default | Description |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0` (disabled) | Number of dedicated async worker threads. When `0`, the async functions (`oxphp_async`, etc.) are registered but throw `OxPHP\Async\AsyncException` on call. Set to a positive value to enable background task execution |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS × 64` | Maximum pending tasks in the async queue. `0` = auto (workers × 64) |

The async worker pool handles fire-and-forget background tasks dispatched from PHP. It is separate from the PHP worker pool and is not required for standard request handling.

## Shared State

In-process concurrency primitives (`OxPHP\Shared\Counter`, `Map`, `Channel`, `Mutex`, `Once`, `Pool`, `Atomic`, `Flag`, `Registry`). See [Shared State](../shared-state/shared-state.md) for the API tour.

| Variable | Default | Description |
|----------|---------|-------------|
| `SHARED_ENABLED` | `true` | Boolean — see [Boolean values](#boolean-values). Master switch for the entire `OxPHP\Shared\*` subsystem |
| `SHARED_MAX_ENTRIES` | `100000` | Global cap on all Shared entries combined. Insert past this fails with `CapacityException` |
| `SHARED_MAX_BYTES` | `1073741824` (1 GiB) | Global cap on estimated memory across all Shared entries |
| `SHARED_SOFT_LIMIT_RATIO` | `0.7` | Start shedding lowest-priority work when usage crosses this fraction of `SHARED_MAX_BYTES` / `SHARED_MAX_ENTRIES` |
| `SHARED_METRICS_ENABLED` | `true` | Boolean. Toggles the `oxphp_shared_*` Prometheus exposition |
| `SHARED_INTROSPECTION_ENABLED` | `true` | Boolean. Toggles the `/__ox_shared/*` introspection API on the internal server |
| `SHARED_INTROSPECTION_PREVIEW_ENABLED` | `true` | Boolean. Toggles value previews in introspection responses (disable when previews could leak sensitive data) |
| `SHARED_CYCLE_DETECT_DEPTH` | `16` | BFS depth during cycle check. Raise for deep legitimate graphs |
| `SHARED_CYCLE_DETECT_EDGES` | `10000` | Edges walked during cycle check. Raise for dense legitimate graphs |
| `SHARED_MAX_VALUE_SIZE` | `1048576` (1 MiB) | Per-value size cap. Inserting a larger value fails fast |
| `SHARED_MAX_CHANNEL_BYTES` | `67108864` (64 MiB) | Per-channel total payload cap |
| `SHARED_POISON_STRICT` | `false` | Boolean. When truthy, a panic inside a Mutex/Once closure poisons the primitive permanently instead of best-effort recovery |
| `SHARED_LOCK_DIAGNOSTICS` | `off` | Lock-contention diagnostics: `off`, `count`, or `trace` |
| `SHARED_LOCK_POLL_INTERVAL_MS` | `100` | Polling interval used by the lock-diagnostics sampler |
| `SHARED_PREVIEW_STRING_LIMIT` | `256` | Per-string truncation in `/entry?id=…` previews |
| `SHARED_PREVIEW_ARRAY_LIMIT` | `20` | Entries sampled in `/entry?id=…` previews |

## Profiling

Sampling profiler that emits xhprof / speedscope traces. See [Profiling](../features/profiling.md) for output formats and viewer integration.

| Variable | Default | Description |
|----------|---------|-------------|
| `PROFILER_ENABLED` | `false` | Boolean — see [Boolean values](#boolean-values). Master switch. All other `PROFILER_*` vars are still parsed at startup so typos surface immediately |
| `PROFILER_SAMPLE_RATE` | `0.0` | Probability (0.0–1.0) that a request is sampled. Values outside the range are clamped |
| `PROFILER_INTERNAL` | `false` | Boolean. When truthy, requests to the internal server (`/health`, `/metrics`, plugin endpoints) are also eligible for sampling |
| `PROFILER_AUTH_TOKEN` | *(unset)* | Optional bearer token. When set, the `oxphp_profiler_*` PHP functions require requests carrying this token to enable on-demand profiling |
| `PROFILER_MAX_SPANS` | `50000` | Per-request cap on profile spans. Profiles exceeding the cap are truncated |
| `PROFILER_MAX_DEPTH` | `256` | Maximum call-stack depth captured per sample. Hard-capped at `65535` |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | Directory for on-disk profile files |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,speedscope` | Comma-separated list of output formats to write to disk |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | Rate limit on profile files written to disk per second |
| `PROFILER_RETENTION_COUNT` | `100` | Maximum profile files kept in `PROFILER_OUTPUT_DIR`. Older files are pruned |
| `PROFILER_EXPORT_URL` | *(unset)* | Remote endpoint to POST profiles to. When set, disk writes still happen unless `PROFILER_OUTPUT_FORMATS` is empty |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | Wire format for `PROFILER_EXPORT_URL` posts |
| `PROFILER_EXPORT_AUTH_TOKEN` | *(unset)* | Optional bearer token sent with each export request |
| `PROFILER_EXPORT_XHGUI` | *(auto-detect)* | Boolean. Forces XHGui-compatible wrapping of the export payload. Unset = auto-detect from `PROFILER_EXPORT_URL` (matches `xhgui` or `/run/import`) |

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
DRAIN_TIMEOUT_SECONDS=30
COMPRESSION_LEVEL=4
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
  "listen_addr": "0.0.0.0:80",
  "document_root": "/var/www/html/public",
  "entry_file": "/var/www/html/public/index.php",
  "log_level": "warn",
  "executor_type": "sapi",
  "php_workers": "8",
  "tokio_workers": 4,
  "queue_capacity": 1024,
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "internal_addr": "127.0.0.1:9090",
  "header_timeout_seconds": 5,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode_enabled": false,
  "worker_max_memory_mib": 0,
  "static_max_age": 2592000,
  "static_revalidate": false,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": true,
  "superglobals_enabled": true,
  "trusted_proxies": false,
  "plugins": {
    "otel": {
      "enabled": true,
      "protocol": "grpc",
      "service_name": "oxphp"
    },
    "apm": {
      "enabled": true,
      "slow_query_ms": 100,
      "db_capture_params": false,
      "hooks_registered": 33
    }
  }
}
```

> **Note:** TLS certificate and key paths are omitted from the output. The `tls_enabled` field indicates whether TLS is active.

## See Also

- [Routing](../features/routing.md) — routing modes and `ENTRY_FILE` behavior
- [Health Checks](health-checks.md) — internal server endpoints
- [Metrics](metrics.md) — Prometheus-compatible metrics reference
- [Graceful Shutdown](graceful-shutdown.md) — how `DRAIN_TIMEOUT_SECONDS` affects shutdown
- [TLS](../features/tls.md) — TLS setup and certificate requirements
- [Rate Limiting](../features/rate-limiting.md) — per-IP rate limiting details
- [Worker Mode](../features/worker-mode.md) — persistent PHP worker architecture
- [Compression](../features/compression.md) — Brotli compression details
- [Static Files](../features/static-files.md) — caching and file serving
- [Distributed Tracing & APM](../features/distributed-tracing.md) — OTel export, auto-instrumentation, and PHP tracing SDK
