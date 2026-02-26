# OxPHP

Asynchronous PHP application server written in Rust. Replaces nginx + PHP-FPM with a single binary that handles HTTP, executes PHP natively via a custom SAPI, and provides built-in observability.

## Features

- **Native PHP execution** via custom SAPI (`oxphp`) with ZTS worker pool
- **Full superglobals** support: `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `php://input`
- **Native Rust↔PHP bridge** — zero-serialization via direct `zval` access through C accessor functions
- **Plugin system** with typed event dispatch, priority ordering, and PHP function registration
- **Structured error logging** — PHP errors routed through `tracing` with `php_error_type`, `php_file`, `php_line` fields
- **HTTP/1.1 + HTTP/2** auto-detection (h2c) via hyper
- **TLS 1.3** with ALPN (h2 + http/1.1) via rustls
- **3 routing modes** — Traditional, Framework (`index.php`), SPA (`index.html`)
- **LRU file cache** for static files (in-memory for files ≤1 MB, streaming for larger)
- **Brotli compression** for text responses (256 B – 3 MB range)
- **Bounded request queue** with 503 backpressure when full
- **Per-IP rate limiting** with `X-RateLimit-*` headers and 429 responses
- **Configurable timeouts** — header read, request, and keep-alive
- **Prometheus metrics** at `/metrics` on internal server
- **Health check** endpoint at `/health` for K8s readiness probes
- **Request ID** generation + pass-through (`X-Request-ID` header)
- **Access logging** via structured JSON tracing (levels: `all`, `error`, off via `ACCESS_LOG`)
- **Custom error pages** — pre-loaded at startup, zero I/O on hot path
- **JSON structured logging** via tracing
- **Path traversal protection** with symlink escape detection
- **Non-root container** execution as www-data (UID 82)
- **mimalloc** allocator for lower allocation latency under contention
- **Configurable Tokio runtime** — multi-threaded by default (CPU/2), tunable via `TOKIO_WORKERS`
- **Worker health monitoring** with automatic dead worker respawning
- **SSE streaming** — real-time Server-Sent Events via `Content-Type: text/event-stream` auto-detection or `oxphp_stream_flush()`
- **Early response** via `oxphp_finish_request()` — send the response immediately and continue background processing
- **Worker mode** — persistent PHP processes with soft reset, automatic recycling by request count or memory, and per-worker Prometheus metrics
- **Panic isolation** via `catch_unwind` — a PHP crash does not take down the server

## Quick Start

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data . /var/www/html
```

```bash
docker build -t my-app . && docker run -p 8080:8080 my-app
curl http://localhost:8080/
```

## Configuration

All settings are via environment variables:

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Address and port to bind |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Filesystem path to serve files from |
| `INDEX_FILE` | *(unset)* | Routing mode: empty = Traditional, `index.php` = Framework, `index.html` = SPA |
| `TOKIO_WORKERS` | `0` (CPU / 2, min 1) | Tokio async I/O threads; `0` = auto, `1` = single-threaded, `N` = multi-threaded with N threads |
| `EXECUTOR` | `sapi` | PHP executor: `sapi` (real PHP) or `stub` (test mode) |
| `PHP_WORKERS` | `0` (CPU / 2, min 1) | Worker pool mode: `N` = fixed pool, `MIN:MAX` = dynamic scaling, `0` = auto |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Idle timeout before retiring a dynamic worker (dynamic mode only) |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Bounded channel size; 503 when full |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Graceful shutdown drain timeout in seconds |
| `LOG_LEVEL` | `info` | Tracing verbosity: `error`, `warn`, `info`, `debug`, `trace` |
| `INTERNAL_ADDR` | *(unset)* | Internal server address for health/metrics/config (e.g. `0.0.0.0:9090`) |
| `RATE_LIMIT` | `0` (off) | Max requests per IP per window |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window in seconds |
| `HEADER_TIMEOUT_SECONDS` | `5` | Header read timeout (Slowloris protection) |
| `IDLE_TIMEOUT_SECONDS` | `60` | Keep-alive idle timeout |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Overall request timeout; 0 = disabled |
| `TLS_CERT` | *(unset)* | Path to TLS certificate PEM file |
| `TLS_KEY` | *(unset)* | Path to TLS private key PEM file |
| `ERROR_PAGES_DIR` | *(unset)* | Directory with custom error pages (`{status}.html`) |
| `COMPRESSION_ENABLED` | `true` | Enable Brotli compression; disable with `false`, `0`, or `off` |
| `ACCESS_LOG` | *(off)* | Per-request JSON access log: `all` (every request), `error` (4xx/5xx only), empty/unset = off |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent connections |
| `WORKER_FILE` | *(unset)* | Path to worker PHP script (relative to `DOCUMENT_ROOT`); enables persistent worker mode |
| `WORKER_MAX_REQUESTS` | `0` (unlimited) | Max requests per worker before recycling; `0` = no limit |
| `WORKER_MAX_MEMORY_MIB` | `0` (unlimited) | Max memory (MiB) per worker before recycling; `0` = no limit |

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

- **Configurable Tokio runtime** — multi-threaded by default (CPU/2, min 1), tunable via `TOKIO_WORKERS`
- **Multi-threaded PHP worker pool** using PHP ZTS, each worker is a dedicated OS thread with `catch_unwind` isolation
- Workers receive requests via `crossbeam::bounded`, respond via `ExecuteResult` (immediate or deferred via `oneshot`)
- **Worker health monitoring** — dead workers are automatically detected and respawned
- **Worker mode** — persistent PHP with soft reset between requests; workers call `oxphp_worker($handler)` in a loop, keeping bootstrap state (autoloaders, DB connections) alive across requests

### Internal Server

When `INTERNAL_ADDR` is set, a lightweight HTTP server starts on a separate port:

| Endpoint | Description |
|----------|-------------|
| `GET /health` | JSON health status (uptime, requests, connections) |
| `GET /metrics` | Prometheus text format metrics |
| `GET /config` | JSON runtime configuration (TLS paths redacted) |

## Build

```bash
# Host (without PHP — runs all tests, no PHP execution)
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

# Docker smoke test
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

## Documentation

- [English](docs/en/)
- [中文](docs/zh/)
- [Русский](docs/ru/)
- [Беларуская](docs/be/)

## License

[AGPL-3.0](LICENSE)
