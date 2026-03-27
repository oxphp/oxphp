---
title: Health Checks
description: Internal server endpoints for health monitoring, Prometheus metrics, and runtime configuration inspection.
---

# Health Checks

OxPHP provides an internal HTTP server on a separate port for health monitoring, metrics collection, and configuration inspection. This server is isolated from application traffic so that monitoring does not compete with user requests.

## Setup

Set `INTERNAL_ADDR` to enable the internal server:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

When `INTERNAL_ADDR` is not set, the internal server does not start and no health endpoints are available.

> **Note:** Bind to `127.0.0.1` in production unless the internal server is behind a firewall. The `/config` endpoint exposes operational details that should not be public.

## GET /health

Returns the server health status as JSON. Use this endpoint for readiness and liveness probes.

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

**HTTP status codes:**

| Code | Meaning |
|------|---------|
| `200 OK` | All subsystems are healthy |
| `503 Service Unavailable` | PHP worker pool is degraded or unavailable, or a plugin reports a failure |

The `/health` endpoint is lightweight — it reads in-memory counters with no disk I/O, database access, or PHP execution.

## GET /metrics

Returns Prometheus-compatible metrics in text exposition format. See [Prometheus Metrics](metrics.md) for the full metric reference.

```bash
curl http://localhost:9090/metrics
```

## GET /config

Returns the active server configuration as JSON. TLS certificate and key paths are omitted for security.

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:80",
  "document_root": "/var/www/html/public",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode": false,
  "worker_file": null,
  "worker_max_requests": 0,
  "worker_max_memory_mib": 0,
  "static_cache_ttl": 2592000,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": false,
  "plugins": {}
}
```

> **Note:** TLS certificate and key paths are omitted. The `tls_enabled` boolean indicates whether TLS is active.

## Kubernetes Integration

Use the `/health` endpoint for both liveness and readiness probes:

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:0.2.0
          env:
            - name: INTERNAL_ADDR
              value: "0.0.0.0:9090"
          ports:
            - containerPort: 8080
            - containerPort: 9090
          livenessProbe:
            httpGet:
              path: /health
              port: 9090
            initialDelaySeconds: 5
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health
              port: 9090
            initialDelaySeconds: 2
            periodSeconds: 5
            failureThreshold: 2
```

A `503` response from `/health` causes Kubernetes to remove the pod from the Service endpoint list (readiness) or restart it (liveness), depending on the probe type.

## Docker Compose Health Check

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.2.0
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
