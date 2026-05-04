---
title: 压缩
description: OxPHP 默认使用 Brotli 压缩响应，减少文本、JSON、SVG 及其他可压缩内容类型的传输大小。
---

# 压缩

OxPHP 默认使用 Brotli 编码压缩 HTTP 响应。对于文本类内容类型，当客户端支持时，压缩会自动应用，无需修改任何应用代码即可减少传输大小。

## 工作原理

1. **Accept-Encoding 检查** — 解析客户端的 `Accept-Encoding` 头，检查是否支持 `br`（Brotli）。不含 `br` 的请求永远不会被压缩。
2. **内容类型检查** — 验证响应 MIME 类型是否在可压缩类型列表中。
3. **已编码检查** — 跳过已有 `Content-Encoding` 头的响应，以避免重复压缩。
4. **大小范围检查** — 只压缩 256 字节到 3 MB 之间的响应。过小的响应收益甚微；过大的响应则不经缓冲直接流式传输。
5. **压缩** — 应用 Brotli 编码。如果压缩后的输出不小于原始大小，则发送未压缩的响应。

压缩发生在 PHP 执行之后和静态文件服务之后。整个压缩后的响应体会短暂保存在内存中，这也是排除 3 MB 以上响应的原因。

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `COMPRESSION_LEVEL` | `4` | Brotli 质量级别（0–11）。值越高输出越小，但 CPU 消耗越多。设为 `0` 可完全禁用压缩 |

默认级别 `4` 在压缩率和 CPU 使用量之间为 Web 服务提供了良好的平衡。9–11 级更适合离线或构建时压缩。

## 可压缩的内容类型

压缩适用于以下 MIME 类型：

**文本类型：**
- `text/html`
- `text/css`
- `text/plain`
- `text/xml`
- `text/javascript`

**应用程序类型：**
- `application/javascript`
- `application/json`
- `application/xml`
- `application/xhtml+xml`
- `application/rss+xml`
- `application/atom+xml`
- `application/manifest+json`
- `application/ld+json`
- `application/wasm`

**其他类型：**
- `image/svg+xml`
- `font/ttf`
- `font/otf`
- `application/x-font-ttf`
- `application/x-font-opentype`
- `application/vnd.ms-fontobject`

## 不压缩的情况

当满足以下任一条件时，响应将不经压缩直接发送：

- 客户端的 `Accept-Encoding` 头中未声明 `br`
- 响应已有 `Content-Encoding` 头（如预压缩内容）
- 响应体小于 256 字节或大于 3 MB
- 内容类型不在可压缩列表中（如 `image/png`、`image/jpeg`、`font/woff2`、`application/zip`——这些格式内部已使用压缩）

## 响应头

应用压缩时，OxPHP 会设置以下响应头：

| 响应头 | 值 |
|--------|-----|
| `Content-Encoding` | `br` |
| `Content-Length` | 更新为压缩后的响应体大小 |
| `Vary` | 追加 `Accept-Encoding`，确保 HTTP 缓存为支持和不支持 Brotli 的客户端分别存储不同版本 |

## 故障排除

### 响应未被压缩

请验证客户端是否发送了 `Accept-Encoding: br`。大多数现代浏览器会发送，但部分 HTTP 测试工具默认不包含此头。

**使用 curl 检查：**

```bash
curl -H "Accept-Encoding: br" -I http://localhost/
```

在响应头中查找 `Content-Encoding: br`。如果不存在，请检查：

1. `COMPRESSION_LEVEL` 未设置为 `0`
2. 响应体至少有 256 字节
3. 响应的 `Content-Type` 在上述可压缩列表中

### 压缩后响应反而更大

对于非常小的响应（几百字节以下），Brotli 的开销偶尔会产生比原始内容更大的输出。OxPHP 会自动检测这种情况并发送未压缩的响应——无需任何配置更改。

### 压缩导致 CPU 使用率高

较高的质量级别（8–11）压缩效果明显更好，但 CPU 消耗也大得多。如果观察到来自压缩的高 CPU 消耗：

**修复：** 将 `COMPRESSION_LEVEL` 降低至 `4` 或 `5`。这些级别以极小的 CPU 代价提供了最高质量 80–90% 的大小缩减效果。

### 预压缩资源被再次压缩

如果您的构建流水线生成了 `.br` 文件，并在这些文件上设置了 `Content-Encoding: br` 响应头，OxPHP 会自动跳过重新压缩。如果您的预压缩内容仍被再次压缩，请验证在压缩运行之前原始响应中是否已存在 `Content-Encoding` 响应头。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - ENTRY_FILE=index.php
      - COMPRESSION_LEVEL=6
```

## 参见

- [静态文件](static-files.md) — 文件服务、MIME 检测和 HTTP 缓存
- [配置参考](../operations/configuration.md) — 完整的环境变量列表
