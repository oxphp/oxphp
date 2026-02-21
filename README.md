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
- **Access logging** via structured JSON tracing (togglable via `ACCESS_LOG`)
- **Custom error pages** — pre-loaded at startup, zero I/O on hot path
- **JSON structured logging** via tracing
- **Path traversal protection** with symlink escape detection
- **Non-root container** execution as www-data (UID 82)
- **mimalloc** allocator for lower allocation latency under contention
- **Configurable Tokio runtime** — single-threaded (default) or multi-threaded via `TOKIO_WORKERS`
- **Worker health monitoring** with automatic dead worker respawning
- **Panic isolation** via `catch_unwind` — a PHP crash does not take down the server

## Quick Start

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data ./src /var/www/html
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
| `DOCUMENT_ROOT` | `/var/www/html` | Filesystem path to serve files from |
| `INDEX_FILE` | *(unset)* | Routing mode: empty = Traditional, `index.php` = Framework, `index.html` = SPA |
| `TOKIO_WORKERS` | `0` (single-threaded) | Tokio async I/O threads; `0` = single-threaded, `N` = multi-threaded |
| `EXECUTOR` | `sapi` | PHP executor: `sapi` (real PHP) or `stub` (test mode) |
| `PHP_WORKERS` | `0` (CPU * 2) | Worker pool mode: `N` = fixed pool, `MIN:MAX` = dynamic scaling, `0` = auto |
| `PHP_WORKERS_IDLE_SEC` | `30` | Idle timeout before retiring a dynamic worker (dynamic mode only) |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Bounded channel size; 503 when full |
| `DRAIN_TIMEOUT_SECS` | `30` | Graceful shutdown drain timeout in seconds |
| `LOG_LEVEL` | `info` | Tracing verbosity: `error`, `warn`, `info`, `debug`, `trace` |
| `INTERNAL_ADDR` | *(unset)* | Internal server address for health/metrics/config (e.g. `0.0.0.0:9090`) |
| `RATE_LIMIT` | `0` (off) | Max requests per IP per window |
| `RATE_WINDOW` | `60` | Rate limit window in seconds |
| `HEADER_TIMEOUT_SECS` | `5` | Header read timeout (Slowloris protection) |
| `IDLE_TIMEOUT_SECS` | `60` | Keep-alive idle timeout |
| `REQUEST_TIMEOUT_SECS` | `120` | Overall request timeout; 0 = disabled |
| `TLS_CERT` | *(unset)* | Path to TLS certificate PEM file |
| `TLS_KEY` | *(unset)* | Path to TLS private key PEM file |
| `ERROR_PAGES_DIR` | *(unset)* | Directory with custom error pages (`{status}.html`) |
| `COMPRESSION` | `true` | Enable Brotli compression; disable with `false`, `0`, or `off` |
| `ACCESS_LOG` | `true` | Enable per-request JSON access log; disable with `false`, `0`, or `off` |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent connections |

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

- **Configurable Tokio runtime** — single-threaded by default (`TOKIO_WORKERS=0`), multi-threaded for high-throughput workloads
- **Multi-threaded PHP worker pool** using PHP ZTS, each worker is a dedicated OS thread with `catch_unwind` isolation
- Workers receive requests via `crossbeam::bounded`, respond via `ExecuteResult` (immediate or deferred via `oneshot`)
- **Worker health monitoring** — dead workers are automatically detected and respawned

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
DOCUMENT_ROOT=./www ./target/release/oxphp
```

## Development

```bash
# Full verification (host, 157 tests)
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
