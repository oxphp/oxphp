---
title: HTTP 请求对象 API
description: OxPHP 中访问 HTTP 请求数据的面向对象 API，用类型安全、懒加载的接口取代 PHP 超全局变量。
---

# HTTP 请求对象 API

OxPHP 提供了一套面向对象的 API 用于访问 HTTP 请求数据。你不再需要读取 `$_GET`、`$_POST`、`$_COOKIE`、`$_FILES` 和 `$_SERVER`，而是通过 `Request` 对象调用方法，精确获取所需的数据——不多也不少。

## 目录

- [概述](#概述)
- [获取请求对象](#获取请求对象)
- [RequestInterface 方法](#requestinterface-方法)
  - [URI 与请求方法](#uri-与请求方法)
  - [协议](#协议)
  - [查询参数](#查询参数)
  - [解析后的请求体](#解析后的请求体)
  - [请求头](#请求头)
  - [Cookie](#cookie)
  - [原始请求体](#原始请求体)
  - [文件上传](#文件上传)
  - [客户端信息](#客户端信息)
  - [请求时间](#请求时间)
  - [属性](#属性)
  - [Session](#session)
- [SessionInterface](#sessioninterface)
- [UploadedFileInterface](#uploadedfileinterface)
- [AttributesInterface](#attributesinterface)
- [异常](#异常)
- [SUPERGLOBALS_ENABLED](#superglobals_enabled)
- [Worker 模式](#worker-模式)
- [IDE 支持](#ide-支持)
- [示例](#示例)

---

## 概述

`oxphp_http_request()` 返回一个只读代理，指向当前 Worker 线程中存储的 HTTP 请求数据。数据按需懒加载——单次方法调用（如 `$request->header('Accept')`）直接访问 Rust 侧的数据结构并只返回该值。全量调用（如 `$request->headers()`）则在首次调用时构建数组，并在请求存续期间缓存到 PHP 对象中。

**为什么使用它而不是超全局变量？**

- **内置 JSON 请求体解析。** `$request->payload()` 无需额外代码即可解析 `application/json`、`application/x-www-form-urlencoded` 和 `multipart/form-data`。
- **消除数组键名拼写错误。** `$request->method()` 比 `$_SERVER['REQUEST_METHOD']` 更难写错。
- **文件类型检测。** `$request->file('avatar')->type()` 返回根据文件实际内容检测出的 MIME 类型，而非客户端上报的值。
- **可测试性。** 行为由接口定义，可在单元测试中注入模拟实现。
- **超全局变量仍然可用。** `SUPERGLOBALS_ENABLED=false` 是可选配置。无论该设置如何，对象 API 均可正常使用。

---

## 获取请求对象

```php
<?php
$request = oxphp_http_request();
```

在活跃 HTTP 请求期间执行的脚本中，可以在任意位置调用 `oxphp_http_request()`，包括在 `oxphp_worker()` 回调内部：

```php
<?php
oxphp_worker(function () {
    $request = oxphp_http_request();
    $method = $request->method();
    // ...
});

---

## RequestInterface 方法

### URI 与请求方法

```php
$request->method(): string
```

返回大写的 HTTP 方法：`"GET"`、`"POST"`、`"PUT"`、`"PATCH"`、`"DELETE"` 等。

```php
$request->isMethod(string $method): bool
```

大小写不敏感的方法检查。

```php
$request->path(): string
```

不含查询字符串的 URI 路径：`"/users/42"`。

```php
$request->fullUri(): string
```

包含协议、主机、可选非标准端口、路径和查询字符串的完整 URI：`"https://example.com:8080/users/42?page=2"`。标准端口（HTTP 的 80 和 HTTPS 的 443）会被省略。

```php
$request->scheme(): string
```

`"https"` 或 `"http"`。

```php
$request->isSecure(): bool
```

协议为 `"https"` 时返回 `true`。

```php
$request->host(): string
```

来自 `Host` 请求头的主机名。当请求头不存在时（如不带 `Host` 头的 HTTP/1.0 请求）返回空字符串。

```php
$request->port(): int
```

来自 `Host` 请求头的端口号。未显式指定时，返回对应协议的默认端口：HTTP 为 `80`，HTTPS 为 `443`。

```php
$request->queryString(): ?string
```

不含前导 `?` 的原始查询字符串。无查询字符串时返回 `null`。

---

### 协议

```php
$request->httpProtocol(): string
```

完整的协议字符串：`"HTTP/1.1"` 或 `"HTTP/2"`。

```php
$request->httpProtocolVersion(): string
```

仅版本号：`"1.1"` 或 `"2"`。

---

### 查询参数

```php
$request->query(?string $key = null, mixed $default = null): mixed
```

访问查询字符串参数。

| 调用形式 | 返回值 |
|----------|--------|
| `$request->query()` | 所有参数组成的数组，包括嵌套数组 |
| `$request->query('page')` | `page` 的值，不存在时返回 `null` |
| `$request->query('page', 1)` | `page` 的值，不存在时返回 `1` |

方括号语法（`?tags[]=php&tags[]=async`）会被解析为嵌套数组：

```php
// 请求：GET /search?q=oxphp&tags[]=php&tags[]=async
$q    = $request->query('q');      // "oxphp"
$tags = $request->query('tags');   // ["php", "async"]
$all  = $request->query();         // ["q" => "oxphp", "tags" => ["php", "async"]]
```

找到的值始终为字符串类型。键不存在时，`$default` 原样返回。

---

### 解析后的请求体

```php
$request->payload(?string $key = null, mixed $default = null): mixed
```

返回解析后的请求体。请求体根据 `Content-Type` 请求头进行解析：

| Content-Type | 返回值 |
|---|---|
| `application/x-www-form-urlencoded` | 字段值的关联数组 |
| `multipart/form-data` | 文本字段值的关联数组 |
| `application/json` | 解码后的数组或标量；JSON 无效时返回 `null` |
| 其他值 | `null` |

`payload()` 不限于 POST 请求——它适用于 PUT、PATCH 以及任何携带请求体的方法。解析结果在首次调用时缓存，并在整个请求生命周期内复用。

| 调用形式 | 返回值 |
|----------|--------|
| `$request->payload()` | 完整的解析后请求体 |
| `$request->payload('email')` | 单个字段值，不存在时返回 `null` |
| `$request->payload('email', '')` | 单个字段值，不存在时返回 `''` |

```php
<?php
// JSON 请求：POST /api/users
// Content-Type: application/json
// Body: {"name": "Alice", "role": "admin"}

$name = $request->payload('name');  // "Alice"
$role = $request->payload('role');  // "admin"
$data = $request->payload();        // ["name" => "Alice", "role" => "admin"]
```

---

### 请求头

```php
$request->header(string $name, ?string $default = null): ?string
```

返回原始请求头的值。请求头名称大小写不敏感。对于多值请求头（`Accept`、`X-Forwarded-For`），完整的头部行以单个字符串返回——解析由调用方负责。

```php
$request->hasHeader(string $name): bool
```

指定请求头存在时返回 `true`。

```php
$request->headers(): array
```

以关联数组形式返回所有请求头。每个键是接收到的原始头部名称（不做规范化），每个值是原始头部字符串。

```php
<?php
$accept = $request->header('Accept');
// "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"

if ($request->hasHeader('Authorization')) {
    $token = $request->header('Authorization');
}

$all = $request->headers();
// ["Content-Type" => "application/json", "Accept" => "...", ...]
```

---

### Cookie

```php
$request->cookie(string $name, ?string $default = null): ?string
```

返回单个 Cookie 的值，Cookie 不存在时返回 `$default`。

```php
$request->cookies(): array
```

以关联数组形式返回所有 Cookie 的名值对。

```php
<?php
$theme   = $request->cookie('theme', 'light');   // "dark" 或 "light"
$session = $request->cookie('session');           // 不存在时返回 null
$all     = $request->cookies();                   // ["theme" => "dark", ...]
```

---

### 原始请求体

```php
$request->body(): string
```

返回原始请求体的字节内容。这是 OxPHP 中等价于 `file_get_contents('php://input')` 的方法。与 `payload()` 不同，`body()` 不缓存结果——每次调用都直接访问底层数据结构。

```php
$request->contentType(): ?string
```

返回 `Content-Type` 请求头的值，不存在时返回 `null`。

```php
<?php
// 读取原始请求体用于签名验证
$raw       = $request->body();
$signature = $request->header('X-Hub-Signature-256');
$valid     = hash_hmac('sha256', $raw, $secret) === $signature;
```

`body()` 与 `payload()` 相互独立，可以在同一个请求中同时调用。

---

### 文件上传

```php
$request->file(string $name): ?UploadedFileInterface
```

返回指定字段名对应的上传文件，字段不存在时返回 `null`。对于数组字段（`name="photos[]"`），返回第一个文件。

```php
$request->files(?string $name = null): array
```

| 调用形式 | 返回值 |
|----------|--------|
| `$request->files()` | 所有上传文件组成的扁平 `UploadedFileInterface` 数组 |
| `$request->files('photos')` | `photos` 字段的所有文件（支持 `name="photos[]"`） |

```php
<?php
$avatar = $request->file('avatar');

if ($avatar && $avatar->isValid()) {
    $mime = $avatar->type();    // 从文件内容检测，而非客户端声明
    $name = $avatar->name();    // 原始文件名
    $avatar->moveTo('/var/uploads/' . basename($name));
}

// 多文件上传
$photos = $request->files('photos');  // UploadedFileInterface[]
foreach ($photos as $photo) {
    if ($photo->isValid()) {
        $photo->moveTo('/var/uploads/' . basename($photo->name()));
    }
}
```

---

### 客户端信息

```php
$request->ip(): string
```

返回客户端 IP 地址（`REMOTE_ADDR`）。当应用位于反向代理后面时，请直接从 `$request->header('X-Forwarded-For')` 读取并应用你自己的信任逻辑。

---

### 请求时间

```php
$request->startTime(bool $asFloat = false): int|float
```

返回本次请求被接收时的 Unix 时间戳。

| 调用形式 | 返回值 |
|----------|--------|
| `$request->startTime()` | 整数秒：`1711234567` |
| `$request->startTime(true)` | 带亚秒精度的浮点数：`1711234567.3412` |

```php
<?php
$elapsed = microtime(true) - $request->startTime(true);
error_log(sprintf("Request took %.3fs so far", $elapsed));
```

---

### 属性

```php
$request->attributes(): AttributesInterface
```

返回当前请求的可变属性容器。使用属性在中间件、路由处理程序及同一请求中的其他代码之间共享数据，无需使用全局变量。

```php
<?php
// 在认证中间件中
$request->attributes()->set('user', $authenticatedUser);

// 在路由处理程序中
$user = $request->attributes()->get('user');
```

属性按请求隔离，在 Worker 模式下每次新请求时重置。使用 Fiber 时，属性在同一 Worker 线程处理同一请求的所有 Fiber 之间共享——但由于 PHP Fiber 是协作式的，不存在并发访问的问题。

---

### Session

```php
$request->session(): ?SessionInterface
```

返回 `$_SESSION` 的只读视图。若未调用 `session_start()`，则返回 `null`。Session 的管理（启动、保存、销毁、写入数据）使用标准 PHP session 函数。

```php
<?php
session_start();
$session = $request->session();

$userId  = $session->get('user_id');
$isAdmin = $session->get('is_admin', false);

// 使用标准 PHP 函数写入 session 数据
$_SESSION['last_seen'] = time();
```

---

## SessionInterface

`SessionInterface` 是对当前活跃 session 的只读视图。

```php
namespace OxPHP\Http;

interface SessionInterface
{
    public function id(): string;
    public function name(): string;
    public function get(string $key, mixed $default = null): mixed;
    public function has(string $key): bool;
    public function all(): array;
}
```

| 方法 | 描述 |
|------|------|
| `id()` | Session ID |
| `name()` | Session 名称（默认：`"PHPSESSID"`） |
| `get(key, default)` | 单个 session 值，键不存在时返回 `$default` |
| `has(key)` | 键存在于 `$_SESSION` 中时返回 `true` |
| `all()` | 以数组形式返回所有 session 数据 |

Session 值反映调用时 `$_SESSION` 的当前状态，而非首次调用 `session()` 时的状态。

---

## UploadedFileInterface

`UploadedFileInterface` 表示单个上传的文件。

```php
namespace OxPHP\Http;

interface UploadedFileInterface
{
    public function name(): string;
    public function clientType(): string;
    public function type(): string;
    public function size(): int;
    public function tmpPath(): string;
    public function error(): int;
    public function isValid(): bool;
    public function moveTo(string $path): bool;
}
```

| 方法 | 描述 |
|------|------|
| `name()` | 客户端发送的原始文件名 |
| `clientType()` | 客户端声明的 MIME 类型——请勿将此值用于安全决策 |
| `type()` | 通过魔数检测从文件实际内容确定的 MIME 类型。无法确定时返回 `"application/octet-stream"`。首次调用时缓存。 |
| `size()` | 文件大小（字节） |
| `tmpPath()` | 磁盘上临时文件的路径 |
| `error()` | `UPLOAD_ERR_*` 常量之一 |
| `isValid()` | `error()` 为 `UPLOAD_ERR_OK` 时返回 `true` |
| `moveTo(path)` | 将文件移动到 `$path`。移动前会调用 `type()`。文件无效或移动失败时返回 `false`。 |

使用上传文件前请务必检查 `isValid()`。在涉及安全的决策中，请使用 `type()` 而非 `clientType()`：

```php
<?php
$file = $request->file('document');

if (!$file || !$file->isValid()) {
    http_response_code(400);
    echo json_encode(['error' => 'Upload failed or missing']);
    return;
}

$detectedMime = $file->type();
$allowed = ['application/pdf', 'image/jpeg', 'image/png'];

if (!in_array($detectedMime, $allowed, true)) {
    http_response_code(415);
    echo json_encode(['error' => "File type not allowed: $detectedMime"]);
    return;
}

$file->moveTo('/var/uploads/' . bin2hex(random_bytes(8)) . '.pdf');
```

---

## AttributesInterface

`AttributesInterface` 是请求对象中唯一可变的部分。它用于存储每个请求的元数据——已认证用户、解析出的路由参数、语言环境、功能开关——供应用的多个部分访问。

```php
namespace OxPHP\Http;

interface AttributesInterface
{
    public function get(string $key, mixed $default = null): mixed;
    public function set(string $key, mixed $value): void;
    public function has(string $key): bool;
    public function remove(string $key): void;
    public function all(): array;
}
```

| 方法 | 描述 |
|------|------|
| `get(key, default)` | 返回 `$key` 对应的值，不存在时返回 `$default` |
| `set(key, value)` | 存储一个值 |
| `has(key)` | 键已设置时返回 `true` |
| `remove(key)` | 移除该键 |
| `all()` | 以关联数组形式返回所有属性 |

---

## 异常

在没有活跃请求的上下文中调用 `oxphp_http_request()` 会抛出 `OxPHP\Http\Exception` 命名空间下的异常。

```php
namespace OxPHP\Http\Exception;

class NoActiveRequestException extends \RuntimeException {}
class AsyncContextException extends NoActiveRequestException {}
class WorkerIdleException extends NoActiveRequestException {}
```

| 异常 | 抛出时机 |
|------|----------|
| `NoActiveRequestException` | 无活跃 HTTP 请求：CLI、MINIT 阶段、关闭后，或超出请求生命周期的 Fiber |
| `AsyncContextException` | 在 `oxphp_async()` 回调内——异步 Worker 运行在独立线程上，没有请求上下文 |
| `WorkerIdleException` | Worker 模式下，两次请求之间——Worker 正在等待下一个请求 |

`AsyncContextException` 和 `WorkerIdleException` 都继承自 `NoActiveRequestException`，因此捕获基类即可覆盖所有情况。

```php
<?php
try {
    $request = oxphp_http_request();
} catch (\OxPHP\Http\Exception\AsyncContextException $e) {
    // 在 oxphp_async() 内部——此处没有请求上下文
} catch (\OxPHP\Http\Exception\WorkerIdleException $e) {
    // Worker 在两次请求之间——不要在此处调用 oxphp_http_request()
} catch (\OxPHP\Http\Exception\NoActiveRequestException $e) {
    // 其他无活跃请求的情况
}
```

在普通的请求处理代码中，不需要这段 try/catch。异常保护在可能于请求上下文之外运行的引导代码中才有意义。

---

## SUPERGLOBALS_ENABLED

```bash
SUPERGLOBALS_ENABLED=true    # 默认值——完整的向后兼容
SUPERGLOBALS_ENABLED=false   # 超全局变量为空数组
```

默认情况下，OxPHP 会照常填充 `$_GET`、`$_POST`、`$_COOKIE`、`$_FILES` 和 `$_SERVER`。在此模式下，HTTP 对象 API 与超全局变量并存可用。

将 `SUPERGLOBALS_ENABLED` 设为 `false` 会使这些数组为空，从而省去每次请求构建它们的开销。无论该设置如何，以下功能始终可用：

| 功能 | `SUPERGLOBALS_ENABLED=false` 时的行为 |
|------|---------------------------------------|
| `oxphp_http_request()` | 始终可用 |
| `php://input` | 可用（它是流，不是超全局变量） |
| `$_SESSION` | 可用（由 PHP 的 session 模块管理） |
| `header()`、`headers_list()` | 可用（SAPI 输出函数） |
| `session_start()`、`session_*()` | 可用（原生 PHP 函数） |
| `$_GET`、`$_POST`、`$_COOKIE`、`$_FILES`、`$_SERVER` | 空数组 |

使用 `oxphp_superglobals_enabled()` 在运行时检查当前设置：

```php
<?php
if (!oxphp_superglobals_enabled()) {
    $method = oxphp_http_request()->method();
} else {
    $method = $_SERVER['REQUEST_METHOD'];
}
```

---

## Worker 模式

在 Worker 模式下，每次新请求到来时都会创建一个新的 Request 对象。请求完成后，上一个请求的对象即告失效——不要在请求之间保存对它的引用。

```php
<?php
// worker.php

require __DIR__ . '/vendor/autoload.php';
$app = new MyApp\Application();

oxphp_worker(function () use ($app) {
    $request = oxphp_http_request();
    $app->handle($request);
});
```

Request 对象上的所有缓存（已解析的请求头、Cookie、查询参数、请求体）会在下一个请求开始时自动清除。

---

## IDE 支持

安装 stub 包，即可在 PhpStorm、VS Code 或任何支持 LSP 的编辑器中获得自动补全和类型检查：

```bash
composer require --dev oxphp/stubs
```

stub 包提供以下内容：

```
oxphp-stubs/
├── OxPHP/Http/
│   ├── RequestInterface.php
│   ├── SessionInterface.php
│   ├── UploadedFileInterface.php
│   ├── AttributesInterface.php
│   ├── Request.php
│   ├── Session.php
│   ├── UploadedFile.php
│   ├── Attributes.php
│   └── Exception/
│       ├── NoActiveRequestException.php
│       ├── AsyncContextException.php
│       └── WorkerIdleException.php
└── functions.php
```

不会添加任何运行时依赖。该包仅作为 `require-dev` 使用。

---

## 示例

### 传统模式

```php
<?php
$request = oxphp_http_request();

$method = $request->method();         // "GET"
$path   = $request->path();           // "/api/articles"
$page   = $request->query('page', 1); // "2" 或 1（默认值）

// 验证 Authorization 头
if (!$request->hasHeader('Authorization')) {
    http_response_code(401);
    echo json_encode(['error' => 'Unauthorized']);
    exit;
}

$token = $request->header('Authorization');

// 带请求元数据的结构化日志
error_log(sprintf(
    '[%s] %s %s from %s',
    oxphp_request_id(),
    $method,
    $path,
    $request->ip()
));
```

### POST 请求与 JSON 请求体

```php
<?php
$request = oxphp_http_request();

if (!$request->isMethod('POST')) {
    http_response_code(405);
    exit;
}

$email    = $request->payload('email');
$password = $request->payload('password');

if (!$email || !$password) {
    http_response_code(400);
    echo json_encode(['error' => 'email and password are required']);
    exit;
}

// payload() 自动处理 JSON、form-urlencoded 和 multipart
// 无需手动调用 json_decode() 或检查 $_POST
$user = authenticate($email, $password);

header('Content-Type: application/json');
echo json_encode(['token' => $user->generateToken()]);
```

### 中间件属性

```php
<?php
// auth-middleware.php
function authenticate_request(\OxPHP\Http\RequestInterface $request): void
{
    $token = $request->header('Authorization');
    if (!$token) {
        http_response_code(401);
        exit;
    }

    $user = verify_token(str_replace('Bearer ', '', $token));
    if (!$user) {
        http_response_code(403);
        exit;
    }

    $request->attributes()->set('user', $user);
}

// route-handler.php
$request = oxphp_http_request();
authenticate_request($request);

$user = $request->attributes()->get('user');
echo json_encode(['id' => $user->id, 'name' => $user->name]);
```

### Worker 模式与 Session

```php
<?php
// worker.php
require __DIR__ . '/vendor/autoload.php';

oxphp_worker(function () {
    $request = oxphp_http_request();

    if ($request->path() === '/login' && $request->isMethod('POST')) {
        $username = $request->payload('username');
        $password = $request->payload('password');

        if (verify_credentials($username, $password)) {
            session_start();
            $_SESSION['user'] = $username;
            $_SESSION['authenticated'] = true;
            header('Location: /dashboard');
        } else {
            http_response_code(401);
            echo 'Invalid credentials';
        }
        return;
    }

    if ($request->path() === '/dashboard') {
        session_start();
        $session = $request->session();

        if (!$session || !$session->get('authenticated')) {
            header('Location: /login');
            return;
        }

        echo 'Welcome, ' . htmlspecialchars($session->get('user'));
    }
});
```

---

## 参见

- [超全局变量](superglobals.md) —— OxPHP 如何填充 `$_SERVER`、`$_GET`、`$_POST`、`$_COOKIE` 和 `$_FILES`
- [PHP 函数](functions.md) —— `oxphp_http_request()`、`oxphp_superglobals_enabled()` 及所有其他内置函数的完整参考
- [Worker 模式](../features/worker-mode.md) —— 持久化 PHP 进程与请求生命周期
- [配置参考](../operations/configuration.md) —— `SUPERGLOBALS_ENABLED` 及其他环境变量
