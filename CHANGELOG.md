# Changelog

All notable changes to OxPHP are documented in this file.

## [Unreleased]

### Breaking Changes

- **Async namespace migration**: All async-related PHP classes moved under `OxPHP\Async\` namespace:
  - `OxPHP\AsyncException` → `OxPHP\Async\Exception`
  - `OxPHP\AsyncTimeoutException` → `OxPHP\Async\TimeoutException`
  - `OxPHP\AsyncBorrowException` → `OxPHP\Async\BorrowException`
  - `OxPHP\BorrowedProxy` → `OxPHP\Async\BorrowedProxy`
- Async functions (`oxphp_async`, `oxphp_async_await`, etc.) are now provided by `plugin-async` feature flag. Without it, functions are not available. Function names are unchanged.

### Changed

- `oxphp_request_heartbeat($time)` now also resets PHP's own `max_execution_time` timer to `$time` seconds alongside the server-side deadline. Previously only the server deadline was extended, so long-running scripts could still be killed by Zend's "Maximum execution time exceeded" fatal even after a heartbeat. Scripts that opted out of the PHP timer via `set_time_limit(0)` or `max_execution_time=0` are left alone — the heartbeat does not re-enable a disabled timer.

## [0.2.0] - 2026-03-27

### Added

#### Async & Concurrency

- Fiber-based request multiplexing in worker mode — concurrent I/O within a single worker thread
- Async promises (`oxphp_async()`) — parallel PHP execution via dedicated thread pool
- Distributed tracing with W3C Trace Context propagation and OpenTelemetry export

#### PHP API

- HTTP Object API (`OxPHP\Http\Request`) with lazy bridge accessors
- HTTP interfaces (`OxPHP\Http\RequestInterface`, `OxPHP\Http\SessionInterface`, `OxPHP\Http\AttributesInterface`) with clone/serialize blocking on request-scoped classes
- Attribute-based decorator system (`oxphp_register_decorator()`, `OxPHP\Decorator\AttributeInterface`) with PHP observer integration

#### Server Variables

- `HTTPS` — set to `"on"` when TLS is active
- `REQUEST_SCHEME` — `"https"` or `"http"` per PHP-FPM/nginx convention
- `DOCUMENT_URI` — alias for `SCRIPT_NAME` for nginx/PHP-FPM compatibility
- `REQUEST_TIME_FLOAT` — request start time with microsecond precision

#### HTTP Compliance

- `Date` header on all HTTP responses per RFC 9110 §6.6.1
- `Content-Type` header on all error responses per RFC 9110

#### Observability

- Request duration histograms, byte counters, and subsystem metrics
- `trace_context` field exposed in `/config` endpoint

#### Static Files

- `STATIC_CACHE=off` mode with mtime-based content cache revalidation via `stat()` checks

### Changed

- Default listen port changed from 8080 to 80 (TLS-aware: defaults to 443 when `TLS_CERT` is set)
- Backpressure response changed from 503 to 529 (Site is overloaded)
- `workers_idle` metric now calculated dynamically during scrape (was always 0 in static pool mode)
- `workers_spawned_total` counter now includes initial worker spawn
- `/config` endpoint now exposes `log_level` and other missing runtime settings

### Fixed

- `SERVER_PROTOCOL` now reflects actual HTTP version (was hardcoded to `HTTP/1.1`) per RFC 3875
- `REQUEST_TIME` now returns request start time (was returning current time)
- IPv6 Host header parsing for `SERVER_NAME` and `SERVER_PORT`
- Request timeout now returns 408 instead of 504 per RFC 9110
- Duplicate `oxphp_response_time_us_total` Prometheus metric removed
- Session state cleanup added to worker soft reset to prevent state leaks between requests
- Missing fiber source files added to alpine-release Dockerfile

## [0.1.0] - 2026-03-08

First public release. OxPHP replaces nginx + PHP-FPM with a single async binary
written in Rust, providing HTTP serving, native PHP execution via custom SAPI,
and built-in observability.

### Core

- Async HTTP/1.1 server built on Hyper + Tokio with graceful shutdown
- Custom PHP SAPI (`oxphp`) with full superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`)
- C bridge library (`liboxphp_bridge.so`) for zero-copy Rust↔PHP communication via direct zval access
- PHP ZTS (Zend Thread Safety) multi-threaded worker pool with bounded queue and 529 backpressure
- Three routing modes: Traditional (direct file mapping), Framework (front controller), SPA (fallback to `index.html`)
- Static file serving with in-memory cache, MIME detection, and HTTP caching (ETag, Last-Modified, 304 responses)
- Brotli compression with configurable quality level (0–11) and minimum size threshold
- TLS support via PEM certificate/key
- PHP 8.4 compatibility

### Worker Mode

- Persistent PHP worker processes (`oxphp_worker()`) that handle multiple requests with soft reset between them
- Early response completion (`oxphp_finish_request()`) for background processing after response is sent
- SSE streaming with real-time chunked delivery (`oxphp_is_streaming()`, `oxphp_stream_flush()`)
- Cooperative timeout and cancellation via `oxphp_request_heartbeat()`
- Worker mode detection (`oxphp_is_worker()`) and introspection (`oxphp_worker_id()`, `oxphp_server_info()`)
- Resilient to `exit`/`die` in PHP 8.4 worker mode

### Worker Pool

- Static pool mode: fixed number of workers (`PHP_WORKERS=N`)
- Dynamic pool mode: auto-scaling between min and max workers (`PHP_WORKERS=MIN:MAX`)
- Auto-detect mode: defaults to CPU/2 workers (`PHP_WORKERS=0`)
- Per-worker memory limits (`WORKER_MAX_MEMORY_MIB`) and request limits (`WORKER_MAX_REQUESTS`)
- Dead worker respawning via health monitor (static) / scale manager (dynamic)
- `catch_unwind` prevents panics from poisoning the channel

### Observability

- Prometheus metrics endpoint (`/metrics`) with request counts, durations, status codes, active connections, queue depth, and worker mode stats
- Health check endpoint (`/health`)
- Runtime configuration endpoint (`/config`)
- Structured JSON access logging with configurable levels (`ACCESS_LOG`: off/error/all)
- Request ID generation (`oxphp_request_id()`) in `{timestamp:08x}{counter:08x}` format
- Structured PHP error logging via `zend_error_cb`

### Security & Limits

- Per-IP rate limiting (`RATE_LIMIT`, `RATE_WINDOW_SECONDS`)
- Header read timeout (`HEADER_TIMEOUT_SECONDS`) and request timeout (`REQUEST_TIMEOUT_SECONDS`)
- Graceful shutdown with drain timeout (`DRAIN_TIMEOUT_SECONDS`)
- Configurable request body limits
- Path traversal protection with canonicalization

### Plugin System

- Plugin trait with lifecycle hooks (init, startup, shutdown)
- Typed event dispatcher with priority ordering at every lifecycle point
- Events: ConnectionAccepted, RequestReceived, RouteResolved, ScriptExecutionComplete, ResponseBuilding, RequestComplete
- Native PHP function registration from plugins (zero-copy zval access, no JSON serialization)
- Plugin context API for handler registration and configuration

### Performance

- mimalloc global allocator for reduced per-alloc latency
- Configurable multi-threaded Tokio runtime (`TOKIO_WORKERS`)
- Route LRU cache for fast path resolution
- Thread-local buffer reuse for server variables
- Single Arc clone per request (reduced from 10)
- OPcache support with correct `sapi_get_request_time()` initialization

### Infrastructure

- Multi-stage Alpine Docker build (`php:8.4-zts-alpine`)
- Multi-platform Docker images (amd64/arm64) published to GHCR
- CI workflows: nightly build, PR checks (fmt, clippy, tests), release tagging
- Best-practice Dockerfile example with separate dev/prod targets
- HTTP QUERY method support (RFC 9110)
- Documentation in English, Russian, Belarusian, and Chinese

### PHP Functions

| Function | Description |
|---|---|
| `oxphp_request_id()` | Current request identifier |
| `oxphp_worker_id()` | Current worker thread ID |
| `oxphp_server_info()` | Server runtime information |
| `oxphp_request_heartbeat()` | Signal liveness for cooperative timeout |
| `oxphp_finish_request()` | Flush response early, continue background work |
| `oxphp_is_worker()` | Whether running in worker mode |
| `oxphp_is_streaming()` | Whether SSE streaming is active |
| `oxphp_stream_flush()` | Flush SSE chunk to client |
| `oxphp_worker(callable)` | Enter persistent worker loop |

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Server listen address |
| `DOCUMENT_ROOT` | `www/public` | Document root path |
| `INDEX_FILE` | `index.php` | Front controller file |
| `PHP_WORKERS` | CPU/2 | Worker pool size (`N`, `MIN:MAX`, or `0`) |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Dynamic pool idle timeout |
| `TOKIO_WORKERS` | CPU/2 | Tokio runtime threads (0 = auto) |
| `QUEUE_CAPACITY` | workers×128 | Bounded queue size |
| `RATE_LIMIT` | `0` (off) | Max requests per window per IP |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window |
| `HEADER_TIMEOUT_SECONDS` | `10` | Header read timeout |
| `REQUEST_TIMEOUT_SECONDS` | `30` | Request execution timeout |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Graceful shutdown drain |
| `COMPRESSION_LEVEL` | `4` | Brotli quality 0–11 (0 = off) |
| `STATIC_CACHE_TTL` | `3600` | Static file Cache-Control max-age |
| `ACCESS_LOG` | `all` | Log level: off/error/all |
| `ERROR_PAGES_DIR` | — | Directory with `{status}.html` pages |
| `TLS_CERT` / `TLS_KEY` | — | TLS certificate/key PEM paths |
| `INTERNAL_ADDR` | `0.0.0.0:9090` | Health/metrics/config endpoint |
| `WORKER_MAX_REQUESTS` | `0` (unlimited) | Max requests per worker before restart |
| `WORKER_MAX_MEMORY_MIB` | `0` (unlimited) | Max worker memory before restart |
| `EXECUTOR` | `sapi` | Executor type: sapi/stub |

[0.2.0]: https://github.com/oxphp/oxphp/releases/tag/v0.2.0
[0.1.0]: https://github.com/oxphp/oxphp/releases/tag/v0.1.0
