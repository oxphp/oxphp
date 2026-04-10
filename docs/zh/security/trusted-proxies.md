---
title: 受信任代理
description: 配置 OxPHP 从反向代理头（Forwarded、X-Forwarded-For/Proto/Host）中提取真实客户端 IP、协议和主机名。
---

# 受信任代理

当 OxPHP 运行在反向代理（Kubernetes Ingress、Cloudflare、AWS ALB、nginx）后面时，所有请求都来自代理的 IP 地址。如果不配置受信任代理，速率限制、访问日志和 `$_SERVER['REMOTE_ADDR']` 都会看到代理 IP 而非真实客户端。

## 配置

```bash
# 逗号分隔的 CIDR 列表
TRUSTED_PROXIES="10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"

# 简写：所有 RFC-1918 + 回环 + 链路本地（IPv4 和 IPv6）
TRUSTED_PROXIES="private"
```

未设置时，OxPHP 忽略所有转发头 — 这是安全的默认行为。

## 工作原理

当请求来自受信任 IP 时，OxPHP 按优先级检查转发头：

1. **`Forwarded`**（[RFC 7239](https://www.rfc-editor.org/rfc/rfc7239)）— 标准化头部
2. **`X-Forwarded-For` / `X-Forwarded-Proto` / `X-Forwarded-Host`** — 事实标准

如果存在 `Forwarded` 头，则忽略 `X-Forwarded-*` 头。

### 客户端 IP 提取

OxPHP 使用 **rightmost-non-trusted** 算法 — 与 nginx（`real_ip_recursive on`）、Caddy、Traefik 和 Apache 相同：

```
X-Forwarded-For: 203.0.113.50, 172.16.1.1, 10.0.0.5
TCP peer: 10.0.0.1（受信任）

从右向左遍历：
  10.0.0.5    → 受信任 → 跳过
  172.16.1.1  → 受信任 → 跳过
  203.0.113.50 → 不受信任 → 客户端 IP
```

这可以防止通过前置值进行欺骗 — 攻击者可以在左侧添加虚假 IP，但最右边的不受信任 IP 是由链中最后一个受信任代理设置的。

## 变化内容

配置 `TRUSTED_PROXIES` 且连接 IP 受信任时：

| 组件 | 无受信任代理 | 有受信任代理 |
|------|------------|------------|
| `$_SERVER['REMOTE_ADDR']` | 代理 IP | 真实客户端 IP |
| `$_SERVER['HTTPS']` | 基于 OxPHP 的 TLS 配置 | 来自 `Forwarded: proto=` 或 `X-Forwarded-Proto` |
| `$_SERVER['REQUEST_SCHEME']` | 基于 TLS 的 `http` 或 `https` | 来自转发协议 |
| `$_SERVER['SERVER_NAME']` | 来自 `Host` 头 | 来自 `Forwarded: host=` 或 `X-Forwarded-Host` |
| `$_SERVER['SERVER_PORT']` | 来自 `Host` 头 | 来自转发主机 |
| 速率限制 | 按代理 IP | 按客户端 IP |
| 访问日志 | 代理 IP | 真实客户端 IP |

## `private` 网络

`private` 简写包括：

| 网络 | 描述 |
|------|------|
| `10.0.0.0/8` | A 类私有网络 |
| `172.16.0.0/12` | B 类私有网络 |
| `192.168.0.0/16` | C 类私有网络 |
| `127.0.0.0/8` | 回环地址 |
| `169.254.0.0/16` | 链路本地 |
| `::1/128` | IPv6 回环 |
| `fc00::/7` | IPv6 唯一本地地址 |
| `fe80::/10` | IPv6 链路本地 |

## 安全性

- **安全默认值** — 未设置 `TRUSTED_PROXIES` 时不处理转发头
- **CIDR 验证** — `TRUSTED_PROXIES` 中的无效值会导致启动错误
- **防欺骗** — rightmost-non-trusted 算法忽略攻击者前置的值
- 来自不受信任 IP 的请求 — 转发头完全被忽略

## 另请参阅

- [速率限制](../features/rate-limiting.md) — per-IP 速率限制使用解析后的客户端 IP
- [访问日志](../features/access-logging.md) — `remote_addr` 字段显示解析后的客户端 IP
- [配置参考](../operations/configuration.md) — 所有环境变量
