---
title: 配置参考
description: OxPHP 完整的环境变量参考。每项配置、默认值及其作用——尽在一处。
---

# 配置参考

OxPHP 完全通过环境变量进行配置。无需管理任何配置文件——每项配置都有合理的默认值，因此无需任何配置即可开箱即用。

### 布尔值

标记为布尔类型的变量只接受固定的规范集合（大小写不敏感、自动去除首尾空白）：

- 真值：`on`、`true`、`1`、`yes`
- 假值：`off`、`false`、`0`、`no`

规范集合之外的非空取值——例如 `ture` 之类的拼写错误——都会在启动时报错并指出变量名。这样可以在流量进入之前就发现配置错误，而不是悄悄把开关拨到错误的方向。

未设置的变量或空赋值（`FOO=`）会回退到文档中规定的默认值。空值被刻意视同未设置：Docker Compose / Kubernetes 中 `FOO=${FOO}` 这样的替换在宿主变量缺失时会得到 `FOO=`，此时不应让服务器拒绝启动。

## 服务器

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:80` | 主 HTTP 服务器的地址和端口 |
| `DOCUMENT_ROOT` | `/var/www/html/public` | 提供文件和 PHP 脚本的根目录 |
| `ENTRY_FILE` | *(未设置)* | 唯一规范的入口脚本。未设置 = 直接文件映射。`*.php` = 前端控制器。非 `.php` = 静态回退（SPA）。当 `WORKER_MODE_ENABLED=true` 时 = Worker 引导脚本。相对路径基于 `DOCUMENT_ROOT` 解析（允许相对路径和 `..`，绝对路径按原样使用）。详见[路由](../features/routing.md) |
| `WORKER_MODE_ENABLED` | `false` | 启用持久化 Worker 模式。要求 `ENTRY_FILE` 指向 `.php` 脚本。布尔——参见[布尔值](#布尔值) |
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
| `WORKER_MAX_MEMORY_MIB` | `0` | 工作进程回收前允许使用的最大内存（MiB）。`0` = 不限制 |

设置 `WORKER_MODE_ENABLED=true` 并将 `ENTRY_FILE` 指向您的 Worker 引导脚本（例如 `ENTRY_FILE=worker.php` 或 `ENTRY_FILE=../worker.php`）。PHP 进程将在多个请求间保持存活，将引导状态（自动加载器、数据库连接）保留在内存中。当达到 `WORKER_MAX_MEMORY_MIB` 时工作进程会被自动回收，应用层可通过 [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit) 主动触发回收。早期版本的 `WORKER_MAX_REQUESTS` 已废弃并被忽略 —— 请勿设置该变量，或迁移到 `Worker::scheduleExit()`。

### 已弃用：`INDEX_FILE` 与 `WORKER_FILE`

旧的 `INDEX_FILE` 和 `WORKER_FILE` 仍会被解析以保持向后兼容。设置后会在启动时输出 `WARN` 日志，并映射到新模型：

| 旧形式 | 当前等效形式 |
|---|---|
| `INDEX_FILE=index.php` | `ENTRY_FILE=index.php` |
| `INDEX_FILE=index.html` | `ENTRY_FILE=index.html` |
| `WORKER_FILE=/path/worker.php` | `WORKER_MODE_ENABLED=true ENTRY_FILE=/path/worker.php` |

若同时设置了新旧变量，`ENTRY_FILE` / `WORKER_MODE_ENABLED` 优先；旧形式将在后续版本中移除。

## SAPI / PHP

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `SUPERGLOBALS_ENABLED` | `true` | 在脚本执行前填充 PHP 超全局变量（`$_GET`、`$_POST`、`$_COOKIE`、`$_FILES`、`$_SERVER`、`php://input`）。设置为[假值](#布尔值)可跳过填充——此时请求数据仅可通过对象 API（`oxphp_http_request()`）获取。适用于直接使用对象 API 且希望避免每次请求都构建超全局变量开销的应用 |

## 超时

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | 连接建立后接收 HTTP 头部的最大秒数（Slowloris 防护） |
| `DRAIN_TIMEOUT_SECONDS` | `25` | 优雅关闭期间等待进行中连接完成的最大秒数 |

PHP 执行时间由 PHP 自身的 `max_execution_time` ini 指令（以及运行时的 `set_time_limit()`）限制，而非 OxPHP 的环境变量。

## 限流

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `RATE_LIMIT` | `0`（关闭） | 每个 IP 在时间窗口内的最大请求数。`0` 禁用限流 |
| `RATE_WINDOW_SECONDS` | `60` | 限流窗口持续时间（秒） |

## 安全

| 变量 | 默认值 | 描述 |
|------|--------|------|
| `FRAME_OPTIONS` | `SAMEORIGIN` | 点击劫持防护。`SAMEORIGIN` 仅允许同源页面嵌入框架，`DENY` 禁止所有框架嵌入，`off` 关闭（适用于通过自定义 CSP 管理框架策略的场景）。任何其他值都会回退到默认值 `SAMEORIGIN` 并在启动时发出警告。为每个响应同时设置 `X-Frame-Options` 和 `Content-Security-Policy: frame-ancestors`。有关具体发出的头部值、服务器头部如何让位于应用程序设置的头部，以及如何选择取值，参见下方的 [点击劫持防护](#点击劫持防护) |
| `TRUSTED_PROXIES` | *（未设置）* | 受信任的反向代理网络（逗号分隔 CIDR 或 `private`）。设置后，OxPHP 使用 rightmost-non-trusted 算法从 `Forwarded`（[RFC 7239](https://www.rfc-editor.org/rfc/rfc7239)）或 `X-Forwarded-For` 中提取真实客户端 IP。同时处理 `X-Forwarded-Proto` 和 `X-Forwarded-Host` 以设置 `$_SERVER['HTTPS']`、`REQUEST_SCHEME`、`SERVER_NAME` 和 `SERVER_PORT`。未设置 = 功能禁用 |
| `PHP_DENY_PATHS` | *（未设置）* | 逗号分隔的 glob 模式列表，禁止其中的 `.php` 文件通过直接 URI 执行（例如 `/uploads/**,/cache/**,/admin/legacy.php`）。模式可以指向整个目录，也可以指向单个文件。在直接文件映射模式（Traditional 与 SPA）下生效；在 Framework 与 Worker 模式下会在启动时发出警告并忽略，因为这两种模式从不直接执行任意 `.php` 文件。同时覆盖经目录索引解析到达的脚本（`/uploads/` → `uploads/index.php`）。对直接 `.php` URI 的匹配发生在任何磁盘 I/O 之前，因此被拒路径无论文件是否存在都返回相同响应（无 existence oracle）。旧名称 `PHP_DENY_DIRS` 作为已弃用别名仍被接受，启动时输出 `WARN`。参见 [PHP 执行拒绝名单](../security/php-deny.md) |
| `PHP_DENY_FALLBACK` | `404` | 命中 `PHP_DENY_PATHS` 时返回什么。可以是 HTTP 状态码 `400`–`599`（与 `ERROR_PAGES_DIR` 配合可自定义 HTML），也可以是以 `/` 开头、指向 `DOCUMENT_ROOT` 内 PHP 回退脚本的 URI 路径。脚本在 `$_SERVER` 中接收 `OXPHP_DENIED_PATH` 与 `OXPHP_DENIED_PATTERN`。启动时严格校验：文件必须存在、规范化路径必须位于 `DOCUMENT_ROOT` 内，且脚本自身不得命中 `PHP_DENY_PATHS`（防止循环） |
| `SYMLINK_ALLOW_PATHS` | *（未设置）* | 逗号分隔的绝对路径列表，列出允许其下符号链接指向 `DOCUMENT_ROOT` 之外的路径。每个条目必须在磁盘上已存在；相对路径与不存在的路径会导致启动失败。未设置 = 不允许任何符号链接逃逸。参见 [Symlink Allow-Paths](../security/symlink-allow-paths.md) |

特殊值 `private` 展开为所有 RFC-1918 私有网络、回环和链路本地地址（IPv4 和 IPv6）：`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`127.0.0.0/8`、`169.254.0.0/16`、`::1/128`、`fc00::/7`、`fe80::/10`。

### 点击劫持防护

点击劫持是一种攻击：恶意页面把你的网站嵌入一个不可见的 `<iframe>`，诱骗用户点击他们看不见的东西——「是的，删除我的账户」按钮、一键购买、OAuth 的「授权」提示。防御手段就是告诉浏览器：谁（如果有的话）被允许把你的页面嵌入框架。这由 `FRAME_OPTIONS` 控制。

由于有两个头部管理框架嵌入——旧的 `X-Frame-Options`（所有浏览器都识别）和新的 `Content-Security-Policy: frame-ancestors`（两者并存时它优先）——OxPHP 会**同时**发出两者，使策略在新旧浏览器上都生效。单个 `FRAME_OPTIONS` 值会映射为匹配的一对：

| `FRAME_OPTIONS` | `X-Frame-Options` | `Content-Security-Policy` | 谁可以嵌入你的页面 |
|-----------------|-------------------|---------------------------|---------------------|
| `SAMEORIGIN`（默认） | `SAMEORIGIN` | `frame-ancestors 'self'` | 仅同源页面 |
| `DENY` | `DENY` | `frame-ancestors 'none'` | 任何人都不行，连你自己的页面也不行 |
| `off` | *（不发送）* | *（不发送）* | 任何人——服务器不施加任何框架嵌入限制 |

**如何选择取值。** `SAMEORIGIN` 是默认值：它阻止点击劫持真正依赖的跨源框架嵌入，同时仍允许你自己的页面互相嵌入——许多应用都合法需要这一点（后台预览、仪表盘小组件、同源托管的支付组件）。当你网站上的任何内容都绝不应被嵌入框架（连它自己也不行）时，选择 `DENY`，这是最严格的策略。仅当你通过应用程序自行设置的完整 `Content-Security-Policy` 来管理框架嵌入时，才选择 `off`（见下文）。

**允许外部源嵌入。** 任何一个 `X-Frame-Options` 取值都无法指定某个具体的被允许源（`ALLOW-FROM` 已从标准中移除）。要允许某个指定的第三方嵌入你的页面，请设置 `FRAME_OPTIONS=off`，并由应用程序发出自己的 `Content-Security-Policy`，其中带有显式的 `frame-ancestors` 列表，例如 `header("Content-Security-Policy: frame-ancestors 'self' https://partner.example.com");`。

**应用程序头部优先。** 服务器头部仅作为兜底，只在响应中尚无该头部时才应用——通过 PHP `header()` 设置了自己的 `X-Frame-Options` 或 `Content-Security-Policy` 的应用程序会保持其原样不动。两个框架嵌入头部被视为同一策略，因此服务器绝不会与应用程序相矛盾：

- 如果应用程序设置了 `X-Frame-Options`，OxPHP 会跳过它的 `frame-ancestors` 兜底（服务器 CSP 在现代浏览器中会覆盖应用程序的选择）。
- 如果应用程序设置了包含 `frame-ancestors` 指令的 `Content-Security-Policy`，OxPHP 会跳过它的 `X-Frame-Options` 兜底（更严格的服务器 `X-Frame-Options` 会在忽略 CSP 的旧浏览器中过度阻止）。

同样的优先级也适用于 `X-Content-Type-Options`，OxPHP 会在每个响应上把它设为 `nosniff`：应用程序设置的值会被逐字保留。请注意，`nosniff` 是唯一有实际作用的值——应用程序用其他任何值覆盖它都会悄然关闭 MIME 嗅探防护。

## TLS

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `TLS_CERT` | *(未设置)* | PEM 编码 TLS 证书的路径。`TLS_CERT` 和 `TLS_KEY` 均设置后才会启用 TLS |
| `TLS_KEY` | *(未设置)* | PEM 编码 TLS 私钥的路径 |
| `TLS_MIN_VERSION` | `1.2` | 接受的最低 TLS 协议版本：`1.2` 或 `1.3`。即使未启用 TLS，也会在启动时（以及 `oxphp config --check` 中）校验——任何其他值（包括非 UTF-8 字节）都会导致硬性启动错误。空值视为未设置 |

## HTTP/2

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `H2_MAX_CONCURRENT_STREAMS` | `PHP_WORKERS_MAX × 4`（最小 32） | 每个 HTTP/2 连接允许的最大并发流数 |
| `H2_MAX_PENDING_RESET` | `20` | 关闭连接前允许排队的 `RST_STREAM` 帧数量上限（Rapid Reset 防护） |
| `H2_MAX_HEADER_LIST_BYTES` | `65536` | 单次请求所有解码后请求头的最大总字节数 |
| `H2_KEEPALIVE_INTERVAL_SECS` | `20` | 发送 PING 帧的时间间隔（秒）；`0` 表示禁用 |
| `H2_KEEPALIVE_TIMEOUT_SECS` | `10` | 等待 PING 回复的超时时间（秒），超时后关闭连接 |

## 静态文件

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `STATIC_MAX_AGE` | `30d` | 静态文件的 `Cache-Control: max-age`。接受以下格式：`30s`、`5m`、`2h`、`30d`、`1w`、`1y`、纯秒数（`3600`），或 `off` 禁用缓存头。替代已弃用的 `STATIC_CACHE_TTL`。 |
| `STATIC_REVALIDATE` | `off` | 布尔——参见[布尔值](#布尔值)。设为真值启用内存内容缓存的 mtime 重新验证：每个文件最多每 3 秒（而非每次请求）重新检查一次文件修改时间，自动清除过期条目，因此更改会在 3 秒内变得可见。替代已弃用的 `STATIC_CACHE`（其中 `off` 含义相反）。 |
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
| `INTERNAL_ADDR` | *(未设置)* | 内部服务器地址（`/health`、`/metrics`、`/config`）。未设置时不启动内部服务器。仅含端口的取值（`:9090` 或 `9090`）绑定 `127.0.0.1`；要将其暴露到主机之外，需显式绑定 `0.0.0.0:9090` |
| `INTERNAL_ALLOW_IPS` | *(未设置)* | 内部服务器的逗号分隔 CIDR/IP 允许名单。名单之外的对端访问 `/metrics`、`/config` 和插件路径时收到 `403`；健康探针（`/health`、`/healthz`、`/readyz`、`/startupz` 及其长格式）始终可达。未设置/为空 = 全部允许。回环地址**不会**隐式放行——需列出 `127.0.0.1/32` 以保留本机访问。格式错误的名单会导致启动失败 |
| `ERROR_PAGES_DIR` | *(未设置)* | 包含自定义错误页面的目录，文件名格式为 `{status}.html`（如 `404.html`、`503.html`） |
| `MAX_QUERY_BODY` | `524288` | 内部查询端点的最大请求体大小（字节，512 KiB） |
| `TRACE_CONTEXT` | `false` | 布尔——参见[布尔值](#布尔值)。当为真值时启用 W3C Trace Context 传播：读取 `traceparent`/`tracestate` 头部并通过 `$_SERVER` 转发给 PHP |

## OpenTelemetry

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | 启用 OpenTelemetry Span 导出。自动设置 `TRACE_CONTEXT=true`。布尔——参见[布尔值](#布尔值) |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | 导出协议：`grpc` 或 `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317`（gRPC）或 `http://localhost:4318`（HTTP） | OTLP 收集器端点 |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | 导出超时（毫秒） |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(未设置)* | 认证头：`key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | 导出 Span 中的服务名称 |
| `OTEL_SERVICE_VERSION` | *(未设置)* | 服务版本属性 |
| `OTEL_RESOURCE_ATTRIBUTES` | *(未设置)* | 额外资源属性：`env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | 采样策略：`always_on`、`always_off`、`traceidratio`、`parentbased_always_on`、`parentbased_always_off`、`parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | 基于比率的采样器的采样比率（0.0–1.0） |

> **注意：** 无效或超出范围的 `OTEL_TRACES_SAMPLER_ARG` 值会被钳制到 `[0.0, 1.0]` 并在 warn 级别记录日志。未知的 `OTEL_TRACES_SAMPLER` 值会回退到 `parentbased_traceidratio` 并记录日志。

## APM

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `OTEL_APM_ENABLED` | `false` | 启用 APM：自动埋点、错误捕获和 PHP 追踪 SDK。需要 `OTEL_ENABLED=true`。布尔——参见[布尔值](#布尔值) |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | 慢查询阈值（毫秒）。超过此值的数据库查询将添加 `oxphp.db.slow=true` Span 属性 |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | 将绑定参数记录到 `db.params` Span 属性中。如果参数可能包含敏感数据，请在生产环境中禁用。布尔——参见[布尔值](#布尔值) |
| `OTEL_APM_STACKTRACE_MAX_BYTES` | `8192` | `exception.stacktrace` 属性的最大字节数。超出时从尾部截断并标记 `…(truncated)`。`0` 表示禁用截断 |
| `OTEL_APM_MESSAGE_MAX_BYTES` | `4096` | `exception.message` 属性的最大字节数（默认值与 New Relic 的单属性值上限一致）。超出时从尾部截断并标记 `…(truncated)`。`0` 表示禁用截断 |

当 APM 启用时，OxPHP 自动 hook 33 个 PHP 内部函数（PDO、mysqli、cURL、Redis、Memcached、文件 I/O）来创建子 Span。无论 APM 是否启用，`oxphp_apm_*()` PHP 函数都会注册——禁用时它们是安全的空操作。

## 异步工作进程

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `ASYNC_WORKERS` | `0`（禁用） | 专用异步工作线程数。为 `0` 时，异步函数（`oxphp_async` 等）已注册但调用时抛出 `OxPHP\Async\AsyncException`。设为正整数可启用后台任务执行 |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS × 64` | 异步队列中的最大待处理任务数。`0` = 自动（工作进程数 × 64） |
| `ASYNC_MAX_FIBERS` | `256` | 每个 Worker 的并发异步任务 fiber 上限。进程级在途上限（排队 + 运行中）为 `ASYNC_MAX_FIBERS × ASYNC_WORKERS`；超出上限的分发会立即以 `OxPHP\Async\AsyncException` 拒绝，因此 fan-out 组合不会死锁 |

异步工作进程池处理从 PHP 分发的即发即忘后台任务。它独立于 PHP 工作进程池，标准请求处理不需要它。

这三个变量中任何一个的格式错误值（例如 `ASYNC_WORKERS=8x`）都会导致启动错误——回退到默认值会静默禁用或错误配置该池。完全为空的值视为未设置。

## 共享状态

进程内并发原语（`OxPHP\Shared\Counter`、`Map`、`Channel`、`Mutex`、`Once`、`Pool`、`Atomic`、`Flag`、`Registry`）。API 速览参见[共享状态](../shared-state/shared-state.md)。

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `SHARED_ENABLED` | `true` | 布尔——参见[布尔值](#布尔值)。整个 `OxPHP\Shared\*` 子系统的总开关 |
| `SHARED_MAX_ENTRIES` | `100000` | 所有 Shared 条目合计的全局上限。超出后插入失败并抛出 `CapacityException` |
| `SHARED_MAX_BYTES` | `1073741824`（1 GiB） | 所有 Shared 条目估算内存的全局上限 |
| `SHARED_SOFT_LIMIT_RATIO` | `0.7` | 当使用量超过 `SHARED_MAX_BYTES` / `SHARED_MAX_ENTRIES` 的此比例时，开始丢弃最低优先级的工作 |
| `SHARED_METRICS_ENABLED` | `true` | 布尔。切换 `oxphp_shared_*` Prometheus 输出 |
| `SHARED_INTROSPECTION_ENABLED` | `true` | 布尔。切换内部服务器上的 `/__ox_shared/*` 自省 API |
| `SHARED_INTROSPECTION_PREVIEW_ENABLED` | `true` | 布尔。切换自省响应中的值预览（当预览可能泄露敏感数据时禁用） |
| `SHARED_CYCLE_DETECT_DEPTH` | `16` | 环检查期间的 BFS 深度。对于合法的深层图可调高 |
| `SHARED_CYCLE_DETECT_EDGES` | `10000` | 环检查期间遍历的边数。对于合法的稠密图可调高 |
| `SHARED_MAX_VALUE_SIZE` | `1048576`（1 MiB） | 单值大小上限。插入更大的值会快速失败 |
| `SHARED_MAX_CHANNEL_BYTES` | `67108864`（64 MiB） | 单个 Channel 的负载总量上限 |
| `SHARED_POISON_STRICT` | `false` | 布尔。为真值时，Mutex/Once 闭包内的 panic 会永久毒化该原语，而非尽力恢复 |
| `SHARED_LOCK_DIAGNOSTICS` | `off` | 锁竞争诊断：`off`、`count` 或 `trace` |
| `SHARED_LOCK_POLL_INTERVAL_MS` | `100` | 锁诊断采样器使用的轮询间隔 |
| `SHARED_PREVIEW_STRING_LIMIT` | `256` | `/entry?id=…` 预览中每个字符串的截断长度 |
| `SHARED_PREVIEW_ARRAY_LIMIT` | `20` | `/entry?id=…` 预览中采样的条目数 |

## 性能剖析

发出 xhprof / speedscope 跟踪的采样剖析器。输出格式与查看器集成参见[剖析](../features/profiling.md)。

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `PROFILER_ENABLED` | `false` | 布尔——参见[布尔值](#布尔值)。总开关。即使关闭，其他所有 `PROFILER_*` 变量仍会在启动时解析，以便拼写错误立即暴露 |
| `PROFILER_SAMPLE_RATE` | `0.0` | 单个请求被采样的概率（0.0–1.0）。超出范围的值会被钳制 |
| `PROFILER_INTERNAL` | `false` | 布尔。为真值时，发往内部服务器（`/health`、`/metrics`、插件端点）的请求也可被采样 |
| `PROFILER_AUTH_TOKEN` | *（未设置）* | 可选 Bearer Token。设置后，`oxphp_profiler_*` PHP 函数要求请求携带该 Token 才能启用按需剖析 |
| `PROFILER_MAX_SPANS` | `50000` | 单个请求的 profile span 上限。超过该上限的 profile 会被截断 |
| `PROFILER_MAX_DEPTH` | `256` | 每次采样捕获的最大调用栈深度。硬上限为 `65535` |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | 磁盘 profile 文件的输出目录 |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,speedscope` | 逗号分隔的输出格式列表，决定写入磁盘的格式 |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | 每秒写入磁盘的 profile 文件数上限 |
| `PROFILER_RETENTION_COUNT` | `100` | `PROFILER_OUTPUT_DIR` 中保留的 profile 文件最大数量。较旧的文件会被清除 |
| `PROFILER_EXPORT_URL` | *（未设置）* | 用于 POST profile 的远程端点。设置后仍会写盘，除非 `PROFILER_OUTPUT_FORMATS` 为空 |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | `PROFILER_EXPORT_URL` 上传时使用的线格式 |
| `PROFILER_EXPORT_AUTH_TOKEN` | *（未设置）* | 每次导出请求附带的可选 Bearer Token |
| `PROFILER_EXPORT_XHGUI` | *（自动检测）* | 布尔。强制导出负载使用 XHGui 兼容的包装。未设置 = 当 `PROFILER_EXPORT_URL` 路径以 `/run/import` 结尾时自动检测（host/query 中的字符串不匹配） |
| `PROFILER_EXPORT_BUGGREGATOR` | *（自动检测）* | 布尔。强制使用 Buggregator 包装。未设置 = 当 `PROFILER_EXPORT_URL` 路径以 `/api/profiler/store` 结尾时自动检测。此信封始终发送 xhprof，因此 `PROFILER_EXPORT_FORMAT` 对它无效（非 xhprof 取值仅告警，不致命）。与 `PROFILER_EXPORT_XHGUI` 互斥——同时启用会在启动时报错 |
| `PROFILER_EXPORT_APP_NAME` | *（未设置）* | Buggregator 的 `app_name`，用于按项目分组 |
| `PROFILER_EXPORT_TAGS` | *（未设置）* | Buggregator 的 `tags`，格式为 `key=value,key2=value2`；非法项、空键或重复键会在启动时报错 |

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
ENTRY_FILE=index.php
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
DRAIN_TIMEOUT_SECONDS=25
COMPRESSION_LEVEL=4
STATIC_MAX_AGE=30d
```

### 生产环境（工作进程模式）

```bash
LISTEN_ADDR=0.0.0.0:80
DOCUMENT_ROOT=/var/www/html/public
WORKER_MODE_ENABLED=true
ENTRY_FILE=../worker.php
PHP_WORKERS=8
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
ENTRY_FILE=index.php
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
  "entry_file": "/var/www/html/public/index.php",
  "log_level": "warn",
  "executor_type": "sapi",
  "php_workers": "8",
  "tokio_workers": 4,
  "queue_capacity": 1024,
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode_enabled": false,
  "worker_max_memory_mib": 0,
  "static_max_age": 2592000,
  "static_revalidate": false,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "async_max_fibers": 256,
  "async_in_flight_cap": 0,
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

> **注意：** 所提供的 `/config` 响应会剔除内部 `Config` 表示中携带的若干键：TLS 证书和私钥路径从不输出（`tls_enabled` 字段表示 TLS 是否已启用），同时 `internal_addr` 和 `error_pages_dir` 也被移除——这些部署拓扑和文件系统路径会帮助攻击者，且指标抓取器并不需要它们。

## 参见

- [路由](../features/routing.md) — 路由模式与 `ENTRY_FILE` 行为
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
