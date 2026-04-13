---
title: 路由
description: 通过三种模式配置 OxPHP 路由——传统文件映射、框架前端控制器和 SPA 回退。每种模式都对应一个熟悉的 nginx try_files 配置。
---

# 路由

OxPHP 使用三种模式之一处理传入的 HTTP 请求，通过单个环境变量进行控制。每种模式都对应一个熟悉的 nginx `try_files` 配置，因此您可以准确预测任何 URL 的处理结果。

## 工作原理

每个请求在进入特定模式逻辑之前都会通过共享管道：

1. **点路径过滤** — 包含隐藏段（`.git`、`.env`）的路径会被阻止，`/.well-known/*` 除外（[RFC 8615](https://www.rfc-editor.org/rfc/rfc8615)）
2. **路由缓存查找** — 最近解析过的 URI 会从 LRU 缓存中返回（10 000 条）
3. **百分号解码 + 清理** — 将 `%2e%2e` 等编码序列解码，并剥离遍历段（`..`、`.`、空段）
4. **well-known PHP 阻止** — 纵深防御：`/.well-known/` 内的 `.php` 脚本永不执行
5. **URI 分类** — 经过清理的路径被一次性分类为 `NoExtension`、`Php` 或 `OtherExtension`
6. **模式分发** — 每种模式按各自规则处理三种 URI 类型
7. **符号链接验证** — 每个解析后的文件系统路径必须规范化到文档根目录内

分类步骤是关键的效率优化：静态资源（`/style.css`、`/logo.png`）的磁盘检查在共享层中对 `OtherExtension` URI **只执行一次**，因此三种模式的代价相同。

## 配置

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `DOCUMENT_ROOT` | `/var/www/html/public` | 用于提供文件和 PHP 脚本的根目录 |
| `INDEX_FILE` | *(未设置)* | 路由模式：未设置 = Traditional，`*.php` = Framework，其他任何值 = SPA |

## 传统模式（Traditional）

当 `INDEX_FILE` **未设置**（或为空）时生效。等效的 nginx 配置：

```nginx
location / {
    try_files $uri $uri/ /index.php /index.html =404;
}
location ~ \.php$ {
    try_files $uri =404;          # PATH_INFO 拆分已启用
}
```

**解析顺序：**

1. **`$uri`** — 磁盘上的精确文件 → 提供文件（如果是 `.php` 则执行）
2. **`$uri/`** — 目录 → 在其中查找 `index.php`，然后是 `index.html`
3. **PATH_INFO 拆分** — 当 URI 包含 `.php/` 时，匹配磁盘上的脚本前缀，剩余部分成为 `PATH_INFO`（例如 `/api.php/users/42` → 脚本 `api.php`，`PATH_INFO=/users/42`）
4. **`/index.php`** — 根前端控制器回退
5. **`/index.html`** — 根静态索引回退
6. **`WORKER_FILE`** — 如果配置了 Worker 模式
7. **404**

**示例：**

| 请求 | 结果 |
|---|---|
| `/about.php` | 执行 `about.php` |
| `/style.css` | 提供 `style.css` |
| `/blog/`（存在 `blog/index.php`） | 执行 `blog/index.php` |
| `/api.php/users/42` | 执行 `api.php`，`PATH_INFO=/users/42` |
| `/missing.txt` | 回退到 `/index.php` |
| `/some/route` | 回退到 `/index.php` |

PATH_INFO 拆分在 Traditional 模式下**始终启用**。没有环境变量开关——之前的 `SPLIT_PATH_INFO_ENABLED` 标志已被移除。

## 框架模式（Framework）

当 `INDEX_FILE=index.php`（或任何以 `.php` 结尾的值）时生效。等效的 nginx 配置：

```nginx
location ~ \.(?!php$)[a-zA-Z0-9]+$ {
    try_files $uri =404;          # 静态资源：不存在即硬 404
}
location / {
    rewrite ^ /index.php last;    # 其他一切 → 前端控制器
}
location = /index.php {
    fastcgi_param PATH_INFO $request_uri;
    fastcgi_pass ...;
}
```

**解析规则：**

| URI 类型 | 行为 |
|---|---|
| `.css`、`.png`、`.js`……（任何非 php 扩展） | 文件存在则提供，否则**硬 404** |
| `.php`（任何路径） | 重写到 `/index.php`，`PATH_INFO` 设置为原始 URI |
| 无扩展名（`/api/users`、`/`） | 重写到 `/index.php`，`PATH_INFO` 设置为原始 URI |

**示例：**

| 请求 | 结果 | `$_SERVER['PATH_INFO']` |
|---|---|---|
| `/style.css`（存在） | 提供 `style.css` | — |
| `/style.css`（不存在） | **404**（无回退） | — |
| `/api/users` | 执行 `index.php` | `/api/users` |
| `/about.php` | 执行 `index.php` | `/about.php` |
| `/api.php/v1/users` | 执行 `index.php` | `/api.php/v1/users` |
| `/index.php`（直接） | 执行 `index.php` | `/index.php` |
| `/` | 执行 `index.php` | `/` |

前端控制器始终在 `PATH_INFO` 中收到**原始 URI**，您的路由器无需单独检查 `REQUEST_URI` 即可决定如何处理。直接访问 `/index.php` 不再被阻止——重写到 `/index.php` 是幂等的，因此直接访问与访问 `/` 的结果相同。

## SPA 模式

当 `INDEX_FILE=index.html`（或任何不以 `.php` 结尾的值）时生效。等效的 nginx 配置：

```nginx
location ~ \.php$ {
    try_files $uri =404;          # PHP：文件必须存在，无回退
}
location ~ \. {
    try_files $uri =404;          # 其他扩展名：不存在即硬 404
}
location / {
    try_files /index.html =404;   # 无扩展名路径：直接到 index.html
}
```

**解析规则：**

| URI 类型 | 行为 |
|---|---|
| `.php` | 文件存在则执行，否则**硬 404** |
| `.css`、`.png`……（任何其他扩展名） | 文件存在则提供，否则**硬 404** |
| 无扩展名（`/dashboard`、`/api/users`、`/`） | 直接提供 `/index.html`——**不对 `$uri` 进行磁盘探测** |

**示例：**

| 请求 | 结果 |
|---|---|
| `/style.css`（存在） | 提供 `style.css` |
| `/style.css`（不存在） | **404** |
| `/dashboard` | 提供 `/index.html` |
| `/users/42/edit` | 提供 `/index.html` |
| `/api.php`（存在） | 执行 `api.php` |
| `/api.php`（不存在） | **404** |
| `/index.html`（直接） | 提供 `index.html` |

两个值得强调的语义：

- **无扩展名路径不访问磁盘** — SPA 模式从不询问"`/dashboard` 在磁盘上是否存在？"它始终返回索引。这对于客户端路由器是正确的，并避免了不必要的 `stat()` 调用。
- **缺失的静态文件是硬 404，而非回退** — 缺失的 `/style.css` 不会静默地提供 `index.html`。这能在早期捕获损坏的资源引用，而不是在 JS 期望 CSS 的地方返回 HTML。

## Worker 模式

当设置了 `WORKER_FILE` 时，Worker 模式会自动激活。它作为最终 404 **之前**的回退步骤插入到所有三种路由模式中：

| 模式 | Worker 的位置 |
|---|---|
| Traditional | 在 `/index.html` 回退之后，404 之前 |
| Framework | 当 `index.php` 自身缺失时 |
| SPA | 当 `/index.html` 自身缺失时 |

也就是说，Worker 模式与路由正交：如果您的前端控制器丢失，Worker 会接管。同时设置 `WORKER_FILE` 和 `INDEX_FILE=index.php` 完全受支持。

详细配置请参见 [Worker 模式](worker-mode.md)。

## PATH_INFO 行为

`$_SERVER['PATH_INFO']` 根据所用模式以不同方式填充：

| 模式 | 何时设置 | 值 |
|---|---|---|
| Traditional | 仅当 URI 包含 `.php/`（PATH_INFO 拆分）时 | 脚本段之后的尾部，例如 `/users/42` |
| Framework | **始终** | 完整的原始 URI，例如 `/api/users` |
| SPA | 永不 | （PHP 仅对精确的 `.php` 文件调用；无 PATH_INFO） |

在 Traditional 模式下，拆分始终启用——之前的 `SPLIT_PATH_INFO_ENABLED` 环境变量已被移除。如果您需要基于 PATH_INFO 的路由，请使用 Traditional 模式并将 `.php` 脚本作为前缀。

## 路径安全

OxPHP 应用多层防护以阻止目录遍历、隐藏文件泄露和符号链接逃逸攻击：

- **百分号解码**在清理之前运行，因此像 `/%2e%2e/etc/passwd` 这样的编码遍历尝试会被捕获
- **路径段过滤**从解析后的路径中移除 `..`、`.` 和空段
- **符号链接验证**将每个解析后的路径规范化，并验证其仍在文档根目录内。指向被服务目录之外的符号链接会被阻止
- **点路径拦截**拦截任何以 `.` 开头的路径段（例如 `/.git/config`、`/.env`），`/.well-known/*` 按 RFC 8615 除外
- **well-known PHP 阻止** — 即使有点路径例外，`/.well-known/` 内的 `.php` 脚本也永不执行（纵深防御）

> **注意：** 如果文档根目录在启动时不存在，服务器将以致命错误退出。符号链接逃逸保护需要一个有效的、可解析的文档根目录路径。

## 故障排除

### Traditional 模式下所有请求都返回 404

检查文档根目录中是否存在 `index.php` 或 `index.html`。Traditional 模式的 `try_files` 链只会回退到这两个——如果两者都缺失且 URL 不匹配任何文件，您会得到 404。

```bash
docker exec <container> ls /var/www/html/public
```

### 缺失的静态资源返回 404 而非 SPA shell

这在 Framework 和 SPA 模式下是有意为之。缺失的 `/style.css` 是硬 404，而非静默回退到 `index.html`。这能在早期捕获损坏的资源引用。如果您需要回退，请使用 Traditional 模式。

### 直接访问 `/index.php` 不再返回 404

在 Framework 模式下，现在允许直接访问前端控制器（重写到 `/index.php` 是幂等的）。如果您之前依赖 404 来检测直接命中，请改为在控制器内部检查 `PATH_INFO`。

### Framework 模式下 `PATH_INFO` 为空

确保 `INDEX_FILE` 以 `.php` 结尾。否则 OxPHP 会选择 SPA 模式，而 SPA 模式不填充 `PATH_INFO`。在 Framework 模式下，该变量现在无条件设置——不再需要任何功能开关。

### 文档根目录内的符号链接返回 404

指向文档根目录之外的符号链接被设计阻止。请将目标内容移入文档根目录，或将其作为目录挂载到正确路径。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.2.0
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
