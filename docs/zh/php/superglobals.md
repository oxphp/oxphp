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
| `SERVER_SOFTWARE` | 服务器标识符（包含运行中的 OxPHP 版本） | `OxPHP/0.10.0` |
| `SERVER_PROTOCOL` | 协商得到的 HTTP 协议版本 | `HTTP/2` |
| `REQUEST_METHOD` | HTTP 方法 | `GET` |
| `REQUEST_URI` | 包含查询字符串的完整 URI | `/app?page=2` |
| `SCRIPT_NAME` | 相对于 `DOCUMENT_ROOT` 的已执行脚本路径——在 Framework 模式下即前端控制器，**而非**请求 URI | `/index.php` |
| `DOCUMENT_URI` | `SCRIPT_NAME` 的别名，用于 nginx/PHP-FPM 兼容性 | `/index.php` |
| `PHP_SELF` | `SCRIPT_NAME` 加上存在时的 `PATH_INFO`，否则等于 `SCRIPT_NAME` | `/index.php/user/42` |
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

### 位于反向代理之后

当配置了 `TRUSTED_PROXIES` 且请求对端属于受信任集合时，OxPHP 会根据转发头（`X-Forwarded-*` 或 RFC 7239 `Forwarded`）改写以下 `$_SERVER` 键：

| 变量 | 对端受信任时的值 | 否则 |
|----------|----------------------------|------------------|
| `REMOTE_ADDR` | `X-Forwarded-For` / `Forwarded` 中最右侧的不受信任地址 | 直接对端 IP |
| `HTTPS` | 当 `X-Forwarded-Proto: https` 时为 `"on"` | 仅当对端连接是 TLS 时设置 |
| `REQUEST_SCHEME` | 由 `X-Forwarded-Proto` 得到的 `"https"` / `"http"` | 基于实际 TLS 状态 |
| `SERVER_NAME` | `X-Forwarded-Host` 的主机部分 | `Host` 头的主机部分 |
| `SERVER_PORT` | `X-Forwarded-Host` 的端口部分，或按协议取 443/80 | `Host` 的端口部分，或 443/80 |

原始的 `HTTP_X_FORWARDED_FOR`、`HTTP_X_FORWARDED_PROTO`、`HTTP_X_FORWARDED_HOST` 和 `HTTP_FORWARDED` 仍保留在 `$_SERVER` 中——改写后的值与原始请求头同时可见。

当 `TRUSTED_PROXIES` **未**设置时，不进行任何改写，`REMOTE_ADDR` 始终是直接对端——通常是你的负载均衡器，而不是终端客户端。手动解析 `X-Forwarded-For` 很容易出错（最左 vs 最右、缺少 CIDR 信任判断）；建议配置 `TRUSTED_PROXIES`。信任算法和配置语法参见[受信任的代理](../security/trusted-proxies.md)。

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
| `PATH_INFO` | 自动设置——详见下方 [PATH_INFO 行为](#path_info-行为)。 |
| `PATH_TRANSLATED` | 未设置。 |
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

// 配置了 TRUSTED_PROXIES 时，REMOTE_ADDR 已经是真实的客户端 IP。
// 未配置时，REMOTE_ADDR 是直接对端（通常是负载均衡器）。
$clientIp = $_SERVER['REMOTE_ADDR'];

// 不通过检查端口来判断是否使用 TLS
if (isset($_SERVER['HTTPS']) && $_SERVER['HTTPS'] === 'on') {
    // 安全连接
}
```

### PATH_INFO 行为

`$_SERVER['PATH_INFO']` 根据当前路由模式自动填充。没有功能开关——之前的 `SPLIT_PATH_INFO_ENABLED` 环境变量已被移除。

| 路由模式 | 何时设置 | 值 |
|---|---|---|
| **Traditional**（未设置 `ENTRY_FILE`） | 仅当 URI 包含 `.php/` 且脚本前缀在磁盘上存在时 | 脚本段之后的尾部 |
| **Framework**（`ENTRY_FILE=index.php`） | 仅当请求显式指定入口文件并带有尾部段时（`/index.php/extra`） | 入口文件之后的尾部，例如 `/news` |
| **SPA**（`ENTRY_FILE=index.html`） | 永不 — PHP 仅对精确的 `.php` 文件运行，无 PATH_INFO | — |

`SCRIPT_NAME` 始终标识实际执行的脚本（相对于文档根的已解析文件），因此在正常路由下 `PATH_INFO` 仅在 `SCRIPT_NAME` 是请求路径的字面前缀时才存在。当请求被重写到 URL 中未指定的前端控制器（应用路由、目录索引、静态未命中回退）时，`PATH_INFO` 不存在，原始路径保留在 `REQUEST_URI` 中。（`PHP_DENY_PATHS` 回退是有意的例外：它将原始的规范化 URI 设置为 `PATH_INFO`，以便回退脚本据此路由。）

#### Traditional 模式示例

OxPHP 从左到右扫描 URI，查找第一个对应磁盘上实际文件的 `.php` 段。其后的所有内容成为 `PATH_INFO`：

| 请求 URI | 磁盘上的文件 | `SCRIPT_NAME` | `PATH_INFO` | `PHP_SELF` |
|---|---|---|---|---|
| `/app.php/user/42` | `app.php` 存在 | `/app.php` | `/user/42` | `/app.php/user/42` |
| `/index.php/api/v2/users` | `index.php` 存在 | `/index.php` | `/api/v2/users` | `/index.php/api/v2/users` |
| `/app.php` | `app.php` 存在 | `/app.php` | *（不存在）* | `/app.php` |
| `/missing.php/foo` | 文件未找到 | 回退到 `/index.php` | — | 取决于回退 |

#### Framework 模式示例

每个非静态请求都会被重写到 `index.php`。`PATH_INFO` 仅在请求显式指定入口文件并带有尾部段时才设置；对于应用路由，原始路径从 `REQUEST_URI` 读取。

| 请求 URI | `SCRIPT_NAME` | `PATH_INFO` |
|---|---|---|
| `/api/users` | `/index.php` | *（不存在）* |
| `/about.php` | `/index.php` | *（不存在）* |
| `/index.php/news/local` | `/index.php` | `/news/local` |
| `/index.php` | `/index.php` | *（不存在）* |

> **注意：** `PATH_TRANSLATED` 不会被填充。它在实践中很少使用，nginx 和 PHP-FPM 默认也不设置此变量。

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

`$_REQUEST` 是 `$_GET`、`$_POST` 及可选的 `$_COOKIE` 的合并数组，由 PHP 根据 `request_order` INI 指令构建（默认值：`"GP"`——先 GET，后 POST）。合并规则本身 OxPHP 不做修改。

```php
<?php
// GET /form?action=preview，POST 请求体为：action=submit
$action = $_REQUEST['action'];  // "submit"（默认顺序下 POST 覆盖 GET）
```

> **Worker 模式：** `$_REQUEST` 会为每个请求重建。PHP 通常只在首次加载提及它的脚本时惰性构建一次——在常驻 worker 中，这意味着之后的每个请求都会读到第一个请求的参数。OxPHP 强制重建，因此合并数组始终描述当前正在处理的请求。

---

## $_ENV

`$_ENV` 保存进程环境。传统模式下的行为与 PHP-FPM 完全一致：PHP 在每个请求中依据 `variables_order` 从环境重新填充该数组。

**Worker 模式会将其固定。** Worker 只引导一次，而 `.env` 加载器（vlucas/phpdotenv、symfony/dotenv、Laravel 的 `Env`）会直接写入 `$_ENV`，并不修改进程环境。若每个请求都重建 `$_ENV`，应用配置从第二个请求起就会被抹掉；因此在 worker 模式下，该数组一旦存在便在 worker 的整个生命周期内保留——引导阶段写入的内容对该 worker 处理的每个请求都可见。

```php
<?php
// 引导阶段，位于 oxphp_worker() 之前
Dotenv\Dotenv::createImmutable(__DIR__)->load();   // 写入 $_ENV

oxphp_worker(function () {
    echo $_ENV['DATABASE_URL'];  // 第 10000 个请求时依然存在
});
```

> **取舍：** 由于 worker 模式下没有任何环节重建 `$_ENV`，`filter_input(INPUT_ENV, …)`、`filter_input_array(INPUT_ENV)` 和 `filter_has_var(INPUT_ENV, …)` 会报告没有变量。请改用 `getenv()` 读取环境，或直接读取 `$_ENV`——其中既有进程的值，也有引导阶段追加的值。此限制仅适用于 worker 模式，传统、框架和 SPA 模式不受影响。

该固定行为依赖 PHP 的默认设置 `auto_globals_jit=1`。当 `auto_globals_jit=0` 时，PHP 会在扩展介入之前于每个请求从进程环境重新填充 `$_ENV`，`.env` 加载器写入的值在 worker 模式下将无法保留。

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

## 禁用超全局变量

设置 `SUPERGLOBALS_ENABLED=false` 可禁用 `$_GET`、`$_POST`、`$_COOKIE`、`$_FILES` 和 `$_SERVER` 的填充。禁用后这些数组将为空。请改用 [HTTP 请求对象 API](request-api.md)（`oxphp_http_request()`）访问请求数据。

```bash
SUPERGLOBALS_ENABLED=false   # 超全局变量为空数组
```

无论此设置如何，以下内容始终可用：

| 内容 | 原因 |
|------|------|
| `$_SESSION` | 由 PHP 会话模块管理，非 SAPI |
| `php://input` | 流，非超全局变量 |
| `header()`、`headers_list()` 等 | SAPI 函数，非超全局变量 |
| `session_start()` 及其他 `session_*()` 函数 | PHP 原生函数 |
| `oxphp_http_request()` | 始终可用 — 推荐替代方案 |

可在运行时检查当前设置：

```php
if (!oxphp_superglobals_enabled()) {
    $request = oxphp_http_request();
    $page = $request->query('page', 1);
}
```

---

## 参见

- [HTTP 请求对象 API](request-api.md) -- 类型化、惰性加载的请求对象，作为超全局变量的替代方案
- [PHP 函数](functions.md) -- `oxphp_request_id()`、`oxphp_worker_id()` 及其他扩展函数
- [Worker 模式](../features/worker-mode.md) -- Worker 请求之间超全局变量如何刷新
- [配置参考](../operations/configuration.md) -- `DOCUMENT_ROOT` 及其他服务器配置变量
