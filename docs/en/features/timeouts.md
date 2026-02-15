---
title: Timeouts
description: Configurable connection and request timeouts
---

OxPHP enforces configurable timeouts to protect against slow clients and runaway requests. Each timeout is configurable via an environment variable.

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `HEADER_TIMEOUT_SECS` | Maximum time to receive HTTP headers after TCP connect | `5` |
| `IDLE_TIMEOUT_SECS` | Maximum idle time between requests on a keep-alive connection | `60` |
| `REQUEST_TIMEOUT_SECS` | Maximum total time for request handling (including PHP execution) | `120` |

```bash
HEADER_TIMEOUT_SECS=5
IDLE_TIMEOUT_SECS=60
REQUEST_TIMEOUT_SECS=120
```

Setting `HEADER_TIMEOUT_SECS=0` skips registering the header read timeout with hyper. Setting `REQUEST_TIMEOUT_SECS=0` disables the request timeout entirely.

## Timeout types

### Header read timeout

Controls how long the server waits for a client to send a complete set of HTTP headers after the TCP connection is established (or after the TLS handshake completes).

This protects against slowloris-style attacks where a client sends headers one byte at a time to hold a connection open indefinitely.

**Implementation note**: hyper-util requires a timer to be registered with `builder.http1().timer(TokioTimer::new())` before `header_read_timeout` can be set. OxPHP always registers this timer. If the timeout is set to zero, the `header_read_timeout` call is skipped.

### Idle timeout

Reserved for controlling how long a keep-alive connection can remain idle between requests. The `IDLE_TIMEOUT_SECS` variable is read from the environment and included in the `/config` internal endpoint, but hyper-util's HTTP/1.1 builder does not expose a `keep_alive_timeout` setting. This timeout is not currently enforced at the connection level.

### Request timeout

Controls the maximum total time for processing a single request, from route resolution through PHP execution, response building, and compression. Implemented as a `tokio::time::timeout` wrapper around the dispatch pipeline. This applies to both regular script execution and handler mode requests.

When the timeout fires, the server returns a `504 Gateway Timeout` response with a warning log entry that includes the request ID and path.

For PHP requests, the request timeout is the outer boundary. PHP's own `max_execution_time` may trigger first, but the request timeout ensures the server-side resources are reclaimed even if PHP does not respect its own time limit.

## How timeouts interact

The header read timeout and request timeout cover different phases of a request:

```
TCP connect (+ TLS handshake if enabled)
  |
  +-- [HEADER_TIMEOUT_SECS] --> headers received
  |                               |
  |                               +-- [REQUEST_TIMEOUT_SECS] --> response sent
  |                                                                |
  |                                                                +-- next request or close
  |                                                                     |
  |                                                                     +-- [HEADER_TIMEOUT_SECS] --> ...
```

On a keep-alive connection, the header timeout and request timeout apply to each request individually.

## Recommended values

| Scenario | Header | Request |
|----------|--------|---------|
| General web serving | 5s | 120s |
| API server | 3s | 30s |
| Long-running PHP tasks | 5s | 300s |
| High-security / anti-slowloris | 2s | 30s |

Adjust these based on your application's characteristics. If your PHP scripts perform long-running operations (report generation, data imports), increase the request timeout accordingly.

## See Also

- [TLS](tls.md) -- header read timeout starts after the TLS handshake completes
- [Rate Limiting](rate-limiting.md) -- rate-limited requests bypass the request timeout (returned as early responses)
