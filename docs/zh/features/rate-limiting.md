---
title: 限速
description: OxPHP 内置的基于 IP 的限速，支持可配置阈值、时间窗口和标准 HTTP 限速响应头。
---

# 限速

OxPHP 内置了基于 IP 的限速器，无需外部依赖或基础设施。启用后，它会跟踪每个客户端 IP 的请求计数，当客户端超过配置的阈值时返回 `429 Too Many Requests` 响应。

## 工作原理

限速器使用以客户端 IP 地址为键的固定窗口计数器。每个 IP 有其独立的计数器和时间窗口。

1. 当请求到达时，OxPHP 在内部跟踪器中查找客户端 IP。
2. 如果没有对应条目，或当前窗口已过期，则以计数器为零启动新窗口。
3. 每次请求递增计数器。
4. 如果计数器超过 `RATE_LIMIT`，服务器立即返回带有限速头的 `429` 响应。请求在路由或 PHP 执行之前被拒绝。

被限速的请求仍会出现在访问日志和指标中。

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `RATE_LIMIT` | `0` | 每个 IP 在窗口内的最大请求数。`0` 以零开销完全禁用限速 |
| `RATE_WINDOW_SECONDS` | `60` | 限速窗口的持续时间（秒） |

```bash
# 允许每个 IP 在 60 秒窗口内发送 100 个请求
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
```

## 响应头

被拒绝的请求返回 `429 Too Many Requests` 响应，包含以下响应头：

| 响应头 | 说明 |
|--------|------|
| `Retry-After` | 当前窗口重置前的秒数 |
| `x-ratelimit-limit` | 每个窗口允许的最大请求数 |
| `x-ratelimit-remaining` | 当前窗口剩余的请求数（被限速时为 `0`） |
| `x-ratelimit-reset` | 当前窗口重置前的秒数 |
| `x-request-id` | 用于将此响应与访问日志关联的请求 ID |

`429` 响应示例：

```http
HTTP/1.1 429 Too Many Requests
Retry-After: 45
x-ratelimit-limit: 100
x-ratelimit-remaining: 0
x-ratelimit-reset: 45
x-request-id: 67e2a1f400000042

429 Too Many Requests
```

## 故障排除

### 合法用户被限速

您的阈值可能对实际流量模式来说太低了。检查指标中 429 响应的频率，并相应地调整 `RATE_LIMIT` 或 `RATE_WINDOW_SECONDS`。

**检查**被限速的请求数：

```bash
curl http://localhost:9090/metrics | grep rate_limited
```

**修复：** 增大 `RATE_LIMIT` 或延长 `RATE_WINDOW_SECONDS` 以给客户端更多余量。

### 企业 NAT 后面的用户共享同一个 IP 计数器

OxPHP 按源 IP 进行限速。共享 NAT 或代理后面的所有用户共享同一个计数器。如果这造成了问题，考虑禁用 OxPHP 的内置限速器（`RATE_LIMIT=0`），并在更高层面（如在您能访问用户标识符的负载均衡器或 API 网关）应用限速。

### 多实例部署中限速不生效

OxPHP 的限速器是基于内存的，且是每实例独立的。如果您在负载均衡器后面运行多个 OxPHP 实例，每个实例都跟踪自己独立的计数器。客户端可以向每个实例发送 `RATE_LIMIT` 次请求而不触发 429。对于跨实例的协调限速，请在负载均衡器或 API 网关层使用外部限速器。

### IP 轮换攻击导致内存增长

OxPHP 最多跟踪 100,000 个唯一 IP 地址。达到此限制时，在添加新条目之前会清除过期条目。如果您观察到攻击者快速轮换 IP 导致的内存增长，自动清理会将影响限制在有界的内存用量内。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "8080:80"
    environment:
      RATE_LIMIT: "100"
      RATE_WINDOW_SECONDS: "60"
    volumes:
      - ./app:/var/www/html:ro
```

## 最佳实践

- **从保守值开始。** 从较低限制开始（如每分钟 60 次请求），根据观察到的流量模式逐步增加。放宽限制比从过载服务器中恢复更容易。
- **多实例部署使用共享限速器。** OxPHP 的限速器是每实例独立的。对于跨实例的协调限速，请在负载均衡器或 API 网关层应用限速。
- **监控 429 响应率。** 在指标中跟踪被限速请求的比例，以检测阈值配置不当或意外的流量峰值。

## 注意事项

- **固定窗口算法。** 限速器使用固定窗口计数器，而非滑动窗口。客户端可以在两个窗口边界处突发发送最多 `2x` 配置限制的请求。
- **仅限基于 IP。** 限速以源 IP 地址为键。不支持 API 密钥或用户 ID 等自定义键。
- **内存状态。** 限速计数器不在多个 OxPHP 实例之间共享。
- **自动清理。** 当跟踪器超过 100,000 个 IP 时，会清理过期条目，移除所有窗口已过期的条目。

## 参见

- [指标](../operations/metrics.md) — 通过 Prometheus 监控被限速的请求计数
- [请求 ID](request-ids.md) — 在日志中关联被限速的请求
- [访问日志](access-logging.md) — 429 响应会出现在访问日志中
- [配置参考](../operations/configuration.md) — 完整的环境变量列表
