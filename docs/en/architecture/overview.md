---
title: Architecture Overview
description: High-level architecture of OxPHP — async I/O runtime, PHP worker pool, and component map
---

OxPHP is a single-binary HTTP server that replaces the traditional nginx + PHP-FPM stack. It combines an asynchronous I/O runtime (Rust/Tokio) with a multi-threaded PHP worker pool (ZTS) in one process.

## Design Principles

- **Single binary**: No external process manager, no sidecar. One binary handles TCP, TLS, HTTP parsing, routing, PHP execution, and observability.
- **Async I/O, synchronous PHP**: Network I/O is multiplexed on an async runtime. PHP scripts run on dedicated OS threads. The two halves communicate through channels.
- **Zero-copy where possible**: Request data passes through the pipeline without unnecessary clones. `Bytes`, `Arc`, and `std::mem::take` avoid allocations on the hot path.

## Runtime Model

```
                    ┌─────────────────────────────────────────────────┐
                    │      Tokio Runtime (single- or multi-thread)    │
                    │                                                 │
                    │  ┌──────────┐  ┌───────────┐  ┌───────────┐     │
  TCP connections──▶│  │ accept   │  │ service   │  │ service   │     │
                    │  │ loop     │  │ task      │  │ task      │     │
                    │  └──────────┘  └─────┬─────┘  └─────┬─────┘     │
                    │                      │              │           │
                    └──────────────────────┼──────────────┼───────────┘
                         ScriptRequest     │              │
              (crossbeam_channel + oneshot)│  ┌───────────┘
                                           │  │
                                           ▼  ▼
                    ┌──────────────────────┼──┼───────────────────────┐
                    │                                                 │
                    │  ┌────────────┐  ┌────────────┐  ┌────────────┐ │
                    │  │php-worker-0│  │php-worker-1│  │php-worker-N│ │
                    │  │            │  │            │  │            │ │
                    │  └────────────┘  └────────────┘  └────────────┘ │
                    │              PHP Worker Pool (OS threads)       │
                    └─────────────────────────────────────────────────┘
```

The **Tokio runtime** is configurable via `TOKIO_WORKERS`. When set to `0` or unset (default), it auto-detects to CPU/2 (min 1). When set to `1`, it uses `Builder::new_current_thread()` for a single-threaded async runtime. When set to `N` (>1), it uses `Builder::new_multi_thread()` with N worker threads for higher throughput. It handles all asynchronous work: accepting TCP connections, TLS handshakes, HTTP parsing, routing, compression, and event dispatch. Each connection is a lightweight Tokio task. The process uses mimalloc as the global allocator for lower allocation latency under thread contention.

The **PHP worker pool** is a set of dedicated OS threads. Each thread owns a PHP ZTS (Zend Thread Safety) interpreter instance. Workers receive `ScriptRequest` structs through a bounded `crossbeam_channel::bounded` channel and return `ScriptResponse` through a `tokio::sync::oneshot` channel.

### Why Not Multi-Threaded PHP Inside Tokio?

PHP's C runtime is not async-safe. Functions like `php_request_startup()` and `php_execute_script()` block the calling thread and make non-thread-safe global state mutations. Running them on a Tokio worker thread would starve the async runtime. Dedicated OS threads isolate PHP's blocking behavior from the async I/O loop.

## Component Map

```
┌─────────────────────────────────────────────────────────────────┐
│  main.rs                                                        │
│  ┌───────────┐  ┌────────────┐  ┌─────────────┐                 │
│  │ Config    │  │ Metrics    │  │ EventDisp.  │                 │
│  │ from_env()│  │ (atomics)  │  │ (typed)     │                 │
│  └───────────┘  └────────────┘  └─────────────┘                 │
│                                                                 │
│  ┌───────────────┐                                              │
│  │ PluginManager │ init_all() → on_ready_all() → shutdown_all() │
│  └───────────────┘                                              │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Server                                                  │   │
│  │  ┌───────────┐  ┌──────────┐  ┌──────────────────────┐   │   │
│  │  │RouteConfig│  │FileCache │  │ScriptExecutor (trait)│   │   │
│  │  │3 modes    │  │(RwLock)  │  │  ├─ SapiExecutor     │   │   │
│  │  └───────────┘  └──────────┘  │  └─ StubExecutor     │   │   │
│  │                               └──────────────────────┘   │   │
│  │  ┌──────────┐  ┌───────────┐  ┌───────────┐              │   │
│  │  │TLS       │  │RateLimiter│  │Compression│              │   │
│  │  │(rustls)  │  │(DashMap)  │  │(brotli)   │              │   │
│  │  └──────────┘  └───────────┘  └───────────┘              │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌──────────────────────────────┐                               │
│  │  Handlers (event-driven)     │                               │
│  │  RequestIdGenerator  (-100)  │                               │
│  │  RateLimitHandler    (-50)   │                               │
│  │  MetricsRequest      (0)     │                               │
│  │  MetricsResponse     (0)     │                               │
│  │  ErrorPagesHandler   (60)    │                               │
│  │  ServerHeaderHandler (100)   │                               │
│  │  AccessLogHandler    (100)   │                               │
│  └──────────────────────────────┘                               │
└─────────────────────────────────────────────────────────────────┘
```

### Core Components

| Component | Location | Purpose |
|---|---|---|
| **Config** | `src/config/` | Reads all configuration from environment variables at startup |
| **Server** | `src/server/mod.rs` | Owns the connection accept loop, hyper-util builder, shutdown flag |
| **RouteConfig** | `src/server/routing.rs` | Resolves URI paths to `Serve`, `Execute`, or `NotFound` |
| **FileCache** | `src/server/response/static_file.rs` | LRU cache for file metadata and canonical path lookups |
| **ScriptExecutor** | `src/executor/mod.rs` | Trait for PHP execution backends (`SapiExecutor`, `StubExecutor`); `execute()` returns `ExecuteResult` |
| **Metrics** | `src/metrics.rs` | Lock-free atomic counters (Prometheus exposition format) |
| **EventDispatcher** | `src/events/dispatcher.rs` | Typed, priority-ordered synchronous event dispatch |
| **PluginManager** | `src/plugin/mod.rs` | Plugin lifecycle management with topological sort |
| **RateLimiter** | `src/server/rate_limit.rs` | Per-IP sliding window rate limiting using `DashMap` |
| **Compression** | `src/server/compression.rs` | Brotli compression for text-based responses |

## Module Structure

```
src/
├── main.rs                  # Entry point, Tokio runtime, accept loop, shutdown
├── lib.rs                   # Public module exports
├── types.rs                 # ScriptRequest, ScriptResponse, ResponseBody, BoxError
├── logging.rs               # JSON structured logging via tracing
├── metrics.rs               # Lock-free atomic Prometheus metrics
├── config/
│   ├── mod.rs               # Config aggregate, from_env()
│   └── server.rs            # ServerConfig: addresses, timeouts, paths
├── server/
│   ├── mod.rs               # Server struct, connection handling, shutdown
│   ├── connection.rs        # Request pipeline: events → route → execute → respond
│   ├── routing.rs           # RouteConfig, 3 routing modes, path sanitization
│   ├── compression.rs       # Brotli compression (quality 4, 256 B – 3 MB)
│   ├── rate_limit.rs        # Per-IP sliding window (DashMap)
│   ├── tls.rs               # TLS via rustls + tokio-rustls
│   ├── error_pages.rs       # Custom HTML error pages (loaded at startup)
│   ├── internal.rs          # Internal server (/health, /metrics, /config)
│   └── response/
│       └── static_file.rs   # Static file serving with MIME detection and caching
├── executor/
│   ├── mod.rs               # ScriptExecutor trait (execute() → ExecuteResult), create_executor() factory
│   ├── stub.rs              # StubExecutor (returns 200 OK, for benchmarking)
│   └── sapi.rs              # SapiExecutor (PHP ZTS worker pool) [feature-gated]
├── events/
│   ├── mod.rs               # Event trait, Priority, Propagation, EventHandler trait
│   ├── types.rs             # 18 concrete event structs
│   └── dispatcher.rs        # Type-erased dispatcher with identity hashing
├── handlers/
│   ├── mod.rs               # Handler module exports
│   ├── request_id.rs        # Generates or preserves X-Request-ID
│   ├── rate_limit.rs        # Wraps RateLimiter as an event handler
│   ├── metrics.rs           # Records request/response metrics
│   ├── error_pages.rs       # Replaces error response bodies with custom HTML
│   ├── server_header.rs     # Adds Server and X-Request-ID headers
│   └── access_log.rs        # Structured access log via tracing
├── plugin/
│   ├── mod.rs               # Plugin trait
│   ├── context.rs           # PluginContext
│   ├── cookies.rs           # Plugin cookie isolation
│   ├── handler.rs           # Handler traits
│   ├── macros.rs            # Plugin helper macros
│   ├── manager.rs           # PluginManager
│   ├── php.rs               # PHP function registration
│   └── wrappers.rs          # Event handler wrappers
├── plugins/
│   └── example.rs           # Example plugin [feature-gated: plugin-example]
└── php/                     # PHP FFI bindings [feature-gated]
    ├── bindings.rs
    └── sapi.rs
```

## Communication Channels

The async and synchronous halves of OxPHP communicate through two channel types:

| Channel | Direction | Type | Purpose |
|---|---|---|---|
| `crossbeam_channel::bounded` | Tokio → PHP worker | `ScriptRequest` | Bounded queue with backpressure (503 on full) |
| `tokio::sync::oneshot` | PHP worker → Tokio | `ScriptResponse` | Single response per request |

`ScriptExecutor::execute()` returns an `ExecuteResult` enum rather than a raw `oneshot::Receiver`. This allows the executor to return an error response immediately (without a worker thread) when the queue is full or the worker pool is unavailable:

```rust
pub enum ExecuteResult {
    Immediate(ScriptResponse),
    Deferred(tokio::sync::oneshot::Receiver<ScriptResponse>),
}
```

This pattern allows the Tokio runtime to dispatch work to PHP workers without blocking and to await responses asynchronously. See [Worker Pool](./worker-pool.md) for details.

## Startup Sequence

1. **Config**: `Config::from_env()` reads all environment variables
2. **Metrics**: `Metrics::new()` initializes lock-free atomic counters (created before executor so worker metrics can be recorded during startup)
3. **Plugin manager**: `PluginManager::new()` creates the manager, plugins are added, then `init_all()` registers plugin event handlers on the dispatcher and populates the bridge plugin function registry
4. **Plugin PHP functions**: Plugin functions are passed to `sapi::register_plugin_functions()` so the bridge registry is populated before PHP engine startup
5. **Executor**: `create_executor(metrics)` initializes TSRM, registers the SAPI module, starts `php_module_startup()` (which triggers MINIT — registering plugin functions with Zend), parses `PHP_WORKERS` mode, and spawns initial PHP worker threads
6. **Tokio runtime**: Configured by `TOKIO_WORKERS` — `0` auto-detects to CPU/2 (min 1), `1` creates a single-threaded runtime, `N` creates a multi-threaded runtime with N workers
7. **Scale manager**: `executor.start_scale_manager()` spawns the worker scaling task (no-op in static mode). In static mode, a background health monitor detects and respawns dead workers
8. **Rate limiter**: Optional, with background cleanup task
9. **TLS**: Optional, loads certificate and key via `rustls`
10. **Event dispatcher**: Built-in handlers registered (note: `AccessLogHandler` is only registered when `config.access_log` is enabled), then `freeze()` sorts by priority
11. **TCP listener**: Binds to the configured address
12. **Internal server**: Optional `/health`, `/metrics`, `/config` on a separate port
13. **Plugin ready**: `plugin_manager.on_ready_all()` notifies plugins that the server is listening
14. **Accept loop**: Spawns a Tokio task per connection, bounded by `Semaphore(max_connections)`

## Shutdown Sequence

1. SIGTERM or Ctrl+C triggers `shutdown_signal()`
2. `plugin_manager.shutdown_all()` notifies plugins, then `server.shutdown()` sets the atomic shutdown flag and calls `executor.shutdown()`
3. The accept loop breaks on `is_shutdown()`
4. Drain phase: waits up to `drain_timeout_seconds` (default 30) for in-flight connections
5. Internal server task is aborted
6. `SapiExecutor::drop()` drops the channel sender, joins all worker threads, then calls `php_module_shutdown()`, `sapi_shutdown()`, and `tsrm_shutdown()`

## See Also

- [Worker Pool](./worker-pool.md) — PHP worker threads, scaling, and backpressure
- [Event System](./event-system.md) — Typed event dispatch and handler registration
- [Request Lifecycle](./request-lifecycle.md) — Step-by-step request pipeline walkthrough
- [SAPI and Bridge](./sapi-bridge.md) — Custom PHP SAPI and C bridge library
- [Configuration](../operations/configuration.md) — Environment variable reference
- [Routing](../features/routing.md) — Three routing modes (traditional, framework, SPA)
- [Graceful Shutdown](../operations/graceful-shutdown.md) — Drain behavior and timeouts
