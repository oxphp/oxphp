---
title: 超全局变量
description: OxPHP 如何为每次请求填充 $_SERVER、$_GET、$_POST、$_COOKIE、$_FILES 及 php://input。
---

# 超全局变量

OxPHP 在脚本执行前填充所有标准 PHP 超全局变量，其行为与 PHP 开发者从传统服务器配置中所期望的一致。所有值从代码的第一行起即可使用——无需任何初始化操作。

## $_SERVER

OxPHP 遵循 CGI/1.1 规范，根据传入的 HTTP 请求构建 `$_SERVER`。进程环境变量首先被导入，CGI 变量随后设置，因此请求特定的值始终会覆盖任何冲突的环境变量键。

### 标准变量

| 变量 | 描述 | 示例 |
|------|------|------|
| `SCRIPT_FILENAME` | 正在执行的 PHP 脚本的绝对文件系统路径 | `/var/www/html/public/index.php` |
| `DOCUMENT_ROOT` | 通过 `DOCUMENT_ROOT` 环境变量配置的 Web 根目录 | `/var/www/html/public` |
| `SERVER_SOFTWARE` | 服务器标识符 | `OxPHP/0.1.0` |
| `SERVER_PROTOCOL` | 始终为 `HTTP/1.1` | `HTTP/1.1` |
| `REQUEST_METHOD` | HTTP 方法 | `GET` |
| `REQUEST_URI` | 包含查询字符串的完整 URI | `/app?page=2` |
| `SCRIPT_NAME` | 不含查询字符串的 URI 路径 | `/app` |
| `DOCUMENT_URI` | `SCRIPT_NAME` 的别名，用于 nginx/PHP-FPM 兼容性 | `/app` |
| `PHP_SELF` | 与 `SCRIPT_NAME` 相同 | `/app` |
| `QUERY_STRING` | URI 的查询部分（不存在时为空字符串） | `page=2` |
| `SERVER_NAME` | 来自 `Host` 请求头的主机名 | `example.com` |
| `SERVER_PORT` | 来自 `Host` 请求头的端口 | `8080` |
| `REMOTE_ADDR` | 客户端 IP 地址 | `172.17.0.1` |
| `REMOTE_PORT` | 客户端端口号 | `54321` |
| `HTTPS` | 连接使用 TLS 时设置为 `"on"`；否则不存在 | `on` |
| `REQUEST_SCHEME` | TLS 连接为 `"https"`，否则为 `"http"` | `https` |
| `CONTENT_TYPE` | `Content-Type` 请求头的值（无 `HTTP_` 前缀） | `application/json` |
| `CONTENT_LENGTH` | `Content-Length` 请求头的值（无 `HTTP_` 前缀） | `128` |
| `REQUEST_TIME` | 请求开始时的 Unix 时间戳（整数） | `1738800000` |
| `REQUEST_TIME_FLOAT` | 精确到微秒的 Unix 时间戳 | `1738800000.123456` |
| `GATEWAY_INTERFACE` | CGI 版本字符串 | `CGI/1.1` |

当 `Host` 请求头不存在时，`SERVER_NAME` 默认为 `localhost`，`SERVER_PORT` 默认为 `80`（TLS 连接为 `443`）。

### HTTP 请求头

所有 HTTP 请求头均以 `HTTP_` 前缀添加到 `$_SERVER` 中。请求头名称转换为大写，连字符替换为下划线，遵循 CGI/1.1 规范：

```text
Accept: text/html            -> HTTP_ACCEPT
X-Forwarded-For: 1.2.3.4    -> HTTP_X_FORWARDED_FOR
Authorization: Bearer abc    -> HTTP_AUTHORIZATION
Cookie: session=xyz          -> HTTP_COOKIE
```

> **注意：** `Content-Type` 和 `Content-Length` 不带 `HTTP_` 前缀，分别以 `CONTENT_TYPE` 和 `CONTENT_LENGTH` 的形式出现——这是 CGI 规范的要求。

### 分布式追踪变量

启用分布式追踪后，OxPHP 会向 `$_SERVER` 添加追踪上下文变量：

| 变量 | 描述 | 示例 |
|------|------|------|
| `OXPHP_TRACE_ID` | 当前请求的 W3C 追踪 ID | `4bf92f3577b34da6a3ce929d0e0e4736` |
| `OXPHP_SPAN_ID` | OxPHP 服务器 span 的 Span ID | `00f067aa0ba902b7` |
| `OXPHP_PARENT_SPAN_ID` | 来自上游服务的父 Span ID（根节点时为空） | `b9c7c989f97918e1` |

这些变量仅在收到有效的 `traceparent` 请求头或 OxPHP 生成新追踪时才会出现。如果未配置追踪，这些键不会存在。

### 与 PHP-FPM 的差异

以下变量与标准 PHP-FPM 配置相比行为有所不同：

| 变量 | 行为 |
|------|------|
| `SERVER_ADDR` | 未设置。OxPHP 不填充本地服务器 IP 地址。 |
| `PATH_INFO` / `PATH_TRANSLATED` | 未设置。OxPHP 不执行路径信息拆分。 |
| `PHP_AUTH_USER` / `PHP_AUTH_PW` / `AUTH_TYPE` | 不从 `Authorization` 请求头中提取。请直接读取 `$_SERVER['HTTP_AUTHORIZATION']`。 |
| `REDIRECT_STATUS` | 未设置。OxPHP 不使用内部重定向机制。 |

### 示例

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$scheme  = $_SERVER['REQUEST_SCHEME'];  // "http" 或 "https"

// 读取自定义请求头
$token = $_SERVER['HTTP_AUTHORIZATION'] ?? '';

// 在受信任反向代理后面时，优先使用 X-Forwarded-For
$xff  = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'];

// 不通过检查端口来判断是否使用 TLS
if (isset($_SERVER['HTTPS']) && $_SERVER['HTTPS'] === 'on') {
    // 安全连接
}
```

---

## $_GET

查询字符串参数会从请求 URI 中自动解析。

```php
<?php
// 请求：GET /search?q=oxphp&page=2
$query = $_GET['q'];     // "oxphp"
$page  = $_GET['page'];  // "2"
```

数组语法按预期工作：

```php
<?php
// 请求：GET /filter?tags[]=php&tags[]=async
$tags = $_GET['tags'];  // ["php", "async"]
```

---

## $_POST

OxPHP 支持表单提交的两种标准内容类型：

- `application/x-www-form-urlencoded` — 标准 HTML 表单数据
- `multipart/form-data` — 文件上传与表单字段的组合

```php
<?php
// 请求：POST /login
// Content-Type: application/x-www-form-urlencoded
// Body: username=admin&password=secret

$username = $_POST['username'];  // "admin"
$password = $_POST['password'];  // "secret"
```

对于 JSON 或其他内容类型，请改用 `php://input`：

```php
<?php
// 请求：POST /api/users
// Content-Type: application/json
// Body: {"name":"Alice","email":"alice@example.com"}

$data  = json_decode(file_get_contents('php://input'), true);
$name  = $data['name'];   // "Alice"
$email = $data['email'];  // "alice@example.com"
```

---

## $_COOKIE

Cookie 从 `Cookie` 请求头中解析。

```php
<?php
// 请求携带：Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

> **注意：** 以 `__oxp_` 为前缀的 Cookie 保留给 OxPHP 内部插件使用。它们会在到达 PHP 之前从 `Cookie` 请求头中剥离，不会出现在 `$_COOKIE` 中。

---

## $_FILES

通过 `multipart/form-data` 上传的文件会以标准 PHP 结构填充 `$_FILES` 数组：

```php
<?php
// $_FILES['avatar'] 结构：
// [
//     'name'     => 'photo.jpg',       // 客户端发送的原始文件名
//     'type'     => 'image/jpeg',      // 客户端声明的 MIME 类型
//     'tmp_name' => '/tmp/phpAb12Cd',  // 服务器上的临时文件路径
//     'error'    => 0,                 // UPLOAD_ERR_OK（0 表示无错误）
//     'size'     => 204800,            // 文件大小（字节）
// ]

if ($_FILES['avatar']['error'] === UPLOAD_ERR_OK) {
    $tmp  = $_FILES['avatar']['tmp_name'];
    $name = basename($_FILES['avatar']['name']);
    move_uploaded_file($tmp, "/uploads/$name");
}
```

---

## $_REQUEST

`$_REQUEST` 是 `$_GET`、`$_POST` 及可选的 `$_COOKIE` 的合并数组，由 PHP 根据 `request_order` INI 指令构建（默认值：`"GP"`——先 GET，后 POST）。OxPHP 不修改此行为。

```php
<?php
// GET /form?action=preview，POST 请求体为：action=submit
$action = $_REQUEST['action'];  // "submit"（默认顺序下 POST 覆盖 GET）
```

---

## php://input

原始请求体可通过 `php://input` 流获取。这是读取 JSON 载荷、XML 或除表单提交以外任何内容类型的标准方式。

```php
<?php
$body = file_get_contents('php://input');
$data = json_decode($body, true);
```

`php://input` 支持回绕，可在同一请求中多次读取。

> **注意：** 对于 `multipart/form-data` 请求，`php://input` 为空。请对此类请求使用 `$_POST` 和 `$_FILES`。

---

## 参见

- [PHP 函数](functions.md) -- `oxphp_request_id()`、`oxphp_worker_id()` 及其他扩展函数
- [Worker 模式](../features/worker-mode.md) -- Worker 请求之间超全局变量如何刷新
- [配置参考](../operations/configuration.md) -- `DOCUMENT_ROOT` 及其他服务器配置变量
