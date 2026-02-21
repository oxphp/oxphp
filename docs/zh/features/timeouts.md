---
title: 超时
description: 可配置的连接和请求超时
---

OxPHP 通过可配置的超时来防护慢速客户端和失控请求。每个超时都可通过环境变量配置。

## 配置

| 变量 | 描述 | 默认值 |
|----------|-------------|---------|
| `HEADER_TIMEOUT_SECS` | TCP 连接后接收 HTTP 头的最大时间 | `5` |
| `IDLE_TIMEOUT_SECS` | Keep-alive 连接上请求之间的最大空闲时间 | `60` |
| `REQUEST_TIMEOUT_SECS` | 请求处理的最大总时间（包括 PHP 执行） | `120` |

```bash
HEADER_TIMEOUT_SECS=5
IDLE_TIMEOUT_SECS=60
REQUEST_TIMEOUT_SECS=120
```

设置 `HEADER_TIMEOUT_SECS=0` 将跳过向 hyper 注册头部读取超时。设置 `REQUEST_TIMEOUT_SECS=0` 将完全禁用请求超时。

## 超时类型

### 头部读取超时

控制服务器在 TCP 连接建立（或 TLS 握手完成）后等待客户端发送完整 HTTP 头的时间。

这可以防护 slowloris 类型的攻击，即客户端每次发送一个字节的头部来无限期占用连接。

**实现说明**：hyper-util 要求在设置 `header_read_timeout` 之前通过 `builder.http1().timer(TokioTimer::new())` 注册一个计时器。OxPHP 始终注册此计时器。如果超时设置为零，则跳过 `header_read_timeout` 调用。

### 空闲超时

用于控制 keep-alive 连接在请求之间可以保持空闲的时间。`IDLE_TIMEOUT_SECS` 变量从环境中读取并包含在 `/config` 内部端点中，但 hyper-util 的 HTTP/1.1 构建器没有暴露 `keep_alive_timeout` 设置。此超时目前不在连接层强制执行。

### 请求超时

控制处理单个请求的最大总时间，从路由解析到 PHP 执行、响应构建和压缩。通过 `tokio::time::timeout` 包装分发流程实现。适用于常规脚本执行和处理器模式请求。

超时触发时，服务器返回 `504 Gateway Timeout` 响应，并记录包含请求 ID 和路径的警告日志。

对于 PHP 请求，请求超时是外层边界。PHP 自身的 `max_execution_time` 可能先触发，但请求超时确保即使 PHP 不遵守自身的时间限制，服务器端资源也能被回收。

## 超时的交互方式

头部读取超时和请求超时覆盖请求的不同阶段：

```
TCP connect (+ TLS handshake if enabled)
  |
  +-- [HEADER_TIMEOUT_SECS] --> headers received
  |                               |
  |                               +-- [REQUEST_TIMEOUT_SECS] --> response sent
  |                                                                |
  |                                                                +-- next request or close
  |                                                                     |
  |                                                                     +-- [HEADER_TIMEOUT_SECS] --> ...
```

在 keep-alive 连接上，头部超时和请求超时对每个请求独立应用。

## 推荐值

| 场景 | 头部超时 | 请求超时 |
|----------|--------|---------|
| 通用 Web 服务 | 5s | 120s |
| API 服务器 | 3s | 30s |
| 长时间运行的 PHP 任务 | 5s | 300s |
| 高安全性 / 防 slowloris | 2s | 30s |

请根据应用特性调整这些值。如果 PHP 脚本执行长时间运行的操作（报表生成、数据导入），请相应增加请求超时。

## 另请参阅

- [TLS](tls.md) -- 头部读取超时在 TLS 握手完成后开始计时
- [速率限制](rate-limiting.md) -- 被限速的请求绕过请求超时（作为提前响应返回）
