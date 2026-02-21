---
title: OxPHP
description: Async PHP application server written in Rust
---

OxPHP is an asynchronous PHP application server written in Rust. It replaces nginx + PHP-FPM with a single binary that handles HTTP serving, PHP execution, and built-in observability in one process.

## Why OxPHP

Traditional PHP deployments require a web server (nginx or Apache), a process manager (PHP-FPM), and separate tooling for metrics, rate limiting, and TLS termination. OxPHP collapses this stack into a single binary with no external dependencies at runtime beyond the PHP runtime library.

The server uses a configurable Tokio async runtime (single-threaded by default, multi-threaded via `TOKIO_WORKERS`) for all I/O and a pool of dedicated OS threads for PHP execution via Zend Thread Safety (ZTS). This architecture keeps the async event loop free of blocking PHP calls while scaling PHP execution across all available cores. The mimalloc allocator provides lower allocation latency under contention.

## Features

- **Static file serving** with in-memory file cache and automatic MIME type detection
- **Three routing modes**: Traditional (direct file mapping), Framework (front controller), and SPA (single-page application fallback)
- **PHP execution** via a custom SAPI (`oxphp`) with full superglobal support (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`)
- **Dynamic worker scaling** -- auto-scales PHP worker threads between configurable min/max bounds based on load
- **Bounded request queue** with backpressure -- returns 503 when the queue is full instead of accepting unbounded work
- **Plugin system** with lifecycle hooks, PHP function registration, and topological dependency sorting
- **Event system** with typed events and priority-ordered handlers at every request lifecycle point
- **Brotli compression** for compressible response types
- **TLS** via rustls (no OpenSSL dependency at the edge)
- **Per-IP rate limiting** with configurable limits and time windows
- **Prometheus-compatible metrics** exposed on a dedicated internal port
- **Health checks** at `/health` on the internal server
- **Request IDs** generated for every request and available in PHP via `oxphp_request_id()`
- **Structured JSON access logging** via the tracing framework
- **Custom error pages** loaded from an on-disk directory
- **Graceful shutdown** with configurable drain timeout
- **OPcache + JIT** support out of the box
- **Worker health monitoring** with automatic dead worker respawning
- **Panic isolation** via `catch_unwind` -- a PHP crash does not take down the server

## License

OxPHP is licensed under [AGPL-3.0](https://www.gnu.org/licenses/agpl-3.0.html).

## Documentation

### Getting Started

- [Installation](getting-started/installation.md)
- [Quick Start](getting-started/quick-start.md)
- [Docker](getting-started/docker.md)

### Architecture

- [Overview](architecture/overview.md) -- runtime model and component map
- [Request Lifecycle](architecture/request-lifecycle.md) -- step-by-step request pipeline
- [Worker Pool](architecture/worker-pool.md) -- PHP execution: InlineExecutor, SapiExecutor, backpressure
- [Event System](architecture/event-system.md) -- typed events and handler registration
- [SAPI and Bridge](architecture/sapi-bridge.md) -- custom PHP SAPI and C bridge library

### Features

- [Routing](features/routing.md) -- three routing modes (Traditional, Framework, SPA)
- [Static Files](features/static-files.md) -- file cache, MIME detection, streaming
- [Compression](features/compression.md) -- Brotli compression
- [TLS](features/tls.md) -- TLS configuration via rustls
- [Rate Limiting](features/rate-limiting.md) -- per-IP rate limiting
- [Error Pages](features/error-pages.md) -- custom HTML error pages
- [Request IDs](features/request-ids.md) -- X-Request-ID generation
- [Timeouts](features/timeouts.md) -- header, request, and idle timeouts
- [Access Logging](features/access-logging.md) -- structured JSON access log

### PHP Integration

- [PHP Functions](php/functions.md) -- built-in and plugin PHP functions
- [Superglobals](php/superglobals.md) -- $_SERVER, $_GET, $_POST, $_COOKIE, $_FILES
- [OPcache](php/opcache.md) -- OPcache and JIT configuration

### Operations

- [Configuration](operations/configuration.md) -- environment variable reference
- [Health Checks](operations/health-checks.md) -- /health, /metrics, /config endpoints
- [Metrics](operations/metrics.md) -- Prometheus metrics reference
- [Graceful Shutdown](operations/graceful-shutdown.md) -- drain behavior and timeouts
