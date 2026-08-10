---
title: Architecture Overview
description: How OxPHP works: async HTTP handling, PHP worker pool, request flow, worker mode, and built-in safety guarantees — from a user's perspective.
---

# Architecture Overview

OxPHP is a single-binary HTTP server that replaces the traditional nginx + PHP-FPM stack. It handles HTTP parsing, TLS termination, routing, PHP execution, compression, and observability in one process with no external dependencies at runtime.

## How OxPHP Works

OxPHP combines two runtime layers in a single process:

1. **Async HTTP layer** — an event-driven network layer accepts TCP connections, performs TLS handshakes, parses HTTP requests, and sends responses. It handles thousands of concurrent connections using non-blocking I/O, so one slow client never blocks another.
2. **PHP worker pool** — a pool of dedicated PHP workers executes your PHP scripts. In standard mode, each worker handles one request at a time. In [Worker mode](../features/worker-mode.md) with [fiber multiplexing](../features/fiber-multiplexing.md) enabled, a single worker can serve multiple concurrent requests — when a script calls `oxphp_sleep()` or `oxphp_async_await()`, the fiber yields the thread and the worker switches to the next request.
3. **Async pool** (optional) — separate OS threads for tasks submitted via `oxphp_async()`. Enabled by setting `ASYNC_WORKERS > 0`. Isolated from the worker pool so background tasks do not block HTTP request handling.

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

Retirement is taken at a moment when the worker has nothing in flight, so a request it is still serving always runs to its own response. The same rule sets the boundary: a worker holding a request that never ends — an open stream, a read from a peer that stops answering — never reaches such a moment, and so is not retired while it holds it.

### Queue and Backpressure

Between the async HTTP layer and the worker pool sits a bounded queue. Its capacity defaults to the initial worker count multiplied by 128 and can be overridden with `QUEUE_CAPACITY`. For a static pool, the initial count is the configured worker count. For a dynamic pool (`MIN:MAX`), the initial count is the minimum.

A request that arrives at a full queue is not rejected outright — it waits for a slot for up to `QUEUE_WAIT_TIMEOUT_MS` (default: 1000 ms) and is admitted as soon as a worker picks up the request ahead of it. Waiting requests are admitted in arrival order. The budget is a single deadline stamped on arrival and it follows the request into the queue: one reached by a worker after that deadline has passed is refused at pickup instead of executed, so the budget bounds the whole wait rather than its admission half — which matters because the queue is deep enough that reaching its tail on a slow pool takes far longer than any budget an operator would set. Load is therefore shed on elapsed wait time rather than on the queue depth at the instant the request arrived: a burst that drains in microseconds is served, while a pool that genuinely cannot keep up still sheds.

The budget bounds how long a request waits to **enter** the queue, not how long it waits overall. An admitted request still sits in the queue until a worker reaches it, so response time under load is the admission wait plus the queue wait plus execution time — the budget is not a ceiling on any of the latter two.

`QUEUE_MAX_WAITING` caps how many requests may be waiting at once (default: initial workers × 128, capped at half of `MAX_CONNECTIONS`); past it, admission goes back to rejecting immediately. Waiting is not free — a waiting request holds its connection, and its already-buffered body, until it is admitted or the budget runs out — so an uncapped waiting set would consume every connection permit under sustained overload, block the accept loop, and leave the server dropping new connections instead of answering them. The default approximates how many requests the pool can actually admit inside the budget: waiting past that only defers a rejection while holding those resources. The `MAX_CONNECTIONS` / 2 ceiling bounds the waiting set alone, which is not by itself headroom for the accept loop: a queued request holds its connection until a worker reaches it, and a running one holds it too — its queue slot was released at pickup — while `QUEUE_CAPACITY` has no connection-derived ceiling at all. So `PHP_WORKERS` + `QUEUE_CAPACITY` + `QUEUE_MAX_WAITING` has to stay below `MAX_CONNECTIONS`, and once it reaches the budget the accept loop parks with every permit taken — a client arriving then gets no response rather than a 529. The server warns about that at startup. Clearing it is necessary rather than sufficient: connections that never reach PHP hold permits too. Note that the terms are sized in connections while the budget is spent by requests: over HTTP/2 one connection carries many requests, so an h2-heavy deployment reaches the cap from a fraction of its connection budget, should set `QUEUE_MAX_WAITING` explicitly, and can legitimately sit above the sum.

Only when the wait budget runs out does OxPHP return a `529 Site is Overloaded` response with a `Retry-After` header. Status code 529 (non-standard, used by Cloudflare and others) clearly distinguishes overload from application errors (500) and maintenance (503), making it easier to configure alerts and load balancers. Setting `QUEUE_WAIT_TIMEOUT_MS=0` disables the wait and rejects the moment the queue is full.

Because waiting requests occupy a connection rather than a queue slot, the concurrency ceiling under overload is set by `MAX_CONNECTIONS` rather than `QUEUE_CAPACITY` — and over HTTP/2 by `MAX_CONNECTIONS` multiplied by `H2_MAX_CONCURRENT_STREAMS`, since each stream carries a request of its own. Size the wait budget with that in mind: the request body is already buffered by the time a request reaches the queue, so a longer budget means a genuinely overloaded server holds proportionally more requests — and their bodies — in memory before answering.

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
Trusted proxy resolution (if TRUSTED_PROXIES set)
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
  └── PHP request ──► Bounded queue (529 if full)
                         │
                         ▼
                    PHP worker executes script
                         │
                    ┌────┼─────────────────┐
                    ▼    ▼                 ▼
                 Normal  SSE streaming   Early response
                 response (chunked)      (finish_request)
                    │       │                │
                    ▼       ▼                ▼
                 Compression Chunks ──► client Response to client
                    │                      + background work
                    ▼
                 Response to client
```

1. **TLS termination** — if `TLS_CERT` and `TLS_KEY` are configured, OxPHP handles TLS directly. No separate reverse proxy is needed.
2. **HTTP parsing and request ID** — the request is parsed and a unique request ID is generated (or an incoming `X-Request-ID` header is preserved).
3. **Trusted proxy resolution** — if `TRUSTED_PROXIES` is set and the connecting IP is trusted, OxPHP extracts the real client IP, protocol, and host from `Forwarded` (RFC 7239) or `X-Forwarded-*` headers. The resolved IP is used for all subsequent steps including rate limiting and access logging. See [Trusted Proxies](../security/trusted-proxies.md).
4. **Rate limiting** — if `RATE_LIMIT` is set, the client's IP is checked against the per-IP request counter. Requests that exceed the limit receive a `429 Too Many Requests` response immediately.
5. **Route resolution** — the URL is matched against the configured routing mode (traditional, framework, or SPA). The result is either a static file, a PHP script, or a 404. Worker mode, if enabled, changes how PHP executes the resolved script but does not change route resolution itself.
6. **Static files** — served directly from an in-memory cache (for frequently accessed files) or streamed from disk. OxPHP adds `ETag`, `Last-Modified`, and `Cache-Control` headers automatically.
7. **PHP execution** — the request is placed in the bounded queue and picked up by an available worker. If the queue is full, the client receives 529 immediately.
8. **Compression** — text-based responses are compressed with Brotli before being sent when the client sends `Accept-Encoding: br` (configurable via `COMPRESSION_LEVEL`).
9. **SSE streaming** — if the script sets `Content-Type: text/event-stream` or calls `oxphp_stream_flush()`, OxPHP switches to streaming mode: each `flush()` call sends a chunk to the client immediately without buffering the entire response. In Worker mode, SSE works cooperatively with [fiber multiplexing](../features/fiber-multiplexing.md).
10. **Early response** — calling `oxphp_finish_request()` sends the HTTP response to the client immediately. The script continues executing in the background — for writing logs, updating caches, sending notifications — without holding the connection open.
11. **Response delivery** — the completed response is sent back over the connection, and if [access logging](../features/access-logging.md) is enabled, a log entry is written.

## Worker Mode vs Standard Mode

OxPHP supports two PHP execution models:

**Standard mode** (default) creates a fresh PHP environment for every request. Autoloaders, configuration, and database connections are initialized on each request and torn down afterward. This model is compatible with all PHP applications out of the box.

**Worker mode** keeps PHP processes alive across requests. Your application bootstraps once — loading the autoloader, configuration, and establishing database connections — and then enters a request loop. Between requests, OxPHP automatically resets superglobals, output buffers, and response headers while preserving the bootstrapped state.

Worker mode eliminates per-request startup overhead, which can reduce response times significantly for framework-based applications (Laravel, Symfony, etc.) where bootstrapping is expensive.

To enable worker mode, set `WORKER_MODE_ENABLED=true` and point `ENTRY_FILE` at a PHP script that calls `oxphp_worker()`:

```php
<?php
require __DIR__ . '/../vendor/autoload.php';
$app = new MyApp\Application();

oxphp_worker(function () use ($app) {
    $app->handle();
});
```

For a detailed guide, see [Worker Mode](../features/worker-mode.md).

## Internal Server

If the `INTERNAL_ADDR` variable is set, OxPHP starts a separate HTTP server on the specified port. It serves three endpoints:

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health status in JSON (uptime, request counters, connections, worker state). Returns `200` during normal operation, `503` during degradation. |
| `GET /metrics` | Prometheus-format metrics — request counters, response time, queue wait time, worker statistics, compression savings. |
| `GET /config` | Snapshot of the active configuration in JSON. TLS file paths are redacted. |

The internal server does not go through the PHP worker pool or the bounded queue — it responds directly from the async HTTP layer, so it remains accessible even when the PHP pool is fully loaded. This makes `/health` suitable for Kubernetes liveness/readiness probes.

For details, see [Internal Server](../features/internal-server.md).

## Safety

OxPHP provides several guarantees to keep your application running reliably in production:

- **Request isolation** — if a PHP script crashes or triggers a fatal error, only that single request is affected. The server continues handling all other requests normally. The crashed worker is automatically replaced with a fresh one.
- **Automatic worker respawn** — OxPHP monitors the health of all PHP workers. If a worker dies unexpectedly, a new worker is started in its place without manual intervention.
- **Backpressure protection** — the bounded request queue prevents overload. When the server is at capacity, new requests receive a 529 response with a `Retry-After` header rather than queueing indefinitely and causing cascading timeouts.
- **Path traversal protection** — all URL paths are sanitized before filesystem access. Percent-encoded traversal attempts, `..` segments, and paths that escape the document root are blocked.
- **Graceful shutdown** — on SIGTERM or SIGINT (Ctrl+C), OxPHP stops accepting new connections and waits for in-flight requests to complete (up to a configurable drain timeout) before exiting.

## See Also

- [Worker Mode](../features/worker-mode.md) — persistent PHP processes and the `oxphp_worker()` API
- [Fiber Multiplexing](../features/fiber-multiplexing.md) — hundreds of concurrent requests on a single worker
- [SSE Streaming](../features/sse.md) — streaming events from PHP
- [Early Response](../features/early-response.md) — `oxphp_finish_request()` and background processing
- [Async Promises](../features/async-promises.md) — `oxphp_async()` / `oxphp_async_await()`
- [Internal Server](../features/internal-server.md) — health, metrics, config
- [Routing](../features/routing.md) — three routing modes and how URLs are resolved
- [Configuration Reference](../operations/configuration.md) — complete list of environment variables
- [Metrics](../operations/metrics.md) — observing the worker pool and request pipeline
- [Quick Start](../getting-started/quick-start.md) — get OxPHP running in under 5 minutes
