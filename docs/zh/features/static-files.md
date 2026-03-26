---
title: 静态文件
description: OxPHP 提供自动 MIME 检测、内存缓存、HTTP 缓存头和条件 304 响应的静态文件服务。
---

# 静态文件

OxPHP 直接从文档根目录提供静态文件，无需调用 PHP。文件服务具备自动 MIME 类型检测、用于快速重复访问的内存缓存，以及完整的 HTTP 缓存支持（包括 ETag 和条件请求）。

## 工作原理

当请求匹配到静态文件时：

1. **文件匹配** — 路由层将 URL 路径解析到磁盘上的文件
2. **MIME 检测** — 根据文件扩展名确定内容类型
3. **缓存检查** — 在访问文件系统之前检查文件缓存
4. **条件检查** — 如果请求携带 `If-None-Match` 或 `If-Modified-Since`，OxPHP 会评估条件，并可能在不发送响应体的情况下返回 `304 Not Modified`
5. **响应** — 1 MiB 以内的文件从内存缓存提供；更大的文件直接从磁盘流式传输

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `STATIC_CACHE_TTL` | `30d` | 静态文件的 Cache-Control max-age。接受 `30s`、`5m`、`2h`、`30d`、`1w`、`1y`，纯秒数（如 `3600`），或 `off`（完全禁用缓存头） |
| `STATIC_CACHE` | *(开启)* | 设为 `off` 启用内存内容缓存的 mtime 重新验证 |

## MIME 检测

MIME 类型根据文件扩展名自动确定。如果无法确定类型，服务器回退到 `application/octet-stream`。常见映射包括：

| 扩展名 | Content-Type |
|--------|-------------|
| `.html` | `text/html` |
| `.css` | `text/css` |
| `.js` | `application/javascript` |
| `.json` | `application/json` |
| `.png` | `image/png` |
| `.svg` | `image/svg+xml` |
| `.woff2` | `font/woff2` |

## 文件缓存

OxPHP 使用内存缓存来减少频繁请求文件的磁盘 I/O：

- **1 MiB 以内**（1,048,576 字节）的文件会被读入内存并缓存。总缓存容量为 64 MiB（67,108,864 字节）。当超出容量时，最近最少使用的条目会被驱逐以腾出空间。
- **大于 1 MiB** 的文件始终直接从磁盘流式传输。`Content-Length` 头从文件元数据中设置，以便客户端提前知道总大小。

文件缓存在首次请求时填充，并在后续请求中保留。默认情况下，缓存条目会持续存在，直到被 LRU 策略驱逐。

### 内容重新验证

设置 `STATIC_CACHE=off` 可启用基于 mtime 的重新验证。在此模式下，每次缓存命中都会执行一次 `stat()` 系统调用（约 1–2 μs）来检查文件的修改时间。如果磁盘上的文件已更改，过期条目将被自动清除并重新读取文件。这非常适合开发环境——无需重启服务器即可立即看到文件更改。

在生产环境中，保持 `STATIC_CACHE` 为默认值（不设置），以获得零额外系统调用开销的最大吞吐量。

## HTTP 缓存

### Cache-Control

当设置了 `STATIC_CACHE_TTL` 时（默认为 `30d`），每个静态文件响应都包含 `Cache-Control` 头：

```http
Cache-Control: public, max-age=2592000
```

`max-age` 值是 TTL 转换为秒数后的结果。设置 `STATIC_CACHE_TTL=off` 可完全省略此头。

### ETag 和 Last-Modified

每个静态文件响应都包含：

- **ETag** — 格式为 `W/"<size>-<mtime_hex>"` 的弱 ETag，由文件大小和最后修改时间派生
- **Last-Modified** — 基于文件修改时间的 RFC 7231 HTTP 日期

这些头允许浏览器和 CDN 在不重新下载文件的情况下验证缓存副本。

### 条件请求（304）

OxPHP 评估条件请求头，以避免发送未更改的文件内容：

- **If-None-Match** — 客户端发送其已缓存的 ETag。如果匹配当前文件，OxPHP 返回无响应体的 `304 Not Modified`。
- **If-Modified-Since** — 客户端发送一个时间戳。如果文件在该时间之后未被修改，OxPHP 返回 304。

按照 RFC 7232，`If-None-Match` 优先于 `If-Modified-Since`。对于已在内存缓存中的文件，条件检查无需任何磁盘 I/O 即可完成。

### 禁用缓存

有两个独立的缓存层，各有对应的变量：

| 变量 | 控制对象 | `off` 的效果 |
|------|---------|-------------|
| `STATIC_CACHE_TTL=off` | **浏览器缓存**（HTTP 头） | 不发送 `Cache-Control`、`ETag`、`Last-Modified` 头 |
| `STATIC_CACHE=off` | **服务器内存缓存** | 每次命中时验证文件 mtime；自动清除过期条目 |

在开发环境中，设置 `STATIC_CACHE=off` 让服务器始终提供最新内容。可选地同时设置 `STATIC_CACHE_TTL=off` 以完全阻止浏览器缓存。

## 故障排除

### 服务器提供过期文件

默认情况下，内存内容缓存不会检查磁盘上的文件是否已更改。设置 `STATIC_CACHE=off` 启用 mtime 重新验证——服务器将以极低开销（每请求约 1–2 μs）自动检测文件更改。

### 浏览器一直提供过期文件

如果服务器返回了最新内容但浏览器仍显示旧版本，问题在于浏览器自身的缓存。设置 `STATIC_CACHE_TTL=off` 停止发送缓存头，或使用浏览器的强制刷新（Shift+F5 或 Cmd+Shift+R）。

### 文件以 `application/octet-stream` 提供

OxPHP 使用文件扩展名来确定 MIME 类型。如果扩展名缺失或无法识别，则回退到 `application/octet-stream`。请为文件添加正确的扩展名，或确保您的框架在 PHP 响应中显式设置 `Content-Type` 头。

### 大文件访问速度慢

大于 1 MiB 的文件在每次请求时都从磁盘流式传输，不缓存在内存中。对于非常大的文件，请在 OxPHP 前面放置 CDN 以在边缘节点缓存它们。或者，重新组织您的静态资源，使频繁提供的文件保持在 1 MiB 以下。

### 预期 200 响应却收到 304

304 表示客户端已拥有当前版本，这是正常行为。如果在开发时需要强制获取新响应，请设置 `STATIC_CACHE_TTL=off` 停止发送 `ETag` 和 `Last-Modified` 头。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INDEX_FILE=index.php
      - STATIC_CACHE_TTL=1y
```

## 最佳实践

- **在生产环境中使用带缓存破坏文件名的长 TTL**（如 `app.a1b2c3.js`）。设置 `STATIC_CACHE_TTL=1y` 以最大化浏览器和 CDN 缓存。
- **在开发环境中设置 `STATIC_CACHE=off`**，让服务器自动检测文件更改。可选地同时设置 `STATIC_CACHE_TTL=off` 以禁用浏览器缓存。
- **在 OxPHP 前面放置 CDN** 以应对高流量站点。`ETag`、`Last-Modified` 和 `Cache-Control` 头与所有主流 CDN 提供商兼容。
- **让构建工具处理静态资源哈希。** Vite 和 Laravel Mix 等框架会自动生成带哈希的文件名，使长缓存 TTL 变得安全。

## 参见

- [压缩](compression.md) — 对可压缩静态文件响应进行 Brotli 压缩
- [路由](routing.md) — URL 路径如何解析到磁盘上的文件
- [配置参考](../operations/configuration.md) — 完整的环境变量列表
