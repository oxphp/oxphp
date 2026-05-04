---
title: Rate Limiting
description: Built-in per-IP rate limiting for OxPHP with configurable thresholds, time windows, and standard HTTP rate limit response headers.
---

# Rate Limiting

OxPHP includes a built-in per-IP rate limiter that requires no external dependencies or infrastructure. When enabled, it tracks request counts per client IP and returns a `429 Too Many Requests` response when a client exceeds the configured threshold.

## How It Works

The rate limiter uses a fixed-window counter keyed by client IP address. Each IP gets its own independent counter and window.

1. When a request arrives, OxPHP looks up the client IP in its internal tracker.
2. If no entry exists, or the current window has expired, a new window starts with the counter at zero.
3. The counter increments for each request.
4. If the counter exceeds `RATE_LIMIT`, the server immediately returns a `429` response with rate limit headers. The request is rejected before routing or PHP execution.

Rate-limited requests still appear in access logs and metrics.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `RATE_LIMIT` | `0` | Maximum requests per IP within the window. `0` disables rate limiting entirely with zero overhead |
| `RATE_WINDOW_SECONDS` | `60` | Duration of the rate limit window in seconds |

```bash
# Allow 100 requests per IP per 60-second window
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
```

## Response Headers

Rejected requests return a `429 Too Many Requests` response with the following headers:

| Header | Description |
|--------|-------------|
| `Retry-After` | Seconds until the current window resets |
| `x-ratelimit-limit` | Maximum requests allowed per window |
| `x-ratelimit-remaining` | Requests remaining in the current window (`0` when rate-limited) |
| `x-ratelimit-reset` | Seconds until the current window resets |
| `x-request-id` | Request ID for correlating this response with access logs |

Example `429` response:

```http
HTTP/1.1 429 Too Many Requests
Retry-After: 45
x-ratelimit-limit: 100
x-ratelimit-remaining: 0
x-ratelimit-reset: 45
x-request-id: 67e2a1f412341a2b0042

429 Too Many Requests
```

## Troubleshooting

### Legitimate users are being rate-limited

Your threshold may be too low for real traffic patterns. Check the rate of 429 responses in your metrics and adjust `RATE_LIMIT` or `RATE_WINDOW_SECONDS` accordingly.

**Check** the rate-limited request count:

```bash
curl http://localhost:9090/metrics | grep rate_limited
```

**Fix:** Increase `RATE_LIMIT` or extend `RATE_WINDOW_SECONDS` to give clients more room.

### Users behind a corporate NAT share the same IP counter

OxPHP rate limits by source IP. All users behind a shared NAT or proxy share one counter. If this is causing problems, consider disabling OxPHP's built-in limiter (`RATE_LIMIT=0`) and applying rate limiting at a higher level (e.g. at your load balancer or API gateway) where you have access to user identifiers.

> **Behind a reverse proxy?** Set `TRUSTED_PROXIES` to ensure rate limiting uses the real client IP instead of the proxy's IP. See [Trusted Proxies](../security/trusted-proxies.md).

### Rate limiting is not working across multiple instances

OxPHP's rate limiter is in-memory and per-instance. If you run multiple OxPHP instances behind a load balancer, each tracks its own independent counters. A client can send `RATE_LIMIT` requests to each instance without triggering a 429. For coordinated rate limiting across instances, use an external limiter at the load balancer or API gateway level.

### Memory grows under IP rotation attacks

OxPHP tracks up to 100,000 unique IP addresses. When this limit is reached, expired entries are purged before adding new ones. If you observe memory growth from an attacker rotating IPs rapidly, the automatic cleanup limits the impact to a bounded amount of memory.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "8080:80"
    environment:
      RATE_LIMIT: "100"
      RATE_WINDOW_SECONDS: "60"
    volumes:
      - ./app:/var/www/html:ro
```

## Best Practices

- **Start conservative.** Begin with a lower limit (e.g. 60 requests per minute) and increase based on observed traffic patterns. It is easier to relax limits than to recover from an overloaded server.
- **Use a shared rate limiter for multi-instance deployments.** OxPHP's rate limiter is per-instance. For coordinated limiting across instances, apply rate limiting at the load balancer or API gateway level.
- **Monitor 429 response rates.** Track the ratio of rate-limited requests in your metrics to detect misconfigured thresholds or unexpected traffic spikes.

## Notes

- **Fixed-window algorithm.** The limiter uses a fixed-window counter, not a sliding window. A client can send up to `2x` the configured limit in a burst at the boundary between two windows.
- **Per-IP only.** Rate limiting is keyed by source IP address. There is no support for custom keys such as API key or user ID.
- **In-memory state.** Rate limit counters are not shared across multiple OxPHP instances.
- **Automatic cleanup.** Expired entries are cleaned up when the tracker exceeds 100,000 IPs, removing all entries whose windows have expired.

## See Also

- [Metrics](../operations/metrics.md) — monitor rate-limited request counts via Prometheus
- [Request IDs](request-ids.md) — correlating rate-limited requests in logs
- [Access Logging](access-logging.md) — 429 responses appear in access logs
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
