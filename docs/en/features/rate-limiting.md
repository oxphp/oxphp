---
title: Rate Limiting
description: Per-IP rate limiting with configurable limits and time windows
---

OxPHP includes a built-in per-IP rate limiter that rejects excess requests with a 429 response. Rate limiting is disabled by default and activates when you set the `RATE_LIMIT` variable to a non-zero value.

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `RATE_LIMIT` | Maximum requests per IP per window | `0` (disabled) |
| `RATE_WINDOW_SECONDS` | Window duration in seconds | `60` |

```bash
# Allow 100 requests per IP per 60-second window
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
```

Setting `RATE_LIMIT=0` disables rate limiting entirely. When disabled, the rate limit handler is not registered in the event dispatcher and adds zero overhead.

## How it works

The rate limiter uses a `DashMap` (sharded concurrent hash map) keyed by client IP address. Each entry stores a request count and the timestamp when the current window started.

### Request flow

1. Look up the client IP in the map (or insert a new entry)
2. If the window has expired, reset the counter to zero and start a new window
3. Increment the counter
4. If the counter exceeds `RATE_LIMIT`, return a 429 response

### 429 response

When a request is rate-limited, the server returns:

```http
HTTP/1.1 429 Too Many Requests
Retry-After: 45
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 45
X-Request-ID: 67890abc00000042

429 Too Many Requests
```

| Header | Description |
|--------|-------------|
| `Retry-After` | Seconds until the current window resets |
| `X-RateLimit-Limit` | Maximum requests allowed per window |
| `X-RateLimit-Remaining` | Requests remaining in the current window (always `0` when rate-limited) |
| `X-RateLimit-Reset` | Seconds until the current window resets |
| `X-Request-ID` | The request ID for this request |

## Event handler integration

Rate limiting is implemented as an event handler that listens for `RequestReceived` events at priority **-50**. This means it runs after the request ID generator (priority -100) but before metrics and other handlers.

When the rate limiter rejects a request, it sets the `early_response` field on the event. The connection handler checks this field and returns the 429 response immediately, skipping routing and PHP execution.

The handler always returns `Propagation::Continue` so that metrics and access logging handlers still process the request. This means rate-limited requests appear in your access logs and metrics.

Each rejected request increments the `oxphp_rate_limited_total` Prometheus counter.

## Cleanup

A background Tokio task runs every 60 seconds and removes entries whose window expired more than `2 * RATE_WINDOW_SECONDS` seconds ago. This prevents the `DashMap` from growing indefinitely when clients send a burst of requests and then disappear.

## Limitations

- **Fixed window algorithm** -- the limiter uses a simple fixed-window counter, not sliding window. A client can send up to `2 * RATE_LIMIT` requests in a burst at the boundary between two windows.
- **Per-IP only** -- rate limiting is keyed by source IP address. Clients behind a shared NAT or proxy share the same counter.
- **In-memory only** -- rate limit state is not shared across multiple OxPHP instances. Each instance tracks its own counters.

## See Also

- [Request IDs](request-ids.md) -- rate-limited responses include the `X-Request-ID` header
- [Access Logging](access-logging.md) -- rate-limited requests appear in access logs
- [Metrics](/operations/metrics.md) -- the `oxphp_rate_limited_total` counter tracks rejected requests
- [Error Pages](error-pages.md) -- custom error pages do not apply to 429 responses (rate limiting sets the response body directly)
