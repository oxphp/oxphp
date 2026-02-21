---
title: 事件系统
description: 类型化事件分发器 — 优先级排序、安全类型擦除与处理器注册
---

OxPHP 使用类型化事件系统将横切关注点（指标、日志、限流、头处理）与核心请求管道解耦。处理器注册到特定事件类型，并按优先级顺序执行。

## 核心概念

事件系统基于三个 trait 和一个枚举构建，定义在 `src/events/mod.rs` 中：

### Event Trait

```rust
pub trait Event: Any + Send + Sync + 'static {
    fn name(&self) -> &'static str;
}
```

每个事件类型实现 `Event`。`Any` 约束在分发器中实现类型擦除。`name()` 方法提供人类可读的调试字符串（例如 `"request.received"`）。

### EventHandler Trait

```rust
pub trait EventHandler<E: Event>: Send + Sync {
    fn handle(&self, event: &mut E) -> Propagation;

    fn priority(&self) -> Priority {
        0
    }
}
```

处理器对特定事件类型 `E` 泛型。它们接收事件的可变引用并返回 `Propagation` 值。默认优先级为 0。

### Priority

```rust
pub type Priority = i32;
```

值越小越先执行。负优先级在默认值（0）之前运行，正优先级在之后运行。可使用 `i32` 的完整范围。

### Propagation

```rust
pub enum Propagation {
    Continue,
    Stop,
}
```

- `Continue`：按优先级顺序运行下一个处理器。
- `Stop`：此次事件分发不再运行后续处理器。分发器向调用者返回 `Propagation::Stop`。

## EventDispatcher

`src/events/dispatcher.rs` 中的 `EventDispatcher` 管理处理器注册和分发。它有两个阶段：**可变**（注册）和**冻结**（仅分发）。

### 注册阶段

服务器启动期间，使用 `on()` 注册处理器：

```rust
let mut dispatcher = EventDispatcher::new();
dispatcher.on(RequestIdGenerator);           // priority -100
dispatcher.on(RateLimitHandler::new(...));   // priority -50
dispatcher.on(MetricsRequestHandler::new(...)); // priority 0
dispatcher.freeze();
```

`freeze()` 之后调用 `on()` 会 panic。

### 冻结

`freeze()` 按优先级（升序）对所有处理器列表排序，并设置一个标志阻止进一步注册：

```rust
pub fn freeze(&mut self) {
    self.frozen = true;
    for handlers in self.handlers.values_mut() {
        handlers.sort_by_key(|(priority, _)| *priority);
    }
}
```

冻结后，分发器包装在 `Arc` 中并在所有 Tokio 任务间不可变共享。

### 分发

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

分发的时间复杂度为 `O(n)`，其中 `n` 是该事件类型的处理器数量。如果某事件类型没有注册任何处理器，分发仅需一次哈希查找即立即返回。

## 类型擦除

分发器需要将不同事件类型的处理器存储在单一集合中。它通过安全的类型擦除实现这一点 — 无 `unsafe` 块。

### 工作原理

每个处理器被包装在一个执行 `dyn Any` 向下转换的闭包中：

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

`ErasedFn` 类型为：

```rust
type ErasedFn = Box<dyn Fn(&mut dyn Any) -> Propagation + Send + Sync>;
```

`TypeId::of::<E>()` 键保证为 `RequestReceived` 注册的处理器只会被 `RequestReceived` 事件调用。`downcast_mut` 调用是运行时类型检查，但只有在分发器本身存在 bug 时才会失败（事件按 `TypeId` 路由）。

### 恒等哈希

处理器 map 使用自定义 `TypeIdHasher`，避免了对 `TypeId` 键使用 SipHash 的开销：

```rust
struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 { self.0 }
    fn write_u128(&mut self, i: u128) { self.0 = i as u64; }
    // ...
}
```

`TypeId` 通过 `write_u128` 哈希。恒等哈希器直接取低 64 位，这是安全的，因为 `TypeId` 值已经分布均匀。这避免了使用默认 `SipHash` 的 `HashMap<TypeId, V>` 的双重哈希开销。

## 事件类型

OxPHP 在 `src/events/types.rs` 中定义了 18 种事件类型，按生命周期阶段组织：

### 服务器生命周期

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `ServerBooting` | `server.booting` | （无） | 服务器引导期间触发，在绑定之前 |
| `ServerStarted` | `server.started` | `listen_addr: String` | 服务器正在监听且已就绪 |
| `ShutdownInitiated` | `server.shutdown_initiated` | （无） | 优雅关闭已开始 |

### 配置

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `ConfigLoading` | `config.loading` | （无） | 正在加载配置 |

### 连接

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `ConnectionAccepted` | `connection.accepted` | `remote_addr` | 新的 TCP 连接已接受 |
| `ConnectionClosed` | `connection.closed` | `remote_addr` | TCP 连接已关闭 |

### 请求

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `RequestReceived` | `request.received` | `parts`, `remote_addr`, `request_id`, `early_response`, `metadata: Vec<(String, String)>` | 收到 HTTP 请求，路由之前 |
| `RouteResolved` | `request.route_resolved` | `request_id`, `path` | 路由已解析，执行之前 |
| `RequestComplete` | `request.complete` | `request_id`, `method: Method`, `path`, `status`, `duration`, `remote_addr` | 请求完全处理完毕 |

### PHP

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `ScriptExecutionStarting` | `php.script_execution_starting` | `request_id`, `script_path` | 即将执行 PHP 脚本 |
| `PhpRequestStartup` | `php.request_startup` | `request_id` | PHP RINIT 阶段 |
| `PhpRequestShutdown` | `php.request_shutdown` | `request_id` | PHP RSHUTDOWN 阶段 |
| `ScriptExecutionComplete` | `php.script_execution_complete` | `request_id`, `execution_time_us` | 脚本执行完毕 |

### 响应

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `ResponseBuilding` | `response.building` | `request_id`, `response` | 在发送前修改响应 |

### 错误

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `RequestTimedOut` | `error.request_timed_out` | `request_id`, `timeout` | 请求超时 |
| `RequestError` | `error.request_error` | `request_id`, `error` | 未处理的请求错误 |

### 服务

| 事件 | 名称 | 字段 | 描述 |
|---|---|---|---|
| `HealthCheckRequested` | `service.health_check` | `executor_healthy` | 健康端点被检查 |
| `MetricsCollected` | `service.metrics_collected` | （无） | 指标被采集 |

## 管道中的活跃事件

当前在请求管道（`src/server/connection.rs`）中分发三个事件：

```
RequestReceived ──▶ [route + execute] ──▶ ResponseBuilding ──▶ [compress] ──▶ RequestComplete
```

其余事件类型为插件系统和自定义处理器注册而定义。

### RequestReceived

处理器可以检查/修改 HTTP 请求 parts、分配请求 ID，并通过设置 `early_response` 短路管道：

```rust
pub struct RequestReceived {
    pub parts: Parts,
    pub remote_addr: SocketAddr,
    pub request_id: String,
    pub early_response: Option<Response<ResponseBody>>,
    pub metadata: Vec<(String, String)>,
}
```

`metadata` 字段允许插件处理器附加随请求在管道中传递的键值数据。

设置 `early_response` **不会**停止传播。限流器返回 `Propagation::Continue`，这样指标处理器（优先级 0）仍然记录请求。管道在所有 `RequestReceived` 处理器运行完毕后才检查 `early_response`。

### ResponseBuilding

处理器可以修改响应 — 替换响应体（错误页面）、添加头（Server、X-Request-ID）：

```rust
pub struct ResponseBuilding {
    pub request_id: String,
    pub response: Response<ResponseBody>,
}
```

### RequestComplete

只读事件，用于日志和指标。所有字段都是拥有所有权的值：

```rust
pub struct RequestComplete {
    pub request_id: String,
    pub method: Method,  // http::Method
    pub path: String,
    pub status: u16,
    pub duration: Duration,
    pub remote_addr: SocketAddr,
}
```

## 处理器

OxPHP 内置七个处理器，定义在 `src/handlers/` 中：

| 处理器 | 事件 | 优先级 | 描述 |
|---|---|---|---|
| `RequestIdGenerator` | `RequestReceived` | -100 | 生成 `{ts:08x}{counter:08x}` 或保留 `X-Request-ID` 头 |
| `RateLimitHandler` | `RequestReceived` | -50 | 检查每 IP 限流，通过 429 设置 `early_response` |
| `MetricsRequestHandler` | `RequestReceived` | 0 | 记录请求计数和方法 |
| `MetricsResponseHandler` | `RequestComplete` | 0 | 记录响应状态类和持续时间 |
| `ErrorPagesHandler` | `ResponseBuilding` | 60 | 用自定义 HTML 替换错误响应体（状态码 >= 400） |
| `ServerHeaderHandler` | `ResponseBuilding` | 100 | 添加 `Server: OxPHP/{version}` 和 `X-Request-ID` 头 |
| `AccessLogHandler` | `RequestComplete` | 100 | 通过 `tracing::info!` 输出结构化 JSON 访问日志（仅在 `config.access_log` 启用时注册） |

### 优先级设计

优先级分配遵循刻意的顺序：

- **RequestIdGenerator (-100)**：必须最先运行，以便所有后续处理器可以使用 `request_id`
- **RateLimitHandler (-50)**：在请求 ID 分配后运行，使被拒绝的请求在访问日志中有 ID
- **MetricsRequestHandler (0)**：计入所有请求，包括被限流的（因为 RateLimitHandler 返回 `Continue`）
- **ErrorPagesHandler (60)**：在 ServerHeaderHandler 之前运行，确保添加头时错误页面体已就位
- **ServerHeaderHandler (100)**：在 ResponseBuilding 中最后运行 — 在所有体修改完成后添加最终头
- **MetricsResponseHandler (0)** 和 **AccessLogHandler (100)**：在响应完全构建后的 RequestComplete 上运行

### 条件注册

并非所有处理器都始终活跃。在 `main.rs` 中：

```rust
// 始终注册
dispatcher.on(RequestIdGenerator);
dispatcher.on(MetricsRequestHandler::new(...));
dispatcher.on(MetricsResponseHandler::new(...));
dispatcher.on(ServerHeaderHandler);

// 仅在配置时注册
if let Some(ref limiter) = rate_limiter {
    dispatcher.on(RateLimitHandler::new(Arc::clone(limiter)));
}
if let Some(ref pages) = error_pages {
    dispatcher.on(ErrorPagesHandler::new(Arc::clone(pages)));
}
if config.access_log {
    dispatcher.on(AccessLogHandler);
}

dispatcher.freeze();
```

插件处理器由 `plugin_manager.init_all(&mut dispatcher)` 在内置处理器之前、启动早期注册。

## 另请参阅

- [架构概览](./overview.md) — 组件全景与启动序列
- [请求生命周期](./request-lifecycle.md) — 事件如何融入请求管道
- [工作线程池](./worker-pool.md) — 产生响应的 PHP 工作线程
- [限流](../features/rate-limiting.md) — RateLimitHandler 配置
- [错误页面](../features/error-pages.md) — ErrorPagesHandler 配置
- [请求 ID](../features/request-ids.md) — RequestIdGenerator 格式与行为
- [访问日志](../features/access-logging.md) — AccessLogHandler 输出格式
