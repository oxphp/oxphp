---
title: Request Lifecycle
description: Step-by-step walkthrough of how OxPHP processes an HTTP request from TCP accept to response
---

Every HTTP request in OxPHP passes through a pipeline of stages, from TCP acceptance to response delivery. This page traces that pipeline through the actual code in `src/server/connection.rs`.

## Pipeline Overview

```
  Client
    │
    ▼
┌──────────────────┐
│ TCP Accept       │  main.rs: listener.accept()
│ + TLS Handshake  │  server/mod.rs: handle_connection()
└────────┬─────────┘
         ▼
┌──────────────────┐
│ HTTP Parse       │  hyper-util auto::Builder
│ (http1/http2)    │  service_fn → handle_request()
└────────┬─────────┘
         ▼
┌──────────────────┐
│ RequestReceived  │  Event dispatch (priority order):
│ Event            │    -100  RequestIdGenerator
│                  │    -50   RateLimitHandler
│                  │      0   MetricsRequestHandler
└────────┬─────────┘
         │
    ┌────┴────┐
    │ Early   │──── Yes ──▶ 429 Too Many Requests
    │ Response│              (skip to RequestComplete)
    │ ?       │
    └────┬────┘
         │ No
         ▼
┌───────────────────┐
│ Plugin Cookie     │  plugin::cookies::strip_plugin_cookies()
│ Strip             │
└────────┬──────────┘
         ▼
┌───────────────────┐
│ Route Resolution  │  routing.rs: resolve_request()
│ Serve / Execute / │  sanitize, validate, file cache
│ NotFound          │
└────────┬──────────┘
         │
    ┌────┴─────────┐
    │              │
    ▼              ▼
┌────────┐  ┌──────────┐
│ Static │  │ PHP      │
│ File   │  │ Execution│
│ Serve  │  │ (worker) │
└───┬────┘  └────┬─────┘
    │            │
    └─────┬──────┘
          ▼
┌───────────────────┐
│ ResponseBuilding  │  Event dispatch (priority order):
│ Event             │     60   ErrorPagesHandler
│                   │    100   ServerHeaderHandler
└────────┬──────────┘
         ▼
┌───────────────────┐
│ Brotli            │  compression.rs: maybe_compress()
│ Compression       │  (if Accept-Encoding: br)
└────────┬──────────┘
         ▼
┌───────────────────┐
│ RequestComplete   │  Event dispatch (priority order):
│ Event             │      0   MetricsResponseHandler
│                   │    100   AccessLogHandler
└────────┬──────────┘
         ▼
  Response sent
```

## Stage-by-Stage Detail

### 1. TCP Accept and Connection Setup

The accept loop in `main.rs` calls `listener.accept()` for each incoming connection. A `Semaphore` with `max_connections` permits bounds total concurrency. Each connection spawns a Tokio task:

```rust
let (stream, remote_addr) = listener.accept().await?;
let permit = semaphore.clone().acquire_owned().await?;
tokio::spawn(async move {
    let _permit = permit;
    server_clone.handle_connection(stream, remote_addr).await;
});
```

In `Server::handle_connection()` (`src/server/mod.rs`), the server records the connection in metrics via `ConnectionGuard` (RAII — automatically decrements on drop) and optionally performs a TLS handshake:

```rust
self.metrics.connection_opened();
let _guard = ConnectionGuard(Arc::clone(&self.metrics));

if let Some(ref acceptor) = self.tls_acceptor {
    let tls_stream = acceptor.accept(stream).await?;
    // ... serve over TLS
} else {
    // ... serve over plaintext
}
```

### 2. HTTP Parsing

`hyper-util`'s `auto::Builder` handles HTTP/1.1 and HTTP/2 protocol detection. A `header_read_timeout` protects against slow-header attacks (requires `TokioTimer` to be set on the builder). The builder calls `service_fn`, which invokes `handle_request()` for each HTTP request on the connection.

### 3. Request Decomposition

At the start of `handle_request()`, the request is split into parts and body:

```rust
let start = Instant::now();
let (parts, body) = req.into_parts();
```

The `Accept-Encoding` header is checked here for Brotli support — a non-allocating check via `is_some_and(compression::accepts_brotli)`.

### 4. RequestReceived Event

The first event dispatch runs three handlers in priority order:

| Priority | Handler | Action |
|---|---|---|
| -100 | `RequestIdGenerator` | Generates `{timestamp_hex:08x}{counter:08x}` (16 hex chars) or preserves incoming `X-Request-ID` |
| -50 | `RateLimitHandler` | Checks per-IP sliding window; sets `early_response` if limit exceeded |
| 0 | `MetricsRequestHandler` | Calls `metrics.record_request(&method)` |

The `RequestReceived` event includes a `metadata: Vec<(String, String)>` field that plugin handlers can use to attach key-value data.

The request ID is extracted with `std::mem::take` (zero-copy move, no clone):

```rust
let request_id = std::mem::take(&mut received_event.request_id);
```

### 5. Early Response Short-Circuit

If any handler set `early_response` on the `RequestReceived` event (the rate limiter sets a 429 response), the pipeline skips directly to `RequestComplete`:

```rust
if let Some(early_resp) = received_event.early_response {
    // Dispatch RequestComplete for metrics/logging, then return
    return Ok(early_resp);
}
```

This ensures that rate-limited requests are still counted in metrics and appear in the access log. Method and path strings are allocated only here in the early path (deferred from step 3 to avoid unnecessary allocations when `early_response` is not set).

### 6. Plugin Cookie Stripping and String Allocation

After the early response check, the pipeline:

1. Retrieves the request parts from the event
2. Allocates method and path strings (`method_str`, `path_str`) — deferred until this point to avoid allocation when the request is short-circuited
3. Calls `plugin::cookies::strip_plugin_cookies()` to remove plugin-internal cookies from the request headers before forwarding to PHP

### 7. Request Timeout

If `REQUEST_TIMEOUT_SECS` is configured (non-zero), the remaining pipeline is wrapped in `tokio::time::timeout`. If the timeout fires, a 504 Gateway Timeout is returned:

```rust
match tokio::time::timeout(server.request_timeout, dispatch_request(...)).await {
    Ok(inner_result) => inner_result,
    Err(_) => Ok(Response::builder()
        .status(StatusCode::GATEWAY_TIMEOUT)
        .body(full_body(Bytes::from_static(b"504 Gateway Timeout")))
        .unwrap()),
}
```

### 8. Route Resolution

`RouteConfig::resolve_request()` in `src/server/routing.rs` resolves the URI path to one of three outcomes:

| Result | Meaning |
|---|---|
| `Serve(PathBuf)` | Serve a static file from disk |
| `Execute(PathBuf)` | Send to a PHP worker thread |
| `NotFound` | Return 404 |

The routing process:

1. Percent-decode the URI
2. Sanitize the path (remove `..` and `.` segments)
3. Block direct access to `INDEX_FILE` and `.php` files in framework mode
4. Check the file cache for existence
5. Fall back to `INDEX_FILE` if configured (framework/SPA mode)
6. Validate that the resolved path does not escape the document root via symlinks

### 9a. Static File Serving

For `Serve` results, `static_file::serve()` reads the file from disk (with file cache for metadata), detects the MIME type, and returns the response with appropriate `Content-Type` and `Content-Length` headers.

### 9b. PHP Execution

For `Execute` results, the request body is collected with a **10 MB limit** (`MAX_POST_BODY`). Body collection only occurs for POST, PUT, and PATCH requests — all other methods (GET, HEAD, DELETE, etc.) receive an empty `Bytes` without reading from the body stream. If the body exceeds this limit, a 413 Payload Too Large response is returned immediately.

```rust
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

let limited = Limited::new(body, MAX_POST_BODY);
let body_bytes = match BodyExt::collect(limited).await {
    Ok(collected) => collected.to_bytes(),
    Err(e) => {
        if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
            return Ok(Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(...)?);
        }
        return Err(e);
    }
};
```

A `ScriptRequest` is constructed and sent to the executor:

```rust
let script_request = ScriptRequest {
    request_id: request_id.to_string(),
    script_path,
    method: parts.method,
    uri: parts.uri,
    query_string,
    headers: parts.headers,
    body: body_bytes,
    remote_addr,
    document_root: ctx.route_config.document_root_arc(),
};

ctx.metrics.request_queued();
let response_rx = ctx.executor.execute(script_request);
```

The Tokio task awaits the `oneshot::Receiver`. When the PHP worker finishes, it sends back a `ScriptResponse` containing the status code, headers, body, and execution time. If the worker channel is broken, a 500 error is returned and `metrics.request_dropped()` is called.

### 10. ResponseBuilding Event

After the response is built (from either static file serving or PHP execution), the `ResponseBuilding` event fires:

| Priority | Handler | Action |
|---|---|---|
| 60 | `ErrorPagesHandler` | Replaces the response body with a custom HTML page for status >= 400 |
| 100 | `ServerHeaderHandler` | Adds `Server: OxPHP/{version}` and `X-Request-ID` headers |

This is the only point where `request_id` is cloned (once), because it is needed again in the `RequestComplete` event.

### 11. Brotli Compression

If the client sent `Accept-Encoding: br` and compression is enabled, `compression::maybe_compress()` runs after the ResponseBuilding event:

- Checks if the content type is compressible (text/html, application/json, etc.)
- Skips responses that already have `Content-Encoding`
- Skips bodies smaller than 256 bytes or larger than 3 MB
- Compresses with Brotli quality 4, window size 20
- Only uses the compressed version if it is actually smaller
- Updates `Content-Encoding`, `Content-Length`, and adds `Vary: Accept-Encoding`

### 12. RequestComplete Event

The final event carries the complete request metadata:

```rust
let mut complete_event = RequestComplete {
    request_id,    // move, no clone
    method,        // http::Method (moved, no clone)
    path: path_str,
    status,
    duration: elapsed,
    remote_addr,
};
```

| Priority | Handler | Action |
|---|---|---|
| 0 | `MetricsResponseHandler` | Calls `metrics.record_response(status, duration)` |
| 100 | `AccessLogHandler` | Emits a structured JSON log entry via `tracing::info!` |

### 13. Response Delivery

The `Ok(response)` is returned to hyper-util, which serializes it to the wire. For keep-alive connections, the `service_fn` closure is called again for the next request on the same connection.

## Error Handling

Errors at each stage produce appropriate HTTP status codes:

| Error | Status | Source |
|---|---|---|
| Rate limited | 429 | `RateLimitHandler` via early response |
| Body too large | 413 | `Limited` body collection |
| Request timeout | 504 | `tokio::time::timeout` |
| PHP worker error | 500 | Broken oneshot channel |
| Queue full | 503 | `SapiExecutor::execute()` via `try_send` |
| File not found | 404 | Route resolution |
| Internal error | 500 | Catch-all in `handle_request` |

## Allocation Budget

The pipeline is designed to minimize allocations per request:

- **0 clones** of `request_id` through most of the pipeline (`std::mem::take`)
- **1 clone** of `request_id` at the `ResponseBuilding` event (needed for reuse in `RequestComplete`)
- **0 clones** of `method` (`http::Method`) and `path_str` (moved through the pipeline)
- Method and path strings are **deferred** until after the early response check — rate-limited requests avoid the allocation entirely
- `Accept-Encoding` is checked with a non-allocating `is_some_and` call
- `RouteConfig` uses pre-computed index paths for the root `/` to avoid `PathBuf::join` on every request

## See Also

- [Architecture Overview](./overview.md) — Component map and high-level data flow
- [Event System](./event-system.md) — Event types, priorities, and handler registration
- [Worker Pool](./worker-pool.md) — How PHP workers process `ScriptRequest`s
- [SAPI and Bridge](./sapi-bridge.md) — PHP worker internal execution flow
- [Routing](../features/routing.md) — Three routing modes and path sanitization
- [Compression](../features/compression.md) — Brotli compression configuration
- [Timeouts](../features/timeouts.md) — Request and header timeout behavior
- [Rate Limiting](../features/rate-limiting.md) — Per-IP rate limiting and 429 responses
