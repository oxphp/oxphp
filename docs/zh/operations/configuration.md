---
title: 配置参考
description: OxPHP 完整的环境变量参考。每项配置、默认值及其作用——尽在一处。
---

# 配置参考

OxPHP 完全通过环境变量进行配置。无需管理任何配置文件——每项配置都有合理的默认值，因此无需任何配置即可开箱即用。

## 服务器

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:80` | 主 HTTP 服务器的地址和端口 |
| `DOCUMENT_ROOT` | `/var/www/html/public` | 提供文件和 PHP 脚本的根目录 |
| `INDEX_FILE` | *(未设置)* | 路由模式：未设置 = Traditional，`*.php` = Framework，其他任何值 = SPA。详见[路由](../features/routing.md) |
| `MAX_CONNECTIONS` | `10000` | 最大并发 TCP 连接数 |
| `TOKIO_WORKERS` | CPU / 2（最少 1） | 异步 I/O 线程数。`1` = 单线程，`N > 1` = 固定线程数，未设置 = 自动检测（CPU / 2，最少 1） |

## PHP 工作进程

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `EXECUTOR` | `sapi` | PHP 执行器后端。`sapi` 用于 PHP 执行，`stub` 用于不依赖 PHP 的基准测试 |
| `PHP_WORKERS` | CPU / 2（最少 1） | 工作进程池大小。`N` = 固定进程池，`MIN:MAX` = 动态扩缩容，`0` = 自动检测 |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | 动态工作进程在空闲多少秒后退出（仅动态模式） |
| `QUEUE_CAPACITY` | 初始工作进程数 × 128 | PHP 队列中最大待处理请求数。队列满时返回 529。对于动态进程池（`MIN:MAX`），初始工作进程数等于最小值 |

### 静态进程池与动态进程池

将 `PHP_WORKERS` 设置为单个数字以使用固定进程池：

```bash
PHP_WORKERS=8      # 固定 8 个工作进程
PHP_WORKERS=0      # 自动检测：CPU / 2（最少 1）
```

将 `PHP_WORKERS` 设置为 `MIN:MAX` 以启用自动扩缩容：

```bash
PHP_WORKERS=2:16   # 在 2 到 16 个工作进程之间扩缩容
PHP_WORKERS=4:0    # 最少 4 个，最多自动检测（CPU × 2）
PHP_WORKERS=0:16   # 自动检测最小值（CPU / 4，最少 1），最多 16 个
```

在动态模式下，当所有工作进程均处于繁忙状态时，OxPHP 会增加工作进程；当工作进程空闲时间超过 `PHP_WORKERS_IDLE_SECONDS` 时，OxPHP 会减少工作进程。

## 工作进程模式

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `WORKER_FILE` | *(未设置)* | 工作进程 PHP 脚本的路径。设置后启用持久化工作进程模式 |
| `WORKER_MAX_REQUESTS` | `0` | 工作进程回收前处理的最大请求数。`0` = 不限制 |
| `WORKER_MAX_MEMORY_MIB` | `0` | 工作进程回收前允许使用的最大内存（MiB）。`0` = 不限制 |

当 `WORKER_FILE` 设置后，PHP 进程在多个请求间保持存活，将引导状态（自动加载器、数据库连接）保留在内存中。工作进程在达到请求数或内存限制时会被自动回收。

## SAPI / PHP

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `SUPERGLOBALS_ENABLED` | `true` | 在脚本执行前填充 PHP 超全局变量（`$_GET`、`$_POST`、`$_COOKIE`、`$_FILES`、`$_SERVER`、`php://input`）。设置为 `false` 或 `0` 可跳过填充——此时请求数据仅可通过对象 API（`oxphp_http_request()`）获取。适用于直接使用对象 API 且希望避免每次请求都构建超全局变量开销的应用 |

## 超时

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | 连接建立后接收 HTTP 头部的最大秒数（Slowloris 防护） |
| `REQUEST_TIMEOUT_SECONDS` | `120` | 整个请求-响应周期的最大秒数。`0` = 禁用 |
| `DRAIN_TIMEOUT_SECONDS` | `30` | 优雅关闭期间等待进行中连接完成的最大秒数 |

## 限流

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `RATE_LIMIT` | `0`（关闭） | 每个 IP 在时间窗口内的最大请求数。`0` 禁用限流 |
| `RATE_WINDOW_SECONDS` | `60` | 限流窗口持续时间（秒） |

## 安全

| 变量 | 默认值 | 描述 |
|------|--------|------|
| `FRAME_OPTIONS` | `DENY` | 点击劫持防护。`DENY` 禁止所有框架嵌入，`SAMEORIGIN` 允许同源嵌入，`off` 关闭（适用于通过自定义 CSP 管理框架策略的场景）。同时设置 `X-Frame-Options` 和 `Content-Security-Policy: frame-ancestors` |
| `TRUSTED_PROXIES` | *（未设置）* | 受信任的反向代理网络（逗号分隔 CIDR 或 `private`）。设置后，OxPHP 使用 rightmost-non-trusted 算法从 `Forwarded`（[RFC 7239](https://www.rfc-editor.org/rfc/rfc7239)）或 `X-Forwarded-For` 中提取真实客户端 IP。同时处理 `X-Forwarded-Proto` 和 `X-Forwarded-Host` 以设置 `$_SERVER['HTTPS']`、`REQUEST_SCHEME`、`SERVER_NAME` 和 `SERVER_PORT`。未设置 = 功能禁用 |

特殊值 `private` 展开为所有 RFC-1918 私有网络、回环和链路本地地址（IPv4 和 IPv6）：`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`127.0.0.0/8`、`169.254.0.0/16`、`::1/128`、`fc00::/7`、`fe80::/10`。

## TLS

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `TLS_CERT` | *(未设置)* | PEM 编码 TLS 证书的路径。`TLS_CERT` 和 `TLS_KEY` 均设置后才会启用 TLS |
| `TLS_KEY` | *(未设置)* | PEM 编码 TLS 私钥的路径 |

## 静态文件

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `STATIC_CACHE_TTL` | `30d` | 静态文件的缓存 TTL。接受以下格式：`30s`、`5m`、`2h`、`30d`、`1w`、`1y`、纯秒数（`3600`），或 `off` 禁用缓存 |
| `STATIC_CACHE` | *(开启)* | 设为 `off` 启用内存内容缓存的 mtime 重新验证。每次缓存命中时检查文件修改时间，自动清除过期条目 |
| `COMPRESSION_LEVEL` | `4` | Brotli 压缩质量（0–11）。`0` 禁用压缩 |

## 日志

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `LOG_LEVEL` | `info` | 日志详细程度：`trace`、`debug`、`info`、`warn`、`error` |
| `ACCESS_LOG` | *(未设置)* | 按请求记录访问日志：`all` = 所有请求，`error` = 仅 4xx/5xx，未设置 = 关闭 |

> **注意：** `ACCESS_LOG` 接受 `all` 或 `error`。不设置则完全禁用访问日志。

## 可观测性

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `INTERNAL_ADDR` | *(未设置)* | 内部服务器地址（`/health`、`/metrics`、`/config`）。未设置时不启动内部服务器 |
| `ERROR_PAGES_DIR` | *(未设置)* | 包含自定义错误页面的目录，文件名格式为 `{status}.html`（如 `404.html`、`503.html`） |
| `MAX_QUERY_BODY` | `524288` | 内部查询端点的最大请求体大小（字节，512 KiB） |
| `TRACE_CONTEXT` | `false` | 启用 W3C Trace Context 传播（`true` 或 `1`）。读取 `traceparent`/`tracestate` 头部并通过 `$_SERVER` 转发给 PHP |

## OpenTelemetry

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | 启用 OpenTelemetry Span 导出。自动设置 `TRACE_CONTEXT=true` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | 导出协议：`grpc` 或 `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317`（gRPC）或 `http://localhost:4318`（HTTP） | OTLP 收集器端点 |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | 导出超时（毫秒） |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(未设置)* | 认证头：`key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | 导出 Span 中的服务名称 |
| `OTEL_SERVICE_VERSION` | *(未设置)* | 服务版本属性 |
| `OTEL_RESOURCE_ATTRIBUTES` | *(未设置)* | 额外资源属性：`env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | 采样策略：`always_on`、`always_off`、`traceidratio`、`parentbased_always_on`、`parentbased_always_off`、`parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | 基于比率的采样器的采样比率（0.0–1.0） |

## APM

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `OTEL_APM_ENABLED` | `false` | 启用 APM：自动埋点、错误捕获和 PHP 追踪 SDK。需要 `OTEL_ENABLED=true` |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | 慢查询阈值（毫秒）。超过此值的数据库查询将添加 `oxphp.db.slow=true` Span 属性 |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | 将绑定参数记录到 `db.params` Span 属性中。如果参数可能包含敏感数据，请在生产环境中禁用 |

当 APM 启用时，OxPHP 自动 hook 33 个 PHP 内部函数（PDO、mysqli、cURL、Redis、Memcached、文件 I/O）来创建子 Span。无论 APM 是否启用，`oxphp_apm_*()` PHP 函数都会注册——禁用时它们是安全的空操作。

## 异步工作进程

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0`（禁用） | 专用异步工作线程数。为 `0` 时，异步函数（`oxphp_async` 等）已注册但调用时抛出 `OxPHP\Async\Exception`。设为正整数可启用后台任务执行 |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS × 64` | 异步队列中的最大待处理任务数。`0` = 自动（工作进程数 × 64） |

异步工作进程池处理从 PHP 分发的即发即忘后台任务。它独立于 PHP 工作进程池，标准请求处理不需要它。

## 配置示例

### 开发环境

```bash
LISTEN_ADDR=127.0.0.1:8080
DOCUMENT_ROOT=./public
LOG_LEVEL=debug
ACCESS_LOG=all
PHP_WORKERS=1
INTERNAL_ADDR=127.0.0.1:9090
```

### 生产环境（框架模式）

```bash
LISTEN_ADDR=0.0.0.0:80
DOCUMENT_ROOT=/var/www/html/public
INDEX_FILE=index.php
PHP_WORKERS=8
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
ACCESS_LOG=error
MAX_CONNECTIONS=10000
INTERNAL_ADDR=127.0.0.1:9090
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
TRUSTED_PROXIES=private
HEADER_TIMEOUT_SECONDS=5
REQUEST_TIMEOUT_SECONDS=60
DRAIN_TIMEOUT_SECONDS=30
COMPRESSION_LEVEL=4
STATIC_CACHE_TTL=30d
```

### 生产环境（工作进程模式）

```bash
LISTEN_ADDR=0.0.0.0:80
DOCUMENT_ROOT=/var/www/html/public
WORKER_FILE=../worker.php
PHP_WORKERS=8
WORKER_MAX_REQUESTS=10000
WORKER_MAX_MEMORY_MIB=128
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
ACCESS_LOG=error
INTERNAL_ADDR=127.0.0.1:9090
```

### TLS

```bash
LISTEN_ADDR=0.0.0.0:443
TLS_CERT=/etc/ssl/oxphp/cert.pem
TLS_KEY=/etc/ssl/oxphp/key.pem
DOCUMENT_ROOT=/var/www/html/public
INDEX_FILE=index.php
```

## 查看当前配置

当内部服务器运行时，查询 `/config` 端点以查看已解析的配置：

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:80",
  "document_root": "/var/www/html/public",
  "index_file": "index.php",
  "log_level": "warn",
  "executor_type": "sapi",
  "php_workers": "8",
  "tokio_workers": 4,
  "queue_capacity": 1024,
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "internal_addr": "127.0.0.1:9090",
  "header_timeout_seconds": 5,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode": false,
  "worker_file": null,
  "worker_max_requests": 0,
  "worker_max_memory_mib": 0,
  "static_cache_ttl": 2592000,
  "static_cache_enabled": true,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": true,
  "superglobals_enabled": true,
  "trusted_proxies": false,
  "plugins": {
    "otel": {
      "enabled": true,
      "protocol": "grpc",
      "service_name": "oxphp"
    },
    "apm": {
      "enabled": true,
      "slow_query_ms": 100,
      "db_capture_params": false,
      "hooks_registered": 33
    }
  }
}
```

> **注意：** TLS 证书和私钥路径不包含在输出中。`tls_enabled` 字段表示 TLS 是否已启用。

## 参见

- [路由](../features/routing.md) — 路由模式与 `INDEX_FILE` 行为
- [健康检查](health-checks.md) — 内部服务器端点
- [指标](metrics.md) — Prometheus 兼容指标参考
- [优雅关闭](graceful-shutdown.md) — `DRAIN_TIMEOUT_SECONDS` 如何影响关闭流程
- [TLS](../features/tls.md) — TLS 配置与证书要求
- [限流](../features/rate-limiting.md) — 基于 IP 的限流详情
- [受信任代理](../security/trusted-proxies.md) — 从反向代理头中提取真实客户端 IP
- [工作进程模式](../features/worker-mode.md) — 持久化 PHP 工作进程架构
- [压缩](../features/compression.md) — Brotli 压缩详情
- [静态文件](../features/static-files.md) — 缓存与文件服务
- [分布式追踪与 APM](../features/distributed-tracing.md) — OTel 导出、自动埋点和 PHP 追踪 SDK
