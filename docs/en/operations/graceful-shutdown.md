---
title: Graceful Shutdown
description: How OxPHP handles shutdown signals and drains connections
---

OxPHP implements graceful shutdown to ensure in-flight requests complete before the process exits. This is essential for zero-downtime deployments, rolling updates, and container orchestration.

## Signal Handling

OxPHP listens for two shutdown signals:

| Signal | Source | Behavior |
|--------|--------|----------|
| `SIGTERM` | Container orchestrators, `kill`, `docker stop` | Initiates graceful shutdown |
| `SIGINT` | Terminal Ctrl+C | Initiates graceful shutdown |

Both signals trigger the same shutdown sequence. The first signal received starts the drain process. The server does not require a second signal to force-quit.

## Shutdown Sequence

When a shutdown signal is received, the server executes these steps in order:

1. **Shut down plugins** --- `PluginManager::shutdown_all()` is called, shutting down plugins in reverse priority order from initialization.

2. **Stop accepting connections** --- the `shutdown` flag is set via an `AtomicBool`, and the accept loop exits on the next iteration.

3. **Drain in-flight connections** --- the server waits for all active connections to complete, checking every 100ms.

4. **Enforce the drain timeout** --- if connections are still active after `DRAIN_TIMEOUT_SECS`, the server logs a warning and proceeds with shutdown. Remaining connections are dropped.

5. **Abort the internal server** --- the health/metrics server task is cancelled.

6. **Shut down the PHP executor** --- when the `SapiExecutor` is dropped, it closes the request channel, joins all worker threads, and calls `php_module_shutdown()`.

7. **Exit** --- the process exits with status 0.

```
SIGTERM received
  ├── plugin_manager.shutdown_all() (reverse priority order)
  ├── server.shutdown() (stop accepting new connections)
  ├── drain loop (wait for active connections, 100ms poll)
  │   ├── all drained → "All connections drained"
  │   └── timeout reached → "Drain timeout reached, forcing shutdown"
  ├── abort internal server
  ├── drop executor (close channel, join workers, PHP shutdown)
  └── exit
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DRAIN_TIMEOUT_SECS` | `30` | Maximum seconds to wait for in-flight connections to complete |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent connections (enforced by a Tokio semaphore) |

### Choosing a Drain Timeout

Set `DRAIN_TIMEOUT_SECS` to accommodate your slowest expected request:

- **API servers** with fast responses: `10`--`15` seconds
- **Applications** with file uploads or long queries: `30`--`60` seconds
- **Batch processing** endpoints: match your longest expected operation

In Kubernetes, set `DRAIN_TIMEOUT_SECS` to less than the pod's `terminationGracePeriodSeconds` to ensure the drain completes before the kubelet sends `SIGKILL`:

```yaml
spec:
  terminationGracePeriodSeconds: 45
  containers:
    - name: oxphp
      env:
        - name: DRAIN_TIMEOUT_SECS
          value: "30"
```

## Connection Limiting

OxPHP uses a Tokio `Semaphore` to enforce the `MAX_CONNECTIONS` limit. Each accepted connection acquires a permit. When all permits are taken, new connections wait in the TCP backlog until a permit is released.

### ConnectionGuard

Active connections are tracked through a RAII guard pattern. When a connection is accepted, `Metrics::connection_opened()` increments the active connection counter. When the `ConnectionGuard` is dropped (the connection handler returns or the task is cancelled), `Metrics::connection_closed()` decrements it automatically.

```
TCP accept
  ├── acquire semaphore permit
  ├── connection_opened() (increment counter)
  ├── serve HTTP requests (may be multiple via keep-alive)
  └── drop ConnectionGuard → connection_closed() (decrement counter)
      └── drop permit (release semaphore slot)
```

This guarantees the counter is always accurate, even when connections are dropped due to errors or timeouts.

## PHP Worker Shutdown

When the `SapiExecutor` is dropped:

1. The global shutdown flag is set, signaling the ScaleManager (if running) to stop.
2. The request channel sender is dropped, which causes the bounded channel to close.
3. Each worker's per-thread shutdown flag is set, and the main thread joins each worker.
4. Workers in static mode see the closed channel on their next `recv()` and exit. Workers in dynamic mode see either the closed channel or the shutdown flag on their next `recv_timeout()`.
5. After all workers have joined, `php_module_shutdown()`, `sapi_shutdown()`, and `tsrm_shutdown()` are called to cleanly tear down the PHP engine.

This means PHP scripts in progress are allowed to complete. No request is interrupted mid-execution.

## Docker

Docker sends `SIGTERM` when you run `docker stop`. The default Docker stop timeout is 10 seconds, after which Docker sends `SIGKILL`.

To give OxPHP enough time to drain, increase the Docker stop timeout:

```bash
docker stop --time 45 oxphp
```

Or set it in `docker-compose.yml`:

```yaml
services:
  oxphp:
    stop_grace_period: 45s
    environment:
      DRAIN_TIMEOUT_SECS: "30"
```

## Kubernetes

For rolling updates in Kubernetes, the shutdown flow is:

1. Kubernetes sends `SIGTERM` to the pod.
2. The pod is removed from the Service's endpoint list (readiness probe starts failing).
3. OxPHP drains in-flight connections.
4. If the pod is still running after `terminationGracePeriodSeconds`, Kubernetes sends `SIGKILL`.

### Pre-Stop Hook

If your service receives traffic from external load balancers that propagate endpoint changes slowly, add a pre-stop hook to delay shutdown:

```yaml
lifecycle:
  preStop:
    exec:
      command: ["sleep", "5"]
```

This gives the load balancer time to remove the pod from its target list before OxPHP stops accepting connections.

## Monitoring Shutdown

The server logs structured JSON messages during the shutdown sequence:

```json
{"level":"INFO","message":"Received shutdown signal, draining connections"}
{"level":"INFO","message":"Draining in-flight connections","active_connections":3}
{"level":"INFO","message":"All connections drained"}
{"level":"INFO","message":"Server stopped"}
```

If the drain timeout is reached before all connections finish:

```json
{"level":"WARN","message":"Drain timeout reached, forcing shutdown","remaining_connections":1}
```

You can use these log messages to set up alerts if the server regularly hits the drain timeout, which may indicate `DRAIN_TIMEOUT_SECS` needs to be increased or long-running requests need investigation.

## See Also

- [Configuration](configuration.md) --- `DRAIN_TIMEOUT_SECS`, `MAX_CONNECTIONS`, and other environment variables
- [Health Checks](health-checks.md) --- how readiness probes interact with graceful shutdown
- [Metrics](metrics.md) --- `oxphp_active_connections` tracks connections during drain
- [Worker Pool](/architecture/worker-pool.md) --- how PHP workers shut down and join
