---
title: Configuration Reference
description: Complete environment variable reference for OxPHP. Every setting, its default, and what it controls — all in one place.
---

# Configuration Reference

OxPHP is configured entirely through environment variables. There are no configuration files to manage — every setting has a sensible default, so a zero-configuration deployment works out of the box.

## Server

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:80` | Address and port for the main HTTP server |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Root directory for serving files and PHP scripts |
| `INDEX_FILE` | *(unset)* | Routing mode: unset = Traditional, `*.php` = Framework, anything else = SPA. See [Routing](../features/routing.md) |
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
| `WORKER_FILE` | *(unset)* | Path to the worker PHP script. Enables persistent worker mode when set |
| `WORKER_MAX_MEMORY_MIB` | `0` | Maximum memory in MiB per worker before recycling. `0` = unlimited |

When `WORKER_FILE` is set, PHP processes stay alive across requests, keeping bootstrap state (autoloaders, database connections) in memory. Workers are recycled automatically when they exceed `WORKER_MAX_MEMORY_MIB`, or on demand when the application calls [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit). The `WORKER_MAX_REQUESTS` knob from earlier releases is deprecated and ignored — set neither, or migrate to `Worker::scheduleExit()`.

## SAPI / PHP

| Variable | Default | Description |
|----------|---------|-------------|
| `SUPERGLOBALS_ENABLED` | `true` | Populate PHP superglobals (`$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_SERVER`, `php://input`) before script execution. Set to `false` or `0` to skip population — request data is then only available through the object API (`oxphp_http_request()`). Useful for applications that consume the object API directly and want to avoid the cost of building superglobals on every request |

## Timeouts

| Variable | Default | Description |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | Maximum seconds to receive HTTP headers after connection (Slowloris protection) |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Maximum seconds for the entire request-response cycle. `0` = disabled |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Maximum seconds to wait for in-flight connections during graceful shutdown |

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
| `PHP_DENY_DIRS` | *(unset)* | Comma-separated glob patterns whose `.php` files must never execute via direct URI (e.g. `/uploads/**,/cache/**`). Traditional mode only — ignored with a startup warning when `INDEX_FILE` is set. Matching happens before disk I/O, so denied paths produce the same response whether the file exists or not (no existence oracle). See [PHP Execution Deny-List](../security/php-deny-dirs.md) |
| `PHP_DENY_FALLBACK` | `404` | What to return on a `PHP_DENY_DIRS` match. Either an HTTP status `400`–`599` (pairs with `ERROR_PAGES_DIR` for custom HTML) or a `/`-prefixed URI path to a PHP fallback script inside `DOCUMENT_ROOT`. The fallback script receives `OXPHP_DENIED_PATH` and `OXPHP_DENIED_PATTERN` in `$_SERVER`. Validated at startup: the script must exist, canonicalize inside `DOCUMENT_ROOT`, and must not itself match `PHP_DENY_DIRS` (loop prevention) |

The special value `private` expands to all RFC-1918 private networks, loopback, and link-local addresses (IPv4 and IPv6): `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `127.0.0.0/8`, `169.254.0.0/16`, `::1/128`, `fc00::/7`, `fe80::/10`.

## TLS

| Variable | Default | Description |
|----------|---------|-------------|
| `TLS_CERT` | *(unset)* | Path to PEM-encoded TLS certificate. Both `TLS_CERT` and `TLS_KEY` must be set to enable TLS |
| `TLS_KEY` | *(unset)* | Path to PEM-encoded TLS private key |

## Static Files

| Variable | Default | Description |
|----------|---------|-------------|
| `STATIC_CACHE_TTL` | `30d` | Cache TTL for static files. Accepts: `30s`, `5m`, `2h`, `30d`, `1w`, `1y`, bare seconds (`3600`), or `off` to disable |
| `STATIC_CACHE` | *(on)* | Set to `off` to enable mtime revalidation on the in-memory content cache. When off, each cache hit checks the file's modification time and evicts stale entries automatically |
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
| `TRACE_CONTEXT` | `false` | Enable W3C Trace Context propagation (`true` or `1`). Reads `traceparent`/`tracestate` headers and forwards them to PHP via `$_SERVER` |

## OpenTelemetry

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | Enable OpenTelemetry span export. Automatically sets `TRACE_CONTEXT=true` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Export protocol: `grpc` or `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` (gRPC) or `http://localhost:4318` (HTTP) | OTLP collector endpoint |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Export timeout in milliseconds |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(unset)* | Authentication headers: `key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | Service name in exported spans |
| `OTEL_SERVICE_VERSION` | *(unset)* | Service version attribute |
| `OTEL_RESOURCE_ATTRIBUTES` | *(unset)* | Additional resource attributes: `env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Sampling strategy: `always_on`, `always_off`, `traceidratio`, `parentbased_always_on`, `parentbased_always_off`, `parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Sampling ratio (0.0–1.0) for ratio-based samplers |

## APM

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_APM_ENABLED` | `false` | Enable APM: automatic instrumentation, error capture, and the PHP tracing SDK. Requires `OTEL_ENABLED=true` |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | Slow query threshold in milliseconds. Database queries exceeding this get an `oxphp.db.slow=true` span attribute |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | Record bind parameters in the `db.params` span attribute. Disable in production if parameters may contain sensitive data |

When APM is enabled, OxPHP automatically hooks 33 internal PHP functions (PDO, mysqli, cURL, Redis, Memcached, file I/O) to create child spans. The `oxphp_apm_*()` PHP functions are registered regardless of whether APM is enabled — when disabled, they are safe no-ops.

## Async Workers

| Variable | Default | Description |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0` (disabled) | Number of dedicated async worker threads. When `0`, the async functions (`oxphp_async`, etc.) are registered but throw `OxPHP\Async\Exception` on call. Set to a positive value to enable background task execution |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS × 64` | Maximum pending tasks in the async queue. `0` = auto (workers × 64) |

The async worker pool handles fire-and-forget background tasks dispatched from PHP. It is separate from the PHP worker pool and is not required for standard request handling.

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
INDEX_FILE=index.php
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
REQUEST_TIMEOUT_SECONDS=60
DRAIN_TIMEOUT_SECONDS=30
COMPRESSION_LEVEL=4
STATIC_CACHE_TTL=30d
```

### Production (Worker Mode)

```bash
LISTEN_ADDR=0.0.0.0:80
DOCUMENT_ROOT=/var/www/html/public
WORKER_FILE=../worker.php
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
INDEX_FILE=index.php
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
  "index_file": "index.php",
  "log_level": "warn",
  "executor_type": "sapi",
  "php_workers": "8",
  "tokio_workers": 4,
  "queue_capacity": 1024,
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "internal_addr": "127.0.0.1:9090",
  "header_timeout_seconds": 5,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode": false,
  "worker_file": null,
  "worker_max_memory_mib": 0,
  "static_cache_ttl": 2592000,
  "static_cache_enabled": true,
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

- [Routing](../features/routing.md) — routing modes and `INDEX_FILE` behavior
- [Health Checks](health-checks.md) — internal server endpoints
- [Metrics](metrics.md) — Prometheus-compatible metrics reference
- [Graceful Shutdown](graceful-shutdown.md) — how `DRAIN_TIMEOUT_SECONDS` affects shutdown
- [TLS](../features/tls.md) — TLS setup and certificate requirements
- [Rate Limiting](../features/rate-limiting.md) — per-IP rate limiting details
- [Worker Mode](../features/worker-mode.md) — persistent PHP worker architecture
- [Compression](../features/compression.md) — Brotli compression details
- [Static Files](../features/static-files.md) — caching and file serving
- [Distributed Tracing & APM](../features/distributed-tracing.md) — OTel export, auto-instrumentation, and PHP tracing SDK
