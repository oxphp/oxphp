---
title: 路由
description: 通过四种模式配置 OxPHP 路由——传统文件映射、框架前端控制器、SPA 回退和 Worker 模式。
---

# 路由

OxPHP 使用四种模式之一处理传入的 HTTP 请求，通过单个环境变量进行控制。所选模式决定了 URL 路径如何映射到磁盘上的文件。

## 工作原理

当请求到达时，OxPHP 在将其解析为文件之前，会通过安全管道处理 URL 路径：

1. **百分号解码** — 将 `%2e%2e` 等编码字符解码为字面值
2. **路径段过滤** — 去除路径遍历段（`..`）、当前目录段（`.`）和空段
3. **基于模式的路由** — 根据当前激活的路由模式，将经过清理的路径与文件系统进行匹配
4. **符号链接验证** — 检查解析后的文件系统路径是否在文档根目录范围内，以防止符号链接逃逸

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `DOCUMENT_ROOT` | `/var/www/html/public` | 用于提供文件和 PHP 脚本的根目录 |
| `INDEX_FILE` | *(未设置)* | 决定路由模式。未设置 = 传统模式，`index.php` = 框架模式，`index.html` = SPA 模式 |

## 传统模式

当未设置 `INDEX_FILE` 时，传统模式生效。URL 直接映射到磁盘上的文件，类似于使用 Apache 或 nginx 的经典 PHP 托管方式。

- `/about.php` 执行 `DOCUMENT_ROOT/about.php`
- `/style.css` 提供 `DOCUMENT_ROOT/style.css`
- `/` 解析为 `index.php`（若存在），否则为 `index.html`
- `/blog/` 依次尝试 `blog/index.php`，然后是 `blog/index.html`
- 任何不匹配文件的路径返回 404

此模式适用于 WordPress、传统 PHP 应用程序，或任何每个 URL 对应特定文件的项目。

## 框架模式

当 `INDEX_FILE=index.php` 时，框架模式生效。所有不匹配现有静态文件的请求都会路由到前端控制器，这与 Laravel、Symfony 等 PHP 框架的预期行为完全一致。

- `/style.css` 直接提供静态文件（若磁盘上存在）
- `/api/users` 执行 `index.php`（该路径不作为文件存在）
- `/about.php` 返回 404（直接访问 `.php` 文件被阻止）
- `/index.php` 返回 404（直接访问前端控制器被阻止）

阻止直接访问 `.php` 文件可以防止 URL 泄露，并强制所有 PHP 请求通过框架的路由器。

## SPA 模式

当 `INDEX_FILE=index.html` 时，SPA 模式生效。不匹配现有文件的请求会回退到 HTML 入口点，允许客户端路由器（React Router、Vue Router 等）处理该路径。

- `/style.css` 提供静态文件
- `/app/dashboard` 提供 `index.html`（客户端路由器处理该路径）
- `/api.php` 若磁盘上存在该 PHP 脚本，则执行它
- `/index.html` 返回 404（直接访问索引文件被阻止）

## Worker 模式

当设置了 `WORKER_FILE` 时，Worker 模式路由会自动激活。所有不匹配磁盘上静态文件的传入请求都会被分发到持久化 PHP Worker 进程，而不是返回 404。

- `/style.css` 直接提供静态文件
- `/api/users` 分发到 Worker（该路径不存在对应文件）
- `/` 若不存在 `index.php` 或 `index.html`，则分发到 Worker

Worker 模式与 `INDEX_FILE` 兼容。同时设置 `WORKER_FILE` 和 `INDEX_FILE=index.php` 可将 Worker 模式路由与框架模式静态文件处理相结合——静态文件直接提供，其他所有内容都发送到 Worker。

详细配置请参见 [Worker 模式](worker-mode.md)。

## 路径安全

OxPHP 应用多层防护来阻止目录遍历和符号链接逃逸攻击：

- **百分号解码**在清理之前运行，因此像 `/%2e%2e/etc/passwd` 这样的编码遍历尝试会被捕获
- **路径段过滤**从解析后的路径中移除 `..`、`.` 和空段
- **符号链接验证**将每个解析后的路径规范化，并验证其仍在文档根目录内。指向被服务目录之外的符号链接会被阻止

> **注意：** 如果文档根目录在启动时不存在，服务器将以致命错误退出。符号链接逃逸保护需要一个有效的、可解析的文档根目录路径。

## 故障排除

### 所有请求都返回 404

验证 `DOCUMENT_ROOT` 指向正确的目录，且该目录在磁盘上存在。如果文档根目录无法解析，OxPHP 在启动时会退出，所以正在运行的服务器表示该目录在启动时是存在的——但卷挂载错误或路径错误仍会导致每个请求都找不到文件。

**检查：** 在容器内确认文档根路径：

```bash
docker exec <container> ls /var/www/html/public
```

**修复：** 更正 `DOCUMENT_ROOT` 或确保卷挂载了正确的路径。

### 框架模式对 PHP 路由返回 404

在框架模式下，直接访问 `.php` 文件是被有意阻止的。如果您的应用直接链接到 `.php` 文件，请切换到传统模式（取消设置 `INDEX_FILE`），或将链接更新为使用简洁 URL。

### 含特殊字符的 URL 返回 404

OxPHP 在路由前对 URL 进行百分号解码。对 `/café/menu` 等路径的请求可以正常工作。如果路径仍然返回 404，请确认磁盘上存在使用解码名称的文件。

### 文档根目录内的符号链接返回 404

指向文档根目录之外的符号链接被设计阻止。请将目标内容移入文档根目录，或将其作为目录挂载到正确路径。

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
```

## 参见

- [静态文件](static-files.md) — 提供文件的 MIME 检测、缓存和流式传输
- [Worker 模式](worker-mode.md) — 持久化 PHP 进程和 Worker 模式路由
- [配置参考](../operations/configuration.md) — 完整的环境变量列表
