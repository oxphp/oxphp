---
title: 架构概览
description: OxPHP 高层架构 — 异步 I/O 运行时、PHP 工作线程池与组件全景
---

OxPHP 是一个单二进制 HTTP 服务器，替代传统的 nginx + PHP-FPM 技术栈。它将异步 I/O 运行时（Rust/Tokio）与多线程 PHP 工作线程池（ZTS）整合在同一个进程中。

## 设计原则

- **单一二进制文件**：无需外部进程管理器，无需 sidecar。一个二进制文件处理 TCP、TLS、HTTP 解析、路由、PHP 执行和可观测性。
- **异步 I/O + 同步 PHP**：网络 I/O 在异步运行时上多路复用，PHP 脚本在专用 OS 线程上运行，两部分通过 channel 通信。
- **尽可能零拷贝**：请求数据在管道中传递时避免不必要的克隆。使用 `Bytes`、`Arc` 和 `std::mem::take` 在热路径上减少分配。

## 运行时模型

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

**Tokio 运行时** 通过 `TOKIO_WORKERS` 进行配置。设为 `0`（默认）时自动检测 CPU / 2（最少 1）个工作线程；设为 `1` 时使用 `Builder::new_current_thread()` 创建单线程异步运行时；设为 `N`（N > 1）时使用 `Builder::new_multi_thread()` 创建 N 个工作线程以获得更高吞吐量。它处理所有异步工作：接受 TCP 连接、TLS 握手、HTTP 解析、路由、压缩和事件分发。每个连接是一个轻量级 Tokio 任务。进程使用 mimalloc 作为全局分配器，在线程竞争下降低分配延迟。

**PHP 工作线程池** 是一组专用 OS 线程。每个线程拥有一个 PHP ZTS（Zend 线程安全）解释器实例。工作线程通过有界 `crossbeam_channel::bounded` channel 接收 `ScriptRequest` 结构体，并通过 `tokio::sync::oneshot` channel 返回 `ScriptResponse`。

### 为什么不在 Tokio 内运行多线程 PHP？

PHP 的 C 运行时不是异步安全的。`php_request_startup()` 和 `php_execute_script()` 等函数会阻塞调用线程并进行非线程安全的全局状态修改。在 Tokio 工作线程上运行它们会导致异步运行时饥饿。专用 OS 线程将 PHP 的阻塞行为与异步 I/O 循环隔离开来。

## 组件全景

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

### 核心组件

| 组件 | 位置 | 用途 |
|---|---|---|
| **Config** | `src/config/` | 启动时从环境变量读取所有配置 |
| **Server** | `src/server/mod.rs` | 持有连接接受循环、hyper-util 构建器和关闭标志 |
| **RouteConfig** | `src/server/routing.rs` | 将 URI 路径解析为 `Serve`、`Execute` 或 `NotFound` |
| **FileCache** | `src/server/response/static_file.rs` | 文件元数据和规范路径查找的 LRU 缓存 |
| **ScriptExecutor** | `src/executor/mod.rs` | PHP 执行后端 trait（`SapiExecutor`、`StubExecutor`）；`execute()` 返回 `ExecuteResult` |
| **Metrics** | `src/metrics.rs` | 无锁原子计数器（Prometheus 输出格式） |
| **EventDispatcher** | `src/events/dispatcher.rs` | 类型化、按优先级排序的同步事件分发 |
| **PluginManager** | `src/plugin/mod.rs` | 插件生命周期管理，含拓扑排序 |
| **RateLimiter** | `src/server/rate_limit.rs` | 基于 `DashMap` 的每 IP 滑动窗口限流 |
| **Compression** | `src/server/compression.rs` | 文本类响应的 Brotli 压缩 |

## 模块结构

```
src/
├── main.rs                  # 入口点，Tokio 运行时，接受循环，关闭
├── lib.rs                   # 公开模块导出
├── types.rs                 # ScriptRequest, ScriptResponse, ResponseBody, BoxError
├── logging.rs               # 基于 tracing 的 JSON 结构化日志
├── metrics.rs               # 无锁原子 Prometheus 指标
├── config/
│   ├── mod.rs               # Config 聚合，from_env()
│   └── server.rs            # ServerConfig：地址、超时、路径
├── server/
│   ├── mod.rs               # Server 结构体，连接处理，关闭
│   ├── connection.rs        # 请求管道：事件 → 路由 → 执行 → 响应
│   ├── routing.rs           # RouteConfig，3 种路由模式，路径清理
│   ├── compression.rs       # Brotli 压缩（质量 4，256 B – 3 MB）
│   ├── rate_limit.rs        # 每 IP 滑动窗口（DashMap）
│   ├── tls.rs               # 基于 rustls + tokio-rustls 的 TLS
│   ├── error_pages.rs       # 自定义 HTML 错误页（启动时加载）
│   ├── internal.rs          # 内部服务器（/health, /metrics, /config）
│   └── response/
│       └── static_file.rs   # 静态文件服务，含 MIME 检测和缓存
├── executor/
│   ├── mod.rs               # ScriptExecutor trait (execute() → ExecuteResult)，create_executor() 工厂
│   ├── stub.rs              # StubExecutor（返回 200 OK，用于基准测试）
│   └── sapi.rs              # SapiExecutor（PHP ZTS 工作线程池）[feature-gated]
├── events/
│   ├── mod.rs               # Event trait, Priority, Propagation, EventHandler trait
│   ├── types.rs             # 18 个具体事件结构体
│   └── dispatcher.rs        # 使用恒等哈希的类型擦除分发器
├── handlers/
│   ├── mod.rs               # Handler 模块导出
│   ├── request_id.rs        # 生成或保留 X-Request-ID
│   ├── rate_limit.rs        # 将 RateLimiter 封装为事件处理器
│   ├── metrics.rs           # 记录请求/响应指标
│   ├── error_pages.rs       # 用自定义 HTML 替换错误响应体
│   ├── server_header.rs     # 添加 Server 和 X-Request-ID 头
│   └── access_log.rs        # 通过 tracing 的结构化访问日志
├── plugin/
│   ├── mod.rs               # Plugin trait
│   ├── context.rs           # PluginContext
│   ├── cookies.rs           # 插件 cookie 隔离
│   ├── handler.rs           # Handler traits
│   ├── macros.rs            # 插件辅助宏
│   ├── manager.rs           # PluginManager
│   ├── php.rs               # PHP 函数注册
│   └── wrappers.rs          # 事件处理器包装器
├── plugins/
│   └── example.rs           # 示例插件 [feature-gated: plugin-example]
└── php/                     # PHP FFI 绑定 [feature-gated]
    ├── bindings.rs
    └── sapi.rs
```

## 通信 Channel

OxPHP 的异步和同步两半通过两种 channel 类型通信：

| Channel | 方向 | 类型 | 用途 |
|---|---|---|---|
| `crossbeam_channel::bounded` | Tokio → PHP 工作线程 | `ScriptRequest` | 带背压的有界队列（满时返回 503） |
| `tokio::sync::oneshot` | PHP 工作线程 → Tokio | `ScriptResponse` | 每请求单次响应 |

`ScriptExecutor::execute()` 返回 `ExecuteResult` 枚举而非裸的 `oneshot::Receiver`。这使得执行器在队列已满或工作线程池不可用时，可以不经过工作线程直接返回错误响应：

```rust
pub enum ExecuteResult {
    Immediate(ScriptResponse),
    Deferred(tokio::sync::oneshot::Receiver<ScriptResponse>),
}
```

此模式让 Tokio 运行时可以在不阻塞的情况下将工作分派给 PHP 工作线程，并异步等待响应。详见[工作线程池](./worker-pool.md)。

## 启动序列

1. **Config**：`Config::from_env()` 读取所有环境变量
2. **Metrics**：`Metrics::new()` 初始化无锁原子计数器（在执行器之前创建，以便启动期间可以记录工作线程指标）
3. **插件管理器**：`PluginManager::new()` 创建管理器，添加插件，然后 `init_all()` 在分发器上注册插件事件处理器并填充 bridge 插件函数注册表
4. **插件 PHP 函数**：插件函数传递给 `sapi::register_plugin_functions()`，以便在 PHP 引擎启动前填充 bridge 注册表
5. **执行器**：`create_executor(metrics)` 初始化 TSRM，注册 SAPI 模块，启动 `php_module_startup()`（触发 MINIT — 向 Zend 注册插件函数），解析 `PHP_WORKERS` 模式，生成初始 PHP 工作线程
6. **Tokio 运行时**：由 `TOKIO_WORKERS` 配置 — `0` 自动检测（CPU / 2，最少 1），`1` 创建单线程运行时，`N` 创建 N 个工作线程的多线程运行时
7. **伸缩管理器**：`executor.start_scale_manager()` 启动工作线程伸缩任务（静态模式下无操作）。静态模式下，后台健康监控器检测并重启崩溃的工作线程
8. **限流器**：可选，带后台清理任务
9. **TLS**：可选，通过 `rustls` 加载证书和密钥
10. **事件分发器**：注册内置处理器（注意：`AccessLogHandler` 仅在 `config.access_log` 启用时注册），然后 `freeze()` 按优先级排序
11. **TCP 监听器**：绑定到配置的地址
12. **内部服务器**：可选，在单独端口上提供 `/health`、`/metrics`、`/config`
13. **插件就绪**：`plugin_manager.on_ready_all()` 通知插件服务器已开始监听
14. **接受循环**：为每个连接生成一个 Tokio 任务，由 `Semaphore(max_connections)` 限制并发

## 关闭序列

1. SIGTERM 或 Ctrl+C 触发 `shutdown_signal()`
2. `plugin_manager.shutdown_all()` 通知插件，然后 `server.shutdown()` 设置原子关闭标志并调用 `executor.shutdown()`
3. 接受循环在 `is_shutdown()` 时中断
4. 排空阶段：最多等待 `drain_timeout_seconds`（默认 30 秒）让进行中的连接完成
5. 内部服务器任务被中止
6. `SapiExecutor::drop()` 释放 channel 发送端，等待所有工作线程结束，然后依次调用 `php_module_shutdown()`、`sapi_shutdown()` 和 `tsrm_shutdown()`

## 另请参阅

- [工作线程池](./worker-pool.md) — PHP 工作线程、伸缩与背压
- [事件系统](./event-system.md) — 类型化事件分发与处理器注册
- [请求生命周期](./request-lifecycle.md) — 逐步请求管道详解
- [SAPI 与 Bridge](./sapi-bridge.md) — 自定义 PHP SAPI 与 C bridge 库
- [配置](../operations/configuration.md) — 环境变量参考
- [路由](../features/routing.md) — 三种路由模式（传统、框架、SPA）
- [优雅关闭](../operations/graceful-shutdown.md) — 排空行为与超时
