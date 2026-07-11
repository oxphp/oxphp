---
title: 自定义错误页面
description: 在 OxPHP 中为 4xx 和 5xx 响应提供品牌化 HTML 错误页面，启动时一次性加载并从内存中提供服务。
---

# 自定义错误页面

OxPHP 为 4xx 和 5xx 响应提供品牌化 HTML 错误页面。错误页面在启动时从磁盘一次性加载并存入内存，因此请求处理期间不会发生磁盘 I/O。

## 工作原理

1. 启动时，OxPHP 读取 `ERROR_PAGES_DIR` 指定的目录，并将所有有效的 `{status}.html` 文件加载到内存中。
2. 文件必须以 400–599 范围内的数字 HTTP 状态码命名（例如 `404.html`、`503.html`）。文件名非数字、状态码超出该范围（包括 `200.html`）或扩展名非 `.html` 的文件会被静默忽略。
3. 当 OxPHP 生成 4xx 或 5xx 响应时，会查找匹配的预加载错误页面。如果存在，OxPHP 仅替换响应体及描述它的头部：将 `Content-Type` 设置为 `text/html; charset=utf-8`，将 `Content-Length` 设置为页面大小，并移除与原始响应体耦合的头部（`Content-Encoding`、`ETag`、`Last-Modified`），以免它们错误地标记或重新验证替换内容（例如被 `ob_gzhandler` 压缩的 PHP 错误响应体不会让 HTML 页面残留 `Content-Encoding: gzip`）。描述响应语义而非响应体的头部会保留到自定义页面中——`416 Range Not Satisfiable` 的 `Content-Range`、`529 Site is overloaded` 的 `Retry-After`，以及 `405 Method Not Allowed` 的 `Allow`。
4. 如果目录在启动时不存在或无法读取，OxPHP 会记录警告并在没有自定义错误页面的情况下继续运行。错误响应将回退到纯文本正文，直到目录修复并重启服务器为止。

## 配置

| 变量 | 默认值 | 说明 |
|----------|---------|-------------|
| `ERROR_PAGES_DIR` | *（未设置）* | 包含自定义错误页面 HTML 文件的目录。文件必须以 `{status}.html` 命名，状态码范围为 400–599。未设置时，错误响应使用纯文本正文 |

## 示例页面

每个错误页面都是一个名为 `{status}.html` 的自包含 HTML 文件。请使用内联样式且不引用外部资源——否则一个自身也失败的次级请求会破坏错误页面本身。

### 可复用模板

将以下内容复制到每个 `{status}.html`，并修改 `<title>`、`<h1>` 和 `<p>`：

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <title>500 - 服务器内部错误</title>
  <style>
    body { font-family: system-ui, sans-serif; text-align: center; padding: 4rem 1rem; color: #333; }
    h1 { font-size: 2rem; margin-bottom: 0.5rem; }
    p { color: #666; }
  </style>
</head>
<body>
  <h1>出错了</h1>
  <p>请稍后重试。</p>
</body>
</html>
```

### 值得提供的状态码

OxPHP 会替换每个到达响应流水线的 4xx 或 5xx 响应的正文。以下是服务器自身返回的状态码，请为每个都提供一个文件：

| 文件 | 状态 | OxPHP 何时返回它 |
|------|--------|-----------------------|
| `400.html` | Bad Request | 未携带 `Content-Type` 头的 `QUERY` 请求（RFC 10008） |
| `404.html` | Not Found | 无匹配的文件或路由；被阻止的 dotfile（`.env`、`.git/`）；Framework 模式下的直接 `.php` 请求；`PHP_DENY_PATHS` 的默认回退 |
| `413.html` | Payload Too Large | 请求正文超过最大大小 |
| `416.html` | Range Not Satisfiable | 静态文件上无法满足的 `Range` 头（保留 `Content-Range`） |
| `500.html` | Internal Server Error | 未捕获的或致命的 PHP 错误 |
| `503.html` | Service Unavailable | 关闭期间的平滑排空（drain） |
| `504.html` | Gateway Timeout | 请求超过 `REQUEST_TIMEOUT_SECONDS` |
| `529.html` | Site is overloaded | 请求队列在 `QUEUE_CAPACITY` 处已满（保留 `Retry-After`） |

任何其他 4xx 或 5xx 也以相同方式工作——为你的 PHP 应用返回的状态码或自定义的 `PHP_DENY_FALLBACK` 状态添加 `403.html`、`451.html` 等。唯一的例外是速率限制器的 `429`，它在此处理器运行之前生成，始终使用其默认正文（见下方「注意事项」）。

### 现成示例

最简 404 页面：

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <title>404 - 页面未找到</title>
  <style>
    body { font-family: system-ui, sans-serif; text-align: center; padding: 4rem 1rem; color: #333; }
    h1 { font-size: 2rem; margin-bottom: 0.5rem; }
    p { color: #666; }
  </style>
</head>
<body>
  <h1>页面未找到</h1>
  <p>您请求的页面不存在。</p>
</body>
</html>
```

带自动刷新的 503 维护页面：

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta http-equiv="refresh" content="30">
  <title>503 - 服务不可用</title>
  <style>
    body { font-family: system-ui, sans-serif; text-align: center; padding: 4rem 1rem; color: #333; }
    h1 { font-size: 2rem; margin-bottom: 0.5rem; }
    p { color: #666; }
  </style>
</head>
<body>
  <h1>服务不可用</h1>
  <p>我们正在进行维护，本页面将自动刷新。</p>
</body>
</html>
```

## 故障排除

### 自定义错误页面未显示

请验证 `ERROR_PAGES_DIR` 已设置，且文件命名正确。

**检查：** 确认生效的目录路径，并确认 OxPHP 在启动时记录了"Loaded custom error page"日志行：

```bash
docker logs my-app 2>&1 | grep "error page"
```

**修复：** 确保目录路径正确，文件命名为 `{status}.html`，且容器对该目录有读取权限。

### 启动时关于错误页面目录缺失的警告

`ERROR_PAGES_DIR` 目录不存在或无法读取时，OxPHP 会记录警告并在没有自定义错误页面的情况下继续运行。错误响应随后会使用纯文本正文。请检查 Docker 中的卷挂载是否正确：

```bash
docker run --rm -v ./errors:/var/www/errors:ro \
  -e ERROR_PAGES_DIR=/var/www/errors \
  ghcr.io/oxphp/oxphp:0.10.0
```

### 429 响应仍显示默认正文

某些在响应处理流水线运行之前生成的响应（例如速率限制拒绝）不会经过错误页面处理器。速率限制器返回的 `429 Too Many Requests` 响应无论是否存在 `429.html` 文件，始终使用其默认正文。

## Docker 示例

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "8080:8080"
    volumes:
      - ./src:/var/www/html:ro
      - ./errors:/var/www/errors:ro
    environment:
      ERROR_PAGES_DIR: "/var/www/errors"
      ENTRY_FILE: "index.php"
```

目录结构：

```text
project/
  src/
    public/
      index.php
  errors/
    400.html
    403.html
    404.html
    500.html
    503.html
    504.html
    529.html
```

## 最佳实践

- 保持错误页面自包含，使用内联 CSS。不要引用外部样式表或脚本——这些次级请求本身也可能失败。
- 在 `503.html` 中加入 `<meta http-equiv="refresh" content="30">` 标签，以便用户在维护完成后自动重试。
- 保持错误页面小巧。每个已加载的页面在服务器进程的整个生命周期内都会占用内存。

## 注意事项

自定义错误页面适用于流经正常请求处理流水线的响应。速率限制器生成的 `429 Too Many Requests` 响应在错误页面处理器运行之前生成，使用其默认纯文本正文。

## 参见

- [路由](routing.md) -- 未匹配路径如何生成 404 响应
- [速率限制](rate-limiting.md) -- 速率限制行为和 429 响应
- [配置参考](../operations/configuration.md) -- 完整的环境变量参考
