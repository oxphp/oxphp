---
title: Timeouts
description: Configure header read timeouts and PHP execution timeouts in OxPHP to protect against slow clients and runaway PHP scripts.
---

# Timeouts

OxPHP enforces two independent timeouts that protect against slow clients and runaway requests. The header timeout guards the connection phase at the server level. PHP execution time is bounded by PHP's own `max_execution_time` ini directive (and the `set_time_limit()` runtime function), exactly as on any other SAPI.

## How It Works

Each request passes through these phases:

1. **Connection accepted** — the header timeout starts. OxPHP waits for the client to send a complete set of HTTP headers.
2. **Headers received** — the header timeout ends. The request is dispatched to a PHP worker.
3. **PHP processes the request** — application code runs under PHP's own `max_execution_time` (SIGALRM-driven). When the limit is reached, the request is cancelled and the unified `Request cancelled (timeout)` fatal fires.
4. **Response sent** — on keep-alive connections, the cycle repeats from step 1.

```text
TCP connect (+ TLS handshake if enabled)
  |
  +-- [HEADER_TIMEOUT_SECONDS] --> headers received
                                   |
                                   +-- [max_execution_time] --> response sent
                                                                    |
                                                                    +-- next request (keep-alive)
```

On keep-alive connections, both timeouts apply independently to each request in the connection.

> **Note:** When TLS is enabled, the header timeout starts after the TLS handshake completes, not when the TCP connection is accepted.

The header timeout protects against slowloris-style attacks, where a client sends headers one byte at a time to hold connections open indefinitely.

PHP execution timing is delegated entirely to PHP. When `max_execution_time` is exceeded, OxPHP's unified cancellation path:

- Sets `connection_status() & PHP_CONNECTION_TIMEOUT` so userland code can detect the cause.
- Runs all `register_shutdown_function()` callbacks, exactly as PHP-FPM does.
- Returns HTTP `500` (PHP fatal error) with the message `Request cancelled (timeout)` written to the error log.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | Maximum seconds to receive request headers after the connection is accepted. Protects against slowloris attacks. Set to `0` to disable |

PHP execution time is configured via `php.ini`, not OxPHP env vars:

```ini
; php.ini
max_execution_time = 30
```

Or per-script at runtime:

```php
set_time_limit(60);  // 60 seconds from now
set_time_limit(0);   // disable for this request
```

## Recommended Values

| Scenario | Header Timeout | `max_execution_time` |
|----------|----------------|----------------------|
| API server | 5s | 30s |
| General web serving | 5s | 60s |
| File uploads | 10s | 300s |
| SSE / long-polling | 5s | 0 (disabled, set per-script) |

Adjust these values based on your application's characteristics. For SSE endpoints, call `set_time_limit(0)` at the top of the streaming script rather than disabling `max_execution_time` globally.

## Troubleshooting

### Clients receive 500 with `Request cancelled (timeout)` unexpectedly

The PHP execution-time limit fired before the script finished.

**Fix:** Raise `max_execution_time` for the affected script, or call `set_time_limit($seconds)` to extend it at runtime:

```php
// at the top of a slow script
set_time_limit(300);
```

For SSE or streaming endpoints where connections must stay open indefinitely, disable the timer for that script:

```php
set_time_limit(0);
```

### Connections are dropped before headers arrive

The header timeout is too short for clients on high-latency links or behind slow load balancers.

**Fix:** Increase the header timeout:

```bash
HEADER_TIMEOUT_SECONDS=15
```

### OPcache causes the first request after a script change to timeout

OPcache recompilation adds latency on the first request after a file change. This is more common in development environments with many files. Raise `max_execution_time` or disable it during development.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "8080:8080"
    environment:
      HEADER_TIMEOUT_SECONDS: "5"
    volumes:
      - ./app:/var/www/html:ro
      - ./php.ini:/usr/local/etc/php/conf.d/zz-app.ini:ro
```

With `php.ini` containing:

```ini
max_execution_time = 30
```

## Best Practices

- **Never set `max_execution_time = 0` globally in production** unless you have SSE or long-polling endpoints that require indefinite connections. Prefer `set_time_limit(0)` per-script.
- **Use shorter limits for API servers.** APIs have predictable response times. A 30-second `max_execution_time` catches stuck requests quickly without affecting normal traffic.
- **Combine with rate limiting.** Timeouts protect individual requests; rate limiting protects against high request volume. Together they provide comprehensive protection against slow and fast attack patterns.

## See Also

- [Rate Limiting](rate-limiting.md) -- per-IP request rate limiting
- [Server-Sent Events (SSE)](sse.md) -- guidance on disabling timeouts for streaming endpoints
- [Configuration Reference](../operations/configuration.md) -- complete environment variable reference
