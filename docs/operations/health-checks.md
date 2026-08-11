---
title: Health Checks
description: Internal server endpoints for health monitoring, Kubernetes probes, Prometheus metrics, and runtime configuration inspection.
---

# Health Checks

OxPHP provides an internal HTTP server on a separate port for health monitoring, metrics collection, and configuration inspection. This server is isolated from application traffic so that monitoring does not compete with user requests.

## Setup

Set `INTERNAL_ADDR` to enable the internal server:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

When `INTERNAL_ADDR` is not set, the internal server does not start and no health endpoints are available.

> **Note:** A port-only `INTERNAL_ADDR` (`:9090` or `9090`) binds `127.0.0.1`; use an explicit `0.0.0.0:9090` to expose the internal server off-host. When it is reachable off-host, restrict access with `INTERNAL_ALLOW_IPS` (a CIDR/IP allow-list — `/metrics`, `/config`, and plugin paths return `403` to peers outside it, while health probes stay reachable); loopback is not implicit, so list `127.0.0.1/32` to keep localhost access. The server warns at startup if the listener is exposed with no allow-list set.

## Kubernetes Probes

OxPHP provides dedicated endpoints for each Kubernetes probe type. Each endpoint is also available under a short alias (`/healthz`, `/readyz`, `/startupz`).

| Endpoint | Alias | Checks | 200 | 503 |
|----------|-------|--------|-----|-----|
| `/health/liveness` | `/healthz` | None (alive if responding) | Always | Never |
| `/health/readiness` | `/readyz` | Not shutting down, executor healthy, no failed plugins | Ready | Not ready |
| `/health/startup` | `/startupz` | Executor healthy | Ready | Not ready |

**Liveness** always returns `200 OK`. If the process can respond to the HTTP request, it is alive. No executor or plugin checks are performed — this prevents Kubernetes from restarting pods due to transient worker pool issues.

**Readiness** returns `503 Service Unavailable` when:
- The server is shutting down (graceful shutdown in progress)
- The PHP worker pool is unhealthy
- Any plugin reports a failure

During graceful shutdown, readiness immediately returns `503`, causing Kubernetes to remove the pod from Service endpoints before the drain completes.

**Startup** returns `503 Service Unavailable` when the executor is not yet ready. Use this probe to prevent premature liveness kills during slow initialization.

All probe endpoints return `Content-Type: text/plain` with the probe name as the body (e.g., `readiness`). Kubernetes only inspects the HTTP status code.

```bash
# Quick check
curl -s -o /dev/null -w '%{http_code}' http://localhost:9090/health/readiness
```

## GET /health

Returns the full server health status as JSON. Use this for dashboards and monitoring systems, not for Kubernetes probes.

```bash
curl http://localhost:9090/health
```

**Healthy response (200 OK):**

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

**Degraded response (503 Service Unavailable):**

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": false,
  "plugins": {}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `"ok"` when all subsystems are healthy, `"degraded"` otherwise |
| `uptime_secs` | integer | Seconds since the server started |
| `total_requests` | integer | Total HTTP requests processed on the main port |
| `active_connections` | integer | Currently open connections on the main port |
| `executor_healthy` | boolean | Whether the PHP worker pool is accepting requests |
| `plugins` | `object<string, string>` | Per-plugin health: keys are plugin names, values are `"ok"`, `"degraded"`, or `"failed"`. Empty `{}` when no plugins report health. A `"failed"` plugin causes the HTTP status to switch to 503; `"degraded"` appears here but keeps the status at 200. |

## GET /metrics

Returns Prometheus-compatible metrics in text exposition format. See [Prometheus Metrics](metrics.md) for the full metric reference.

```bash
curl http://localhost:9090/metrics
```

## GET /config

Returns the active server configuration as JSON. TLS certificate and key paths, `internal_addr`, and `error_pages_dir` are scrubbed from the response for security.

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
  "queue_wait_timeout_ms": 1000,
  "queue_max_waiting": 1024,
  "queue_max_waiting_bytes": 67108864,
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

> **Note:** TLS certificate and key paths are never emitted (`tls_enabled` indicates whether TLS is active), and `internal_addr` and `error_pages_dir` are scrubbed from the served response.

## Kubernetes Integration

Use dedicated probe endpoints for each probe type:

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:latest
          env:
            - name: INTERNAL_ADDR
              value: "0.0.0.0:9090"
          ports:
            - containerPort: 8080
            - containerPort: 9090
          startupProbe:
            httpGet:
              path: /health/startup
              port: 9090
            initialDelaySeconds: 1
            periodSeconds: 2
            failureThreshold: 15
          livenessProbe:
            httpGet:
              path: /health/liveness
              port: 9090
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health/readiness
              port: 9090
            periodSeconds: 5
            failureThreshold: 2
```

| Probe | Effect on failure |
|-------|-------------------|
| Startup | Kubernetes waits — does not kill the pod during initialization |
| Liveness | Kubernetes restarts the pod |
| Readiness | Kubernetes removes the pod from Service endpoints (no restart) |

The short aliases (`/healthz`, `/readyz`, `/startupz`) are fully equivalent and can be used instead.

## Docker Compose Health Check

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:latest
    ports:
      - "8080:80"
    environment:
      INTERNAL_ADDR: "127.0.0.1:9090"
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

Docker marks the container as `unhealthy` after the configured number of retries fail, which can trigger restart policies or load balancer removal.

## See Also

- [Prometheus Metrics](metrics.md) — full reference for all exposed metrics
- [Graceful Shutdown](graceful-shutdown.md) — how health probes interact with shutdown draining
- [Configuration Reference](configuration.md) — all environment variables including `INTERNAL_ADDR`
