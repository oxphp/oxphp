---
title: 路由
description: 支持传统 PHP、框架和单页应用的三种路由模式
---

OxPHP 支持三种路由模式，通过单个环境变量进行控制。每种模式决定了传入的 URL 路径如何映射到磁盘上的文件。

## 路由模式

`INDEX_FILE` 环境变量用于选择路由模式：

| 模式 | `INDEX_FILE` 值 | 适用场景 |
|------|-------------------|----------|
| 传统模式 | *（未设置）* | 经典 PHP 托管，WordPress 逐文件路由 |
| 框架模式 | `index.php` | Laravel、Symfony 或任何前端控制器框架 |
| SPA 模式 | `index.html` | React、Vue、Angular 等客户端路由应用 |

### 传统模式

当 `INDEX_FILE` 未设置时，OxPHP 将 URL 直接映射到磁盘文件。

- `/style.css` 提供 `DOCUMENT_ROOT/style.css`
- `/about.php` 执行 `DOCUMENT_ROOT/about.php`
- `/` 解析为 `index.php`（如果存在），否则为 `index.html`
- `/subdir/` 依次尝试 `subdir/index.php`、`subdir/index.html`
- 找不到的文件返回 404

```bash
# 不设置 INDEX_FILE —— 默认为传统模式
DOCUMENT_ROOT=/var/www/html/public
```

### 框架模式

当 `INDEX_FILE=index.php` 时，所有不匹配现有静态文件的请求都会路由到前端控制器。

- `/style.css` 直接提供静态文件
- `/api/users` 执行 `index.php`（文件在磁盘上不存在）
- `/about.php` 返回 404（禁止直接访问 `.php` 文件）
- `/index.php` 返回 404（禁止直接访问索引文件）

```bash
INDEX_FILE=index.php
DOCUMENT_ROOT=/var/www/html/public
```

禁止直接访问 `.php` 文件可以防止 URL 泄露，并确保所有 PHP 请求都通过框架的路由器处理。

### SPA 模式

当 `INDEX_FILE=index.html` 时，找不到的路径会回退到 HTML 入口文件。PHP 文件仍然正常执行。

- `/style.css` 提供静态文件
- `/app/dashboard` 提供 `index.html`（由客户端路由器处理）
- `/api.php` 执行 PHP 脚本
- `/index.html` 返回 404（禁止直接访问索引文件）

```bash
INDEX_FILE=index.html
DOCUMENT_ROOT=/var/www/html/public
```

## 根路径解析

对 `/` 的请求使用预计算路径，以避免每次请求都进行内存分配。服务器先检查 `index.php`，然后检查 `index.html`。如果两者都不存在，返回 404。

带有尾部斜杠的子目录路径（如 `/blog/`）遵循相同的索引解析逻辑：先 `index.php`，后 `index.html`。

## 路径净化

每个传入的 URI 路径在到达文件系统之前都会经过净化处理流程：

1. **百分号解码** -- `%2e%2e` 在净化捕获之前被解码为 `..`
2. **路径段过滤** -- `..`、`.` 和空段被移除
3. **符号链接验证** -- 解析后的路径会与规范化的文档根目录进行比对检查

类似 `/%2e%2e/etc/passwd` 的请求会被解码为 `/../etc/passwd`，净化为 `etc/passwd`，然后验证是否在文档根目录范围内。

## 符号链接逃逸防护

启动时，OxPHP 会规范化文档根目录路径。每个解析后的文件路径都会被规范化，并检查是否仍在文档根目录内。这可以阻止指向服务目录外部的符号链接。

规范路径结果会被缓存，以避免重复的 `realpath(3)` 系统调用。规范路径缓存与元数据缓存共享 200 条目的容量限制，但存储在独立的 HashMap 中，独立进行淘汰。

如果启动时无法规范化文档根目录（例如目录尚不存在），符号链接防护将被禁用，并记录一条警告日志。

### TOCTOU 缓解

路由缓存缓存已验证的 `RouteResult` 条目。TOCTOU 重新规范化在每次请求时执行，发生在 `static_file::serve()` 中、读取磁盘之前，而不是在路由层。这可以缓解在路由解析和文件读取之间替换符号链接的检查时间与使用时间攻击。

## 配置

| 变量 | 描述 | 默认值 |
|----------|-------------|---------|
| `DOCUMENT_ROOT` | 提供文件的文件系统路径 | `/var/www/html/public` |
| `INDEX_FILE` | 索引文件名，控制路由模式 | *（未设置）* |

## 另请参阅

- [静态文件](static-files.md) -- 文件缓存、MIME 检测和流式传输
- [压缩](compression.md) -- 应用于静态文件响应的 Brotli 压缩
- [自定义错误页面](error-pages.md) -- 404 等错误响应的自定义 HTML 页面
