---
title: OxPHP 文档
description: OxPHP 文档 —— 高性能异步 PHP 应用服务器，内置 TLS、压缩、限速、指标、SSE 流式传输和 Worker 模式。
---

# OxPHP 文档

OxPHP 是一个高性能 PHP 应用服务器，用单一二进制文件取代 nginx + PHP-FPM —— 内置 TLS、Brotli 压缩、限速、健康检查、Prometheus 指标、SSE 流式传输和持久化 Worker 模式。

## 为什么选择 OxPHP

传统 PHP 部署需要多个组件协同工作：Web 服务器、进程管理器、TLS 代理，以及独立的指标采集和限速工具。OxPHP 将整个技术栈整合为一个二进制文件。

- **单一二进制** —— 无需 nginx、无需 PHP-FPM、无需进程管理器。一个容器即可运行完整应用。
- **内置 TLS** —— 无需反向代理，直接终止 TLS。只需两个环境变量即可完成配置。
- **Brotli 压缩** —— 文本响应压缩开箱即用，支持可配置的质量级别。
- **限速** —— 内置基于 IP 的限速功能，支持可配置的限制次数和时间窗口。
- **健康检查** —— 在专用内部端口上提供 `/health`、`/metrics` 和 `/config` 端点，供 Kubernetes 探针和监控系统使用。
- **Prometheus 指标** —— 在 `/metrics` 端点提供请求数量、响应时间、队列等待、Worker 池统计、压缩节省量等指标。
- **静态文件服务** —— 内存缓存、自动 MIME 检测、ETag/Last-Modified 响应头，以及无需配置的可定制缓存 TTL。
- **Worker 模式** —— 持久化 PHP 进程，启动一次即可处理数千个请求，消除 Laravel、Symfony 等框架的每次请求启动开销。
- **SSE 流式传输** —— 从 PHP 向浏览器推送实时 Server-Sent Events，无需轮询。
- **提前响应** —— 立即发送 HTTP 响应，同时在后台继续处理。
- **四种路由模式** —— 传统文件映射、框架前置控制器（`index.php`）、SPA 回退（`index.html`）和 Worker 模式。
- **异步 Promise** —— 在专用线程池上运行 PHP 闭包并等待结果，不阻塞 Worker。
- **装饰器** —— 使用 PHP 8 属性拦截函数和方法调用，实现日志记录、计时、缓存和访问控制。
- **W3C Trace Context** —— 通过 `$_SERVER` 将上游服务的分布式追踪头传递到 PHP。
- **OpenTelemetry** —— 将请求 span 导出到 Jaeger、Grafana Tempo、Zipkin 或任何兼容 OTLP 的后端。

## 快速开始

使用 Docker 在 30 秒内启动 OxPHP：

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

```bash
docker build -t my-app .
docker run -p 8080:80 my-app
curl http://localhost:8080/
```

OxPHP 镜像包含服务器二进制文件、PHP 8.4、带 JIT 的 OPcache 及所有必要依赖。应用代码放置在 `/var/www/html`，默认文档根目录为 `/var/www/html/public`。

完整的操作指南请参阅[快速开始](getting-started/quick-start.md)文档。

## 入门

- [安装](getting-started/installation.md) —— 系统要求和安装方式
- [快速开始](getting-started/quick-start.md) —— 5 分钟内构建并运行第一个 OxPHP 应用
- [Docker 指南](getting-started/docker.md) —— Dockerfile、Compose 配置、数据卷和部署模式

## 功能特性

- [路由](features/routing.md) —— 四种路由模式：传统文件映射、框架前置控制器、SPA 回退和 Worker 模式
- [静态文件](features/static-files.md) —— 文件缓存、MIME 检测、ETag/Last-Modified 响应头和流式传输
- [Worker 模式](features/worker-mode.md) —— 请求之间自动软重置的持久化 PHP 进程
- [Fiber 多路复用](features/fiber-multiplexing.md) —— 通过协作式多任务，每个 Worker 线程处理数百个并发请求
- [压缩](features/compression.md) —— 针对文本响应的 Brotli 压缩
- [TLS](features/tls.md) —— 内置 TLS 终止，支持证书和密钥配置
- [限速](features/rate-limiting.md) —— 基于 IP 的限速，支持可配置的时间窗口和限制次数
- [超时](features/timeouts.md) —— 请求头读取超时和请求超时
- [访问日志](features/access-logging.md) —— 包含请求 ID、方法、路径、状态码和耗时的结构化 JSON 访问日志
- [请求 ID](features/request-ids.md) —— 自动生成 `X-Request-ID` 并透传
- [错误页面](features/error-pages.md) —— 为任意 HTTP 状态码自定义 HTML 错误页面
- [SSE](features/sse.md) —— 从 PHP 进行实时 Server-Sent Events 流式传输
- [提前响应](features/early-response.md) —— 立即发送响应并继续后台处理
- [异步 Promise](features/async-promises.md) —— 在后台线程运行 PHP 闭包并等待结果
- [装饰器](features/decorators.md) —— 使用 PHP 8 属性拦截函数和方法调用
- [分布式追踪](features/distributed-tracing.md) —— W3C Trace Context、OpenTelemetry 集成和日志关联
- [内部服务器](features/internal-server.md) —— 用于健康检查、Prometheus 指标和实时配置的专用端口

## PHP

- [函数](php/functions.md) —— OxPHP 提供的内置 PHP 函数（`oxphp_worker()`、`oxphp_request_id()`、`oxphp_server_info()` 等）
- [超全局变量](php/superglobals.md) —— `$_SERVER`、`$_GET`、`$_POST`、`$_COOKIE`、`$_FILES` 和 `php://input` 的填充方式
- [OPcache 与 JIT](php/opcache.md) —— OPcache 配置和 JIT 编译设置

## 运维

- [配置参考](operations/configuration.md) —— 完整的环境变量列表，包含默认值和说明
- [健康检查](operations/health-checks.md) —— 内部服务器的 `/health`、`/metrics` 和 `/config` 端点
- [指标](operations/metrics.md) —— Prometheus 兼容指标参考
- [优雅关闭](operations/graceful-shutdown.md) —— 排空行为、超时配置和关闭顺序

## 架构

- [架构概览](architecture/overview.md) —— OxPHP 的工作原理：异步 HTTP 处理、PHP Worker 池、请求流程和安全保障
