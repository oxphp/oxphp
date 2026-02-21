---
title: TLS
description: 通过 rustls 实现 HTTPS 支持，支持自动 ALPN 协商
---

OxPHP 使用 rustls 直接终结 TLS，不依赖 OpenSSL。配置 TLS 后，服务器接受 HTTPS 连接并通过 ALPN 协商 HTTP/2 或 HTTP/1.1。

## 配置

| 变量 | 描述 | 默认值 |
|----------|-------------|---------|
| `TLS_CERT` | PEM 编码证书文件的路径 | *（未设置）* |
| `TLS_KEY` | PEM 编码私钥文件的路径 | *（未设置）* |

两个变量都必须设置才能启用 TLS。如果只提供了其中一个，TLS 不会启用。

```bash
TLS_CERT=/etc/oxphp/cert.pem
TLS_KEY=/etc/oxphp/key.pem
LISTEN_ADDR=0.0.0.0:443
```

## 工作原理

启动时，OxPHP 读取证书和密钥文件，将其解析为 PEM 格式，并从 rustls 配置创建 `TlsAcceptor`。

TLS 配置包括：

- **加密提供者**：ring（通过 `rustls::crypto::ring::default_provider()`）
- **协议版本**：rustls 选择的安全默认值（TLS 1.2 和 1.3）
- **客户端认证**：已禁用（不进行客户端证书验证）
- **ALPN 协议**：`h2` 和 `http/1.1`，按此顺序

当 TCP 连接到达时，服务器调用 `acceptor.accept(stream)` 执行 TLS 握手，然后将加密流传递给 hyper 进行 HTTP 处理。

## 证书格式

证书文件必须包含一个或多个 PEM 编码的证书（服务器证书，后跟中间证书（如适用））。密钥文件必须包含单个 PEM 编码的私钥（RSA、ECDSA 或 Ed25519）。

### 开发用自签名证书

为本地开发生成自签名证书：

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout key.pem -out cert.pem -days 365 -nodes \
  -subj "/CN=localhost"
```

然后配置 OxPHP：

```bash
TLS_CERT=./cert.pem
TLS_KEY=./key.pem
```

### Docker 示例

将证书挂载到容器中：

```yaml
services:
  oxphp:
    image: oxphp:latest
    ports:
      - "443:443"
    environment:
      LISTEN_ADDR: "0.0.0.0:443"
      TLS_CERT: /certs/cert.pem
      TLS_KEY: /certs/key.pem
    volumes:
      - ./certs:/certs:ro
```

## 混合模式运行

OxPHP 不在同一个监听器上同时提供 HTTP 和 HTTPS 服务。要支持两种协议，可以运行两个实例或使用反向代理进行 HTTP 到 HTTPS 的重定向。

## 无 OpenSSL 依赖

使用 rustls 意味着服务器二进制文件完全不链接 OpenSSL。这消除了生产部署中常见的 CVE 来源，并简化了容器镜像（无需 `libssl` 包）。

## 另请参阅

- [超时](timeouts.md) -- 头部读取超时在 TLS 握手完成后开始计时
- [速率限制](rate-limiting.md) -- 每 IP 速率限制适用于 HTTP 和 HTTPS 连接
