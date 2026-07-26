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

1. **Stop accepting new connections** — the server stops accepting new TCP connections on the main port. PHP workers continue running to process in-flight requests.
2. **Wind down live connections** — HTTP/2 clients receive a `GOAWAY` frame and idle HTTP/1.1 keep-alive connections are closed, so clients move to a healthy instance instead of multiplexing new requests into the dying one. Open streams are ended promptly and cleanly — any response that has started flushing chunked output counts, finite downloads as well as SSE — see [Server-Sent Events](../features/sse.md#behaviour-on-shutdown).
3. **Drain in-flight requests** — ordinary active requests are left alone to finish with their full responses. The server checks for completion every 100ms. The internal health/metrics server remains available throughout the drain, so readiness probes continue to work. Work that a request keeps doing after `oxphp_finish_request()` counts as in-flight too, even though its connection is already gone.
4. **Enforce the drain deadline** — requests still running after `DRAIN_TIMEOUT_SECONDS` are cancelled (their `register_shutdown_function()` callbacks still run) and given ~2 seconds to unwind before the server proceeds.
5. **Flush plugins** — access-log entries and APM spans buffered during the drain window are flushed.
6. **Shut down async pool** — the background async task pool is stopped.
7. **Abort the internal server** — the health/metrics server is stopped after the drain completes.
8. **Exit** — the process exits with status code 0.

> **Note:** PHP worker threads are not stopped at the start of the sequence — they are signalled to exit and joined at the very end, once the drain has finished and the process is on its way out. That is why the drain has to account for work still running on a worker even when no client connection is left.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DRAIN_TIMEOUT_SECONDS` | `25` | Maximum seconds in-flight requests get to complete before being cancelled; the process exits within ~2 seconds after the deadline. The default leaves headroom for the post-deadline unwind and telemetry flush inside Kubernetes' default 30-second termination grace period |

Set `DRAIN_TIMEOUT_SECONDS` to accommodate your slowest expected request:

- **API servers** with fast responses: `10`–`15` seconds
- **Applications** with file uploads or long queries: `30`–`60` seconds
- **Worker mode** with background processing: match your longest expected operation

## Kubernetes

In Kubernetes, the shutdown flow during a rolling update is:

1. Kubernetes sends `SIGTERM` to the pod.
2. The pod is removed from the Service endpoint list.
3. OxPHP drains in-flight connections within `DRAIN_TIMEOUT_SECONDS`, then cancels stragglers and exits within ~2 more seconds.
4. If the pod is still running after `terminationGracePeriodSeconds`, Kubernetes sends `SIGKILL`.

Set `terminationGracePeriodSeconds` above `DRAIN_TIMEOUT_SECONDS` + 2 so the drain — including the post-deadline unwind and telemetry flush — completes before the forced kill:

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      terminationGracePeriodSeconds: 45
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:0.10.0
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
    image: ghcr.io/oxphp/oxphp:0.10.0
    stop_grace_period: 45s
    environment:
      DRAIN_TIMEOUT_SECONDS: "30"
```

## Log Messages

During a graceful shutdown, OxPHP emits structured log messages you can monitor:

**Successful drain:**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"INFO","message":"Draining in-flight connections","active_connections":3,"in_flight_requests":4}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

The two counts overlap rather than partition: `active_connections` is live client connections, `in_flight_requests` is requests still executing on PHP workers, and a request holding a connection is counted in both. A request that ended its response early with `oxphp_finish_request()` appears only in `in_flight_requests` — its connection is already gone while its background work runs.

**Drain deadline reached:**

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"WARN","message":"Drain timeout reached, cancelling in-flight requests","remaining_connections":1,"in_flight_requests":1}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

If you regularly see the "Drain timeout reached" warning, increase `DRAIN_TIMEOUT_SECONDS` or investigate long-running requests using the `oxphp_request_duration_us` histogram.

## See Also

- [Health Checks](health-checks.md) — readiness probes and shutdown interaction
- [Configuration Reference](configuration.md) — all environment variables including `DRAIN_TIMEOUT_SECONDS`
- [Metrics](metrics.md) — `oxphp_active_connections` tracks connections during drain
