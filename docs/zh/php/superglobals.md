---
title: 超全局变量
description: OxPHP 如何填充 PHP 超全局数组
---

OxPHP 填充所有标准 PHP 超全局变量，使现有 PHP 代码无需修改即可运行。所有值在 `php_request_startup()` 运行**之前**设置，因此超全局变量从脚本的第一行起即可使用 --- 包括在请求初始化期间读取它们的扩展（如 OPcache）。

## `$_SERVER`

自定义 SAPI 通过 `register_server_variables` 回调注册完整的 CGI/1.1 变量集。这些变量从 HTTP 请求构建，在 PHP 请求启动期间注入到 `$_SERVER` 中。

### 标准 CGI 变量

| 变量 | 来源 | 示例 |
|------|------|------|
| `REQUEST_METHOD` | HTTP 方法 | `GET` |
| `REQUEST_URI` | 包含查询字符串的完整 URI | `/app?page=2` |
| `QUERY_STRING` | URI 的查询部分 | `page=2` |
| `SERVER_PROTOCOL` | 始终为 `HTTP/1.1` | `HTTP/1.1` |
| `SCRIPT_NAME` | 不含查询字符串的 URI 路径 | `/app` |
| `PHP_SELF` | 与 `SCRIPT_NAME` 相同 | `/app` |
| `SCRIPT_FILENAME` | 脚本的绝对文件系统路径 | `/var/www/html/index.php` |
| `DOCUMENT_ROOT` | Web 根目录 | `/var/www/html` |
| `SERVER_SOFTWARE` | 服务器标识 | `OxPHP/0.1.0` |
| `GATEWAY_INTERFACE` | CGI 版本 | `CGI/1.1` |
| `REMOTE_ADDR` | 客户端 IP 地址 | `172.17.0.1` |
| `REMOTE_PORT` | 客户端端口 | `54321` |
| `SERVER_NAME` | 来自 `Host` 头（主机部分） | `example.com` |
| `SERVER_PORT` | 来自 `Host` 头（端口部分） | `8080` |
| `CONTENT_TYPE` | 来自 `Content-Type` 头 | `application/json` |
| `CONTENT_LENGTH` | 来自 `Content-Length` 头 | `128` |

当缺少 `Host` 头时，`SERVER_NAME` 默认为 `localhost`，`SERVER_PORT` 默认为 `80`。

### HTTP 请求头

所有 HTTP 请求头以 `HTTP_` 前缀添加到 `$_SERVER`，破折号转换为下划线，遵循标准 CGI 惯例：

```
Accept: text/html         → HTTP_ACCEPT = "text/html"
X-Forwarded-For: 1.2.3.4 → HTTP_X_FORWARDED_FOR = "1.2.3.4"
Authorization: Bearer ... → HTTP_AUTHORIZATION = "Bearer ..."
```

`Content-Type` 和 `Content-Length` **不**带 `HTTP_` 前缀 --- 它们直接以 `CONTENT_TYPE` 和 `CONTENT_LENGTH` 出现，符合 CGI 规范要求。

### 使用示例

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$docroot = $_SERVER['DOCUMENT_ROOT'];

// 访问自定义头
$token   = $_SERVER['HTTP_AUTHORIZATION'] ?? '';
$xff     = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'];
```

## `$_GET`

查询字符串参数由 PHP 标准引擎从 `QUERY_STRING` 服务器变量解析。OxPHP 从请求 URI 设置 `QUERY_STRING`，PHP 在请求启动期间自动填充 `$_GET`。

```php
<?php
// 请求: GET /search?q=oxphp&page=2
$query = $_GET['q'];     // "oxphp"
$page  = $_GET['page'];  // "2"
```

## `$_POST`

SAPI 提供 `read_post` 回调，将请求体提供给 PHP 的标准 POST 解析器。PHP 处理 `application/x-www-form-urlencoded` 和 `multipart/form-data` 两种内容类型。请求体增量读取 --- PHP 重复调用回调直到返回 0 字节。

```php
<?php
// 请求: POST /login，内容类型为 application/x-www-form-urlencoded
$username = $_POST['username'];
$password = $_POST['password'];
```

原始请求体也可通过 `php://input` 获取：

```php
<?php
$json = json_decode(file_get_contents('php://input'), true);
```

## `$_COOKIE`

SAPI 提供 `read_cookies` 回调，返回原始 `Cookie` 头字符串。PHP 引擎在请求启动期间将其解析为 `$_COOKIE` 数组。

```php
<?php
// 请求头: Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

## `$_REQUEST`

`$_REQUEST` 由 PHP 根据 `request_order` INI 指令填充（默认值：`"GP"` --- 先 GET 后 POST）。它按配置的顺序合并 `$_GET` 和 `$_POST`（以及可选的 `$_COOKIE`）。OxPHP 不覆盖此行为。

## `$_FILES`

通过 `multipart/form-data` 发送的文件上传由 PHP 的标准 `read_post` 机制解析。`$_FILES` 数组自动填充，包括每个上传文件的 `name`、`type`、`tmp_name`、`error` 和 `size`。

```php
<?php
if ($_FILES['avatar']['error'] === UPLOAD_ERR_OK) {
    $tmp  = $_FILES['avatar']['tmp_name'];
    $name = $_FILES['avatar']['name'];
    move_uploaded_file($tmp, "/uploads/$name");
}
```

## 实现细节

### 批量 FFI 模式

OxPHP 在单个 Rust `Vec<(CString, CString)>` 中构建所有 `$_SERVER` 键值对，并在 PHP 启动请求前将其存储在线程本地存储中。在 `register_server_variables` 回调期间，一次遍历整个向量，对每个条目调用 `php_register_variable_safe()`。这避免了逐变量的 FFI 调用，无论头数量多少，开销都保持恒定。

### 数据生命周期要求

所有按请求的数据 --- 服务器变量、Cookie 字符串和请求体 --- 必须在调用 `php_request_startup()` **之前**存储在线程本地存储中。这些值必须在 `php_request_shutdown()` 之后仍然有效，因为 PHP 引擎持有指向它们的原始指针。OxPHP 将它们存储在 `thread_local! { RefCell }` 中的 `RequestData` 结构中，仅在请求完全关闭后才清除。

### 线程本地桥接

C 桥接库（`liboxphp_bridge.so`）使用 `__thread` TLS 在 Rust 和 PHP 扩展之间共享按请求的上下文（请求 ID、工作线程 ID、请求时间）。Rust 二进制文件和 PHP 扩展都链接到同一个共享库，因此可以访问相同的线程本地存储。这是跨 `dlopen` 边界共享状态的唯一可靠机制。

## 另请参阅

- [PHP 扩展函数](functions.md) --- 用于访问请求 ID、工作线程信息和服务器元数据的内置函数
- [OPcache 兼容性](opcache.md) --- 请求时间如何启用 OPcache 的 `file_update_protection`
- [SAPI 桥接](/architecture/sapi-bridge.md) --- C 桥接库和线程本地存储机制
- [请求生命周期](/architecture/request-lifecycle.md) --- 请求数据如何从 Rust 流向 PHP
