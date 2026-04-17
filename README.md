<p align="center">
  <img src="logo.svg" alt="OxPHP" width="300">
</p>

<h3 align="center">Multithreaded PHP application server built for cloud-native infrastructure.</h3>

<p align="center">
  OxPHP is an asynchronous PHP application server written in Rust —<br>
  built for production workloads that demand low latency, high concurrency, and zero-config observability.
</p>

<p align="center">
  <b>English</b> · <a href="README.ru.md">Русский</a> · <a href="README.zh.md">中文</a>
</p>

<p align="center">
  Documents: <a href="docs/en/">English</a> · <a href="docs/ru/">Русский</a> · <a href="docs/zh/">中文</a>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> · <a href="#why-oxphp">Why OxPHP</a> · <a href="#features">Features</a> · <a href="#configuration">Configuration</a> · <a href="#roadmap">Roadmap</a>
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/rust-powered-orange">
  <img alt="PHP" src="https://img.shields.io/badge/php-8.4-blue">
  <img alt="License" src="https://img.shields.io/github/license/oxphp/oxphp">
  <img alt="Release" src="https://img.shields.io/github/v/release/oxphp/oxphp">
  <img alt="Stars" src="https://img.shields.io/github/stars/oxphp/oxphp?style=flat">
  <img alt="Docker" src="https://img.shields.io/badge/docker-ghcr.io-2496ED?logo=docker&logoColor=white">
  <img alt="HTTP/2" src="https://img.shields.io/badge/HTTP%2F2-supported-brightgreen">
  <img alt="TLS" src="https://img.shields.io/badge/TLS-1.3-brightgreen">
</p>

---

## Quick Start

Two lines. That's it.

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.2.0

COPY --chown=www-data:www-data . /var/www/html
```

> **Note:** By default, `DOCUMENT_ROOT` is `/var/www/html/public`. Place your entry point scripts (e.g. `index.php`) inside the `public/` subdirectory. OxPHP serves files from there, not from the root of `/var/www/html`. This matches the standard layout of Laravel, Symfony, and Slim.

```bash
docker build -t my-app . && docker run -p 80:80 my-app
curl http://localhost/
```

No nginx config. No PHP-FPM pool tuning. No process manager. Just your app.

---

## Why OxPHP?

OxPHP replaces nginx + PHP-FPM with a single container. The server works out of the box — TLS, Brotli compression, rate limiting, Prometheus metrics, health checks, and structured JSON logs are configured via environment variables.

| | nginx + PHP-FPM | FrankenPHP | RoadRunner | **OxPHP** |
|---|---|---|---|---|
| Language | C / C | Go + C | Go | **Rust** |
| HTTP/2 | ✅ | ✅ | ✅ | ✅ |
| TLS built-in | ✅ | ✅ | ✅ | ✅ (rustls, TLS 1.3) |
| Worker mode | ❌ | ✅ | ✅ | ✅ |
| Backpressure / 529 | manual | ❌ | ❌ | ✅ built-in |
| Prometheus metrics | plugin | plugin | plugin | ✅ built-in |
| Per-IP rate limiting | nginx module | ❌ | ❌ | ✅ built-in |
| Custom error pages | ✅ (nginx config) | ✅ (Caddyfile) | ❌ | ✅ preloaded at startup |
| HTTP/3 | ✅ | ✅ | ✅ experimental | 🔜 roadmap |
| HTTP 103 Early Hints | ✅ (v1.29+) | ✅ | ✅ | 🔜 roadmap |
| Memory safety | ❌ | partial | partial | ✅ Rust |

See the full [documentation](docs/en/index.md) for details.

---

## Benchmarks

> Formal benchmarks are coming soon. We are working on a reproducible test suite covering req/s, latency (p50/p99), memory usage, and worker throughput under concurrent load.
 
---

## Features

### PHP Runtime
- **Native PHP execution** — PHP runs directly inside the server process, in a dedicated thread pool
- **Full superglobals** support: `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `php://input`
- **HTTP Object API** — `oxphp_http_request()` returns a typed, lazy-loading request object with built-in JSON body parsing, content-detected MIME types for uploads, and a mutable attributes container for middleware; see [HTTP Request API docs](docs/en/php/request-api.md)
- **Plugin system** with typed event dispatch, priority ordering, and PHP function registration
- **Attribute-based decorators** — intercept function/method calls via PHP 8+ attributes with zero overhead on undecorated code; supports `TARGET_FUNCTION`, `TARGET_METHOD`, `TARGET_CLASS`
- **Crash isolation** — a fatal error in one request does not take down the server

### Worker Model
- **Worker mode** — persistent PHP processes that stay alive across requests; autoloaders, service containers, and DB connections are initialized once and reused
- **Fiber multiplexing** — each worker handles multiple concurrent requests via PHP 8.4 Fibers; `oxphp_sleep()` and `oxphp_async_await()` yield the current fiber instead of blocking the worker thread
- **Automatic recycling** by request count or memory threshold
- **Worker health monitoring** — crashed workers are automatically detected and restarted
- **Early response** via `oxphp_finish_request()` — send the response and keep running background work

### Async Promises
- **`oxphp_async()` / `oxphp_async_await()`** — dispatch closures to a dedicated thread pool for true parallel execution
- **Portable serialization** for `use` variables, arguments, and return values — safe cross-thread binary transfer
- Supported types: scalars, strings, arrays (nested). Resources and objects rejected with `E_WARNING`
- **Exception & die() safety** — exceptions, `die()`, and `exit()` are caught and re-thrown as `OxPHP\Async\Exception`
- **Timeout support** — per-task timeouts with `OxPHP\Async\TimeoutException`
- **`oxphp_async_await_all()` / `oxphp_async_await_any()`** — batch and race primitives

### HTTP & Networking
- **HTTP/1.1 + HTTP/2** with automatic protocol detection (h2c)
- **TLS 1.3** with ALPN — both HTTP/2 and HTTP/1.1 over TLS
- **3 routing modes** — Traditional (file mapping + always-on PATH_INFO), Framework (`index.php` rewrite with `PATH_INFO=$request_uri`), SPA (`index.html` for no-extension paths, hard 404 for missing assets). Each mode mirrors a familiar nginx `try_files` configuration
- **SSE streaming** via `Content-Type: text/event-stream` auto-detection or `oxphp_stream_flush()` — cooperative with fiber multiplexing
- **Configurable timeouts** — header read, request, and keep-alive

### Performance
- **LRU file cache** for static files (in-memory ≤1 MB, streaming for larger)
- **HTTP caching** with ETag, Last-Modified, and 304 Not Modified
- **Brotli compression** for text responses (256 B – 3 MB range)
- **mimalloc** allocator for lower allocation latency under contention
- **Configurable HTTP server threads** — multi-threaded by default (CPU/2), tunable via `TOKIO_WORKERS`

### Observability
- **W3C Trace Context** — automatic `traceparent`/`tracestate` propagation, `$_SERVER['OXPHP_TRACE_ID']` for PHP log correlation
- **OpenTelemetry** — OTLP span export (gRPC/HTTP) with semantic conventions, configurable sampling, batch processing
- **APM auto-instrumentation** — 33 internal PHP functions (PDO, mysqli, cURL, Redis, Memcached, file I/O) hooked at the engine level; every call becomes a span with zero code changes
- **`#[OxPHP\Tracing\Trace]` decorator** — annotate any function or method with a PHP 8 attribute to create spans automatically
- **PHP tracing SDK** — 10 `oxphp_trace_*()` functions for manual span creation, attributes, events, error recording, and trace context propagation
- **Prometheus metrics** at `/metrics` — per-worker, zero dependencies
- **Health check** at `/health` — ready for K8s readiness probes
- **Structured error logging** — PHP errors appear in the server log with `php_error_type`, `php_file`, `php_line` fields
- **JSON access logging** with optional `trace_id`/`span_id` fields (levels: `all`, `error`, off via `ACCESS_LOG`)
- **Request ID** generation + pass-through (`X-Request-ID`); trace-derived when OTel enabled

### Reliability & Operations
- **Bounded request queue** with 529 backpressure when full
- **Per-IP rate limiting** with `X-RateLimit-*` headers and 429 responses
- **Custom error pages** — pre-loaded at startup, zero I/O on the hot path
- **Path traversal protection** with symlink escape detection
- **Trusted proxy support** — real client IP extraction from `Forwarded` (RFC 7239) and `X-Forwarded-*` headers with CIDR-based trust
- **Dot-path blocking** — returns 404 for hidden files (`.env`, `.git/`) with `.well-known` exception (RFC 8615)
- **Non-root container** execution as www-data (UID 82)

---

## Architecture

```mermaid
flowchart TD
    Client([Client])
    HTTP["Async HTTP server<br/>single- or multi-threaded"]
    Route{Route dispatch}
    Static["Static file<br/>LRU cache"]
    Queue[("Bounded queue<br/>529 when full")]
    NF["404 Not Found"]
    Pool["Async pool<br/>oxphp_async / oxphp_async_await"]

    Client --> HTTP
    HTTP --> Route
    Route -->|static| Static
    Route -->|miss| NF
    Route -->|PHP| Queue
    Queue --> PhpWorkers
    PhpWorkers -.-> Pool
    Pool --> AsyncWorkers

    subgraph PhpWorkers [PHP workers — dedicated OS threads]
        direction BT
        W1[Worker]
        W2[Worker]
        W3[Worker]
    end

    subgraph AsyncWorkers [Async workers — dedicated OS threads]
        direction BT
        A1[Worker]
        A2[Worker]
        A3[Worker]
    end
```

- **Async HTTP server** — multi-threaded by default, tunable via `TOKIO_WORKERS`
- **PHP worker pool** — each worker is a dedicated OS thread; a crash in one worker does not affect the others
- Requests wait in a bounded queue between the HTTP server and the PHP workers; the queue returns 529 when full
- **Async pool** — separate threads for `oxphp_async()` tasks, preventing slowdowns in the main worker pool
- **Worker mode** — persistent PHP processes that stay alive between requests; autoloaders and DB connections are shared across all requests handled by that worker

### Internal Server

When `INTERNAL_ADDR` is set, a lightweight HTTP server starts on a separate port:

| Endpoint | Description |
|----------|-------------|
| `GET /health` | JSON health status (uptime, requests, connections) |
| `GET /metrics` | Prometheus text format metrics |
| `GET /config` | JSON runtime configuration (TLS paths redacted) |

---

## Configuration

All settings are via environment variables — no config files required.

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:80` | Address and port to bind |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Filesystem path to serve files from |
| `INDEX_FILE` | *(unset)* | Routing mode: empty = Traditional, `*.php` = Framework, anything else = SPA |
| `TOKIO_WORKERS` | `0` (CPU / 2, min 1) | HTTP server threads for handling connections; `0` = auto |
| `EXECUTOR` | `sapi` | PHP executor: `sapi` (real PHP) or `stub` (test mode) |
| `PHP_WORKERS` | `0` (CPU / 2, min 1) | Worker pool: `N` = fixed, `MIN:MAX` = dynamic, `0` = auto |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Idle timeout before retiring a dynamic worker |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Max pending requests in the queue before the server returns 529 |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Graceful shutdown drain timeout |
| `LOG_LEVEL` | `info` | Tracing verbosity: `error`, `warn`, `info`, `debug`, `trace` |
| `INTERNAL_ADDR` | *(unset)* | Internal server for health/metrics/config (e.g. `0.0.0.0:9090`) |
| `RATE_LIMIT` | `0` (off) | Max requests per IP per window |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window in seconds |
| `HEADER_TIMEOUT_SECONDS` | `5` | Header read timeout (Slowloris protection) |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Overall request timeout; 0 = disabled |
| `TLS_CERT` | *(unset)* | Path to TLS certificate PEM file |
| `TLS_KEY` | *(unset)* | Path to TLS private key PEM file |
| `ERROR_PAGES_DIR` | *(unset)* | Directory with custom error pages (`{status}.html`) |
| `STATIC_CACHE_TTL` | `30d` | Static file cache TTL (`30s`, `5m`, `2h`, `30d`, `1y`, `off`) |
| `STATIC_CACHE` | *(on)* | Set to `off` to enable mtime revalidation on the in-memory content cache |
| `COMPRESSION_LEVEL` | `4` | Brotli quality (0 = off, 1–11) |
| `ACCESS_LOG` | *(off)* | Per-request JSON log: `all`, `error`, or unset |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent connections |
| `WORKER_FILE` | *(unset)* | Path to worker PHP script; enables persistent worker mode |
| `WORKER_MAX_REQUESTS` | `0` (unlimited) | Max requests per worker before recycling |
| `WORKER_MAX_MEMORY_MIB` | `0` (unlimited) | Max memory (MiB) per worker before recycling |
| `SUPERGLOBALS_ENABLED` | `true` | Populate `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_SERVER`; set `false` to rely solely on `oxphp_http_request()` |
| `ASYNC_WORKERS` | `0` (disabled) | Dedicated async worker threads for `oxphp_async()` |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS * 64` | Max pending async tasks in the queue; tasks are rejected when full |
| `TRACE_CONTEXT` | `false` | W3C Trace Context propagation (`traceparent`/`tracestate`). Auto-enabled when `OTEL_ENABLED=true` |
| `TRUSTED_PROXIES` | *(unset)* | Trusted proxy CIDRs: `10.0.0.0/8,172.16.0.0/12` or `private` (all RFC-1918). Enables real client IP extraction from `Forwarded`/`X-Forwarded-*` headers |

### OpenTelemetry (`plugin-otel` feature)

| Variable | Default | Description |
|---|---|---|
| `OTEL_ENABLED` | `false` | Enable span export. Implies `TRACE_CONTEXT=true` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP collector endpoint |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Export protocol: `grpc` (port 4317) or `http/protobuf` (port 4318) |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Export timeout in milliseconds |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(unset)* | Auth headers for hosted backends (`key=value,key=value`) |
| `OTEL_SERVICE_NAME` | `oxphp` | Service name in exported traces |
| `OTEL_SERVICE_VERSION` | *(unset)* | Service version in exported traces |
| `OTEL_RESOURCE_ATTRIBUTES` | *(unset)* | Resource attributes (`key=value,key=value`) |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Sampler: `always_on`, `always_off`, `traceidratio`, `parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Sampling ratio (0.0–1.0) |

### APM (`plugin-apm` feature)

| Variable | Default | Description |
|---|---|---|
| `OTEL_APM_ENABLED` | `false` | Enable APM: auto-instrumentation, error capture, PHP tracing SDK. Requires `OTEL_ENABLED=true` |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | Slow query threshold (ms). Queries above this get `oxphp.db.slow=true` |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | Record bind parameters in `db.params` span attribute |

---

## Build

```bash
# Host (without PHP — all tests pass, no PHP execution)
cargo build --release

# Docker (with PHP — full functionality)
docker compose build
```

### Run locally (static files only)

```bash
DOCUMENT_ROOT=./www/public ./target/release/oxphp
```

## Development

```bash
# Full verification (host)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Docker smoke tests
docker compose build && docker compose up -d
curl http://localhost/
curl "http://localhost/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost/test_superglobals.php

# Async promises
curl http://localhost/test_async.php
curl http://localhost/test_async_parallel.php
curl http://localhost/test_async_die.php

# Internal server
INTERNAL_ADDR=127.0.0.1:9090 ./target/release/oxphp &
curl http://localhost:9090/health
curl http://localhost:9090/metrics
```

---

## Roadmap

> Items are not ordered by priority. Presence on this list does not guarantee implementation.

| Feature | Description |
|---|---|
| **PHP 8.5** | Support for PHP 8.5 |
| ~~**Trace Context (W3C)**~~ | ✅ Implemented — automatic propagation of `traceparent` / `tracestate` headers (W3C spec), enabled via `TRACE_CONTEXT=true` |
| ~~**OpenTelemetry**~~  | ✅ Implemented — OTLP trace export via `plugin-otel` feature, W3C context propagation, per-request spans with standard semantic conventions |
| ~~**APM & Auto-Instrumentation**~~ | ✅ Implemented — `plugin-apm` feature: automatic tracing of 33 internal PHP functions (PDO, mysqli, cURL, Redis, Memcached, file I/O), `#[OxPHP\Tracing\Trace]` decorator, 10 `oxphp_trace_*()` SDK functions, PHP error capture |
| **Custom Metrics** | PHP API for registering application-defined Prometheus metrics from userland code |
| **Built-in PHP Profiler** | Low-overhead profiling via attribute decorators (`#[Timer]`, `#[Span]`), integrated with server metrics and tracing |
| **Dockerfile.bookworm** | Official Debian Bookworm-based image as an alternative to Alpine |
| **Non-Docker Install** | Native installation via system package managers (apt, brew, etc.) |
| **HTTP/3** | QUIC-based HTTP/3 support |
| **HTTP 103 Early Hints** | Send `103 Early Hints` responses to allow clients to preload resources before the final response |
| **Ecosystem Plugins** | Expanded plugin system: more lifecycle hooks, richer PHP API, and documentation for third-party plugin authors |
| ~~**Shared Async Runtime**~~ | ✅ Implemented — the same async runtime powers both the HTTP server and `oxphp_async()` / `oxphp_async_await()` with timeouts, result delivery, and race coordination |
| **Database Connection Pool** | Built-in connection pooling via `sqlx`, reducing per-request connection overhead |
| **gRPC Server** | *(speculative)* An alternative server mode — gRPC instead of HTTP; very uncertain, may not happen |
| ~~**Promise API**~~ | ✅ Implemented — `oxphp_async()` / `oxphp_async_await()` with dedicated thread pool, portable serialization, and exception safety |
| ~~**Fiber Multiplexing**~~ | ✅ Implemented — each worker handles multiple concurrent requests via PHP 8.4 Fibers; `oxphp_sleep()` / `oxphp_usleep()` and `oxphp_async_await()` yield the fiber cooperatively |
| **Diagnostics** | Production doctor: checks OS limits (ulimit, TCP backlog, epoll/kqueue, container settings), identifies performance bottlenecks (worker queue depth, lock contention, GC/alloc pressure, ZTS stats), and gives specific actionable recommendations |

## Documentation

- [English](docs/en/)
- [Русский](docs/ru/)
- [中文](docs/zh/)

## License

[AGPL-3.0](LICENSE)