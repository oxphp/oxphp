---
title: OxPHP 文档
description: OxPHP 文档 —— 高性能异步 PHP 应用服务器，内置 TLS、压缩、限速、指标、SSE 流式传输和 Worker 模式。
---

[English](../en/index.md) · [Русский](../ru/index.md) · **中文**

[入门](#入门) · [示例](#示例) · [功能特性](#功能特性) · [共享状态](#共享状态) · [安全](#安全) · [PHP](#php) · [运维](#运维) · [架构](#架构)

# OxPHP 文档

OxPHP 是一个高性能 PHP 应用服务器，用单一二进制文件取代 nginx + PHP-FPM —— 内置 TLS、Brotli 压缩、限速、健康检查、Prometheus 指标、SSE 流式传输和持久化 Worker 模式。

## 为什么选择 OxPHP

典型的 PHP 生产环境应用需要多个容器：nginx、PHP-FPM，有时还需要单独的 TLS 代理和指标导出器。配置分散在各处，要让它们协同工作，就必须同步套接字设置、超时和路径。OxPHP 用[一个容器](getting-started/docker.md)取代了整个技术栈。内部是一个进程，负责接收 HTTP 连接、执行 PHP 并提供静态文件服务。

服务器开箱即用，带有合理的默认值。微调通过[环境变量](operations/configuration.md)完成：[TLS](features/tls.md) 只需两个变量即可启用（`TLS_CERT`、`TLS_KEY`），[限速](features/rate-limiting.md)只需一个变量（`RATE_LIMIT`），[Brotli 压缩](features/compression.md)默认开启。无需编辑 nginx 配置或构建额外模块。

在专用的[内部端口](features/internal-server.md)上，可以访问[健康检查](operations/health-checks.md)（`/health`）、[Prometheus 指标](operations/metrics.md)（`/metrics`）和配置快照（`/config`）。这足以满足 Kubernetes 存活/就绪探针和 Grafana 接入，无需额外的 sidecar 容器。

[日志](features/access-logging.md)是结构化 JSON：每一行都包含方法、路径、状态码、响应时间和[请求 ID](features/request-ids.md)。在 Loki、Elasticsearch 或任何其他工具中都可以轻松解析，无需额外的 grok 模式。

如果想尝试 [Worker 模式](features/worker-mode.md)——即 PHP 进程不在每次请求时重建——只需设置 `WORKER_MODE_ENABLED=true` 并将 `ENTRY_FILE=worker.php`。框架初始化一次，随后处理数千个请求而无需重新加载。要切换回经典模式，只需删除这两个变量。

此外，OxPHP 还包含通常需要单独工具或第三方库才能实现的功能：

- **[静态文件服务](features/static-files.md)** —— 内存缓存、ETag/Last-Modified、自动 MIME 类型
- **[三种路由模式](features/routing.md)** —— 文件映射、框架和 SPA（每种都可与持久化 [Worker 模式](features/worker-mode.md) 叠加）
- **[提前响应](features/early-response.md)** —— 立即发送响应并继续后台处理
- **[Worker 模式](features/worker-mode.md)** —— 持久化 PHP 进程，支持 [Fiber 多路复用](features/fiber-multiplexing.md)
- **[SSE 流式传输](features/sse.md)** —— 从 PHP 推送实时 Server-Sent Events
- **[异步 Promise](features/async-promises.md)** —— 后台执行 PHP 闭包，不阻塞 Worker
- **[共享状态](shared-state/shared-state.md)** —— 进程内并发原语（Counter、Flag、Once、Mutex、Channel、Map、Pool），无需 Redis 或 APCu 即可协调工作线程
- **[装饰器](features/decorators.md)** —— 通过 PHP 8 属性拦截调用
- **[分布式追踪与 APM](features/distributed-tracing.md)** —— W3C Trace Context、OpenTelemetry、数据库/HTTP/缓存/文件调用的自动埋点，以及 PHP 追踪 SDK

---

## 入门

- [安装](getting-started/installation.md) —— 系统要求和安装方式
- [快速开始](getting-started/quick-start.md) —— 5 分钟内构建并运行第一个 OxPHP 应用
- [Docker 指南](getting-started/docker.md) —— Dockerfile、Compose 配置、数据卷和部署模式
- [命令行接口](getting-started/cli.md) —— `oxphp` 命令语法：`serve`、`run` 运行单个 PHP 脚本、`config` 以及 `--user` 降权

## 示例

在 OxPHP 上运行流行 PHP 应用的端到端实战教程 —— 每个都是完整的 Docker Compose 项目，包含 `Dockerfile`、`docker-compose.yml`、安装步骤，以及标准（nginx + PHP-FPM）文档未涵盖的 OxPHP 特有说明。

- [示例部署](examples/index.md) —— 概览、九应用矩阵，以及每个教程共通的模式
- 框架模式：[Laravel](examples/framework/laravel.md) · [Symfony](examples/framework/symfony.md) · [Yii3](examples/framework/yii3.md)
- CMS：[WordPress](examples/cms/wordpress.md) · [Drupal](examples/cms/drupal.md) · [Craft CMS](examples/cms/craft.md) · [October CMS](examples/cms/october.md)
- 电商：[Magento](examples/ecommerce/magento.md) · [OpenCart](examples/ecommerce/opencart.md)

## 功能特性

- [路由](features/routing.md) —— 三种路由模式：传统文件映射、框架前置控制器和 SPA 回退。Worker 模式是一个正交的执行模型开关，可叠加在任意路由模式之上
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
- [分布式追踪与 APM](features/distributed-tracing.md) —— W3C Trace Context、OpenTelemetry、自动埋点和 PHP 追踪 SDK
- [内部服务器](features/internal-server.md) —— 用于健康检查、Prometheus 指标和实时配置的专用端口

## 共享状态

进程内并发原语，让工作线程无需 Redis、Memcached 或 APCu 即可协调可变状态 —— 一切都在进程内，因此每次操作的开销是微秒级，而非网络往返。

- [概览](shared-state/shared-state.md) —— 注册表模型、句柄生命周期，以及何时该使用共享状态
- [Counter](shared-state/shared-counter.md) —— 原子 int64 累加器（`get`、`set`、`add`、`compareAndSet`）
- [Atomic](shared-state/shared-atomic.md) —— 带显式内存序控制的原子 int64
- [Flag](shared-state/shared-flag.md) —— 用于一次性状态转换的原子布尔
- [Once](shared-state/shared-once.md) —— 带可重入安全工厂的一次性容器
- [Mutex](shared-state/shared-mutex.md) —— 基于存储值的中毒互斥锁，带死锁检测
- [Channel](shared-state/shared-channel.md) —— 有界、fiber 感知的 MPMC 队列
- [Map](shared-state/shared-map.md) —— 支持批量访问的并发字符串键存储
- [Pool](shared-state/shared-pool.md) —— 带线程亲和性的有界对象池
- [命名约定](shared-state/shared-naming.md) —— `Shared\*` 家族的方法命名速查
- [可观测性](shared-state/shared-observability.md) —— Prometheus 计数器和 JSON 内省端点
- [迁移到外部存储](shared-state/migrating-to-external-store.md) —— 何时以及如何迁移到 Redis 或 APCu

## 安全

- [点路径拦截](security/dot-path-blocking.md) — 自动拦截隐藏文件和目录（`.env`、`.git/`、`.htaccess`）
- [受信任代理](security/trusted-proxies.md) — 从 `Forwarded`（RFC 7239）和 `X-Forwarded-*` 头中提取真实客户端 IP、协议和主机名
- [PHP 执行拒绝名单](security/php-deny.md) — 阻止可写公共路径（如 `/uploads/**`，或具体的遗留脚本）的 `.php` 执行，抵御遗留应用的上传 shell 攻击
- [符号链接允许路径](security/symlink-allow-paths.md) — 针对 `DOCUMENT_ROOT` 外符号链接目标的 opt-in 允许列表；在不削弱默认符号链接逃逸防护的前提下支持 Laravel 风格 `storage:link` 和共享资源卷

## PHP

- [HTTP 请求对象 API](php/request-api.md) —— 通过 `oxphp_http_request()` 以面向对象方式访问请求数据：查询参数、解析后的请求体、请求头、Cookie、文件上传等
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
