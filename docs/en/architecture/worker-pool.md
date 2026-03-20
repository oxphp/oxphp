---
title: Worker Pool
description: PHP worker thread pool architecture — static/dynamic scaling, bounded channels, backpressure, and the ScriptExecutor trait
---

OxPHP executes PHP scripts on a pool of dedicated OS threads, isolated from the async I/O runtime. This page covers the `ScriptExecutor` trait, the bounded channel design, backpressure behavior, and automatic worker scaling.

## ScriptExecutor Trait

All PHP execution backends implement the `ScriptExecutor` trait defined in `src/executor/mod.rs`:

```rust
pub trait ScriptExecutor: Send + Sync {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult;

    fn shutdown(&self);

    fn is_healthy(&self) -> bool {
        true
    }

    fn start_scale_manager(&self) {}
}
```

| Method | Purpose |
|---|---|
| `execute()` | Accept a request and return an `ExecuteResult` (immediate or deferred response) |
| `shutdown()` | Signal the executor to stop accepting work |
| `is_healthy()` | Health check for the `/health` internal endpoint |
| `start_scale_manager()` | Start the background scaling task (no-op in stub; static mode spawns a health monitor) |

The trait returns `ExecuteResult` rather than a raw `Future` or `oneshot::Receiver`. This allows the executor to return an error response immediately (e.g., 529 when the queue is full) without involving a worker thread, while still supporting the deferred case where the Tokio task awaits a `oneshot::Receiver` for the response.

```rust
pub enum ExecuteResult {
    Immediate(ScriptResponse),
    Deferred(tokio::sync::oneshot::Receiver<ScriptResponse>),
}
```

## Data Types

Requests and responses are defined in `src/types.rs`:

```rust
pub struct ScriptRequest {
    pub request_id: String,
    pub script_path: PathBuf,
    pub method: Method,
    pub uri: Uri,
    pub query_string: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub remote_addr: SocketAddr,
    pub document_root: Arc<PathBuf>,
}

pub struct ScriptResponse {
    pub status: u16,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub body: Bytes,
    pub execution_time_us: u64,
}
```

The `document_root` is wrapped in `Arc<PathBuf>` for cheap sharing across requests. The `headers` in the response use a `Vec` (not `HeaderMap`) because the PHP worker pre-parses header strings into typed `HeaderName`/`HeaderValue` pairs on the worker thread, avoiding parsing cost on the Tokio runtime.

## SapiExecutor

The production executor (`src/executor/sapi.rs`, feature-gated behind `--features php`) manages a pool of PHP ZTS worker threads connected through a bounded `crossbeam_channel`. It supports two operational modes: **static** (fixed pool size) and **dynamic** (auto-scaling between min/max bounds).

### Architecture

```
                         ┌──────────────────────┐
                         │   crossbeam_channel  │
Tokio tasks ──try_send──▶│   bounded(CAPACITY)  │──recv──▶ php-worker-0
             (non-block) │                      │──recv──▶ php-worker-1
                         │                      │──recv──▶ php-worker-N
                         └──────────────────────┘
                                                          ▲
                         ┌──────────────────────┐         │
                         │   ScaleManager       │─────────┘
                         │   (tokio task,       │  spawn/retire workers
                         │    dynamic mode only)│  based on idle count
                         └──────────────────────┘

Each worker:
  ┌──────────────────────────────────────────────────┐
  │  recv(WorkerRequest)                             │
  │    ├── sapi::clear_buffers()                     │
  │    ├── sapi::set_request_data(request)           │
  │    ├── php_request_startup()                     │
  │    ├── zend_stream_init_filename(file_handle)    │
  │    ├── php_execute_script(file_handle)           │
  │    ├── zend_destroy_file_handle(file_handle)     │
  │    ├── php_request_shutdown()                    │
  │    ├── sapi::take_response() → (output, headers) │
  │    └── tx.send(ScriptResponse)                   │
  └──────────────────────────────────────────────────┘
```

### Worker Modes

The `PHP_WORKERS` environment variable controls the worker pool mode:

| Format | Mode | Example | Behavior |
|--------|------|---------|----------|
| `N` | Static | `PHP_WORKERS=8` | Fixed pool of 8 workers |
| `0` or unset | Static | `PHP_WORKERS=0` | Fixed pool of CPU / 2 workers (min 1) |
| `MIN:MAX` | Dynamic | `PHP_WORKERS=2:16` | Starts at MIN, scales up to MAX under load |
| `MIN:0` | Dynamic | `PHP_WORKERS=2:0` | MIN explicit, MAX auto-detected (CPU count * 2) |
| `0:0` | Dynamic | `PHP_WORKERS=0:0` | MIN auto (CPU/4, min 1), MAX auto (CPU * 2) |

In **static mode**, the pool size never changes after startup. Workers use a blocking `recv()` loop with zero CPU overhead when idle.

In **dynamic mode**, a background ScaleManager task periodically checks worker utilization and spawns or retires workers. Workers use `recv_timeout(200ms)` to allow periodic shutdown-flag checks.

### Startup Sequence

The `SapiExecutor::new(metrics)` constructor performs PHP initialization on the main thread before any worker threads are spawned:

1. **TSRM startup**: `php_tsrm_startup()` initializes Zend Thread Safety. This must happen on the main thread before any async runtime signal handlers are installed.
2. **SAPI registration**: `sapi_startup()` registers the custom `oxphp` SAPI module.
3. **PHP engine start**: `php_module_startup()` initializes the PHP engine, loads extensions, and parses `php.ini`. This triggers MINIT for all extensions, including the OxPHP extension which registers plugin functions with Zend.
4. **Error callback**: `sapi::install_error_cb()` replaces the default error handler with structured JSON logging.
5. **Worker mode parsing**: `parse_php_workers()` reads `PHP_WORKERS` and returns `WorkerMode::Static(n)` or `WorkerMode::Dynamic { min, max }`.
6. **Channel creation**: `crossbeam_channel::bounded(queue_capacity)` creates the bounded work queue. Capacity defaults to `worker_count * 128` (using min for dynamic mode).
7. **Worker spawn**: Initial workers are spawned — the full count for static mode, or `min` for dynamic mode. Each is wrapped in a `ManagedWorker` struct.
8. **Metrics initialization**: `metrics.set_workers_min/max/current` are set to reflect the initial pool state.

### ManagedWorker

Each worker is tracked by a `ManagedWorker` struct:

```rust
struct ManagedWorker {
    id: usize,                       // Unique ID (for debug display)
    handle: JoinHandle<()>,          // OS thread handle
    shutdown: Arc<AtomicBool>,       // Per-worker shutdown flag
    last_active: Arc<AtomicU64>,     // Epoch millis of last request (dynamic only)
}
```

The `shutdown` flag allows individual workers to be retired without closing the shared channel. The `last_active` timestamp is used by the ScaleManager to identify idle workers for scale-down.

### Worker Thread Lifecycle

Each worker thread:

1. Initializes TSRM thread-local storage via `ts_resource_ex()`
2. Enters the receive loop (mode-dependent):
   - **Static mode**: Blocking `while let Ok(wr) = request_rx.recv()` — zero CPU when idle
   - **Dynamic mode**: `recv_timeout(200ms)` with periodic `shutdown` flag checks and `last_active` tracking
3. For each request:
   - Clears output buffers via `sapi::clear_buffers()`
   - Sets request data (SAPI state, superglobals) via `sapi::set_request_data()`
   - Creates a `RequestDataGuard` (RAII — clears SAPI data on drop, even on panic)
   - Calls `php_request_startup()` (triggers RINIT for all extensions)
   - Opens the script file with `zend_stream_init_filename()`
   - Executes with `php_execute_script()`
   - Destroys the file handle with `zend_destroy_file_handle()`
   - Calls `php_request_shutdown()` (triggers RSHUTDOWN)
   - Collects the response: output buffer, headers, status code via `sapi::take_response()`
   - Parses raw header strings into typed `HeaderName`/`HeaderValue` pairs on the worker thread
   - Sends the response through the oneshot channel
4. Exit conditions:
   - **Static mode**: Channel sender is dropped (shutdown), `recv()` returns `Err`
   - **Dynamic mode**: `shutdown` flag set by ScaleManager, or channel disconnected

### ScaleManager (Dynamic Mode)

In **static mode**, `start_scale_manager()` spawns a worker health monitor task rather than a no-op. The health monitor periodically checks for dead workers (workers whose OS thread has exited unexpectedly) and respawns them to maintain the configured target count. This prevents a crashed worker from permanently reducing pool capacity.

When `PHP_WORKERS=MIN:MAX` is configured, `start_scale_manager()` instead spawns an auto-scaling ScaleManager task. The ScaleManager runs on the Tokio runtime and checks worker utilization every 500ms:

**Scale-up** (all conditions must be true):
- Zero idle workers detected (idle = last_active > 200ms ago)
- Current worker count is below MAX
- At least 500ms since the last scale-up

**Scale-down** (all conditions must be true):
- Current worker count is above MIN
- A worker has been idle longer than `PHP_WORKERS_IDLE_SECONDS` (default 30s)
- At least 5 seconds since the last scale-down

The ScaleManager drops the Mutex lock before spawning new OS threads to avoid blocking the Tokio runtime. Retired workers are joined in a background thread.

### Configuration

| Variable | Default | Description |
|---|---|---|
| `PHP_WORKERS` | `0` (CPU / 2, min 1) | Worker pool mode. `N` for static, `MIN:MAX` for dynamic |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Idle timeout before a dynamic worker is retired |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Bounded channel capacity (uses initial count for dynamic) |

## Worker Mode (Persistent PHP)

Worker mode is an alternative execution model where PHP processes stay alive across requests, avoiding the overhead of `php_request_startup()` / `php_request_shutdown()` on every request. This is enabled by setting the `WORKER_FILE` environment variable.

### How It Works

Instead of the standard per-request lifecycle (startup → execute → shutdown), worker mode executes a single PHP script that calls `oxphp_worker()` with a handler callback. The handler is invoked for each request, and between requests a **soft reset** cleans per-request state without destroying the PHP heap:

```
Worker thread lifecycle:

  php_request_startup()           ← runs ONCE
  require worker.php              ← bootstrap: autoload, DB connect, config
  oxphp_worker(function() {       ← enters worker loop
      ┌─────────────────────────┐
      │ wait for request        │ ← blocks on crossbeam channel
      │ soft reset              │ ← repopulate superglobals, clear output
      │ call handler()          │ ← execute user code
      │ send response           │ ← response to HTTP layer
      │ check limits            │ ← max_requests, max_memory
      └─────────────────────────┘
      │ loop back ↑             │
  })
  php_request_shutdown()          ← runs ONCE (on exit)
```

The soft reset between requests:
- Repopulates `$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES` from the new request
- Clears and resets output buffers
- Resets HTTP response headers and status code
- Calls and clears `register_shutdown_function()` handlers

### Recycling

Workers are recycled (exit and respawn) based on configurable limits:

| Exit Reason | Trigger | Metric Label |
|---|---|---|
| `max_requests` | `WORKER_MAX_REQUESTS` reached | `reason="max_requests"` |
| `max_memory` | `WORKER_MAX_MEMORY_MIB` (MiB) exceeded | `reason="max_memory"` |
| `error` | Uncaught exception or fatal error in handler | `reason="error"` |
| `shutdown` | Server graceful shutdown | *(not counted as recycle)* |

When a worker exits for a non-shutdown reason, the health monitor (static mode) or scale manager (dynamic mode) respawns it automatically. The new worker re-executes the entire worker script, including bootstrap code.

### Metrics

Worker mode exposes dedicated Prometheus metrics for monitoring persistent worker health:

- **`oxphp_worker_requests_handled_total`** — total requests processed across all workers
- **`oxphp_worker_recycles_total`** / **`oxphp_worker_recycles_by_reason_total`** — recycle counts, globally and by reason
- **`oxphp_worker_memory_bytes{worker="N"}`** — current PHP heap usage per worker
- **`oxphp_worker_uptime_seconds{worker="N"}`** — time since each worker was spawned
- **`oxphp_worker_request_duration_us`** — histogram of PHP handler execution time (excludes queue wait)

See [Metrics](../operations/metrics.md#worker-mode) for the full reference and PromQL query examples.

### Configuration

| Variable | Default | Description |
|---|---|---|
| `WORKER_FILE` | *(none)* | Path to the worker PHP script (relative to `DOCUMENT_ROOT`). Enables worker mode when set |
| `WORKER_MAX_REQUESTS` | `0` | Maximum requests before recycling. `0` = no limit |
| `WORKER_MAX_MEMORY_MIB` | `0` | Maximum memory (MiB) before recycling. `0` = no limit |

### Routing Integration

When `WORKER_FILE` is set, routing changes: non-static-file requests that don't match a file on disk are routed to the worker script instead of returning 404. Static files (CSS, JS, images) are still served directly from disk. This is similar to nginx's `try_files` directive.

## Bounded Queue and Backpressure

The channel between Tokio and PHP workers uses `crossbeam_channel::bounded(QUEUE_CAPACITY)`. The executor calls `try_send()` (non-blocking) to enqueue requests:

```rust
if let Err(e) = self.request_tx.as_ref().unwrap().try_send(worker_request) {
    let (status, body) = match e {
        TrySendError::Full(_) => (529, "Site is overloaded"),
        TrySendError::Disconnected(_) => (500, "PHP worker pool unavailable"),
    };
    return ExecuteResult::Immediate(ScriptResponse {
        status,
        headers: vec![],
        body: Bytes::from_static(body.as_bytes()),
        execution_time_us: 0,
    });
}
```

| Condition | Behavior |
|---|---|
| Queue has space | Request is enqueued, Tokio task awaits the oneshot response |
| Queue is full | 529 Site is overloaded returned immediately with `Retry-After: 3` header |
| Workers disconnected | 500 Internal Server Error (worker pool is down) |

This design provides backpressure: when PHP workers cannot keep up, new requests are rejected immediately rather than queued indefinitely. The `Retry-After: 3` header signals clients to retry after a brief delay.

### Metrics

The connection handler tracks queue state through the `Metrics` struct:

| Method | When |
|---|---|
| `metrics.request_queued()` | Just before `executor.execute()` |
| `metrics.request_dequeued()` | When the oneshot response arrives |
| `metrics.request_dropped()` | When the oneshot channel is broken (worker crashed) |

These expose as Prometheus gauges/counters: `oxphp_pending_requests`, `oxphp_busy_workers`, `oxphp_dropped_requests_total`.

## StubExecutor

The `StubExecutor` (`src/executor/stub.rs`) is a zero-overhead testing and benchmarking backend. It returns a hardcoded 200 OK response synchronously without spawning any threads:

```rust
impl ScriptExecutor for StubExecutor {
    fn execute(&self, _request: ScriptRequest) -> ExecuteResult {
        ExecuteResult::Immediate(ScriptResponse {
            status: 200,
            headers: vec![(
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("text/plain"),
            )],
            body: Bytes::from_static(b"OK"),
            execution_time_us: 0,
        })
    }
}
```

Use the stub executor by setting `EXECUTOR=stub`. It activates automatically when the binary is compiled without `--features php`.

## Executor Selection

The `create_executor()` factory in `src/executor/mod.rs` selects the backend based on the `EXECUTOR` environment variable and compile-time features:

| `EXECUTOR` | `--features php` | Result |
|---|---|---|
| `sapi` (default) | yes | `SapiExecutor` (PHP worker pool) |
| `sapi` (default) | no | `StubExecutor` (fallback with warning) |
| `stub` | any | `StubExecutor` (benchmark mode) |

## Shutdown

The `SapiExecutor` implements `Drop` for orderly shutdown:

1. **Global shutdown flag**: `global_shutdown.store(true)` — stops the ScaleManager (if running)
2. **Drop the channel sender**: Workers see `recv()` return `Err` (static) or disconnected (dynamic) and exit their loops
3. **Per-worker shutdown**: Sets each worker's `shutdown` flag, ensuring dynamic workers exit their timeout loops
4. **Join all worker threads**: Blocks until every worker has finished its current request
5. **PHP cleanup**: `php_module_shutdown()`, `sapi_shutdown()`, `tsrm_shutdown()` in sequence

This guarantees that no PHP request is interrupted mid-execution during shutdown.

## See Also

- [Architecture Overview](./overview.md) — High-level component map and startup sequence
- [SAPI and Bridge](./sapi-bridge.md) — How PHP workers interact with the bridge library
- [Request Lifecycle](./request-lifecycle.md) — How requests flow from Tokio to PHP workers
- [Configuration](../operations/configuration.md) — `PHP_WORKERS`, `QUEUE_CAPACITY`, and other env vars
- [Metrics](../operations/metrics.md) — Worker pool metrics (pending, busy, dropped)
- [Graceful Shutdown](../operations/graceful-shutdown.md) — Drain behavior and worker teardown
