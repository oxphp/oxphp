---
title: 工作线程池
description: PHP 工作线程池架构 — 静态/动态伸缩、有界 channel、背压与 ScriptExecutor trait
---

OxPHP 在一组专用 OS 线程上执行 PHP 脚本，与异步 I/O 运行时隔离。本页介绍 `ScriptExecutor` trait、有界 channel 设计、背压行为和自动工作线程伸缩。

## ScriptExecutor Trait

所有 PHP 执行后端实现定义在 `src/executor/mod.rs` 中的 `ScriptExecutor` trait：

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

| 方法 | 用途 |
|---|---|
| `execute()` | 接受请求并返回 `ExecuteResult`（立即或延迟响应） |
| `shutdown()` | 通知执行器停止接受工作 |
| `is_healthy()` | `/health` 内部端点的健康检查 |
| `start_scale_manager()` | 启动后台伸缩任务（stub 中无操作；静态模式生成健康监控器） |

该 trait 返回 `ExecuteResult` 而非裸的 `Future` 或 `oneshot::Receiver`。这使得执行器可以在不涉及工作线程的情况下立即返回错误响应（例如队列满时返回 503），同时仍支持 Tokio 任务等待 `oneshot::Receiver` 的延迟场景。

```rust
pub enum ExecuteResult {
    Immediate(ScriptResponse),
    Deferred(tokio::sync::oneshot::Receiver<ScriptResponse>),
}
```

## 数据类型

请求和响应定义在 `src/types.rs` 中：

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

`document_root` 用 `Arc<PathBuf>` 包装以实现跨请求的低开销共享。响应中的 `headers` 使用 `Vec`（而非 `HeaderMap`），因为 PHP 工作线程在工作线程上预解析头字符串为类型化的 `HeaderName`/`HeaderValue` 对，避免了在 Tokio 运行时上的解析开销。

## SapiExecutor

生产执行器（`src/executor/sapi.rs`，通过 `--features php` feature-gate）管理一个由有界 `crossbeam_channel` 连接的 PHP ZTS 工作线程池。支持两种运行模式：**静态**（固定池大小）和**动态**（在最小/最大范围内自动伸缩）。

### 架构

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

### 工作模式

`PHP_WORKERS` 环境变量控制工作线程池模式：

| 格式 | 模式 | 示例 | 行为 |
|--------|------|---------|----------|
| `N` | 静态 | `PHP_WORKERS=8` | 固定 8 个工作线程 |
| `0` 或未设置 | 静态 | `PHP_WORKERS=0` | 固定 CPU 数量 * 2 个工作线程 |
| `MIN:MAX` | 动态 | `PHP_WORKERS=2:16` | 从 MIN 开始，负载下扩展到 MAX |
| `MIN:0` | 动态 | `PHP_WORKERS=2:0` | MIN 明确指定，MAX 自动检测（CPU 数量 * 2） |
| `0:0` | 动态 | `PHP_WORKERS=0:0` | MIN 自动（CPU/2，最少 2），MAX 自动（CPU * 2） |

**静态模式**下，池大小在启动后不会改变。工作线程使用阻塞 `recv()` 循环，空闲时零 CPU 开销。

**动态模式**下，后台 ScaleManager 任务定期检查工作线程利用率并生成或退役工作线程。工作线程使用 `recv_timeout(200ms)` 以允许定期检查关闭标志。

### 启动序列

`SapiExecutor::new(metrics)` 构造函数在生成任何工作线程之前在主线程上执行 PHP 初始化：

1. **TSRM 启动**：`php_tsrm_startup()` 初始化 Zend 线程安全。必须在任何异步运行时信号处理器安装之前在主线程上执行。
2. **SAPI 注册**：`sapi_startup()` 注册自定义 `oxphp` SAPI 模块。
3. **PHP 引擎启动**：`php_module_startup()` 初始化 PHP 引擎，加载扩展，解析 `php.ini`。这会触发所有扩展的 MINIT，包括注册插件函数到 Zend 的 OxPHP 扩展。
4. **错误回调**：`sapi::install_error_cb()` 用结构化 JSON 日志替换默认错误处理器。
5. **工作模式解析**：`parse_php_workers()` 读取 `PHP_WORKERS` 并返回 `WorkerMode::Static(n)` 或 `WorkerMode::Dynamic { min, max }`。
6. **Channel 创建**：`crossbeam_channel::bounded(queue_capacity)` 创建有界工作队列。容量默认为 `worker_count * 128`（动态模式使用 min 值）。
7. **工作线程生成**：生成初始工作线程 — 静态模式为全部数量，动态模式为 `min` 个。每个线程包装在 `ManagedWorker` 结构体中。
8. **指标初始化**：设置 `metrics.set_workers_min/max/current` 以反映初始池状态。

### ManagedWorker

每个工作线程由 `ManagedWorker` 结构体跟踪：

```rust
struct ManagedWorker {
    id: usize,                       // 唯一 ID（用于调试显示）
    handle: JoinHandle<()>,          // OS 线程句柄
    shutdown: Arc<AtomicBool>,       // 每线程关闭标志
    last_active: Arc<AtomicU64>,     // 上次请求的 epoch 毫秒（仅动态模式）
}
```

`shutdown` 标志允许在不关闭共享 channel 的情况下退役单个工作线程。`last_active` 时间戳由 ScaleManager 用于识别空闲工作线程以进行缩容。

### 工作线程生命周期

每个工作线程：

1. 通过 `ts_resource_ex()` 初始化 TSRM 线程本地存储
2. 进入接收循环（取决于模式）：
   - **静态模式**：阻塞 `while let Ok(wr) = request_rx.recv()` — 空闲时零 CPU
   - **动态模式**：`recv_timeout(200ms)` 并定期检查 `shutdown` 标志和更新 `last_active`
3. 对每个请求：
   - 通过 `sapi::clear_buffers()` 清除输出缓冲区
   - 通过 `sapi::set_request_data()` 设置请求数据（SAPI 状态、超全局变量）
   - 创建 `RequestDataGuard`（RAII — 即使 panic 也会在 drop 时清除 SAPI 数据）
   - 调用 `php_request_startup()`（触发所有扩展的 RINIT）
   - 用 `zend_stream_init_filename()` 打开脚本文件
   - 用 `php_execute_script()` 执行
   - 用 `zend_destroy_file_handle()` 销毁文件句柄
   - 调用 `php_request_shutdown()`（触发 RSHUTDOWN）
   - 收集响应：输出缓冲区、头、状态码，通过 `sapi::take_response()`
   - 在工作线程上将原始头字符串解析为类型化的 `HeaderName`/`HeaderValue` 对
   - 通过 oneshot channel 发送响应
4. 退出条件：
   - **静态模式**：channel 发送端被 drop（关闭），`recv()` 返回 `Err`
   - **动态模式**：`shutdown` 标志由 ScaleManager 设置，或 channel 断开

### ScaleManager（动态模式）

**静态模式**下，`start_scale_manager()` 生成一个工作线程健康监控任务而非无操作。健康监控器定期检查崩溃的工作线程（OS 线程意外退出的工作线程）并重新生成它们以维持配置的目标数量。这防止了崩溃的工作线程永久减少池容量。

当配置 `PHP_WORKERS=MIN:MAX` 时，`start_scale_manager()` 转而生成一个自动伸缩的 ScaleManager 任务。ScaleManager 在 Tokio 运行时上运行，每 500ms 检查一次工作线程利用率：

**扩容**（所有条件必须为真）：
- 检测到零个空闲工作线程（空闲 = last_active > 200ms 前）
- 当前工作线程数量低于 MAX
- 距上次扩容至少 500ms

**缩容**（所有条件必须为真）：
- 当前工作线程数量高于 MIN
- 某个工作线程空闲时间超过 `PHP_WORKERS_IDLE_SEC`（默认 30 秒）
- 距上次缩容至少 5 秒

ScaleManager 在生成新 OS 线程之前释放 Mutex 锁以避免阻塞 Tokio 运行时。退役的工作线程在后台线程中被 join。

### 配置

| 变量 | 默认值 | 描述 |
|---|---|---|
| `PHP_WORKERS` | `0`（CPU 数量 * 2，静态） | 工作线程池模式。`N` 为静态，`MIN:MAX` 为动态 |
| `PHP_WORKERS_IDLE_SEC` | `30` | 动态工作线程退役前的空闲超时 |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | 有界 channel 容量（动态模式使用初始数量） |

## 工作进程模式（持久化 PHP）

工作进程模式是一种替代执行模型，PHP 进程在请求间保持存活，避免每次请求都执行 `php_request_startup()` / `php_request_shutdown()` 的开销。通过设置 `WORKER_FILE` 环境变量启用。

### 工作原理

与标准的每请求生命周期（启动 → 执行 → 关闭）不同，工作进程模式执行一个调用 `oxphp_worker()` 并传入处理器回调的 PHP 脚本。处理器对每个请求被调用，请求之间进行**软重置**，在不销毁 PHP 堆的情况下清理每请求状态：

```
Worker thread lifecycle:

  php_request_startup()           ← 仅运行一次
  require worker.php              ← 引导：自动加载、数据库连接、配置
  oxphp_worker(function() {       ← 进入工作循环
      ┌─────────────────────────┐
      │ wait for request        │ ← 阻塞在 crossbeam channel 上
      │ soft reset              │ ← 重新填充超全局变量，清除输出
      │ call handler()          │ ← 执行用户代码
      │ send response           │ ← 响应发送到 HTTP 层
      │ check limits            │ ← max_requests、max_memory
      └─────────────────────────┘
      │ loop back ↑             │
  })
  php_request_shutdown()          ← 仅运行一次（退出时）
```

请求之间的软重置：
- 从新请求数据重新填充 `$_GET`、`$_POST`、`$_SERVER`、`$_COOKIE`、`$_FILES`
- 清除并重置输出缓冲区
- 重置 HTTP 响应头和状态码
- 调用并清除 `register_shutdown_function()` 处理器

### 回收

工作进程根据可配置的限制被回收（退出并重新生成）：

| 退出原因 | 触发条件 | 指标标签 |
|---|---|---|
| `max_requests` | 达到 `WORKER_MAX_REQUESTS` | `reason="max_requests"` |
| `max_memory` | 超过 `WORKER_MAX_MEMORY`（MiB） | `reason="max_memory"` |
| `error` | 处理器中的未捕获异常或致命错误 | `reason="error"` |
| `shutdown` | 服务器优雅关闭 | *(不计为回收)* |

当工作进程因非关闭原因退出时，健康监控器（静态模式）或 ScaleManager（动态模式）会自动重新生成它。新工作进程会重新执行整个工作脚本，包括引导代码。

### 指标

工作进程模式暴露专用的 Prometheus 指标用于监控持久化工作进程健康状况：

- **`oxphp_worker_requests_handled_total`** — 所有工作进程处理的请求总数
- **`oxphp_worker_recycles_total`** / **`oxphp_worker_recycles_by_reason_total`** — 回收计数（全局和按原因分类）
- **`oxphp_worker_memory_bytes{worker="N"}`** — 每工作进程的当前 PHP 堆使用量
- **`oxphp_worker_uptime_seconds{worker="N"}`** — 每工作进程生成后的时间
- **`oxphp_worker_request_duration_us`** — PHP 处理器执行时间直方图（不包括队列等待时间）

完整参考和 PromQL 查询示例请参见[指标](../operations/metrics.md#工作进程模式)。

### 配置

| 变量 | 默认值 | 描述 |
|---|---|---|
| `WORKER_FILE` | *(无)* | 工作 PHP 脚本路径（相对于 `DOCUMENT_ROOT`）。设置后启用工作进程模式 |
| `WORKER_MAX_REQUESTS` | `0` | 回收前的最大请求数。`0` = 不限制 |
| `WORKER_MAX_MEMORY` | `0` | 回收前的最大内存（MiB）。`0` = 不限制 |

### 路由集成

当设置了 `WORKER_FILE` 时，路由行为改变：不匹配磁盘文件的非静态文件请求将路由到工作脚本，而非返回 404。静态文件（CSS、JS、图片）仍直接从磁盘提供。这类似于 nginx 的 `try_files` 指令。

## 有界队列与背压

Tokio 与 PHP 工作线程之间的 channel 使用 `crossbeam_channel::bounded(QUEUE_CAPACITY)`。执行器调用 `try_send()`（非阻塞）来入队请求：

```rust
if let Err(e) = self.request_tx.as_ref().unwrap().try_send(worker_request) {
    let (status, body) = match e {
        TrySendError::Full(_) => (503, "Service Unavailable: queue full"),
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

| 条件 | 行为 |
|---|---|
| 队列有空间 | 请求入队，Tokio 任务等待 oneshot 响应 |
| 队列已满 | 立即返回 503 Service Unavailable，带 `Retry-After: 1` 头 |
| 工作线程断开 | 500 Internal Server Error（工作线程池已停止） |

此设计提供背压：当 PHP 工作线程跟不上时，新请求立即被拒绝而非无限排队。`Retry-After: 1` 头信号客户端在短暂延迟后重试。

### 指标

连接处理器通过 `Metrics` 结构体跟踪队列状态：

| 方法 | 时机 |
|---|---|
| `metrics.request_queued()` | 在 `executor.execute()` 之前 |
| `metrics.request_dequeued()` | 当 oneshot 响应到达时 |
| `metrics.request_dropped()` | 当 oneshot channel 断开时（工作线程崩溃） |

这些以 Prometheus gauge/counter 暴露：`oxphp_pending_requests`、`oxphp_busy_workers`、`oxphp_dropped_requests_total`。

## StubExecutor

`StubExecutor`（`src/executor/stub.rs`）是零开销的测试和基准测试后端。它同步返回硬编码的 200 OK 响应，不生成任何线程：

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

设置 `EXECUTOR=stub` 使用 stub 执行器。在不带 `--features php` 编译时自动激活。

## 执行器选择

`src/executor/mod.rs` 中的 `create_executor()` 工厂根据 `EXECUTOR` 环境变量和编译时 feature 选择后端：

| `EXECUTOR` | `--features php` | 结果 |
|---|---|---|
| `sapi`（默认） | yes | `SapiExecutor`（PHP 工作线程池） |
| `sapi`（默认） | no | `StubExecutor`（带警告的回退） |
| `stub` | any | `StubExecutor`（基准测试模式） |

## 关闭

`SapiExecutor` 实现 `Drop` 以进行有序关闭：

1. **全局关闭标志**：`global_shutdown.store(true)` — 停止 ScaleManager（如果正在运行）
2. **释放 channel 发送端**：工作线程看到 `recv()` 返回 `Err`（静态）或断开（动态）并退出循环
3. **每线程关闭**：设置每个工作线程的 `shutdown` 标志，确保动态工作线程退出超时循环
4. **Join 所有工作线程**：阻塞直到每个工作线程完成当前请求
5. **PHP 清理**：依次调用 `php_module_shutdown()`、`sapi_shutdown()`、`tsrm_shutdown()`

这保证了关闭期间没有 PHP 请求被中途中断。

## 另请参阅

- [架构概览](./overview.md) — 高层组件全景与启动序列
- [SAPI 与 Bridge](./sapi-bridge.md) — PHP 工作线程如何与 bridge 库交互
- [请求生命周期](./request-lifecycle.md) — 请求如何从 Tokio 流向 PHP 工作线程
- [配置](../operations/configuration.md) — `PHP_WORKERS`、`QUEUE_CAPACITY` 等环境变量
- [指标](../operations/metrics.md) — 工作线程池指标（pending、busy、dropped）
- [优雅关闭](../operations/graceful-shutdown.md) — 排空行为与工作线程清理
