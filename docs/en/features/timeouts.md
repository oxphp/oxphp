---
title: Timeouts
description: Configure header read and request processing timeouts in OxPHP to protect against slow clients and runaway PHP scripts.
---

# Timeouts

OxPHP enforces two independent timeouts that protect against slow clients and runaway requests. The header timeout guards the connection phase, while the request timeout bounds total processing time including PHP execution.

## How It Works

Each request passes through two sequential timeout phases:

1. **Connection accepted** — the header timeout starts. OxPHP waits for the client to send a complete set of HTTP headers.
2. **Headers received** — the header timeout ends. The request timeout starts and covers routing, PHP execution, and response building.
3. **PHP processes the request** — application code runs within the request timeout boundary.
4. **Response sent** — both timeouts have elapsed. On keep-alive connections, the cycle repeats from step 1.

```text
TCP connect (+ TLS handshake if enabled)
  |
  +-- [HEADER_TIMEOUT_SECONDS] --> headers received
                                   |
                                   +-- [REQUEST_TIMEOUT_SECONDS] --> response sent
                                                                    |
                                                                    +-- next request (keep-alive)
```

On keep-alive connections, both timeouts apply independently to each request in the connection.

> **Note:** When TLS is enabled, the header timeout starts after the TLS handshake completes, not when the TCP connection is accepted.

The header timeout protects against slowloris-style attacks, where a client sends headers one byte at a time to hold connections open indefinitely. The request timeout ensures server resources are reclaimed even if PHP does not respect its own `max_execution_time` setting.

When the request timeout fires, OxPHP returns a `504 Gateway Timeout` response and logs a warning with the request ID and configured timeout value.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | Maximum seconds to receive request headers after the connection is accepted. Protects against slowloris attacks. Set to `0` to disable |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Maximum seconds for total request processing including PHP execution. Returns `504` on timeout. Set to `0` to disable |

## Recommended Values

| Scenario | Header Timeout | Request Timeout |
|----------|----------------|-----------------|
| API server | 5s | 30s |
| General web serving | 5s | 60s |
| File uploads | 10s | 300s |
| SSE / long-polling | 5s | 0 (disabled) |

Adjust these values based on your application's characteristics. If your PHP scripts perform long-running operations such as report generation or data imports, increase the request timeout accordingly. For SSE endpoints, disable the request timeout so long-lived connections are not terminated prematurely.

## Troubleshooting

### Clients receive 504 Gateway Timeout unexpectedly

The request timeout is firing before PHP finishes executing.

**Check:** Query the `/config` endpoint to confirm the active timeout value:

```bash
curl http://localhost:9090/config | grep request_timeout
```

**Fix:** Increase the timeout or investigate which PHP operation is slow. For known long-running operations, raise the limit:

```bash
REQUEST_TIMEOUT_SECONDS=300
```

For SSE or streaming endpoints where connections must stay open indefinitely, disable the timeout:

```bash
REQUEST_TIMEOUT_SECONDS=0
```

### Connections are dropped before headers arrive

The header timeout is too short for clients on high-latency links or behind slow load balancers.

**Fix:** Increase the header timeout:

```bash
HEADER_TIMEOUT_SECONDS=15
```

### OPcache causes the first request after a script change to timeout

OPcache recompilation adds latency on the first request after a file change. This is more common in development environments with many files. Raise the timeout or disable it during development.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
    ports:
      - "8080:8080"
    environment:
      HEADER_TIMEOUT_SECONDS: "5"
      REQUEST_TIMEOUT_SECONDS: "30"
    volumes:
      - ./app:/var/www/html:ro
```

## Best Practices

- **Never disable the request timeout in production** unless you have SSE or long-polling endpoints that require indefinite connections. A missing timeout allows runaway PHP scripts to occupy workers indefinitely.
- **Use shorter timeouts for API servers.** APIs have predictable response times. A 30-second request timeout catches stuck requests quickly without affecting normal traffic.
- **Combine with rate limiting.** Timeouts protect individual requests; rate limiting protects against high request volume. Together they provide comprehensive protection against slow and fast attack patterns.

## See Also

- [Rate Limiting](rate-limiting.md) -- per-IP request rate limiting
- [Server-Sent Events (SSE)](sse.md) -- guidance on disabling timeouts for streaming endpoints
- [Configuration Reference](../operations/configuration.md) -- complete environment variable reference
