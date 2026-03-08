<p align="center">
  <img src="logo.svg" alt="OxPHP" width="300">
</p>

<p align="center">
  用 Rust 编写的异步 PHP 应用服务器。以单一二进制文件取代 nginx + PHP-FPM，处理 HTTP 请求、通过自定义 SAPI 原生执行 PHP，并提供内置可观测性支持。
</p>

## 功能特性

- **原生 PHP 执行** — 通过自定义 SAPI（`oxphp`）配合 ZTS 工作池运行
- **完整超全局变量**支持：`$_SERVER`、`$_GET`、`$_POST`、`$_COOKIE`、`$_FILES`、`php://input`
- **原生 Rust↔PHP 桥接** — 通过 C 访问函数直接操作 `zval`，零序列化开销
- **插件系统** — 支持类型化事件分发、优先级排序及 PHP 函数注册
- **结构化错误日志** — PHP 错误通过 `tracing` 输出，包含 `php_error_type`、`php_file`、`php_line` 字段
- **HTTP/1.1 + HTTP/2** 自动协商（h2c），基于 hyper 实现
- **TLS 1.3**，支持 ALPN（h2 + http/1.1），基于 rustls 实现
- **3 种路由模式** — 传统模式、框架模式（`index.php`）、SPA 模式（`index.html`）
- **LRU 文件缓存** — 静态文件内存缓存（≤1 MB 完整缓存，更大文件流式传输）
- **HTTP 缓存** — 支持 ETag、Last-Modified 和 304 Not Modified 条件请求
- **Brotli 压缩** — 对文本响应启用（范围：256 B – 3 MB）
- **有界请求队列** — 队列满时返回 503 进行背压控制
- **基于 IP 的限流** — 携带 `X-RateLimit-*` 响应头，超限返回 429
- **可配置超时** — 请求头读取、整体请求及 keep-alive 超时
- **Prometheus 指标** — 通过内部服务器 `/metrics` 端点暴露
- **健康检查**端点 `/health`，支持 K8s 就绪探针
- **请求 ID** 生成与透传（`X-Request-ID` 请求头）
- **访问日志** — 通过结构化 JSON tracing 输出（级别：`all`、`error`，通过 `ACCESS_LOG` 控制）
- **自定义错误页面** — 启动时预加载，热路径零 I/O
- **JSON 结构化日志** — 基于 tracing
- **路径穿越防护** — 包含符号链接逃逸检测
- **非 root 容器**运行 — 以 www-data（UID 82）身份执行
- **mimalloc** 分配器 — 降低高并发下的内存分配延迟
- **可配置 Tokio 运行时** — 默认自动（CPU / 2，最少 1），可通过 `TOKIO_WORKERS` 调整
- **工作线程健康监控** — 自动检测并重启崩溃的工作线程
- **SSE 流式传输** — 通过自动检测 `Content-Type: text/event-stream` 或 `oxphp_stream_flush()` 实现实时 Server-Sent Events
- **提前响应** — 通过 `oxphp_finish_request()` 立即发送响应并继续后台处理
- **工作进程模式** — 持久化 PHP 进程，请求间软重置，根据请求数或内存自动回收，并提供每工作进程的 Prometheus 指标
- **Panic 隔离** — 通过 `catch_unwind` 确保 PHP 崩溃不影响服务器整体运行

## 快速开始

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

```bash
docker build -t my-app . && docker run -p 8080:8080 my-app
curl http://localhost:8080/
```

## 配置

所有配置均通过环境变量设置：

| 变量 | 默认值 | 描述 |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | 监听地址和端口 |
| `DOCUMENT_ROOT` | `/var/www/html/public` | 静态文件服务的根目录路径 |
| `INDEX_FILE` | *(未设置)* | 路由模式：空 = 传统模式，`index.php` = 框架模式，`index.html` = SPA 模式 |
| `TOKIO_WORKERS` | `0`（CPU / 2，最少 1） | Tokio 异步 I/O 线程数；`0` = 自动，`1` = 单线程，`N` = 多线程 |
| `EXECUTOR` | `sapi` | PHP 执行器：`sapi`（真实 PHP）或 `stub`（测试模式） |
| `PHP_WORKERS` | `0`（CPU / 2，最少 1） | 工作池模式：`N` = 固定数量，`MIN:MAX` = 动态伸缩，`0` = 自动 |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | 动态模式下，工作线程的空闲超时时间（仅动态模式有效） |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | 有界队列大小；队列满时返回 503 |
| `DRAIN_TIMEOUT_SECONDS` | `30` | 优雅关闭的排空等待超时（秒） |
| `LOG_LEVEL` | `info` | 日志级别：`error`、`warn`、`info`、`debug`、`trace` |
| `INTERNAL_ADDR` | *(未设置)* | 内部服务器地址，用于健康检查/指标/配置（例如 `0.0.0.0:9090`） |
| `RATE_LIMIT` | `0`（关闭） | 每个 IP 每个时间窗口内的最大请求数 |
| `RATE_WINDOW_SECONDS` | `60` | 限流时间窗口（秒） |
| `HEADER_TIMEOUT_SECONDS` | `5` | 请求头读取超时（Slowloris 防护） |
| `REQUEST_TIMEOUT_SECONDS` | `120` | 整体请求超时；`0` 表示禁用 |
| `TLS_CERT` | *(未设置)* | TLS 证书 PEM 文件路径 |
| `TLS_KEY` | *(未设置)* | TLS 私钥 PEM 文件路径 |
| `ERROR_PAGES_DIR` | *(未设置)* | 自定义错误页面目录（文件名格式：`{status}.html`） |
| `STATIC_CACHE_TTL` | `30d` | 静态文件缓存 TTL。支持格式：`30s`、`5m`、`2h`、`30d`、`1w`、`1y`、纯数字秒数（`3600`）或 `off` 禁用 |
| `COMPRESSION_LEVEL` | `4` | Brotli 压缩质量级别（0-11）。`0` 禁用压缩，`1`-`11` 设置质量级别 |
| `ACCESS_LOG` | *(关闭)* | 每请求 JSON 访问日志：`all`（所有请求）、`error`（仅 4xx/5xx）、空/未设置 = 关闭 |
| `MAX_CONNECTIONS` | `10000` | 最大并发连接数 |
| `WORKER_FILE` | *(未设置)* | 工作进程 PHP 脚本路径（相对于 `DOCUMENT_ROOT`）；设置后启用持久化工作进程模式 |
| `WORKER_MAX_REQUESTS` | `0`（无限制） | 每个工作进程回收前的最大请求数；`0` = 无限制 |
| `WORKER_MAX_MEMORY_MIB` | `0`（无限制） | 每个工作进程回收前的最大内存（MiB）；`0` = 无限制 |

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
                    │(backpressure)│  503 when full
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         PHP Worker   PHP Worker   PHP Worker    OS threads (ZTS)
         (SAPI exec)  (SAPI exec)  (SAPI exec)   with thread-local state
```

- **可配置 Tokio 运行时** — 默认自动（`TOKIO_WORKERS=0`，CPU / 2，最少 1），`1` = 单线程，高吞吐场景可设为更大值
- **多线程 PHP 工作池** — 基于 PHP ZTS，每个工作线程为独立操作系统线程，通过 `catch_unwind` 实现故障隔离
- 工作线程通过 `crossbeam::bounded` 接收请求，通过 `ExecuteResult`（即时或经由 `oneshot` 延迟）返回结果
- **工作线程健康监控** — 自动检测崩溃线程并重启
- **工作进程模式** — 持久化 PHP 进程，请求间软重置；工作进程循环调用 `oxphp_worker($handler)`，保持引导状态（自动加载器、数据库连接）跨请求存活

### 内部服务器

设置 `INTERNAL_ADDR` 后，将在独立端口上启动一个轻量 HTTP 服务器：

| 端点 | 描述 |
|----------|-------------|
| `GET /health` | JSON 格式健康状态（运行时长、请求数、连接数） |
| `GET /metrics` | Prometheus 文本格式指标 |
| `GET /config` | JSON 格式运行时配置（TLS 路径已脱敏） |

## 构建

```bash
# 宿主机（不含 PHP — 运行所有测试，无 PHP 执行）
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
# 完整验证（宿主机，167 个测试）
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Docker 冒烟测试
docker compose build && docker compose up -d
curl http://localhost:8080/
curl "http://localhost:8080/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost:8080/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost:8080/test_superglobals.php

# 内部服务器
INTERNAL_ADDR=127.0.0.1:9090 ./target/release/oxphp &
curl http://localhost:9090/health
curl http://localhost:9090/metrics
```

## 文档

- [English](docs/en/)
- [中文](docs/zh/)
- [Русский](docs/ru/)
- [Беларуская](docs/be/)

## 许可证

[AGPL-3.0](LICENSE)
