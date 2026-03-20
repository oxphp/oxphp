---
title: Docker
description: Docker 镜像使用、compose.yml 参考及部署技巧
---

OxPHP 以预构建的 Docker 镜像形式发布，地址为 `ghcr.io/oxphp/oxphp:0.1.0`。本页介绍如何使用该镜像、通过 `compose.yml` 进行配置，以及常见的部署注意事项。

## 使用镜像

运行 OxPHP 最简单的方式是基于基础镜像扩展并加入你的应用文件：

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

镜像包含：

- `oxphp` 二进制文件
- PHP 8.4 ZTS 运行时（`libphp.so`）
- Bridge 库（`liboxphp_bridge.so`）
- PHP 扩展（`oxphp_sapi.so`），提供 `oxphp_request_id()`、`oxphp_server_info()` 等函数
- 依赖极少的 Alpine Linux 基础系统
- 用于非 root 执行的 `www-data` 用户（UID 82，GID 82）

默认文档根目录为 `/var/www/html/public`。服务器监听 8080 端口。`CMD` 为 `["oxphp"]`。

## compose.yml 参考

```yaml
services:
  oxphp:
    build: .
    ports:
      - "8080:8080"   # 主 HTTP 服务器
      - "9090:9090"   # 内部服务器（健康检查/指标/配置）
    volumes:
      - ./www:/var/www/html:ro
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
      - ./certs:/etc/ssl/oxphp:ro
    environment:
      # 服务器
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html/public
      # - INDEX_FILE=index.php       # 启用 Framework 路由模式
      - EXECUTOR=sapi                # "sapi" 或 "stub"
      # - PHP_WORKERS=0              # 静态模式：0 = CPU/2（最少 1），或固定数量 N
      # - PHP_WORKERS=2:16           # 动态模式：在 2 到 16 之间扩缩容
      # - PHP_WORKERS_IDLE_SECONDS=30    # 动态缩容的空闲超时时间
      # - QUEUE_CAPACITY=512         # 默认值：PHP_WORKERS * 128

      # 日志
      - LOG_LEVEL=info

      # 内部服务器
      - INTERNAL_ADDR=0.0.0.0:9090

      # 超时（秒）
      - HEADER_TIMEOUT_SECONDS=5
      - REQUEST_TIMEOUT_SECONDS=120
      - DRAIN_TIMEOUT_SECONDS=30

      # 限流（0 = 禁用）
      # - RATE_LIMIT=100
      # - RATE_WINDOW_SECONDS=60

      # TLS
      # - TLS_CERT=/etc/ssl/oxphp/server.pem
      # - TLS_KEY=/etc/ssl/oxphp/server.key

      # 错误页面
      # - ERROR_PAGES_DIR=/var/www/errors

      # 压缩级别（0-11，0=禁用，默认：4）
      # - COMPRESSION_LEVEL=4
    restart: unless-stopped
```

在开发环境中，可以将源码目录挂载为卷，而无需将文件复制到镜像中：

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "8080:8080"
    volumes:
      - ./www:/var/www/html:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html/public
```

### 环境变量

| 变量 | 默认值 | 描述 |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | 主 HTTP 服务器的地址和端口 |
| `DOCUMENT_ROOT` | `/var/www/html/public` | 提供文件服务的根目录 |
| `INDEX_FILE` | _(未设置)_ | 设为 `index.php` 启用 Framework 模式，设为 `index.html` 启用 SPA 模式 |
| `EXECUTOR` | `sapi` | PHP 执行器类型：`sapi`（真实 PHP）或 `stub`（占位符） |
| `PHP_WORKERS` | `0`（CPU / 2，最少 1，静态） | Worker 池模式。`N` = 固定池，`MIN:MAX` = 动态扩缩容 |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | 动态 worker 退出前的空闲超时时间 |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | 有界请求队列大小。队列满时返回 529 |
| `LOG_LEVEL` | `info` | 日志级别：`trace`、`debug`、`info`、`warn`、`error` |
| `MAX_CONNECTIONS` | `10000` | 最大并发连接数 |
| `INTERNAL_ADDR` | _(未设置)_ | 内部服务器地址。未设置则禁用 |
| `HEADER_TIMEOUT_SECONDS` | `5` | 读取请求头的超时时间 |
| `REQUEST_TIMEOUT_SECONDS` | `120` | 最大请求处理时间。设为 0 则禁用超时 |
| `DRAIN_TIMEOUT_SECONDS` | `30` | 关机期间进行中连接的宽限期 |
| `RATE_LIMIT` | `0` | 每个 IP 每窗口期的最大请求数。0 表示禁用限流 |
| `RATE_WINDOW_SECONDS` | `60` | 限流窗口时长（秒） |
| `TLS_CERT` | _(未设置)_ | TLS 证书 PEM 文件路径 |
| `TLS_KEY` | _(未设置)_ | TLS 私钥 PEM 文件路径 |
| `ERROR_PAGES_DIR` | _(未设置)_ | 包含 `{status}.html` 错误页面文件的目录 |
| `COMPRESSION_LEVEL` | `4` | Brotli 压缩质量级别（0-11）。`0` 禁用压缩 |
| `TOKIO_WORKERS` | `0` | Tokio 异步运行时线程数（0 = CPU / 2，最少 1） |
| `ACCESS_LOG` | *(关闭)* | 每请求 JSON 访问日志：`all`、`error`（仅 4xx/5xx）、空 = 关闭 |


### 端口

| 端口 | 用途 |
|------|---------|
| `8080` | 主 HTTP 服务器（如配置了 TLS 则为 HTTPS） |
| `9090` | 内部服务器：`/health`、`/metrics`、`/config` |

### 卷挂载

| 主机路径 | 容器路径 | 用途 |
|-----------|---------------|---------|
| `./www` | `/var/www/html` | 应用文件（PHP 脚本、静态资源）。建议以 `:ro` 挂载 |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | PHP 配置（OPcache、sessions）。建议以 `:ro` 挂载 |
| `./certs` | `/etc/ssl/oxphp` | TLS 证书和密钥文件。建议以 `:ro` 挂载 |

## PHP 配置

要自定义 PHP 设置（OPcache、JIT、sessions 等），请创建 `oxphp.ini` 文件并将其挂载到容器中：

```ini
[opcache]
opcache.enable=1
opcache.jit=1255
opcache.jit_buffer_size=64M
```

```yaml
volumes:
  - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
```

推荐设置请参见 [OPcache](../php/opcache.md)。

## Alpine www-data 用户

镜像以 `www-data`（UID 82，GID 82）身份运行，与 nginx 和 Apache 的惯例保持兼容。如果你的应用需要向特定目录写入（sessions、缓存、上传文件），请确保这些目录对 UID 82 可写。

## 从源码构建

如果需要从源码构建 OxPHP（例如启用自定义 Cargo feature 或修改服务器），请参阅[安装](installation.md)指南中的源码构建说明。OxPHP 仓库包含一个多阶段 Dockerfile，可从源码编译 bridge 库、PHP 扩展和 Rust 二进制文件。

## 参见

- [安装](installation.md) -- 源码构建前置条件及说明
- [快速开始](quick-start.md) -- 5 分钟内运行 OxPHP
- [配置](../operations/configuration.md) -- 完整的环境变量参考
- [优雅关机](../operations/graceful-shutdown.md) -- 排空行为和超时设置
