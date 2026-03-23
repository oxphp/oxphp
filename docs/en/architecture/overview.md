---
title: Architecture Overview
description: How OxPHP works: async HTTP handling, PHP worker pool, request flow, worker mode, and built-in safety guarantees — from a user's perspective.
---

# Architecture Overview

OxPHP is a single-binary HTTP server that replaces the traditional nginx + PHP-FPM stack. It handles HTTP parsing, TLS termination, routing, PHP execution, compression, and observability in one process with no external dependencies at runtime.

## How OxPHP Works

OxPHP combines two runtime layers in a single process:

1. **Async HTTP layer** — an event-driven network layer accepts TCP connections, performs TLS handshakes, parses HTTP requests, and sends responses. It handles thousands of concurrent connections using non-blocking I/O, so one slow client never blocks another.
2. **PHP worker pool** — a pool of dedicated PHP workers executes your PHP scripts. Each worker runs one request at a time with full isolation from other workers.

The two layers communicate through a bounded queue. When an HTTP request arrives that needs PHP execution, the async layer places it in the queue. An available PHP worker picks it up, executes the script, and returns the response to the async layer for delivery to the client.

This separation means that network I/O — accepting connections, reading headers, compressing responses, serving static files — never competes with PHP execution for resources. Each layer scales independently.

## Worker Pool

The PHP worker pool determines how many PHP scripts can execute concurrently. OxPHP supports two pool modes.

### Static Pool

A fixed number of workers start at boot and remain running for the lifetime of the server. This is the default mode.

```bash
PHP_WORKERS=8    # exactly 8 workers
PHP_WORKERS=0    # auto-detect (default): half of available CPU cores, minimum 1
```

### Dynamic Pool

Workers scale up and down based on demand. Specify a minimum and maximum count separated by a colon:

```bash
PHP_WORKERS=2:16    # start with 2, scale up to 16 under load
```

When all current workers are busy, OxPHP spawns new workers up to the maximum. When a worker has been idle for longer than `PHP_WORKERS_IDLE_SECONDS` (default: 30 seconds), it is retired back down toward the minimum.

### Queue and Backpressure

Between the async HTTP layer and the worker pool sits a bounded queue. Its capacity defaults to the initial worker count multiplied by 128 and can be overridden with `QUEUE_CAPACITY`. For a static pool, the initial count is the configured worker count. For a dynamic pool (`MIN:MAX`), the initial count is the minimum.

When the queue is full — meaning all workers are busy and the queue has reached capacity — OxPHP immediately returns a `503 Service Unavailable` response to the client. This backpressure mechanism prevents the server from accepting unbounded work that would exhaust memory or increase latency for all requests.

## Request Flow

Every request passes through the same pipeline, regardless of whether it serves a static file or executes PHP:

```text
Client
  │
  ▼
TLS termination (if configured)
  │
  ▼
HTTP parsing + Request ID assignment
  │
  ▼
Rate limiting check
  │
  ▼
Route resolution
  │
  ├── Static file ──► File cache / disk read
  │                         │
  │                         ▼
  │                    Compression + Response headers
  │                         │
  │                         ▼
  │                    Response to client
  │
  └── PHP request ──► Bounded queue
                         │
                         ▼
                    PHP worker executes script
                         │
                         ▼
                    Compression + Response headers
                         │
                         ▼
                    Response to client
```

1. **TLS termination** — if `TLS_CERT` and `TLS_KEY` are configured, OxPHP handles TLS directly. No separate reverse proxy is needed.
2. **HTTP parsing and request ID** — the request is parsed and a unique request ID is generated (or an incoming `X-Request-ID` header is preserved).
3. **Rate limiting** — if `RATE_LIMIT` is set, the client's IP is checked against the per-IP request counter. Requests that exceed the limit receive a `429 Too Many Requests` response immediately.
4. **Route resolution** — the URL is matched against the configured routing mode (traditional, framework, SPA, or worker). The result is either a static file, a PHP script, or a 404.
5. **Static files** — served directly from an in-memory cache (for frequently accessed files) or streamed from disk. OxPHP adds `ETag`, `Last-Modified`, and `Cache-Control` headers automatically.
6. **PHP execution** — the request is placed in the bounded queue and picked up by an available worker. If the queue is full, the client receives 503 immediately.
7. **Compression** — text-based responses are compressed with Brotli before being sent when the client sends `Accept-Encoding: br` (configurable via `COMPRESSION_LEVEL`).
8. **Response delivery** — the completed response is sent back over the connection, and access logging fires if enabled.

## Worker Mode vs Standard Mode

OxPHP supports two PHP execution models:

**Standard mode** (default) creates a fresh PHP environment for every request. Autoloaders, configuration, and database connections are initialized on each request and torn down afterward. This model is compatible with all PHP applications out of the box.

**Worker mode** keeps PHP processes alive across requests. Your application bootstraps once — loading the autoloader, configuration, and establishing database connections — and then enters a request loop. Between requests, OxPHP automatically resets superglobals, output buffers, and response headers while preserving the bootstrapped state.

Worker mode eliminates per-request startup overhead, which can reduce response times significantly for framework-based applications (Laravel, Symfony, etc.) where bootstrapping is expensive.

To enable worker mode, set `WORKER_FILE` to a PHP script that calls `oxphp_worker()`:

```php
<?php
require __DIR__ . '/../vendor/autoload.php';
$app = new MyApp\Application();

oxphp_worker(function () use ($app) {
    $app->handle();
});
```

For a detailed guide, see [Worker Mode](../features/worker-mode.md).

## Safety

OxPHP provides several guarantees to keep your application running reliably in production:

- **Request isolation** — if a PHP script crashes or triggers a fatal error, only that single request is affected. The server continues handling all other requests normally. The crashed worker is automatically replaced with a fresh one.
- **Automatic worker respawn** — OxPHP monitors the health of all PHP workers. If a worker dies unexpectedly, a new worker is started in its place without manual intervention.
- **Backpressure protection** — the bounded request queue prevents overload. When the server is at capacity, new requests receive a clear 503 response rather than queueing indefinitely and causing cascading timeouts.
- **Path traversal protection** — all URL paths are sanitized before filesystem access. Percent-encoded traversal attempts, `..` segments, and paths that escape the document root are blocked.
- **Graceful shutdown** — on SIGTERM, OxPHP stops accepting new connections and waits for in-flight requests to complete (up to a configurable drain timeout) before exiting.

## See Also

- [Worker Mode](../features/worker-mode.md) — persistent PHP processes and the `oxphp_worker()` API
- [Routing](../features/routing.md) — four routing modes and how URLs are resolved
- [Configuration Reference](../operations/configuration.md) — complete list of environment variables
- [Metrics](../operations/metrics.md) — observing the worker pool and request pipeline
- [Quick Start](../getting-started/quick-start.md) — get OxPHP running in under 5 minutes
