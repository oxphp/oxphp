---
title: Internal Server
description: A dedicated HTTP server for health checks, Prometheus metrics, live configuration, and plugin endpoints.
---

# Internal Server

OxPHP runs a separate HTTP server on a dedicated port for health checks, Prometheus metrics, and live configuration inspection. This server is completely isolated from the main application listener — it has no TLS, no rate limiting, no request IDs, and no event processing.

## How It Works

1. **Set `INTERNAL_ADDR`** to a listen address (e.g. `127.0.0.1:9090`). OxPHP starts a second HTTP listener on that address
2. The internal server exposes the built-in endpoints below:
   - `/health` — aggregate JSON status
   - `/health/liveness`, `/healthz` (alias) — liveness probe
   - `/health/readiness`, `/readyz` (alias) — readiness probe
   - `/health/startup`, `/startupz` (alias) — startup probe
   - `/metrics` — Prometheus exposition
   - `/config` — active server configuration JSON
3. Plugins can register additional endpoints under the `/__<plugin>/` prefix
4. During graceful shutdown, the internal server stays available until the main server finishes draining connections

> **Note:** The internal server only starts when `INTERNAL_ADDR` is explicitly set. Without it, health, metrics, and config endpoints are not available over HTTP.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `INTERNAL_ADDR` | *(unset)* | Address for the internal server. Not started when unset. A port-only value (`:9090` or `9090`) binds `127.0.0.1`; use an explicit `0.0.0.0:9090` to expose it off-host. Example: `127.0.0.1:9090` |
| `INTERNAL_ALLOW_IPS` | *(unset)* | Comma-separated CIDR/IP allow-list. Peers outside the list receive `403` on `/metrics`, `/config`, and `/__<plugin>/` paths; health probes always stay reachable. Unset/empty = allow all. Loopback is **not** implicit — list `127.0.0.1/32` to keep localhost access. A malformed list aborts startup |

## Endpoints

### GET /health

Returns a JSON health status. Use this for Kubernetes readiness and liveness probes.

**200 OK** — all systems healthy:

```json
{
  "status": "ok",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {
    "otel": "ok"
  }
}
```

**503 Service Unavailable** — a plugin has reported failure:

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {
    "otel": "failed"
  }
}
```

The `plugins` object lists every loaded plugin with its health status: `"ok"`, `"degraded"`, or `"failed"`. The endpoint returns 503 when any plugin reports `"failed"` **or** the script executor is unhealthy (`executor_healthy: false`). `"degraded"` plugins appear in the JSON body but the HTTP status remains 200.

### GET /metrics

Returns Prometheus-compatible metrics in text format (`text/plain; version=0.0.4; charset=utf-8`). Always returns 200.

```bash
curl http://localhost:9090/metrics
```

```text
# HELP oxphp_requests_total Total HTTP requests
# TYPE oxphp_requests_total counter
oxphp_requests_total 48203
# HELP oxphp_active_connections Current open connections
# TYPE oxphp_active_connections gauge
oxphp_active_connections 7
...
```

For the full list of available metrics, see [Metrics](../operations/metrics.md).

### GET /config

Returns the active server configuration as JSON. Always returns 200.

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:80",
  "document_root": "/var/www/html/public",
  "entry_file": "/var/www/html/public/index.php",
  "log_level": "info",
  "executor_type": "sapi",
  "php_workers": "8",
  "tokio_workers": 4,
  "queue_capacity": 1024,
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode_enabled": false,
  "worker_max_memory_mib": 0,
  "static_max_age": 2592000,
  "static_revalidate": false,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "async_max_fibers": 256,
  "async_in_flight_cap": 0,
  "trace_context": false,
  "superglobals_enabled": true,
  "trusted_proxies": false,
  "plugins": {}
}
```

TLS certificate and key file paths are never emitted (only the `tls_enabled` boolean is exposed), and `internal_addr` and `error_pages_dir` are scrubbed from the served response — deployment topology and filesystem paths that aid an attacker and are not needed by scrapers.

### Plugin Endpoints

Plugins can register custom endpoints under the `/__<plugin_name>/` prefix. For example, a plugin named `otel` could expose `/__otel/status`. These endpoints are only available if the corresponding plugin is loaded and registered a handler.

Any path not matching a built-in or plugin endpoint returns `404 Not Found`.

## Kubernetes Integration

### Readiness Probe

Use `/health` to control whether Kubernetes routes traffic to the pod:

```yaml
readinessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 2
  periodSeconds: 5
```

When `/health` returns 503, Kubernetes removes the pod from the Service endpoint list. Traffic resumes when the endpoint returns 200 again.

### Liveness Probe

Use the same endpoint to restart pods that become unresponsive:

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3
```

### Startup Probe

For applications with slow bootstrap (large frameworks, heavy autoloaders):

```yaml
startupProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 1
  periodSeconds: 2
  failureThreshold: 15
```

## Security

The internal server has no bearer-token authentication; access is controlled by where it binds and by `INTERNAL_ALLOW_IPS`. To protect it:

- **Bind to localhost** — a port-only `INTERNAL_ADDR` (`:9090`) already binds `127.0.0.1`; an explicit `127.0.0.1:9090` keeps the port reachable only from within the container or host
- **Set `INTERNAL_ALLOW_IPS`** when the listener must be reachable off-host — a CIDR/IP allow-list that returns `403` on `/metrics`, `/config`, and plugin paths to peers outside it, while health probes stay reachable. Loopback is not implicit, so include `127.0.0.1/32` if you still need localhost access. The server warns at startup if the listener is exposed off-host with no allow-list set
- **Do not expose as a Kubernetes Service** — declare the port as a `containerPort` but do not create a Service for it. Kubernetes probes access container ports directly
- **Use network policies** — restrict access at the network layer if the port must be exposed

The `/config` endpoint reveals operational details (document root, rate limits, worker counts, timeout values). TLS paths, `internal_addr`, and `error_pages_dir` are scrubbed, but consider whether the rest should be accessible from outside the pod.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.9.0
    ports:
      - "80:80"
      - "9090:9090"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
```

For production, bind the internal server to localhost and use Kubernetes probes:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.9.0
    ports:
      - "80:80"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=127.0.0.1:9090
```

## Troubleshooting

### Health, metrics, and config endpoints not available

`INTERNAL_ADDR` is not set.

**Fix:** Add the environment variable:

```bash
INTERNAL_ADDR=0.0.0.0:9090
```

### /health returns 503 but the application works

A loaded plugin is reporting `PluginHealth::Failed`. Check the `plugins` object in the health response to identify which plugin is failing:

```bash
curl -s http://localhost:9090/health | jq '.plugins'
```

### Cannot reach the internal server from outside the container

The server is bound to `127.0.0.1`, which is only accessible from inside the container.

**Fix:** Change to `0.0.0.0` for external access, or use Kubernetes probes that access the container directly.

### Metrics not showing worker mode or async metrics

Worker mode metrics appear only when worker mode is enabled (`WORKER_MODE_ENABLED=true`). Async metrics appear only when `ASYNC_WORKERS > 0` and at least one task has been dispatched or rejected.

## See Also

- [Metrics](../operations/metrics.md) -- full Prometheus metrics reference
- [Health Checks](../operations/health-checks.md) -- detailed health check behavior
- [Configuration Reference](../operations/configuration.md) -- all environment variables
- [Graceful Shutdown](../operations/graceful-shutdown.md) -- shutdown sequence and internal server lifecycle
