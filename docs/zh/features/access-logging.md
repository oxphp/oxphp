---
title: 访问日志
description: 通过 tracing 框架实现的结构化 JSON 访问日志
---

OxPHP 为每个完成的 HTTP 请求生成结构化的 JSON 日志条目。日志通过非阻塞后台写入器写入 stdout，因此日志 I/O 永远不会阻塞请求处理流程。

## 配置

| 变量 | 描述 | 默认值 |
|----------|-------------|---------|
| `ACCESS_LOG` | 访问日志级别：`all`、`error` 或 空/未设置（关闭） | *(关闭)* |
| `LOG_LEVEL` | 最低日志级别（trace、debug、info、warn、error） | `info` |

访问日志默认关闭。设置 `ACCESS_LOG` 控制日志详细程度：

- **`all`** — 记录每个完成的请求（方法、路径、状态码、耗时）
- **`error`** — 仅记录错误响应（HTTP 状态码 >= 400：404、403、500 等）
- **空/未设置** — 不记录访问日志

```bash
# 记录所有请求
ACCESS_LOG=all

# 仅记录错误（4xx/5xx）
ACCESS_LOG=error

# 关闭访问日志（默认）
# ACCESS_LOG=
```

`RUST_LOG` 环境变量也受支持，设置后优先于 `LOG_LEVEL`。这遵循标准的 `tracing`/`env_filter` 约定。

## 日志格式

每条访问日志条目是写入 stdout 的单行 JSON：

```json
{
  "timestamp": "2026-02-11T12:34:56.789Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc00000042",
    "trace_id": "4bf92f3577b16e8264cabd64a999f321",
    "span_id": "a1b2c3d4e5f6a7b8",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1234,
    "remote_addr": "10.0.0.1:54321",
    "message": "request completed"
  }
}
```

当 `TRACE_CONTEXT` 禁用时，`trace_id` 和 `span_id` 字段将从日志条目中省略。

### 字段

| 字段 | 类型 | 描述 |
|-------|------|-------------|
| `request_id` | string | 唯一请求标识符（参见[请求 ID](/features/request-ids/)） |
| `method` | string | HTTP 方法（`GET`、`POST` 等） |
| `path` | string | 请求 URI 路径 |
| `status` | number | HTTP 响应状态码 |
| `duration_us` | number | 请求处理总时间（微秒） |
| `remote_addr` | string | 客户端 IP 地址和端口 |
| `trace_id` | string | W3C trace ID（32 个十六进制字符）。仅当 `TRACE_CONTEXT=true` 时存在 |
| `span_id` | string | Span ID（16 个十六进制字符）。仅当 `TRACE_CONTEXT=true` 时存在 |

## 工作原理

访问日志作为事件处理器实现，监听优先级 **100** 的 `RequestComplete` 事件（在同优先级的处理器中最后运行）。处理器发出带有 `access_log` 目标的 `tracing::info!` 调用。

日志基础设施使用：

- **tracing** 用于结构化事件输出
- **tracing-subscriber** 搭配 JSON 格式化器用于输出
- **tracing-appender** 搭配非阻塞写入器用于异步 I/O

非阻塞写入器启动一个专用后台线程来缓冲日志写入并刷新到 stdout。初始化返回的 `WorkerGuard` 必须持有到关机时，以确保所有缓冲条目被刷新。

## 日志目标

OxPHP 对不同的日志类型使用不同的 tracing 目标：

- `access_log` -- 每请求访问日志条目
- 默认目标 -- 服务器生命周期事件、错误、警告

您可以使用 `RUST_LOG` 独立控制它们：

```bash
# 显示 info 级别的访问日志，抑制其他 info 级别的消息
RUST_LOG=warn,access_log=info
```

## 与日志聚合工具集成

由于日志是 stdout 上的 JSON 行，它们可以直接与容器日志驱动和聚合工具集成：

- **Docker**：通过容器的日志驱动自动收集
- **Kubernetes**：由节点的日志代理（Fluentd、Fluent Bit 等）采集
- **journald**：以 systemd 服务运行时，通过 stdout 日志捕获

无需 sidecar 或基于文件的日志传输。

## 另请参阅

- [请求 ID](request-ids.md) -- 请求 ID 如何生成和透传
- [分布式追踪](distributed-tracing.md) -- 日志条目中的 `trace_id` 和 `span_id` 字段
- [速率限制](rate-limiting.md) -- 被限速的请求仍然会出现在访问日志中
