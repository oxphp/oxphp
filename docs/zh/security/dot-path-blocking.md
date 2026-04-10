---
title: 点路径拦截
description: OxPHP 拦截以 "." 开头的隐藏文件和目录的访问，防止敏感配置、版本控制数据和其他点文件的泄露。
---

# 点路径拦截

OxPHP 拦截任何包含以 `.` 开头的路径段的 URL 请求，返回 404 Not Found。这可以防止 `.env`、`.git/`、`.htaccess`、`.svn/` 和 `.DS_Store` 等敏感文件被自动扫描器访问。

此防护始终开启且无法禁用。它适用于所有路由模式（传统、框架、SPA 和 Worker）。

## 被拦截的内容

任何路径段以 `.` 开头的请求都返回 404：

| 请求 | 结果 |
|------|------|
| `/.env` | 404 |
| `/.git/config` | 404 |
| `/.htaccess` | 404 |
| `/.DS_Store` | 404 |
| `/.docker/config.json` | 404 |
| `/path/.hidden/file.txt` | 404 |
| `/path/to/.env` | 404 |

百分号编码的绕过尝试会被捕获——`/%2egit/HEAD` 解码为 `/.git/HEAD` 并被拦截。

## `.well-known` 例外

[RFC 8615](https://www.rfc-editor.org/rfc/rfc8615) 将 `/.well-known/` 定义为站点元数据的标准位置。OxPHP 允许访问 `.well-known` 内的子路径，但有以下限制：

| 请求 | 结果 |
|------|------|
| `/.well-known/security.txt` | 作为静态文件提供 |
| `/.well-known/acme-challenge/token` | 作为静态文件提供（Let's Encrypt） |
| `/.well-known/openid-configuration` | 若文件存在则提供，否则回退到 INDEX_FILE 或 404 |
| `/.well-known` | 404（裸路径） |
| `/.well-known/` | 404（目录列表） |
| `/.well-known/test.php` | 404（PHP 执行被阻止） |

`.well-known` 目录内的 PHP 文件永远不会执行——始终返回 404。此目录仅提供静态内容。

MIME 类型由文件扩展名决定。没有扩展名的文件（如 `openid-configuration`）以 `application/octet-stream` 提供。

## 参见

- [路由](../features/routing.md) —— 路由模式和路径安全
