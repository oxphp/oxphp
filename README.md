<p align="center">
  <img src="logo.svg" alt="OxPHP" width="300">
</p>

<h3 align="center">Multithreaded PHP application server built for cloud-native infrastructure.</h3>

<p align="center">
  OxPHP is an asynchronous PHP application server written in Rust —<br>
  built for production workloads that demand low latency, high concurrency, and zero-config observability.
</p>

<p align="center">
  <a href="docs/en/">Docs</a> · <a href="#quick-start">Quick Start</a> · <a href="#why-oxphp">Why OxPHP</a> · <a href="#configuration">Configuration</a>
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
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

> **Note:** By default, `DOCUMENT_ROOT` is `/var/www/html/public`. Place your entry point scripts (e.g. `index.php`) inside the `public/` subdirectory — OxPHP will serve files from there, not from the root of `/var/www/html`. This matches the conventional layout of frameworks like Laravel, Symfony, and Slim out of the box.

```bash
docker build -t my-app . && docker run -p 8080:8080 my-app
curl http://localhost:8080/
```

No nginx config. No PHP-FPM pool tuning. No process manager. Just your app.

---

## Why OxPHP?

The traditional PHP stack is three moving parts glued together: a web server, a process manager, and a PHP runtime. Each adds config surface, failure modes, and operational overhead.

OxPHP collapses all three into one Rust binary with PHP baked in.

| | nginx + PHP-FPM | FrankenPHP | RoadRunner | **OxPHP** |
|---|---|---|---|---|
| Language | C / C | Go + C | Go | **Rust** |
| HTTP/2 | ✅ | ✅ | ✅ | ✅ |
| TLS built-in | ✅ | ✅ | ✅ | ✅ (rustls, TLS 1.3) |
| Worker mode | ❌ | ✅ | ✅ | ✅ |
| Backpressure / 503 | manual | ❌ | ❌ | ✅ built-in |
| Prometheus metrics | plugin | plugin | plugin | ✅ built-in |
| Per-IP rate limiting | nginx module | ❌ | ❌ | ✅ built-in |
| Custom error pages | ✅ (nginx config) | ✅ (Caddyfile) | ❌ | ✅ preloaded at startup |
| HTTP/3 | ✅ | ✅ | ✅ experimental | 🔜 roadmap |
| HTTP 103 Early Hints | ✅ (v1.29+) | ✅ | ✅ | 🔜 roadmap |
| Memory safety | ❌ | partial | partial | ✅ Rust |

---

## Benchmarks

> Formal benchmarks are coming soon. We are working on a reproducible test suite covering req/s, latency (p50/p99), memory usage, and worker throughput under concurrent load.
 
---

## Features

### PHP Runtime
- **Native PHP execution** via custom SAPI (`oxphp`) with ZTS worker pool
- **Full superglobals** support: `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `php://input`
- **Native Rust↔PHP bridge** — zero-serialization via direct `zval` access through C accessor functions
- **Plugin system** with typed event dispatch, priority ordering, and PHP function registration
- **Panic isolation** via `catch_unwind` — a PHP crash does not take down the server

### Worker Model
- **Worker mode** — persistent PHP processes with soft reset, keeping autoloaders and DB connections alive across requests
- **Automatic recycling** by request count or memory threshold
- **Worker health monitoring** — dead workers are automatically detected and respawned
- **Early response** via `oxphp_finish_request()` — send the response and keep running background work

### HTTP & Networking
- **HTTP/1.1 + HTTP/2** auto-detection (h2c) via hyper
- **TLS 1.3** with ALPN (h2 + http/1.1) via rustls
- **3 routing modes** — Traditional, Framework (`index.php`), SPA (`index.html`)
- **SSE streaming** via `Content-Type: text/event-stream` auto-detection or `oxphp_stream_flush()`
- **Configurable timeouts** — header read, request, and keep-alive

### Performance
- **LRU file cache** for static files (in-memory ≤1 MB, streaming for larger)
- **HTTP caching** with ETag, Last-Modified, and 304 Not Modified
- **Brotli compression** for text responses (256 B – 3 MB range)
- **mimalloc** allocator for lower allocation latency under contention
- **Configurable Tokio runtime** — multi-threaded by default (CPU/2), tunable via `TOKIO_WORKERS`

### Reliability & Operations
- **Bounded request queue** with 503 backpressure when full
- **Per-IP rate limiting** with `X-RateLimit-*` headers and 429 responses
- **Prometheus metrics** at `/metrics` — per-worker, zero dependencies
- **Health check** at `/health` — ready for K8s readiness probes
- **Structured error logging** — PHP errors routed through `tracing` with `php_error_type`, `php_file`, `php_line`
- **JSON access logging** (levels: `all`, `error`, off via `ACCESS_LOG`)
- **Custom error pages** — pre-loaded at startup, zero I/O on the hot path
- **Path traversal protection** with symlink escape detection
- **Non-root container** execution as www-data (UID 82)
- **Request ID** generation + pass-through (`X-Request-ID`)

---

## Architecture

```
                    ┌──────────────┐
                    │  Tokio async │  configurable: single- or multi-threaded
                    │  HTTP server │  (hyper + hyper-util + mimalloc)
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │Route dispatch│  static file / PHP / 404
                    └──────┬───────┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Static file   PHP request   Not found
         (LRU cache)   (channel)      (404)
                           │
                    ┌──────▼───────┐
                    │Bounded queue │  crossbeam bounded channel
                    │(backpressure)│  503 when full
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         PHP Worker   PHP Worker   PHP Worker    OS threads (ZTS)
         (SAPI exec)  (SAPI exec)  (SAPI exec)   with thread-local state
```

- **Tokio async runtime** — multi-threaded by default, tunable via `TOKIO_WORKERS`
- **ZTS worker pool** — each worker is a dedicated OS thread with `catch_unwind` isolation
- Workers receive requests via `crossbeam::bounded`, respond via `ExecuteResult` (immediate or deferred via `oneshot`)
- **Worker mode** — persistent PHP with soft reset; keeps bootstrap state (autoloaders, DB connections) alive

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
| `LISTEN_ADDR` | `0.0.0.0:8080` | Address and port to bind |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Filesystem path to serve files from |
| `INDEX_FILE` | *(unset)* | Routing mode: empty = Traditional, `index.php` = Framework, `index.html` = SPA |
| `TOKIO_WORKERS` | `0` (CPU / 2, min 1) | Async I/O threads; `0` = auto |
| `EXECUTOR` | `sapi` | PHP executor: `sapi` (real PHP) or `stub` (test mode) |
| `PHP_WORKERS` | `0` (CPU / 2, min 1) | Worker pool: `N` = fixed, `MIN:MAX` = dynamic, `0` = auto |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Idle timeout before retiring a dynamic worker |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Bounded channel size; 503 when full |
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
| `COMPRESSION_LEVEL` | `4` | Brotli quality (0 = off, 1–11) |
| `ACCESS_LOG` | *(off)* | Per-request JSON log: `all`, `error`, or unset |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent connections |
| `WORKER_FILE` | *(unset)* | Path to worker PHP script; enables persistent worker mode |
| `WORKER_MAX_REQUESTS` | `0` (unlimited) | Max requests per worker before recycling |
| `WORKER_MAX_MEMORY_MIB` | `0` (unlimited) | Max memory (MiB) per worker before recycling |

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
# Full verification (host, 167 tests)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Docker smoke tests
docker compose build && docker compose up -d
curl http://localhost:8080/
curl "http://localhost:8080/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost:8080/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost:8080/test_superglobals.php

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
| **PHP 8.5** | Support for PHP 8.5 as soon as it is released |
| **Trace Context (W3C)** | Automatic propagation of `traceparent` / `tracestate` headers across requests |
| **OpenTelemetry** | Export traces and metrics via OTLP to any compatible backend |
| **Custom Metrics** | PHP API for registering application-defined Prometheus metrics from userland code |
| **Built-in PHP Profiler** | Low-overhead profiling without xdebug or external agents, integrated directly into the server |
| **Dockerfile.bookworm** | Official Debian Bookworm-based image as an alternative to Alpine |
| **Non-Docker Install** | Native installation via system package managers (apt, brew, etc.) |
| **HTTP/3** | QUIC-based HTTP/3 support |
| **HTTP 103 Early Hints** | Send `103 Early Hints` responses to allow clients to preload resources before the final response |
| **Ecosystem Plugins** | Expanded plugin system: more lifecycle hooks, richer PHP API, and documentation for third-party plugin authors |
| **Shared Async Runtime** | Expose the Tokio runtime to PHP workers, enabling async-aware operations from userland |
| **Database Connection Pool** | Built-in connection pooling via `sqlx`, reducing per-request connection overhead |
| **gRPC Server** | *(speculative)* An alternative server mode — gRPC instead of HTTP; very uncertain, may not happen |
| **Promise API** | *(speculative)* `OxPHP\Promise` and `AsyncTask` — a PHP-side API for async task execution backed by the Tokio runtime; under consideration |
| **Diagnostics** | Production doctor: checks OS limits (ulimit, TCP backlog, epoll/kqueue, container settings), identifies performance bottlenecks (worker queue depth, lock contention, GC/alloc pressure, ZTS stats), and gives specific actionable recommendations |

## Documentation

- [English](docs/en/)
- [中文](docs/zh/)
- [Русский](docs/ru/)
- [Беларуская](docs/be/)

## License

[AGPL-3.0](LICENSE)