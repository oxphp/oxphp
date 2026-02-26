---
title: 配置
description: OxPHP 环境变量完整参考
---

OxPHP 完全通过环境变量进行配置，没有配置文件。每个变量都有合理的默认值，因此零配置部署即可在开发环境中直接使用。

## 环境变量参考

### 服务器

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | 主 HTTP 服务器的监听地址和端口 |
| `DOCUMENT_ROOT` | `/var/www/html/public` | 文件和 PHP 脚本的根目录 |
| `INDEX_FILE` | *(空)* | 控制路由模式。参见[路由模式](#路由模式) |
| `TOKIO_WORKERS` | `0`（CPU / 2，最少 1） | Tokio 异步 I/O 线程数。`0` = 自动检测（CPU / 2，最少 1），`1` = 单线程运行时，`N` = 使用 N 个工作线程的多线程运行时 |
| `MAX_CONNECTIONS` | `10000` | 最大并发 TCP 连接数。超过此限制的新连接将等待信号量许可 |

### PHP 执行

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `EXECUTOR` | `sapi` | PHP 执行器类型。`sapi` 为真实 PHP 执行，`stub` 为占位响应（用于基准测试） |
| `PHP_WORKERS` | `0`（CPU / 2，最少 1，静态） | 工作池模式。设置 `N` 为固定池，或 `MIN:MAX` 为动态伸缩。参见[工作线程模式](#工作线程模式) |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | 动态工作线程空闲超过此秒数后被回收。仅在动态模式下生效 |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | PHP 队列中等待的最大请求数。队列满时，新的 PHP 请求将收到 `503 Service Unavailable` 响应。动态模式下使用初始工作线程数计算 |

### 日志

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `LOG_LEVEL` | `info` | 日志级别。可选值：`trace`、`debug`、`info`、`warn`、`error` |
| `ACCESS_LOG` | *(关闭)* | 每请求 JSON 访问日志。值：`all`（所有请求）、`error`（仅 4xx/5xx）、空/未设置 = 关闭 |

### 超时

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `HEADER_TIMEOUT_SECONDS` | `5` | TCP 连接建立后等待请求头的最大秒数 |
| `IDLE_TIMEOUT_SECONDS` | `60` | Keep-alive 空闲超时。超过此时间无活动的连接将被关闭 |
| `REQUEST_TIMEOUT_SECONDS` | `120` | 整个请求-响应周期的最大秒数。设为 `0` 可禁用 |
| `DRAIN_TIMEOUT_SECONDS` | `30` | 优雅关闭期间等待进行中连接完成的最大秒数 |

### 速率限制

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `RATE_LIMIT` | `0` | 每个 IP 地址在时间窗口内的最大请求数。`0` 表示禁用速率限制 |
| `RATE_WINDOW_SECONDS` | `60` | 速率限制窗口时长（秒） |

### TLS

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `TLS_CERT` | *(无)* | TLS 证书 PEM 文件路径。必须同时设置 `TLS_CERT` 和 `TLS_KEY` 才能启用 TLS |
| `TLS_KEY` | *(无)* | TLS 私钥 PEM 文件路径 |

### 可观测性

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `INTERNAL_ADDR` | *(无)* | 内部服务器地址（健康检查、指标、配置）。未设置时不启动 |
| `ERROR_PAGES_DIR` | *(无)* | 自定义错误页面 HTML 文件目录，文件名格式为 `{status}.html`（例如 `404.html`、`503.html`） |

### 工作进程模式

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `WORKER_FILE` | *(无)* | PHP 工作脚本路径（相对于 `DOCUMENT_ROOT`）。设置后启用持久化工作进程模式，PHP 进程在请求间保持存活 |
| `WORKER_MAX_REQUESTS` | `0` | 工作进程回收前处理的最大请求数。`0` 表示不限制 |
| `WORKER_MAX_MEMORY_MIB` | `0` | 工作进程回收前使用的最大内存（兆字节）。`0` 表示不限制 |

### 压缩

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `COMPRESSION` | `true` | 对可压缩的响应类型启用 Brotli 压缩。使用 `false`、`0` 或 `off` 禁用 |

## 工作线程模式

`PHP_WORKERS` 变量控制 PHP 工作池使用固定大小还是动态伸缩。

### 静态模式（默认）

将 `PHP_WORKERS` 设为一个数字（或留空/设为 `0` 自动检测）：

```bash
PHP_WORKERS=8      # 固定 8 个工作线程
PHP_WORKERS=0      # 自动检测：CPU / 2（最少 1）
```

工作线程在启动时创建，之后不会改变。每个工作线程使用阻塞 `recv()`，空闲时 CPU 开销为零。

### 动态模式

将 `PHP_WORKERS` 设为 `MIN:MAX` 启用自动伸缩：

```bash
PHP_WORKERS=2:16       # 在 2 到 16 个工作线程之间伸缩
PHP_WORKERS=4:0        # 最少 4 个，最多自动检测（CPU * 2）
PHP_WORKERS=0:0        # 两者均自动检测（最少 CPU/4（至少 1），最多 CPU*2）
```

ScaleManager 每 500ms 运行一次：
- 当所有工作线程都在忙碌且池大小低于 MAX 时**扩容**（500ms 冷却期）
- 当工作线程空闲超过 `PHP_WORKERS_IDLE_SECONDS` 且池大小高于 MIN 时**缩容**（5s 冷却期）

动态工作线程使用 `recv_timeout(200ms)` 以便定期检查关闭标志。

## 路由模式

`INDEX_FILE` 变量控制 OxPHP 如何路由传入请求。共有三种模式：

| 模式 | `INDEX_FILE` 值 | 行为 |
|------|-----------------|------|
| 传统模式 | *(空 / 未设置)* | 直接文件映射。`/about.php` 提供 `about.php`。`/` 提供 `index.php` 或 `index.html`（如果存在） |
| 框架模式 | `index.php` | 所有非文件请求路由到 `index.php`（前端控制器）。直接访问 `.php` 被阻止 |
| SPA 模式 | `index.html` | 找不到文件时回退到 `index.html`。`.php` 文件仍正常执行 |

### 传统模式（默认）

URL 直接映射到磁盘上的文件。这是经典 PHP 应用的标准行为。

```bash
# 未设置 INDEX_FILE — 传统模式为默认模式
DOCUMENT_ROOT=/var/www/html/public
```

### 框架模式

所有请求通过单一入口点。这是 Laravel、Symfony 等框架的标准模式。

```bash
INDEX_FILE=index.php
DOCUMENT_ROOT=/var/www/html/public
```

### SPA 模式

静态资源直接提供。所有其他请求返回 `index.html`，由 JavaScript 路由器处理导航。

```bash
INDEX_FILE=index.html
DOCUMENT_ROOT=/var/www/html/dist
```

## 配置示例

### 开发环境

```bash
LISTEN_ADDR=127.0.0.1:8080
DOCUMENT_ROOT=./www
LOG_LEVEL=debug
PHP_WORKERS=1
INTERNAL_ADDR=127.0.0.1:9090
```

### Laravel 生产环境（静态池）

```bash
LISTEN_ADDR=0.0.0.0:8080
DOCUMENT_ROOT=/var/www/html/public
INDEX_FILE=index.php
PHP_WORKERS=8
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
MAX_CONNECTIONS=10000
INTERNAL_ADDR=127.0.0.1:9090
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
HEADER_TIMEOUT_SECONDS=5
IDLE_TIMEOUT_SECONDS=30
REQUEST_TIMEOUT_SECONDS=60
DRAIN_TIMEOUT_SECONDS=30
COMPRESSION=true
```

### Laravel 生产环境（动态池）

```bash
LISTEN_ADDR=0.0.0.0:8080
DOCUMENT_ROOT=/var/www/html/public
INDEX_FILE=index.php
PHP_WORKERS=4:32
PHP_WORKERS_IDLE_SECONDS=60
QUEUE_CAPACITY=512
LOG_LEVEL=warn
MAX_CONNECTIONS=10000
INTERNAL_ADDR=127.0.0.1:9090
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
HEADER_TIMEOUT_SECONDS=5
IDLE_TIMEOUT_SECONDS=30
REQUEST_TIMEOUT_SECONDS=60
DRAIN_TIMEOUT_SECONDS=30
COMPRESSION=true
```

### Docker Compose

```yaml
services:
  oxphp:
    image: oxphp:latest
    ports:
      - "8080:8080"
    environment:
      LISTEN_ADDR: "0.0.0.0:8080"
      DOCUMENT_ROOT: "/var/www/html/public"
      INDEX_FILE: "index.php"
      PHP_WORKERS: "4"             # Or "2:16" for dynamic scaling
      # PHP_WORKERS_IDLE_SECONDS: "30" # Idle timeout (dynamic mode only)
      QUEUE_CAPACITY: "512"
      LOG_LEVEL: "info"
      INTERNAL_ADDR: "127.0.0.1:9090"
      COMPRESSION: "true"
    volumes:
      - ./src:/var/www/html
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
```

### 工作进程模式（持久化 PHP）

```bash
LISTEN_ADDR=0.0.0.0:8080
DOCUMENT_ROOT=/var/www/html/public
WORKER_FILE=../worker.php
PHP_WORKERS=8
WORKER_MAX_REQUESTS=10000
WORKER_MAX_MEMORY_MIB=128
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
INTERNAL_ADDR=127.0.0.1:9090
```

工作进程模式使 PHP 进程在请求间保持存活。工作脚本调用 `oxphp_worker()` 并传入处理器回调。当工作进程达到 `WORKER_MAX_REQUESTS` 或 `WORKER_MAX_MEMORY_MIB` 时自动回收。将两者均设为 `0` 可禁用回收。

### TLS 终止

```bash
LISTEN_ADDR=0.0.0.0:443
TLS_CERT=/etc/oxphp/tls/cert.pem
TLS_KEY=/etc/oxphp/tls/key.pem
```

OxPHP 使用 rustls 实现 TLS，因此不依赖 OpenSSL。证书和密钥必须为 PEM 格式。

## 查看当前配置

当内部服务器运行时，可以查看已解析的配置：

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:8080",
  "document_root": "/var/www/html/public",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "idle_timeout_seconds": 60,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": "/etc/oxphp/error-pages",
  "compression": true,
  "access_log": true,
  "plugins": {}
}
```

输出中不包含 TLS 密钥和证书路径。`tls_enabled` 布尔值表示 TLS 是否已启用。`plugins` 对象包含已加载插件的配置。

## 另请参阅

- [路由](/features/routing.md) --- 三种路由模式的详细说明
- [健康检查](health-checks.md) --- 内部服务器的 `/health`、`/metrics` 和 `/config` 端点
- [指标](metrics.md) --- Prometheus 兼容指标参考
- [优雅关闭](graceful-shutdown.md) --- `DRAIN_TIMEOUT_SECONDS` 如何影响关闭流程
- [TLS](/features/tls.md) --- TLS 配置和证书要求
- [速率限制](/features/rate-limiting.md) --- 按 IP 速率限制详情
- [工作池](/architecture/worker-pool.md) --- 静态和动态工作池架构
