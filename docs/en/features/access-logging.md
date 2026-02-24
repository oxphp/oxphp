---
title: Access Logging
description: Structured JSON access logs via the tracing framework
---

OxPHP emits a structured JSON log entry for every completed HTTP request. Logs are written to stdout using a non-blocking background writer, so log I/O never blocks the request pipeline.

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `ACCESS_LOG` | Access log level: `all`, `error`, or empty/unset (off) | *(off)* |
| `LOG_LEVEL` | Minimum log level (trace, debug, info, warn, error) | `info` |

Access logging is disabled by default. Set `ACCESS_LOG` to control verbosity:

- **`all`** — log every completed request (method, path, status, duration)
- **`error`** — only log error responses (HTTP status >= 400: 404, 403, 500, etc.)
- **empty/unset** — no access log entries

```bash
# Log all requests
ACCESS_LOG=all

# Log only errors (4xx/5xx)
ACCESS_LOG=error

# Disable access logging (default)
# ACCESS_LOG=
```

The `RUST_LOG` environment variable is also supported and takes precedence over `LOG_LEVEL` when set. This follows the standard `tracing`/`env_filter` convention.

## Log format

Every access log entry is a single JSON line written to stdout:

```json
{
  "timestamp": "2026-02-11T12:34:56.789Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc00000042",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1234,
    "remote_addr": "10.0.0.1:54321",
    "message": "request completed"
  }
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `request_id` | string | Unique request identifier (see [Request IDs](/features/request-ids/)) |
| `method` | string | HTTP method (`GET`, `POST`, etc.) |
| `path` | string | Request URI path |
| `status` | number | HTTP response status code |
| `duration_us` | number | Total request handling time in microseconds |
| `remote_addr` | string | Client IP address and port |

## How it works

Access logging is implemented as an event handler that listens for `RequestComplete` events at priority **100** (runs last among handlers at the same priority). The handler emits a `tracing::info!` call with the `access_log` target.

The logging infrastructure uses:

- **tracing** for structured event emission
- **tracing-subscriber** with JSON formatting for output
- **tracing-appender** with a non-blocking writer for async I/O

The non-blocking writer spawns a dedicated background thread that buffers log writes and flushes to stdout. The `WorkerGuard` returned by initialization must be held until shutdown to ensure all buffered entries are flushed.

## Log targets

OxPHP uses different tracing targets for different log types:

- `access_log` -- per-request access log entries
- Default target -- server lifecycle events, errors, warnings

You can use `RUST_LOG` to control these independently:

```bash
# Show access logs at info, suppress other info-level messages
RUST_LOG=warn,access_log=info
```

## Integration with log aggregators

Since logs are JSON lines on stdout, they integrate directly with container log drivers and aggregation tools:

- **Docker**: collected automatically via the container's log driver
- **Kubernetes**: picked up by the node's log agent (Fluentd, Fluent Bit, etc.)
- **journald**: captured when running as a systemd service with stdout logging

No sidecar or file-based log shipping is needed.

## See Also

- [Request IDs](request-ids.md) -- how request IDs are generated and passed through
- [Rate Limiting](rate-limiting.md) -- rate-limited requests still appear in access logs
