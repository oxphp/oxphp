---
title: Graceful Shutdown
description: How OxPHP handles SIGTERM and SIGINT signals to drain connections and shut down cleanly, with Kubernetes and Docker configuration examples.
---

# Graceful Shutdown

OxPHP handles `SIGTERM` and `SIGINT` signals to ensure in-flight requests complete before the process exits. This is essential for zero-downtime deployments and rolling updates in container orchestration.

## Signal Handling

OxPHP responds to two shutdown signals:

| Signal | Source | Behavior |
|--------|--------|----------|
| `SIGTERM` | Container orchestrators, `docker stop`, `kill` | Initiates graceful shutdown |
| `SIGINT` | Terminal Ctrl+C | Initiates graceful shutdown |

Both signals trigger the same shutdown sequence. Only the first signal is needed — the server begins draining immediately.

## Shutdown Sequence

When a shutdown signal is received, OxPHP follows this sequence:

1. **Stop accepting new connections** — the server stops accepting new TCP connections on the main port and shuts down plugins. PHP workers continue running to process in-flight requests.
2. **Drain in-flight requests** — active connections are allowed to finish processing. The server checks for completion every 100ms. The internal health/metrics server remains available throughout the drain, so readiness probes continue to work.
3. **Enforce drain timeout** — if connections remain active after `DRAIN_TIMEOUT_SECONDS`, the server logs a warning and proceeds. Remaining connections are dropped.
4. **Shut down async pool** — the background async task pool is stopped.
5. **Abort the internal server** — the health/metrics server is stopped after the drain completes.
6. **Exit** — the process exits with status code 0.

> **Note:** PHP workers shut down implicitly when the request queue closes. Worker threads exit after finishing any in-progress request.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DRAIN_TIMEOUT_SECONDS` | `30` | Maximum seconds to wait for in-flight connections to complete before forcing shutdown |

Set `DRAIN_TIMEOUT_SECONDS` to accommodate your slowest expected request:

- **API servers** with fast responses: `10`–`15` seconds
- **Applications** with file uploads or long queries: `30`–`60` seconds
- **Worker mode** with background processing: match your longest expected operation

## Kubernetes

In Kubernetes, the shutdown flow during a rolling update is:

1. Kubernetes sends `SIGTERM` to the pod.
2. The pod is removed from the Service endpoint list.
3. OxPHP drains in-flight connections within `DRAIN_TIMEOUT_SECONDS`.
4. If the pod is still running after `terminationGracePeriodSeconds`, Kubernetes sends `SIGKILL`.

Set `DRAIN_TIMEOUT_SECONDS` lower than `terminationGracePeriodSeconds` to ensure the drain completes before the forced kill:

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      terminationGracePeriodSeconds: 45
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:0.5.0
          env:
            - name: DRAIN_TIMEOUT_SECONDS
              value: "30"
```

### Pre-Stop Hook

If your service receives traffic from external load balancers that propagate endpoint changes slowly, add a pre-stop hook to delay the shutdown sequence:

```yaml
lifecycle:
  preStop:
    exec:
      command: ["sleep", "5"]
```

This gives the load balancer time to remove the pod from its target list before OxPHP stops accepting connections.

## Docker

Docker sends `SIGTERM` when you run `docker stop`. The default Docker stop timeout is 10 seconds, after which Docker sends `SIGKILL`.

To give OxPHP enough time to drain, increase the stop timeout:

```bash
docker stop --time 45 my-oxphp-container
```

Or set it in your Compose file:

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.5.0
    stop_grace_period: 45s
    environment:
      DRAIN_TIMEOUT_SECONDS: "30"
```

## Log Messages

During a graceful shutdown, OxPHP emits structured log messages you can monitor:

**Successful drain:**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"INFO","message":"Draining in-flight connections","active_connections":3}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

**Drain timeout reached:**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"WARN","message":"Drain timeout reached, forcing shutdown","remaining_connections":1}
{"level":"INFO","message":"Server stopped"}
```

If you regularly see the "Drain timeout reached" warning, increase `DRAIN_TIMEOUT_SECONDS` or investigate long-running requests using the `oxphp_request_duration_us` histogram.

## See Also

- [Health Checks](health-checks.md) — readiness probes and shutdown interaction
- [Configuration Reference](configuration.md) — all environment variables including `DRAIN_TIMEOUT_SECONDS`
- [Metrics](metrics.md) — `oxphp_active_connections` tracks connections during drain
