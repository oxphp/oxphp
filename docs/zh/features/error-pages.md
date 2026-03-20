---
title: 自定义错误页面
description: 预加载的 4xx 和 5xx 响应 HTML 错误页面
---

OxPHP 可以为 4xx 和 5xx 响应提供自定义 HTML 错误页面。错误页面在启动时从磁盘加载一次，在热路径上从内存提供。

## 配置

| 变量 | 描述 | 默认值 |
|----------|-------------|---------|
| `ERROR_PAGES_DIR` | 包含错误页面 HTML 文件的目录 | *（未设置）* |

```bash
ERROR_PAGES_DIR=/var/www/errors
```

当未设置此变量时，错误响应使用默认的纯文本响应体（例如 `404 Not Found`）。

## 文件命名

错误页面文件必须遵循 `{status}.html` 的命名规范，其中 `{status}` 是 HTTP 状态码：

```
errors/
  403.html
  404.html
  500.html
  502.html
  503.html
  529.html
```

仅加载 400-599 范围内的状态码。非数字名称、超出此范围或非 `.html` 扩展名的文件将被忽略。

## 工作原理

### 加载

启动时，OxPHP 读取配置的目录并将每个有效的 `{status}.html` 文件加载到 `HashMap<u16, Bytes>` 中。文件内容以引用计数的字节缓冲区存储，实现零拷贝服务。每个加载的页面以 `info` 级别记录日志。

如果目录不存在或无法读取，将记录警告日志，服务器在没有自定义错误页面的情况下启动。

### 服务

`ErrorPagesHandler` 作为事件处理器运行在优先级 **60** 的 `ResponseBuilding` 事件上。这使其在大部分处理之后但在服务器头和访问日志处理器（优先级 100）之前执行。

对于每个状态码为 400 或更高的响应，处理器检查预加载的页面。如果存在匹配的页面，响应体将被替换为自定义 HTML 内容，`Content-Type` 头设置为 `text/html; charset=utf-8`。

2xx 或 3xx 状态的响应不受影响。

### 示例页面

一个最小的 404 错误页面：

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>404 - Page Not Found</title>
</head>
<body>
  <h1>Page Not Found</h1>
  <p>The requested page does not exist.</p>
</body>
</html>
```

将此文件保存为 `ERROR_PAGES_DIR` 指定目录中的 `404.html`。

## 性能

错误页面在启动时加载到内存中一次。提供自定义错误页面只需一次 `HashMap::get()` 和一次 `Bytes::clone()`（一个原子引用计数递增）。请求处理过程中不会发生磁盘 I/O。

## 限制

自定义错误页面仅适用于通过正常请求处理流程的响应。作为提前返回设置的响应（如速率限制的 429）会绕过 `ResponseBuilding` 事件，不受自定义错误页面的影响。

## 另请参阅

- [路由](routing.md) -- 404 响应是如何生成的
- [速率限制](rate-limiting.md) -- 被限速的响应绕过自定义错误页面
- [请求 ID](request-ids.md) -- 错误响应包含 `X-Request-ID` 头
