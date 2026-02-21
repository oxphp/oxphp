---
title: 压缩
description: 针对可压缩响应类型的 Brotli 压缩
---

当客户端支持且响应类型可压缩时，OxPHP 使用 Brotli 压缩 HTTP 响应。压缩默认启用，可通过环境变量开关控制。

## 配置

| 变量 | 描述 | 默认值 |
|----------|-------------|---------|
| `COMPRESSION` | 启用 Brotli 压缩 | `true` |

禁用压缩：

```bash
COMPRESSION=false
```

`false`、`0` 和 `off` 均可禁用压缩。其他任何值（或不设置该变量）均表示启用。

## 工作原理

压缩流程在响应构建完成后、发送给客户端之前执行。仅当请求包含 `Accept-Encoding` 头时才会触发 -- 没有该头的请求会完全跳过压缩函数，避免异步开销。

### 决策流程

1. **Accept-Encoding 检查** -- 解析头部，按 `,` 分割，提取 `;` 质量参数之前的编码名称，查找 `br`
2. **Content-Type 检查** -- 验证响应 MIME 类型是否在可压缩列表中
3. **已编码检查** -- 如果响应已有 `Content-Encoding` 头则跳过
4. **Content-Length 守卫** -- 如果 `Content-Length` 头存在且不在 256 B 到 3 MB 范围内则跳过
5. **Body 大小提示守卫** -- 如果没有 `Content-Length` 时 body 大小提示不在 256 B 到 3 MB 范围内则跳过
6. **收集 body** -- 将响应体物化到内存中
7. **运行时大小检查** -- 验证收集的 body 是否在范围内（用于没有预先大小提示的响应）
8. **压缩** -- 应用 Brotli，如果压缩后的输出不比原始数据小则丢弃结果

### Brotli 参数

| 参数 | 值 | 说明 |
|-----------|-------|-------|
| 质量 | 4 | 在 Web 服务场景下平衡速度和压缩比 |
| 窗口大小 | 20 | 1 MB 窗口，适合典型的 Web 响应 |

选择质量级别 4 作为折中方案：它对文本类 Web 内容的压缩效果足够好，同时避免了更高质量级别（9-11）的 CPU 开销 -- 那些更适合离线压缩。

## 可压缩类型

压缩应用于以下 19 种 MIME 类型（精确匹配，非前缀匹配）：

**文本类型：**
- `text/html`
- `text/css`
- `text/plain`
- `text/xml`
- `text/javascript`

**应用类型：**
- `application/javascript`
- `application/json`
- `application/xml`
- `application/xhtml+xml`
- `application/rss+xml`
- `application/atom+xml`
- `application/manifest+json`
- `application/ld+json`
- `application/wasm`

**其他：**
- `image/svg+xml`
- `font/ttf`
- `font/otf`
- `application/x-font-ttf`
- `application/x-font-opentype`
- `application/vnd.ms-fontobject`

`image/png`、`image/jpeg`、`font/woff2` 和 `application/zip` 等类型不会被压缩，因为它们已使用内部压缩。

## 大小限制

| 限制 | 值 | 原因 |
|-------|-------|--------|
| 最小值 | 256 字节 | 小响应不太可能从压缩中获益 |
| 最大值 | 3 MB | 更大的响应应从磁盘流式传输，而不是收集到内存中 |

超出此范围的响应将以未压缩形式发送。

## 响应头

应用压缩时，设置以下头部：

| 头部 | 值 |
|--------|-------|
| `Content-Encoding` | `br` |
| `Content-Length` | 更新为压缩后的大小 |
| `Vary` | `Accept-Encoding`（追加） |

`Vary` 头确保 HTTP 缓存为支持 Brotli 和不支持的客户端分别存储不同版本。

## 另请参阅

- [静态文件](static-files.md) -- 文件服务和内容缓存
- [超时](timeouts.md) -- 请求超时适用于包括压缩在内的完整处理流程
