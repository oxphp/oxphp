---
title: OxPHP
description: 用 Rust 编写的异步 PHP 应用服务器
---

OxPHP 是一个用 Rust 编写的异步 PHP 应用服务器。它用单个二进制文件替代 nginx + PHP-FPM，在一个进程中处理 HTTP 服务、PHP 执行和内置可观测性。

## 为什么选择 OxPHP

传统的 PHP 部署需要 Web 服务器（nginx 或 Apache）、进程管理器（PHP-FPM），以及用于指标、速率限制和 TLS 终止的独立工具。OxPHP 将这些组件整合为单个二进制文件，运行时除 PHP 运行时库外无任何外部依赖。

服务器使用可配置的 Tokio 异步运行时（默认单线程，可通过 `TOKIO_WORKERS` 启用多线程）处理所有 I/O，并使用一组专用的操作系统线程通过 Zend 线程安全（ZTS）执行 PHP。这种架构使异步事件循环不受阻塞式 PHP 调用的影响，同时将 PHP 执行扩展到所有可用的 CPU 核心。mimalloc 分配器在高并发场景下提供更低的内存分配延迟。

## 功能特性

- **静态文件服务**，支持内存文件缓存和自动 MIME 类型检测
- **三种路由模式**：传统模式（直接文件映射）、框架模式（前端控制器）和 SPA 模式（单页应用回退）
- **PHP 执行**，通过自定义 SAPI（`oxphp`）实现，完整支持超全局变量（`$_GET`、`$_POST`、`$_SERVER`、`$_COOKIE`、`$_FILES`）
- **动态 Worker 伸缩** -- 根据负载在可配置的最小/最大值之间自动伸缩 PHP Worker 线程
- **有界请求队列**，支持背压 -- 队列满时返回 503，而非无限制地接受请求
- **插件系统**，支持生命周期钩子、PHP 函数注册和拓扑依赖排序
- **事件系统**，在请求生命周期的每个阶段提供类型化事件和按优先级排序的处理器
- **Brotli 压缩**，用于可压缩的响应类型
- **TLS**，通过 rustls 实现（边缘无需依赖 OpenSSL）
- **Per-IP 速率限制**，支持可配置的限制值和时间窗口
- **Prometheus 兼容指标**，通过专用内部端口暴露
- **健康检查**，位于内部服务器的 `/health` 端点
- **请求 ID**，为每个请求生成，PHP 中可通过 `oxphp_request_id()` 获取
- **结构化 JSON 访问日志**，基于 tracing 框架
- **自定义错误页面**，从磁盘目录加载
- **优雅关闭**，支持可配置的排空超时
- **OPcache + JIT** 开箱即用
- **Worker 健康监控**，自动重启异常终止的 Worker
- **SSE 流式传输** -- 通过自动检测 `Content-Type: text/event-stream` 或 `oxphp_stream_flush()` 实现实时 Server-Sent Events
- **提前响应**，通过 `oxphp_finish_request()` -- 立即发送 HTTP 响应并继续后台处理
- **Panic 隔离**，通过 `catch_unwind` 实现 -- PHP 崩溃不会导致服务器宕机

## 许可证

OxPHP 使用 [AGPL-3.0](https://www.gnu.org/licenses/agpl-3.0.html) 许可证。

## 文档

### 快速入门

- [安装](getting-started/installation.md)
- [快速启动](getting-started/quick-start.md)
- [Docker](getting-started/docker.md)

### 架构

- [概述](architecture/overview.md) -- 运行时模型和组件图
- [请求生命周期](architecture/request-lifecycle.md) -- 逐步请求处理流程
- [Worker 池](architecture/worker-pool.md) -- PHP 执行：InlineExecutor、SapiExecutor、背压
- [事件系统](architecture/event-system.md) -- 类型化事件和处理器注册
- [SAPI 与桥接](architecture/sapi-bridge.md) -- 自定义 PHP SAPI 和 C 桥接库

### 功能特性

- [路由](features/routing.md) -- 三种路由模式（传统、框架、SPA）
- [静态文件](features/static-files.md) -- 文件缓存、MIME 检测、流式传输
- [压缩](features/compression.md) -- Brotli 压缩
- [TLS](features/tls.md) -- 通过 rustls 配置 TLS
- [速率限制](features/rate-limiting.md) -- 基于 IP 的速率限制
- [错误页面](features/error-pages.md) -- 自定义 HTML 错误页面
- [请求 ID](features/request-ids.md) -- X-Request-ID 生成
- [超时](features/timeouts.md) -- 请求头、请求和空闲超时
- [访问日志](features/access-logging.md) -- 结构化 JSON 访问日志

### PHP 集成

- [PHP 函数](php/functions.md) -- 内置和插件 PHP 函数
- [超全局变量](php/superglobals.md) -- $_SERVER、$_GET、$_POST、$_COOKIE、$_FILES
- [OPcache](php/opcache.md) -- OPcache 和 JIT 配置

### 运维

- [配置](operations/configuration.md) -- 环境变量参考
- [健康检查](operations/health-checks.md) -- /health、/metrics、/config 端点
- [指标](operations/metrics.md) -- Prometheus 指标参考
- [优雅关闭](operations/graceful-shutdown.md) -- 排空行为和超时
