<p align="center">
  <img src="logo.svg" alt="OxPHP" width="300">
</p>

<h3 align="center">为云原生基础设施打造的多线程 PHP 应用服务器。</h3>

<p align="center">
  OxPHP 是一个用 Rust 编写的异步 PHP 应用服务器 ——<br>
  专为对低延迟、高并发和零配置可观测性有严格要求的生产工作负载而构建。
</p>

<p align="center">
  <a href="docs/zh/">文档</a> · <a href="docs/en/">EN</a> · <a href="docs/ru/">RU</a> · <a href="README.md">README EN</a> · <a href="README.ru.md">README RU</a>
  <br>
  <a href="#快速开始">快速开始</a> · <a href="#为什么选择-oxphp">为什么选择 OxPHP</a> · <a href="#配置">配置</a>
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/rust-powered-orange">
  <img alt="PHP" src="https://img.shields.io/badge/php-8.4-blue">
  <img alt="License" src="https://img.shields.io/github/license/oxphp/oxphp">
  <img alt="Release" src="https://img.shields.io/github/v/release/oxphp/oxphp">
  <img alt="Stars" src="https://img.shields.io/github/stars/oxphp/oxphp?style=flat">
  <img alt="Docker" src="https://img.shields.io/badge/docker-ghcr.io-2496ED?logo=docker&logoColor=white">
  <img alt="HTTP/2" src="https://img.shields.io/badge/HTTP%2F2-supported-brightgreen">
  <img alt="TLS" src="https://img.shields.io/badge/TLS-1.3-brightgreen">
</p>

---

## 快速开始

两行命令，仅此而已。

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.2.0

COPY --chown=www-data:www-data . /var/www/html
```

> **注意：** 默认情况下，`DOCUMENT_ROOT` 为 `/var/www/html/public`。请将入口脚本（如 `index.php`）放在 `public/` 子目录中 —— OxPHP 将从该目录提供文件服务，而非 `/var/www/html` 根目录。这与 Laravel、Symfony 和 Slim 等框架的标准目录布局一致，开箱即用。

```bash
docker build -t my-app . && docker run -p 8080:8080 my-app
curl http://localhost:8080/
```

无需 nginx 配置。无需 PHP-FPM 进程池调优。无需进程管理器。只需你的应用。

---

## 为什么选择 OxPHP？

OxPHP 用一个容器替代 nginx + PHP-FPM。服务器开箱即用 —— TLS、Brotli 压缩、限流、Prometheus 指标、健康检查和结构化 JSON 日志均通过环境变量配置。

| | nginx + PHP-FPM | FrankenPHP | RoadRunner | **OxPHP** |
|---|---|---|---|---|
| Language | C / C | Go + C | Go | **Rust** |
| HTTP/2 | ✅ | ✅ | ✅ | ✅ |
| TLS built-in | ✅ | ✅ | ✅ | ✅ (rustls, TLS 1.3) |
| Worker mode | ❌ | ✅ | ✅ | ✅ |
| Backpressure / 529 | manual | ❌ | ❌ | ✅ built-in |
| Prometheus metrics | plugin | plugin | plugin | ✅ built-in |
| Per-IP rate limiting | nginx module | ❌ | ❌ | ✅ built-in |
| Custom error pages | ✅ (nginx config) | ✅ (Caddyfile) | ❌ | ✅ preloaded at startup |
| HTTP/3 | ✅ | ✅ | ✅ experimental | 🔜 roadmap |
| HTTP 103 Early Hints | ✅ (v1.29+) | ✅ | ✅ | 🔜 roadmap |
| Memory safety | ❌ | partial | partial | ✅ Rust |

详细功能介绍请参阅[文档](docs/zh/index.md)。

---

## 基准测试

> 正式基准测试即将推出。我们正在开发一套可复现的测试套件，涵盖 req/s、延迟（p50/p99）、内存使用以及并发负载下的工作池吞吐量。

---

## 功能特性

### PHP 运行时
- **原生 PHP 执行** — 通过自定义 SAPI（`oxphp`）配合 ZTS 工作池运行
- **完整超全局变量**支持：`$_SERVER`、`$_GET`、`$_POST`、`$_COOKIE`、`$_FILES`、`php://input`
- **HTTP Object API** — `oxphp_http_request()` 返回类型化、惰性加载的请求对象，内置 JSON 请求体解析、基于文件内容的 MIME 类型检测以及用于中间件的可变属性容器；参见 [HTTP Request API 文档](docs/zh/php/request-api.md)
- **原生 Rust↔PHP 桥接** — 通过 C 访问函数直接操作 `zval`，零序列化开销
- **插件系统** — 支持类型化事件分发、优先级排序及 PHP 函数注册
- **基于属性的装饰器** — 通过 PHP 8+ 属性拦截函数/方法调用，对未装饰代码零开销；支持 `TARGET_FUNCTION`、`TARGET_METHOD`、`TARGET_CLASS`
- **Panic 隔离** — 通过 `catch_unwind` 确保 PHP 崩溃不影响服务器整体运行

### 工作进程模型
- **工作进程模式** — 持久化 PHP 进程，请求间软重置，保持自动加载器和数据库连接跨请求存活
- **Fiber 多路复用** — 每个工作线程通过 PHP 8.4 Fiber 处理多个并发请求；`oxphp_sleep()` 和 `oxphp_async_await()` 让出 Fiber 而非阻塞工作线程
- **自动回收** — 按请求数或内存阈值自动回收工作进程
- **工作线程健康监控** — 自动检测崩溃线程并重启
- **提前响应** — 通过 `oxphp_finish_request()` 立即发送响应并继续后台处理

### 异步 Promise
- **`oxphp_async()` / `oxphp_async_await()`** — 将闭包分发到专用线程池进行真正的并行执行
- **可移植序列化** — `use` 变量、参数和返回值安全跨线程二进制传输
- 支持类型：标量、字符串、数组（嵌套）。资源和对象将被拒绝并触发 `E_WARNING`
- **异常与 die() 安全** — 异常、`die()` 和 `exit()` 被捕获并重新抛出为 `OxPHP\Async\Exception`
- **超时支持** — 每任务超时，抛出 `OxPHP\Async\TimeoutException`
- **`oxphp_async_await_all()` / `oxphp_async_await_any()`** — 批量等待和竞速原语

### HTTP 与网络
- **HTTP/1.1 + HTTP/2** 自动协商（h2c），基于 hyper 实现
- **TLS 1.3**，支持 ALPN（h2 + http/1.1），基于 rustls 实现
- **3 种路由模式** — 传统模式、框架模式（`index.php`）、SPA 模式（`index.html`）
- **SSE 流式传输** — 通过自动检测 `Content-Type: text/event-stream` 或 `oxphp_stream_flush()` 实现 —— 与 Fiber 多路复用协作运行
- **可配置超时** — 请求头读取、整体请求及 keep-alive 超时

### 性能
- **LRU 文件缓存** — 静态文件内存缓存（≤1 MB 完整缓存，更大文件流式传输）
- **HTTP 缓存** — 支持 ETag、Last-Modified 和 304 Not Modified 条件请求
- **Brotli 压缩** — 对文本响应启用（范围：256 B – 3 MB）
- **mimalloc** 分配器 — 降低高并发下的内存分配延迟
- **可配置 Tokio 运行时** — 默认多线程（CPU / 2），可通过 `TOKIO_WORKERS` 调整

### 可观测性
- **W3C Trace Context** — 自动传播 `traceparent`/`tracestate`，`$_SERVER['OXPHP_TRACE_ID']` 用于 PHP 日志关联
- **OpenTelemetry** — 通过 `plugin-otel` 特性进行 OTLP Span 导出（gRPC/HTTP），支持语义化约定、可配置采样和批处理
- **APM 自动埋点** — 在引擎层面 hook 33 个 PHP 内部函数（PDO、mysqli、cURL、Redis、Memcached、文件 I/O）；每次调用自动成为 Span，无需修改代码
- **`#[OxPHP\Tracing\Trace]` 装饰器** — 通过 PHP 8 属性注解任意函数或方法，自动创建 Span
- **PHP 追踪 SDK** — 10 个 `oxphp_trace_*()` 函数，支持手动创建 Span、设置属性、记录事件、错误记录和追踪上下文传播
- **Prometheus 指标** — 通过 `/metrics` 暴露，按工作进程统计，零外部依赖
- **健康检查**端点 `/health` — 支持 K8s 就绪探针
- **结构化错误日志** — PHP 错误通过 `tracing` 输出，包含 `php_error_type`、`php_file`、`php_line` 字段
- **JSON 访问日志** — 可选 `trace_id`/`span_id` 字段（级别：`all`、`error`，通过 `ACCESS_LOG` 控制）
- **请求 ID** 生成与透传（`X-Request-ID`）；OTel 启用时使用追踪衍生格式

### 可靠性与运维
- **有界请求队列** — 队列满时返回 529 进行背压控制
- **基于 IP 的限流** — 携带 `X-RateLimit-*` 响应头，超限返回 429
- **受信任代理** — 通过 CIDR 信任从 `Forwarded`（RFC 7239）和 `X-Forwarded-*` 头中提取真实客户端 IP
- **自定义错误页面** — 启动时预加载，热路径零 I/O
- **路径穿越防护** — 包含符号链接逃逸检测
- **非 root 容器**运行 — 以 www-data（UID 82）身份执行

---

## 架构

```
                    ┌──────────────┐
                    │  Tokio async │  configurable: single- or multi-threaded
                    │  HTTP server │  (hyper + hyper-util + mimalloc)
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │Route dispatch│  static file / PHP / 404
                    └──────┬───────┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Static file   PHP request   Not found
         (LRU cache)   (channel)      (404)
                           │
                    ┌──────▼───────┐
                    │Bounded queue │  crossbeam bounded channel
                    │(backpressure)│  529 when full
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         PHP Worker   PHP Worker   PHP Worker    OS threads (ZTS)
         (SAPI exec)  (SAPI exec)  (SAPI exec)   with thread-local state
         ──────────────────┬──────────────────
                           │
                    ┌──────▼───────┐
                    │ Async pool   │  oxphp_async() / oxphp_async_await()
                    │(crossbeam ch)│  dedicated OS threads (ZTS)
                    └──────┬───────┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Async Worker  Async Worker  Async Worker
```

- **Tokio 异步运行时** — 默认多线程，可通过 `TOKIO_WORKERS` 调整
- **ZTS 工作池** — 每个工作线程为独立操作系统线程，通过 `catch_unwind` 实现故障隔离
- 工作线程通过 `crossbeam::bounded` 接收请求，通过 `ExecuteResult`（即时或经由 `oneshot` 延迟）返回结果
- **异步池** — 独立操作系统线程用于 `oxphp_async()` 任务，防止与 HTTP 池死锁
- **工作进程模式** — 持久化 PHP 进程，请求间软重置；保持引导状态（自动加载器、数据库连接）跨请求存活

### 内部服务器

设置 `INTERNAL_ADDR` 后，将在独立端口上启动一个轻量 HTTP 服务器：

| 端点 | 描述 |
|----------|-------------|
| `GET /health` | JSON 格式健康状态（运行时长、请求数、连接数） |
| `GET /metrics` | Prometheus 文本格式指标 |
| `GET /config` | JSON 格式运行时配置（TLS 路径已脱敏） |

---

## 配置

所有配置均通过环境变量设置 —— 无需配置文件。

| 变量 | 默认值 | 描述 |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | 监听地址和端口 |
| `DOCUMENT_ROOT` | `/var/www/html/public` | 静态文件服务的根目录路径 |
| `INDEX_FILE` | *(未设置)* | 路由模式：空 = 传统模式，`index.php` = 框架模式，`index.html` = SPA 模式 |
| `TOKIO_WORKERS` | `0`（CPU / 2，最少 1） | 异步 I/O 线程数；`0` = 自动 |
| `EXECUTOR` | `sapi` | PHP 执行器：`sapi`（真实 PHP）或 `stub`（测试模式） |
| `PHP_WORKERS` | `0`（CPU / 2，最少 1） | 工作池模式：`N` = 固定数量，`MIN:MAX` = 动态伸缩，`0` = 自动 |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | 动态模式下工作线程的空闲超时时间 |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | 有界队列大小；队列满时返回 529 |
| `DRAIN_TIMEOUT_SECONDS` | `30` | 优雅关闭的排空等待超时 |
| `LOG_LEVEL` | `info` | 日志级别：`error`、`warn`、`info`、`debug`、`trace` |
| `INTERNAL_ADDR` | *(未设置)* | 内部服务器地址，用于健康检查/指标/配置（例如 `0.0.0.0:9090`） |
| `RATE_LIMIT` | `0`（关闭） | 每个 IP 每个时间窗口内的最大请求数 |
| `RATE_WINDOW_SECONDS` | `60` | 限流时间窗口（秒） |
| `HEADER_TIMEOUT_SECONDS` | `5` | 请求头读取超时（Slowloris 防护） |
| `REQUEST_TIMEOUT_SECONDS` | `120` | 整体请求超时；`0` 表示禁用 |
| `TLS_CERT` | *(未设置)* | TLS 证书 PEM 文件路径 |
| `TLS_KEY` | *(未设置)* | TLS 私钥 PEM 文件路径 |
| `ERROR_PAGES_DIR` | *(未设置)* | 自定义错误页面目录（文件名格式：`{status}.html`） |
| `STATIC_CACHE_TTL` | `30d` | 静态文件缓存 TTL（`30s`、`5m`、`2h`、`30d`、`1y`、`off`） |
| `STATIC_CACHE` | *(开启)* | 设为 `off` 启用内存内容缓存的 mtime 重新验证 |
| `COMPRESSION_LEVEL` | `4` | Brotli 压缩质量（0 = 关闭，1-11） |
| `ACCESS_LOG` | *(关闭)* | 每请求 JSON 日志：`all`、`error`，或不设置 |
| `MAX_CONNECTIONS` | `10000` | 最大并发连接数 |
| `WORKER_FILE` | *(未设置)* | 工作进程 PHP 脚本路径；设置后启用持久化工作进程模式 |
| `WORKER_MAX_REQUESTS` | `0`（无限制） | 每个工作进程回收前的最大请求数 |
| `WORKER_MAX_MEMORY_MIB` | `0`（无限制） | 每个工作进程回收前的最大内存（MiB） |
| `ASYNC_WORKERS` | `0`（禁用） | `oxphp_async()` 专用异步工作线程数 |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS * 64` | 异步任务有界队列；队列满时拒绝任务 |
| `SPLIT_PATH_INFO_ENABLED` | `false` | 对 `/script.php/extra/path` 形式的 URI 启用 PATH_INFO 拆分（旧版 CGI 兼容） |
| `TRACE_CONTEXT` | `false` | W3C Trace Context 传播（`traceparent`/`tracestate`）。当 `OTEL_ENABLED=true` 时自动启用 |
| `TRUSTED_PROXIES` | *（未设置）* | 受信任代理 CIDR 列表：`10.0.0.0/8,172.16.0.0/12` 或 `private`（所有 RFC-1918）。从 `Forwarded`/`X-Forwarded-*` 头中提取真实客户端 IP |

### OpenTelemetry（`plugin-otel` 特性）

| 变量 | 默认值 | 描述 |
|---|---|---|
| `OTEL_ENABLED` | `false` | 启用 Span 导出。隐含 `TRACE_CONTEXT=true` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP 收集器端点 |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | 导出协议：`grpc`（端口 4317）或 `http/protobuf`（端口 4318） |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | 导出超时（毫秒） |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(未设置)* | 托管后端的认证头（`key=value,key=value`） |
| `OTEL_SERVICE_NAME` | `oxphp` | 导出追踪中的服务名称 |
| `OTEL_SERVICE_VERSION` | *(未设置)* | 导出追踪中的服务版本 |
| `OTEL_RESOURCE_ATTRIBUTES` | *(未设置)* | 资源属性（`key=value,key=value`） |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | 采样器：`always_on`、`always_off`、`traceidratio`、`parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | 采样比率（0.0-1.0） |

### APM（`plugin-apm` 特性）

| 变量 | 默认值 | 描述 |
|---|---|---|
| `OTEL_APM_ENABLED` | `false` | 启用 APM：自动埋点、错误捕获、PHP 追踪 SDK。需要 `OTEL_ENABLED=true` |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | 慢查询阈值（毫秒）。超过此值的查询将标记 `oxphp.db.slow=true` |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | 将绑定参数记录到 `db.params` Span 属性中 |

---

## 构建

```bash
# 宿主机（不含 PHP — 所有测试通过，无 PHP 执行）
cargo build --release

# Docker（含 PHP — 完整功能）
docker compose build
```

### 本地运行（仅静态文件）

```bash
DOCUMENT_ROOT=./www/public ./target/release/oxphp
```

## 开发

```bash
# 完整验证（宿主机）
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Docker 冒烟测试
docker compose build && docker compose up -d
curl http://localhost:8080/
curl "http://localhost:8080/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost:8080/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost:8080/test_superglobals.php

# 异步 Promise
curl http://localhost:8080/test_async.php
curl http://localhost:8080/test_async_parallel.php
curl http://localhost:8080/test_async_die.php

# 内部服务器
INTERNAL_ADDR=127.0.0.1:9090 ./target/release/oxphp &
curl http://localhost:9090/health
curl http://localhost:9090/metrics
```

---

## 路线图

> 以下项目未按优先级排序。列入此表并不代表一定会实现。

| Feature | 描述 |
|---|---|
| **PHP 8.5** | 支持 PHP 8.5 |
| ~~**Trace Context (W3C)**~~ | ✅ 已实现 — 自动传播 `traceparent` / `tracestate` 头（W3C 规范），通过 `TRACE_CONTEXT=true` 启用 |
| ~~**OpenTelemetry**~~ | ✅ 已实现 — 通过 `plugin-otel` 特性进行 OTLP 追踪导出，W3C context 传播，每请求 Span 支持标准语义化约定 |
| ~~**APM & Auto-Instrumentation**~~ | ✅ 已实现 — `plugin-apm` 特性：自动追踪 33 个 PHP 内部函数（PDO、mysqli、cURL、Redis、Memcached、文件 I/O），`#[OxPHP\Tracing\Trace]` 装饰器，10 个 `oxphp_trace_*()` SDK 函数，PHP 错误捕获 |
| **Custom Metrics** | 提供 PHP API，允许从用户代码注册应用自定义的 Prometheus 指标 |
| **Built-in PHP Profiler** | 通过属性装饰器（`#[Timer]`、`#[Span]`）实现低开销性能分析，与服务器指标和追踪直接集成 |
| **Dockerfile.bookworm** | 提供基于 Debian Bookworm 的官方镜像，作为 Alpine 的替代方案 |
| **Non-Docker Install** | 通过系统包管理器（apt、brew 等）原生安装 |
| **HTTP/3** | 基于 QUIC 的 HTTP/3 支持 |
| **HTTP 103 Early Hints** | 发送 `103 Early Hints` 响应，允许客户端在最终响应前预加载资源 |
| **Ecosystem Plugins** | 扩展插件系统：更多生命周期钩子、更丰富的 PHP API，以及第三方插件作者文档 |
| ~~**Shared Async Runtime**~~ | ✅ 已实现 — Tokio 运行时驱动 `oxphp_async()` / `oxphp_async_await()`，支持超时、结果传递和竞争协调 |
| **Database Connection Pool** | 通过 `sqlx` 提供内置连接池，减少每请求的连接建立开销 |
| **gRPC Server** | *(探索性)* 替代服务器模式 —— gRPC 而非 HTTP；高度不确定，可能不会实现 |
| ~~**Promise API**~~ | ✅ 已实现 — `oxphp_async()` / `oxphp_async_await()`，支持专用线程池、可移植序列化和异常安全 |
| ~~**Fiber Multiplexing**~~ | ✅ 已实现 — 每个工作线程通过 PHP 8.4 Fiber 处理多个并发请求；`oxphp_sleep()` / `oxphp_usleep()` 和 `oxphp_async_await()` 协作式让出 Fiber |
| **Diagnostics** | 生产诊断工具：检查操作系统限制（ulimit、TCP backlog、epoll/kqueue、容器设置），识别性能瓶颈（工作队列深度、锁竞争、GC/内存分配压力、ZTS 统计），并给出针对性的可操作建议 |

## 文档

- [English](docs/en/)
- [Русский](docs/ru/)
- [中文](docs/zh/)

## 许可证

[AGPL-3.0](LICENSE)
