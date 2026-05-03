---
title: Access Logging
description: Structured JSON access logs for every HTTP request in OxPHP, with configurable modes for all requests or errors only.
---

# Access Logging

OxPHP emits structured JSON access logs for every HTTP request, written to stdout. Logging is asynchronous and never blocks request handling.

## How It Works

1. When `ACCESS_LOG` is set, OxPHP writes one JSON line to stdout after each completed request.
2. Log writes are buffered in a background writer thread and never block the request pipeline.
3. `ACCESS_LOG=all` logs every request. `ACCESS_LOG=error` logs only responses with status 400 or higher.
4. Each log line includes a `request_id` field that correlates access log entries with your application logs.
5. When W3C Trace Context propagation is enabled, log entries also include `trace_id` and `span_id` fields.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `ACCESS_LOG` | *(unset)* | Access log mode. `all` logs every request; `error` logs only 4xx and 5xx responses. Leave unset or empty to disable |

> **Note:** The only accepted values are `all` and `error`. Setting an unrecognized value logs a warning and disables access logging.

## Log Format

Every access log entry is a single JSON line written to stdout:

```json
{
  "timestamp": "2026-02-11T12:34:56.789012Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc12341a2b0042",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1234,
    "remote_addr": "10.0.0.1:54321",
    "message": "request completed"
  }
}
```

When W3C Trace Context is active, `trace_id` and `span_id` are included alongside the standard fields:

```json
{
  "timestamp": "2026-02-11T12:34:56.789012Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc12341a2b0042",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "span_id": "00f067aa0ba902b7",
    "method": "POST",
    "path": "/api/orders",
    "status": 201,
    "duration_us": 8421,
    "remote_addr": "10.0.0.1:54322",
    "message": "request completed"
  }
}
```

## Fields

| Field | Type | Description |
|-------|------|-------------|
| `request_id` | string | Unique request identifier. See [Request IDs](request-ids.md) |
| `method` | string | HTTP method (`GET`, `POST`, etc.) |
| `path` | string | Request URI path |
| `status` | number | HTTP response status code |
| `duration_us` | number | Total request handling time in microseconds |
| `remote_addr` | string | Client IP address and port. When `TRUSTED_PROXIES` is configured, shows the real client IP extracted from forwarding headers, not the proxy's IP |
| `trace_id` | string | W3C trace ID (present only when `TRACE_CONTEXT=true`) |
| `span_id` | string | W3C span ID (present only when `TRACE_CONTEXT=true`) |

## Fine-Grained Filtering

OxPHP uses the internal `access_log` logging target for access log entries. Use the `RUST_LOG` variable to filter access logs independently from other log output:

```bash
# Suppress general info messages, keep access logs
RUST_LOG=warn,access_log=info
```

> **Note:** The `access_log` target is used for `RUST_LOG` filtering but is not included in the JSON output. To identify access log entries in downstream systems, use the `"message": "request completed"` field and the characteristic field set (`method`, `path`, `status`, `duration_us`).

## Troubleshooting

### No access log entries appear

Access logging is disabled when `ACCESS_LOG` is unset or empty.

**Fix:** Set the variable to `all` or `error`:

```bash
ACCESS_LOG=all
```

### Access logs show `error` mode but successful requests are missing

`ACCESS_LOG=error` only logs responses with status 400 or higher. This is by design — check the value and switch to `all` if you need all requests logged.

**Check:** Confirm the active setting:

```bash
curl -s http://localhost:9090/config | jq '.access_log'
```

### Log entries appear without `trace_id` and `span_id`

Trace context fields are only present when W3C Trace Context propagation is enabled.

**Fix:** Enable it with:

```bash
TRACE_CONTEXT=true
```

And ensure your upstream client or load balancer sends a `traceparent` header on requests.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
    ports:
      - "80:80"
      - "9090:9090"
    volumes:
      - ./src:/var/www/html:ro
    environment:
      ACCESS_LOG: "all"
      ENTRY_FILE: "index.php"
      INTERNAL_ADDR: "0.0.0.0:9090"
```

## Best Practices

- Use `ACCESS_LOG=error` in production to reduce log volume while still capturing all failed requests. Successful requests are not logged, but errors at any status 400+ are always captured.
- Include the request ID in your application logs via `oxphp_request_id()` so you can correlate PHP-level log entries with access log entries.
- Use a structured log aggregator such as Elasticsearch, Loki, or Datadog to query and filter JSON log lines efficiently.

## Integration

Since logs are JSON lines on stdout, they integrate directly with container log drivers and aggregation tooling:

- **Docker** — collected automatically via the container log driver (json-file, fluentd, and others)
- **Kubernetes** — picked up by the node's log agent (Fluentd, Fluent Bit, Filebeat, and others)
- **systemd** — captured when running as a systemd service with stdout logging via journald

No sidecar or file-based log shipping is needed.

## See Also

- [Request IDs](request-ids.md) -- every log entry includes a `request_id` for tracing
- [Configuration Reference](../operations/configuration.md) -- complete environment variable reference
