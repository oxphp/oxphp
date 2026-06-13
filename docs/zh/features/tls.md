---
title: TLS
description: 在 OxPHP 中配置原生 TLS 终止，支持 TLS 1.2、TLS 1.3、HTTP/2 ALPN 协商和 PEM 证书。
---

# TLS

OxPHP 原生处理 TLS 终止——无需反向代理或外部 SSL 库。配置完成后，服务器接受 HTTPS 连接并自动协商最佳可用协议。

## 工作原理

要启用 TLS，请将 `TLS_CERT` 和 `TLS_KEY` 指向您的 PEM 编码证书和私钥文件。一旦两者都设置，服务器就会在 `LISTEN_ADDR` 指定的地址上监听 HTTPS 连接。

TLS 握手发生在任何 HTTP 处理之前：

1. TCP 连接到达 `LISTEN_ADDR`。
2. 服务器使用配置的证书和密钥执行 TLS 握手。
3. 协议协商（ALPN）根据客户端支持选择 HTTP/2（`h2`）或 HTTP/1.1。
4. 加密连接被传递到 HTTP 层进行正常的请求处理。

> **注意：** 启用 TLS 时，请求头和请求超时在 TLS 握手完成后按请求应用。

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `TLS_CERT` | *(未设置)* | PEM 编码证书文件的路径。必须同时设置 `TLS_CERT` 和 `TLS_KEY` 才能启用 TLS |
| `TLS_KEY` | *(未设置)* | PEM 编码私钥文件的路径 |
| `LISTEN_ADDR` | `0.0.0.0:80` | 监听的地址和端口。使用 TLS 时请更改为 `0.0.0.0:443` |

如果只提供了 `TLS_CERT` 或 `TLS_KEY` 中的一个，TLS 不会启用，服务器以纯 HTTP 模式启动。

## 支持的协议

| 能力 | 详情 |
|------|------|
| TLS 版本 | TLS 1.2 和 TLS 1.3 |
| ALPN 协议 | `h2`（HTTP/2）和 `http/1.1`，按此顺序协商 |
| 客户端证书 | 不支持（无双向 TLS） |

## HTTP/2

OxPHP 在同一端口上同时提供 HTTP/2 和 HTTP/1.1。协议按每个连接选择——没有开关用于启用或禁用 HTTP/2：

- **基于 TLS** 时，协议在握手期间通过 ALPN 协商。OxPHP 先通告 `h2`，再通告 `http/1.1`，因此支持 HTTP/2 的客户端获得 HTTP/2，其他客户端则透明回退到 HTTP/1.1。
- **无 TLS（h2c）** 时，OxPHP 识别 HTTP/2 连接前言，并向以先验知识（prior knowledge）连接的客户端（例如 `curl --http2-prior-knowledge`）提供明文 HTTP/2。不支持 HTTP/2 的客户端在同一端口上继续使用 HTTP/1.1。（不使用 `Upgrade: h2c` 握手——明文 HTTP/2 需要先验知识。）

HTTP/2 的流控窗口被提升到高于协议默认值——每连接 8 MB、每流 4 MB，而默认值为 64 KB——以避免在典型的 PHP 响应（通常大于一个默认窗口）上出现停顿。

PHP 在 `$_SERVER['SERVER_PROTOCOL']` 中看到协商后的协议（`"HTTP/2"` 或 `"HTTP/1.1"`）。

### 验证

```bash
# 基于 TLS 的 HTTP/2（通过 ALPN 协商）
curl -k --http2 -I https://localhost/

# 明文 HTTP/2（h2c，先验知识）
curl --http2-prior-knowledge -I http://localhost/
```

在响应行中查找 `HTTP/2 200`。

### 连接限制

OxPHP 在 HTTP/2 连接层面施加限制，以约束单个 TCP 连接对 PHP 工作进程池造成的
放大效应。每个被接受的流都会成为工作进程队列中的一个 PHP 请求，因此来自单个
连接的无限制流数量将独自耗尽整个工作进程池。

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `H2_MAX_CONCURRENT_STREAMS` | `PHP_WORKERS_MAX × 4`（最小 32） | 每个连接允许的最大并发流数。超出上限的流将收到 `REFUSED_STREAM` |
| `H2_MAX_PENDING_RESET` | `20` | 关闭连接前允许排队的 `RST_STREAM` 帧数量上限（CVE-2023-44487 Rapid Reset 防护） |
| `H2_MAX_HEADER_LIST_BYTES` | `65536` | 单次请求所有解码后请求头的最大总字节数（HPACK 炸弹防护） |
| `H2_KEEPALIVE_INTERVAL_SECS` | `20` | 发送 PING 帧的时间间隔（秒）；`0` 表示禁用 keepalive |
| `H2_KEEPALIVE_TIMEOUT_SECS` | `10` | 等待 PING 回复的超时时间（秒），超时后关闭连接 |

`PHP_WORKERS_MAX` 是通过 `PHP_WORKERS` 配置的最大工作进程数。对于 `4:16`
这样的动态范围，取最大值（16）。默认值随工作进程数扩展，确保单个连接上合法的
并发页面加载请求不会超出工作进程池的承受能力。

## 支持的密钥类型

私钥文件必须包含以下格式之一的单个 PEM 编码密钥：

- **RSA**
- **ECDSA**（如 prime256v1、secp384r1）
- **Ed25519**

证书文件可以包含一个或多个 PEM 编码证书。用于生产环境时，请包含完整的证书链：服务器证书后跟任何中间证书。

## 用于开发的自签名证书

为本地开发生成自签名 ECDSA 证书：

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout key.pem -out cert.pem -days 365 -nodes \
  -subj "/CN=localhost"
```

然后配置 OxPHP 使用生成的文件：

```bash
TLS_CERT=./cert.pem
TLS_KEY=./key.pem
LISTEN_ADDR=0.0.0.0:443
```

## 故障排除

### 服务器启动但 TLS 未激活

OxPHP 要求**同时**设置 `TLS_CERT` 和 `TLS_KEY`。如果其中任一缺失，服务器将以纯 HTTP 模式启动，不发出任何警告。请确认两个变量都已设置：

```bash
docker exec <container> env | grep TLS
```

### 启动时出现 `no private key found in PEM file` 错误

密钥文件为空、损坏或只包含证书。请验证密钥文件是否包含 `-----BEGIN ... PRIVATE KEY-----` 块：

```bash
grep "PRIVATE KEY" key.pem
```

如果密钥缺失，请重新生成证书和密钥对。

### 启动时出现 `no certificates found in PEM file` 错误

证书文件为空或损坏。请验证证书文件是否包含至少一个 `-----BEGIN CERTIFICATE-----` 块：

```bash
grep "BEGIN CERTIFICATE" cert.pem
```

### 客户端看到证书链错误

服务器只发送了叶证书，未包含中间证书。请将完整的证书链合并到单个 PEM 文件中：

```bash
cat cert.pem intermediate.pem > fullchain.pem
```

然后设置 `TLS_CERT=./fullchain.pem`。

### 证书已过期

OxPHP 在启动时读取证书文件并将其保存在内存中。在磁盘上更新证书文件不会有任何效果，直到服务器重启。

**修复：** 证书更新后重启 OxPHP。使用证书更新工具自动化此操作（如 certbot 的 `--deploy-hook` 选项）。

### 无法在同一端口上同时提供 HTTP 和 HTTPS

OxPHP 监听单个端口。要同时支持两种协议，请使用处理 HTTP 到 HTTPS 重定向的反向代理（Caddy、Traefik、nginx），或在 80 端口上运行第二个专门用于重定向流量的 OxPHP 实例。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.8.0
    ports:
      - "443:443"
    environment:
      LISTEN_ADDR: "0.0.0.0:443"
      TLS_CERT: "/etc/ssl/oxphp/cert.pem"
      TLS_KEY: "/etc/ssl/oxphp/key.pem"
    volumes:
      - ./app:/var/www/html:ro
      - ./certs:/etc/ssl/oxphp:ro
```

## 最佳实践

- **在 PEM 链中包含中间证书。** 将服务器证书放在首位，然后按顺序排列中间证书，以便客户端能够验证完整的信任路径。
- **自动化证书更新。** 使用 certbot 或 acme.sh 在证书过期前更新证书，然后重启 OxPHP 以加载新文件。
- **使用反向代理处理 HTTP 到 HTTPS 的重定向。** OxPHP 不能在同一端口上同时提供 HTTP 和 HTTPS 服务。

## 注意事项

- OxPHP 不依赖 OpenSSL。TLS 由内置实现处理，消除了外部库 CVE 的常见来源。
- 证书和密钥文件只在启动时读取。在磁盘上更新证书需要重启服务器。

## 参见

- [配置参考](../operations/configuration.md) — 完整的环境变量列表
- [Docker 指南](../getting-started/docker.md) — Docker 中的卷挂载和证书管理
