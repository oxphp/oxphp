# OxPHP

Asynchronous PHP application server written in Rust. Replaces nginx + PHP-FPM with a single binary that handles HTTP, executes PHP natively via a custom SAPI, and provides built-in observability.

## Features

- **Native PHP execution** via custom SAPI (`oxphp`) with ZTS worker pool
- **Full superglobals** support: `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `php://input`
- **Structured error logging** — PHP errors routed through `tracing` with `php_error_type`, `php_file`, `php_line` fields
- **HTTP/1.1 + HTTP/2** auto-detection (h2c) via hyper
- **3 routing modes** — Traditional, Framework (`index.php`), SPA (`index.html`)
- **LRU file cache** for static files (in-memory for files ≤1MB, streaming for larger)
- **Bounded request queue** with 503 backpressure when full
- **JSON structured logging** via tracing
- **Path traversal protection** with symlink escape detection
- **Non-root container** execution as www-data (UID 82)

## Quick Start

```bash
docker compose build && docker compose up -d
curl http://localhost:8080/
```

## Configuration

All settings are via environment variables:

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Address and port to bind |
| `DOCUMENT_ROOT` | `/var/www/html` | Filesystem path to serve files from |
| `INDEX_FILE` | *(unset)* | Routing mode: empty = Traditional, `index.php` = Framework, `index.html` = SPA |
| `EXECUTOR` | `sapi` | PHP executor: `sapi` (real PHP) or `stub` (test mode) |
| `PHP_WORKERS` | CPU count | Number of PHP worker threads |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Bounded channel size; 503 when full |
| `LOG_LEVEL` | `info` | Tracing verbosity: `error`, `warn`, `info`, `debug`, `trace` |

## Architecture

```
                    ┌──────────────┐
                    │  Tokio async │  single-threaded event loop
                    │  HTTP server │  (hyper + hyper-util)
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

- **Single-threaded Tokio runtime** for all async I/O (TCP, HTTP)
- **Multi-threaded PHP worker pool** using PHP ZTS, each worker is a dedicated OS thread
- Workers receive requests via `crossbeam::bounded`, respond via `oneshot::Sender`

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
# Full verification (host, 47 tests)
cargo fmt -- --check && cargo clippy -- -D warnings && cargo test

# Docker smoke test
docker compose build && docker compose up -d
curl http://localhost:8080/
curl "http://localhost:8080/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost:8080/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost:8080/test_superglobals.php
```

## License

[AGPL-3.0](LICENSE)
