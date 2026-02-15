---
title: Event System
description: Typed event dispatcher with priority ordering, safe type erasure, and handler registration
---

OxPHP uses a typed event system to decouple cross-cutting concerns (metrics, logging, rate limiting, headers) from the core request pipeline. Handlers register for specific event types and run in priority order.

## Core Concepts

The event system is built on three traits and one enum, defined in `src/events/mod.rs`:

### Event Trait

```rust
pub trait Event: Any + Send + Sync + 'static {
    fn name(&self) -> &'static str;
}
```

Every event type implements `Event`. The `Any` bound enables type erasure in the dispatcher. The `name()` method provides a human-readable string for debugging (e.g., `"request.received"`).

### EventHandler Trait

```rust
pub trait EventHandler<E: Event>: Send + Sync {
    fn handle(&self, event: &mut E) -> Propagation;

    fn priority(&self) -> Priority {
        0
    }
}
```

Handlers are generic over a specific event type `E`. They receive a mutable reference to the event and return a `Propagation` value. The default priority is 0.

### Priority

```rust
pub type Priority = i32;
```

Lower values run first. Negative priorities run before the default (0), positive priorities run after. The full range of `i32` is available.

### Propagation

```rust
pub enum Propagation {
    Continue,
    Stop,
}
```

- `Continue`: The next handler in priority order runs.
- `Stop`: No further handlers run for this event dispatch. The dispatcher returns `Propagation::Stop` to the caller.

## EventDispatcher

The `EventDispatcher` in `src/events/dispatcher.rs` manages handler registration and dispatch. It has two phases: **mutable** (registration) and **frozen** (dispatch-only).

### Registration Phase

During server startup, handlers are registered with `on()`:

```rust
let mut dispatcher = EventDispatcher::new();
dispatcher.on(RequestIdGenerator);           // priority -100
dispatcher.on(RateLimitHandler::new(...));   // priority -50
dispatcher.on(MetricsRequestHandler::new(...)); // priority 0
dispatcher.freeze();
```

`on()` panics if called after `freeze()`.

### Freeze

`freeze()` sorts all handler lists by priority (ascending) and sets a flag that prevents further registration:

```rust
pub fn freeze(&mut self) {
    self.frozen = true;
    for handlers in self.handlers.values_mut() {
        handlers.sort_by_key(|(priority, _)| *priority);
    }
}
```

After freezing, the dispatcher is wrapped in `Arc` and shared immutably across all Tokio tasks.

### Dispatch

```rust
pub fn dispatch<E: Event>(&self, event: &mut E) -> Propagation {
    let type_id = TypeId::of::<E>();
    let Some(handlers) = self.handlers.get(&type_id) else {
        return Propagation::Continue;
    };

    for (_, handler_fn) in handlers {
        if handler_fn(event) == Propagation::Stop {
            return Propagation::Stop;
        }
    }

    Propagation::Continue
}
```

Dispatch is `O(n)` where `n` is the number of handlers for that event type. If no handlers are registered for an event type, dispatch is a single hash lookup that returns immediately.

## Type Erasure

The dispatcher needs to store handlers for different event types in a single collection. It achieves this with safe type erasure — no `unsafe` blocks.

### How It Works

Each handler is wrapped in a closure that performs the `dyn Any` downcast:

```rust
pub fn on<E: Event>(&mut self, handler: impl EventHandler<E> + 'static) {
    let priority = handler.priority();
    let f: ErasedFn = Box::new(move |event: &mut dyn Any| {
        handler.handle(event.downcast_mut::<E>().expect("event type mismatch"))
    });

    self.handlers
        .entry(TypeId::of::<E>())
        .or_default()
        .push((priority, f));
}
```

The type `ErasedFn` is:

```rust
type ErasedFn = Box<dyn Fn(&mut dyn Any) -> Propagation + Send + Sync>;
```

The `TypeId::of::<E>()` key guarantees that a handler registered for `RequestReceived` is only invoked with a `RequestReceived` event. The `downcast_mut` call is a runtime type check, but it can only fail if there is a bug in the dispatcher itself (events are routed by `TypeId`).

### Identity Hashing

The handler map uses a custom `TypeIdHasher` that avoids the overhead of SipHash for `TypeId` keys:

```rust
struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 { self.0 }
    fn write_u128(&mut self, i: u128) { self.0 = i as u64; }
    // ...
}
```

`TypeId` hashes via `write_u128`. The identity hasher takes the lower 64 bits directly, which is safe because `TypeId` values are already well-distributed. This avoids the double-hashing overhead of `HashMap<TypeId, V>` with the default `SipHash`.

## Event Types

OxPHP defines 18 event types in `src/events/types.rs`, organized by lifecycle stage:

### Server Lifecycle

| Event | Name | Fields | Description |
|---|---|---|---|
| `ServerBooting` | `server.booting` | (none) | Fired during server boot, before binding |
| `ServerStarted` | `server.started` | `listen_addr: String` | Server is listening and ready |
| `ShutdownInitiated` | `server.shutdown_initiated` | (none) | Graceful shutdown has begun |

### Configuration

| Event | Name | Fields | Description |
|---|---|---|---|
| `ConfigLoading` | `config.loading` | (none) | Configuration loading in progress |

### Connection

| Event | Name | Fields | Description |
|---|---|---|---|
| `ConnectionAccepted` | `connection.accepted` | `remote_addr` | New TCP connection accepted |
| `ConnectionClosed` | `connection.closed` | `remote_addr` | TCP connection closed |

### Request

| Event | Name | Fields | Description |
|---|---|---|---|
| `RequestReceived` | `request.received` | `parts`, `remote_addr`, `request_id`, `early_response`, `metadata` | HTTP request received, before routing |
| `RouteResolved` | `request.route_resolved` | `request_id`, `path` | Route resolved, before execution |
| `RequestComplete` | `request.complete` | `request_id`, `method`, `path`, `status`, `duration`, `remote_addr` | Request fully processed |

### PHP

| Event | Name | Fields | Description |
|---|---|---|---|
| `ScriptExecutionStarting` | `php.script_execution_starting` | `request_id`, `script_path` | About to execute a PHP script |
| `PhpRequestStartup` | `php.request_startup` | `request_id` | PHP RINIT phase |
| `PhpRequestShutdown` | `php.request_shutdown` | `request_id` | PHP RSHUTDOWN phase |
| `ScriptExecutionComplete` | `php.script_execution_complete` | `request_id`, `execution_time_us` | Script finished |

### Response

| Event | Name | Fields | Description |
|---|---|---|---|
| `ResponseBuilding` | `response.building` | `request_id`, `response` | Modifying the response before sending |

### Error

| Event | Name | Fields | Description |
|---|---|---|---|
| `RequestTimedOut` | `error.request_timed_out` | `request_id`, `timeout` | Request exceeded timeout |
| `RequestError` | `error.request_error` | `request_id`, `error` | Unhandled request error |

### Service

| Event | Name | Fields | Description |
|---|---|---|---|
| `HealthCheckRequested` | `service.health_check` | `executor_healthy` | Health endpoint checked |
| `MetricsCollected` | `service.metrics_collected` | (none) | Metrics scraped |

## Active Events in the Pipeline

Three events are currently dispatched in the request pipeline (`src/server/connection.rs`):

```
RequestReceived ──▶ [route + execute] ──▶ ResponseBuilding ──▶ [compress] ──▶ RequestComplete
```

The remaining event types are defined for use by the plugin system and for custom handler registration.

### RequestReceived

Handlers can inspect/modify the HTTP request parts, assign a request ID, and short-circuit the pipeline by setting `early_response`:

```rust
pub struct RequestReceived {
    pub parts: Parts,
    pub remote_addr: SocketAddr,
    pub request_id: String,
    pub early_response: Option<Response<ResponseBody>>,
    pub metadata: HashMap<String, String>,
}
```

The `metadata` field allows plugin handlers to attach key-value data that travels with the request through the pipeline.

Setting `early_response` does **not** stop propagation. The rate limiter returns `Propagation::Continue` so that the metrics handler (priority 0) still records the request. The pipeline checks for `early_response` after all `RequestReceived` handlers have run.

### ResponseBuilding

Handlers can modify the response — replace the body (error pages), add headers (Server, X-Request-ID):

```rust
pub struct ResponseBuilding {
    pub request_id: String,
    pub response: Response<ResponseBody>,
}
```

### RequestComplete

Read-only event for logging and metrics. All fields are owned values:

```rust
pub struct RequestComplete {
    pub request_id: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration: Duration,
    pub remote_addr: SocketAddr,
}
```

## Handlers

Seven handlers ship with OxPHP, defined in `src/handlers/`:

| Handler | Event | Priority | Description |
|---|---|---|---|
| `RequestIdGenerator` | `RequestReceived` | -100 | Generates `{ts:08x}{counter:08x}` or preserves `X-Request-ID` header |
| `RateLimitHandler` | `RequestReceived` | -50 | Checks per-IP rate limit, sets `early_response` with 429 |
| `MetricsRequestHandler` | `RequestReceived` | 0 | Records request count and method |
| `MetricsResponseHandler` | `RequestComplete` | 0 | Records response status class and duration |
| `ErrorPagesHandler` | `ResponseBuilding` | 60 | Replaces error response body with custom HTML (status >= 400) |
| `ServerHeaderHandler` | `ResponseBuilding` | 100 | Adds `Server: OxPHP/{version}` and `X-Request-ID` headers |
| `AccessLogHandler` | `RequestComplete` | 100 | Emits structured JSON access log via `tracing::info!` |

### Priority Design

The priority assignments follow a deliberate order:

- **RequestIdGenerator (-100)**: Must run first so all subsequent handlers can use `request_id`
- **RateLimitHandler (-50)**: Runs after request ID is assigned, so rejected requests have IDs in the access log
- **MetricsRequestHandler (0)**: Counts all requests, including rate-limited ones (since RateLimitHandler returns `Continue`)
- **ErrorPagesHandler (60)**: Runs before ServerHeaderHandler so the error page body is in place when headers are added
- **ServerHeaderHandler (100)**: Runs last in ResponseBuilding — adds final headers after all body modifications
- **MetricsResponseHandler (0)** and **AccessLogHandler (100)**: Run on RequestComplete after the response is fully built

### Conditional Registration

Not all handlers are always active. In `main.rs`:

```rust
// Always registered
dispatcher.on(RequestIdGenerator);
dispatcher.on(MetricsRequestHandler::new(...));
dispatcher.on(MetricsResponseHandler::new(...));
dispatcher.on(ServerHeaderHandler);
dispatcher.on(AccessLogHandler);

// Only if configured
if let Some(ref limiter) = rate_limiter {
    dispatcher.on(RateLimitHandler::new(Arc::clone(limiter)));
}
if let Some(ref pages) = error_pages {
    dispatcher.on(ErrorPagesHandler::new(Arc::clone(pages)));
}

dispatcher.freeze();
```

Plugin handlers are registered by `plugin_manager.init_all(&mut dispatcher)` before the built-in handlers, during early startup.

## See Also

- [Architecture Overview](./overview.md) — Component map and startup sequence
- [Request Lifecycle](./request-lifecycle.md) — How events fit into the request pipeline
- [Worker Pool](./worker-pool.md) — PHP worker threads that produce responses
- [Rate Limiting](../features/rate-limiting.md) — RateLimitHandler configuration
- [Error Pages](../features/error-pages.md) — ErrorPagesHandler configuration
- [Request IDs](../features/request-ids.md) — RequestIdGenerator format and behavior
- [Access Logging](../features/access-logging.md) — AccessLogHandler output format
