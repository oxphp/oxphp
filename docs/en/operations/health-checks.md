---
title: Health Checks
description: Internal server endpoints for health monitoring and container orchestration
---

OxPHP runs an internal HTTP server on a separate port for health checks, metrics, and configuration inspection. This server is isolated from the main traffic port so that monitoring traffic does not compete with application requests.

## Enabling the Internal Server

Set the `INTERNAL_ADDR` environment variable to start the internal server:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

When this variable is not set, the internal server does not start.

## Endpoints

### `GET /health`

Returns the server health status as JSON.

```bash
curl http://localhost:9090/health
```

**Response (healthy):**

```json
{
  "status": "ok",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {}
}
```

**Response (degraded):**

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": false,
  "plugins": {
    "example_plugin": "failed"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `status` | `string` | `"ok"` when all subsystems are healthy, `"degraded"` otherwise |
| `uptime_secs` | `integer` | Seconds since the server started |
| `total_requests` | `integer` | Total HTTP requests processed on the main port |
| `active_connections` | `integer` | Currently open connections on the main port |
| `executor_healthy` | `boolean` | Whether the PHP worker pool is accepting requests |
| `plugins` | `object` | Health status of each loaded plugin. Values are `"healthy"` or `"failed"` |

**HTTP status codes:**

| Code | Meaning |
|------|---------|
| `200 OK` | Executor and all plugins are healthy |
| `503 Service Unavailable` | Executor or any plugin reports a failed state |

The `executor_healthy` check calls the `is_healthy()` method on the PHP executor. If the worker pool has shut down or is otherwise unable to process requests, this returns `false`. Additionally, if any plugin reports `Failed` health status, the overall status is `"degraded"` and the endpoint returns 503.

### `GET /metrics`

Returns Prometheus-compatible metrics in text exposition format. See the [Metrics](metrics.md) page for the full metric reference. Plugins can contribute additional metrics to this output.

```bash
curl http://localhost:9090/metrics
```

### `GET /config`

Returns the active server configuration as JSON. Sensitive values (TLS key paths) are redacted. Plugin configuration is included under the `plugins` key.

```bash
curl http://localhost:9090/config
```

```json
{
  "listen_addr": "0.0.0.0:8080",
  "document_root": "/var/www/html",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_secs": 30,
  "header_timeout_secs": 5,
  "idle_timeout_secs": 60,
  "request_timeout_secs": 120,
  "rate_limit": 100,
  "rate_window": 60,
  "tls_enabled": true,
  "error_pages_dir": "/etc/oxphp/error-pages",
  "compression": true,
  "access_log": true,
  "plugins": {}
}
```

### Plugin Internal Routes

Paths starting with `/__` are reserved for plugin-defined internal endpoints. If no plugin handles the path, a `404 Not Found` response is returned.

Any other path returns `404 Not Found`.

## Container Health Checks

### Docker

```yaml
# docker-compose.yml
services:
  oxphp:
    image: oxphp:latest
    environment:
      INTERNAL_ADDR: "127.0.0.1:9090"
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

### Dockerfile HEALTHCHECK

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=5s \
  CMD wget -qO- http://127.0.0.1:9090/health || exit 1
```

### Kubernetes

```yaml
# Liveness probe — restarts the pod if the server is unresponsive
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3

# Readiness probe — removes the pod from the service if degraded
readinessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 2
  periodSeconds: 5
  failureThreshold: 2
```

For Kubernetes, use the `executor_healthy` field and the HTTP status code to drive readiness. A `503` response means the PHP worker pool or a plugin is degraded, and the pod should be removed from the service's endpoint list until it recovers.

## Load Balancer Integration

Most load balancers support HTTP health checks. Point them at the internal port:

| Load Balancer | Health Check Target |
|---------------|-------------------|
| AWS ALB/NLB | `http://instance:9090/health` |
| HAProxy | `option httpchk GET /health` on port 9090 |
| nginx upstream | `proxy_pass http://backend:9090/health` |
| Traefik | `traefik.http.services.oxphp.loadbalancer.healthcheck.path=/health` |

The `/health` endpoint is lightweight --- it reads atomic counters and calls `is_healthy()` on the executor. There is no disk I/O, database access, or PHP execution involved.

## Security Considerations

The internal server binds to `127.0.0.1` by default, making it accessible only from the local machine. If you need to expose it to a monitoring network, bind to a specific interface:

```bash
# Accessible from the monitoring network
INTERNAL_ADDR=10.0.1.5:9090
```

Do **not** bind the internal server to `0.0.0.0` in production unless it is behind a firewall or network policy that restricts access. The `/config` endpoint exposes operational details that should not be public.

## See Also

- [Metrics](metrics.md) --- full reference for Prometheus-compatible metrics
- [Configuration](configuration.md) --- all environment variables and their defaults
- [Graceful Shutdown](graceful-shutdown.md) --- how health checks interact with shutdown draining
