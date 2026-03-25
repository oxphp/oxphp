---
title: HTTP Request API
description: Object-oriented API for accessing HTTP request data in OxPHP, replacing PHP superglobals with a type-safe, lazy-loading interface.
---

# HTTP Request API

OxPHP provides an object-oriented API for accessing HTTP request data. Instead of reading `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, and `$_SERVER`, you call methods on a `Request` object that returns exactly what you ask for — no more, no less.

## Table of Contents

- [Overview](#overview)
- [Getting the Request Object](#getting-the-request-object)
- [RequestInterface Methods](#requestinterface-methods)
  - [URI and Method](#uri-and-method)
  - [Protocol](#protocol)
  - [Query Parameters](#query-parameters)
  - [Parsed Body](#parsed-body)
  - [Headers](#headers)
  - [Cookies](#cookies)
  - [Raw Body](#raw-body)
  - [File Uploads](#file-uploads)
  - [Client](#client)
  - [Timing](#timing)
  - [Attributes](#attributes)
  - [Session](#session)
- [SessionInterface](#sessioninterface)
- [UploadedFileInterface](#uploadedfileinterface)
- [AttributesInterface](#attributesinterface)
- [Exceptions](#exceptions)
- [SUPERGLOBALS_ENABLED](#superglobals_enabled)
- [Worker Mode](#worker-mode)
- [IDE Support](#ide-support)
- [Examples](#examples)

---

## Overview

`oxphp_http_request()` returns a read-only proxy to the HTTP request data stored in the current worker thread. Data is fetched lazily — a single method call like `$request->header('Accept')` goes directly to the Rust-side data structure and returns just that value. Full-array calls like `$request->headers()` build the array once and cache it in the PHP object for the duration of the request.

**Why use it instead of superglobals?**

- **JSON body parsing is built in.** `$request->payload()` parses `application/json`, `application/x-www-form-urlencoded`, and `multipart/form-data` without extra code.
- **No array key typos.** `$request->method()` is harder to mistype than `$_SERVER['REQUEST_METHOD']`.
- **Type-detected file uploads.** `$request->file('avatar')->type()` returns the MIME type determined from the file's actual contents, not the client-supplied value.
- **Testable.** Because behavior is defined by interfaces, you can inject mock implementations in unit tests.
- **Superglobals stay available.** Setting `SUPERGLOBALS_ENABLED=false` is optional. The object API works regardless.

---

## Getting the Request Object

```php
<?php
$request = oxphp_http_request();
```

Call `oxphp_http_request()` anywhere in a script executing inside an active HTTP request. In worker mode, you can also receive the request as a parameter to the `oxphp_worker()` callback:

```php
<?php
oxphp_worker(function (\OxPHP\Http\RequestInterface $request) {
    $method = $request->method();
    // ...
});
```

Both styles return the same object — the parameter form is a convenience shorthand.

---

## RequestInterface Methods

### URI and Method

```php
$request->method(): string
```

Returns the HTTP method in uppercase: `"GET"`, `"POST"`, `"PUT"`, `"PATCH"`, `"DELETE"`, etc.

```php
$request->isMethod(string $method): bool
```

Case-insensitive method check.

```php
$request->path(): string
```

The URI path without the query string: `"/users/42"`.

```php
$request->fullUri(): string
```

The complete URI including scheme, host, optional non-standard port, path, and query string: `"https://example.com:8080/users/42?page=2"`. Standard ports (80 for HTTP, 443 for HTTPS) are omitted.

```php
$request->scheme(): string
```

`"https"` or `"http"`.

```php
$request->isSecure(): bool
```

`true` when the scheme is `"https"`.

```php
$request->host(): string
```

Hostname from the `Host` header. Returns an empty string when the header is absent (HTTP/1.0 requests without a `Host` header).

```php
$request->port(): int
```

Port from the `Host` header. When not explicitly present, returns the default for the scheme: `80` for HTTP, `443` for HTTPS.

```php
$request->queryString(): ?string
```

The raw query string without the leading `?`. Returns `null` when there is no query string.

---

### Protocol

```php
$request->httpProtocol(): string
```

The full protocol string: `"HTTP/1.1"` or `"HTTP/2"`.

```php
$request->httpProtocolVersion(): string
```

The version number only: `"1.1"` or `"2"`.

---

### Query Parameters

```php
$request->query(?string $key = null, mixed $default = null): mixed
```

Access query string parameters.

| Call | Returns |
|------|---------|
| `$request->query()` | All parameters as an array, including nested arrays |
| `$request->query('page')` | The value of `page`, or `null` if absent |
| `$request->query('page', 1)` | The value of `page`, or `1` if absent |

Bracket notation (`?tags[]=php&tags[]=async`) is parsed into nested arrays:

```php
// Request: GET /search?q=oxphp&tags[]=php&tags[]=async
$q    = $request->query('q');      // "oxphp"
$tags = $request->query('tags');   // ["php", "async"]
$all  = $request->query();         // ["q" => "oxphp", "tags" => ["php", "async"]]
```

Found values are always strings. `$default` is returned as-is when the key is absent.

---

### Parsed Body

```php
$request->payload(?string $key = null, mixed $default = null): mixed
```

Returns the parsed request body. The body is parsed according to the `Content-Type` header:

| Content-Type | Returns |
|---|---|
| `application/x-www-form-urlencoded` | Associative array of field values |
| `multipart/form-data` | Associative array of text field values |
| `application/json` | Decoded array or scalar; `null` for invalid JSON |
| Any other value | `null` |

`payload()` is not limited to POST requests — it works with PUT, PATCH, and any other method that sends a body. The parsed result is cached on the first call and reused for the lifetime of the request.

| Call | Returns |
|------|---------|
| `$request->payload()` | The entire parsed body |
| `$request->payload('email')` | A single field value, or `null` if absent |
| `$request->payload('email', '')` | A single field value, or `''` if absent |

```php
<?php
// JSON request: POST /api/users
// Content-Type: application/json
// Body: {"name": "Alice", "role": "admin"}

$name = $request->payload('name');  // "Alice"
$role = $request->payload('role');  // "admin"
$data = $request->payload();        // ["name" => "Alice", "role" => "admin"]
```

---

### Headers

```php
$request->header(string $name, ?string $default = null): ?string
```

Returns the raw header value. Header names are case-insensitive. For multi-value headers (`Accept`, `X-Forwarded-For`), the full header line is returned as a single string — parsing is your responsibility.

```php
$request->hasHeader(string $name): bool
```

Returns `true` if the named header is present.

```php
$request->headers(): array
```

Returns all headers as an associative array. Each key is the header name as received (no normalization), and each value is the raw header string.

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

### Cookies

```php
$request->cookie(string $name, ?string $default = null): ?string
```

Returns the value of a single cookie, or `$default` if the cookie is not present.

```php
$request->cookies(): array
```

Returns all cookies as an associative array of name-value pairs.

```php
<?php
$theme   = $request->cookie('theme', 'light');   // "dark" or "light"
$session = $request->cookie('session');           // null if absent
$all     = $request->cookies();                   // ["theme" => "dark", ...]
```

---

### Raw Body

```php
$request->body(): string
```

Returns the raw request body bytes. This is the OxPHP equivalent of `file_get_contents('php://input')`. Unlike `payload()`, `body()` is not cached — each call goes to the underlying data structure.

```php
$request->contentType(): ?string
```

Returns the value of the `Content-Type` header, or `null` if absent.

```php
<?php
// Read raw body for signature verification
$raw       = $request->body();
$signature = $request->header('X-Hub-Signature-256');
$valid     = hash_hmac('sha256', $raw, $secret) === $signature;
```

`body()` and `payload()` are independent. You can call both in the same request.

---

### File Uploads

```php
$request->file(string $name): ?UploadedFileInterface
```

Returns the uploaded file for the given field name, or `null` if the field is not present. For array fields (`name="photos[]"`), returns the first file.

```php
$request->files(?string $name = null): array
```

| Call | Returns |
|------|---------|
| `$request->files()` | All uploaded files as a flat array of `UploadedFileInterface` |
| `$request->files('photos')` | All files for the `photos` field (supports `name="photos[]"`) |

```php
<?php
$avatar = $request->file('avatar');

if ($avatar && $avatar->isValid()) {
    $mime = $avatar->type();    // Detected from file contents, not the client claim
    $name = $avatar->name();    // Original filename
    $avatar->moveTo('/var/uploads/' . basename($name));
}

// Multiple files
$photos = $request->files('photos');  // UploadedFileInterface[]
foreach ($photos as $photo) {
    if ($photo->isValid()) {
        $photo->moveTo('/var/uploads/' . basename($photo->name()));
    }
}
```

---

### Client

```php
$request->ip(): string
```

Returns the client IP address (`REMOTE_ADDR`). When your application sits behind a reverse proxy, read the `X-Forwarded-For` header directly from `$request->header('X-Forwarded-For')` and apply your own trust logic.

---

### Timing

```php
$request->startTime(bool $asFloat = false): int|float
```

Returns the Unix timestamp for when this request was received.

| Call | Returns |
|------|---------|
| `$request->startTime()` | Integer seconds: `1711234567` |
| `$request->startTime(true)` | Float with sub-second precision: `1711234567.3412` |

```php
<?php
$elapsed = microtime(true) - $request->startTime(true);
error_log(sprintf("Request took %.3fs so far", $elapsed));
```

---

### Attributes

```php
$request->attributes(): AttributesInterface
```

Returns the mutable attributes container for the current request. Use attributes to share data between middleware, route handlers, and other code in the same request without using global variables.

```php
<?php
// In authentication middleware
$request->attributes()->set('user', $authenticatedUser);

// In the route handler
$user = $request->attributes()->get('user');
```

Attributes are per-request and are reset with each new request in worker mode. When using Fibers, attributes are shared across all Fibers running on the same worker thread for the same request — but because PHP Fibers are cooperative, concurrent access is not possible.

---

### Session

```php
$request->session(): ?SessionInterface
```

Returns a read-only view of `$_SESSION`. Returns `null` if `session_start()` has not been called. Session management (starting, saving, destroying, writing values) uses the standard PHP session functions.

```php
<?php
session_start();
$session = $request->session();

$userId  = $session->get('user_id');
$isAdmin = $session->get('is_admin', false);

// Write session data using standard PHP functions
$_SESSION['last_seen'] = time();
```

---

## SessionInterface

`SessionInterface` is a read-only view of the active session.

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

| Method | Description |
|--------|-------------|
| `id()` | The session ID |
| `name()` | The session name (default: `"PHPSESSID"`) |
| `get(key, default)` | A single session value, or `$default` if the key is absent |
| `has(key)` | `true` if the key exists in `$_SESSION` |
| `all()` | All session data as an array |

Session values reflect the current state of `$_SESSION` at the time of the call, not the state when `session()` was first called.

---

## UploadedFileInterface

`UploadedFileInterface` represents a single uploaded file.

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

| Method | Description |
|--------|-------------|
| `name()` | Original filename sent by the client |
| `clientType()` | MIME type declared by the client — do not trust this value for security decisions |
| `type()` | MIME type determined from the file's actual contents using magic byte detection. Returns `"application/octet-stream"` when the type cannot be determined. Cached on first call. |
| `size()` | File size in bytes |
| `tmpPath()` | Path to the temporary file on disk |
| `error()` | One of the `UPLOAD_ERR_*` constants |
| `isValid()` | `true` when `error()` is `UPLOAD_ERR_OK` |
| `moveTo(path)` | Moves the file to `$path`. Calls `type()` before moving. Returns `false` if the file is invalid or the move fails. |

Always check `isValid()` before using an uploaded file. Use `type()` rather than `clientType()` when making security-sensitive decisions:

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

`AttributesInterface` is the only mutable part of the request object. It is intended for storing per-request metadata — authenticated user, resolved route parameters, locale, feature flags — that multiple parts of the application need to access.

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

| Method | Description |
|--------|-------------|
| `get(key, default)` | Returns the value for `$key`, or `$default` if absent |
| `set(key, value)` | Stores a value |
| `has(key)` | `true` if the key has been set |
| `remove(key)` | Removes the key |
| `all()` | All attributes as an associative array |

---

## Exceptions

Calling `oxphp_http_request()` outside an active request context throws an exception from the `OxPHP\Http\Exception` namespace.

```php
namespace OxPHP\Http\Exception;

class NoActiveRequestException extends \RuntimeException {}
class AsyncContextException extends NoActiveRequestException {}
class WorkerIdleException extends NoActiveRequestException {}
```

| Exception | When thrown |
|-----------|-------------|
| `NoActiveRequestException` | No active HTTP request: CLI, MINIT, after shutdown, or a Fiber that outlives its request |
| `AsyncContextException` | Inside an `oxphp_async()` callback — async workers run on separate threads with no request context |
| `WorkerIdleException` | Worker mode, between requests — the worker is waiting for the next request |

`AsyncContextException` and `WorkerIdleException` both extend `NoActiveRequestException`, so catching the base class handles all cases.

```php
<?php
try {
    $request = oxphp_http_request();
} catch (\OxPHP\Http\Exception\AsyncContextException $e) {
    // Inside oxphp_async() — no request context here
} catch (\OxPHP\Http\Exception\WorkerIdleException $e) {
    // Worker is between requests — do not call oxphp_http_request() here
} catch (\OxPHP\Http\Exception\NoActiveRequestException $e) {
    // Any other case with no active request
}
```

In normal request-handling code, you do not need this try/catch. The exception guard is useful in bootstrap code that might run outside a request context.

---

## SUPERGLOBALS_ENABLED

```bash
SUPERGLOBALS_ENABLED=true    # default — full backward compatibility
SUPERGLOBALS_ENABLED=false   # superglobals are empty arrays
```

By default, OxPHP populates `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, and `$_SERVER` as usual. The HTTP Object API is available alongside superglobals in this mode.

Setting `SUPERGLOBALS_ENABLED=false` makes those arrays empty, which eliminates the cost of building them for every request. The following still work regardless of this setting:

| Feature | Behavior with `SUPERGLOBALS_ENABLED=false` |
|---------|-------------------------------------------|
| `oxphp_http_request()` | Always available |
| `php://input` | Available (it is a stream, not a superglobal) |
| `$_SESSION` | Available (managed by PHP's session module) |
| `header()`, `headers_list()` | Available (SAPI output functions) |
| `session_start()`, `session_*()` | Available (native PHP functions) |
| `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_SERVER` | Empty arrays |

Use `oxphp_superglobals_enabled()` to check the current setting at runtime:

```php
<?php
if (!oxphp_superglobals_enabled()) {
    $method = oxphp_http_request()->method();
} else {
    $method = $_SERVER['REQUEST_METHOD'];
}
```

---

## Worker Mode

In worker mode, a new Request object is created for each incoming request. The previous request's object becomes invalid once the request completes — do not store a reference to it between requests.

```php
<?php
// worker.php

require __DIR__ . '/vendor/autoload.php';
$app = new MyApp\Application();

oxphp_worker(function (\OxPHP\Http\RequestInterface $request) use ($app) {
    $app->handle($request);
});
```

The Request object passed to the callback is identical to calling `oxphp_http_request()` inside the callback — both read from the same thread-local state.

All caches on the Request object (parsed headers, cookies, query parameters, payload) are cleared automatically when the next request begins.

---

## IDE Support

Install the stub package to get autocompletion and type checking in PhpStorm, VS Code, or any LSP-aware editor:

```bash
composer require --dev oxphp/stubs
```

The stubs package provides:

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

No runtime dependency is added. The package is `require-dev` only.

---

## Examples

### Traditional Mode

```php
<?php
$request = oxphp_http_request();

$method = $request->method();         // "GET"
$path   = $request->path();           // "/api/articles"
$page   = $request->query('page', 1); // "2" or 1 (default)

// Authorization header
if (!$request->hasHeader('Authorization')) {
    http_response_code(401);
    echo json_encode(['error' => 'Unauthorized']);
    exit;
}

$token = $request->header('Authorization');

// Structured logging with request metadata
error_log(sprintf(
    '[%s] %s %s from %s',
    oxphp_request_id(),
    $method,
    $path,
    $request->ip()
));
```

### POST with JSON Body

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

// payload() handles JSON, form-urlencoded, and multipart
// No manual json_decode() or $_POST check needed
$user = authenticate($email, $password);

header('Content-Type: application/json');
echo json_encode(['token' => $user->generateToken()]);
```

### Middleware Attributes

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

### Worker Mode with Session

```php
<?php
// worker.php
require __DIR__ . '/vendor/autoload.php';

oxphp_worker(function (\OxPHP\Http\RequestInterface $request) {
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

## See Also

- [Superglobals](superglobals.md) — how OxPHP populates `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, and `$_FILES`
- [PHP Functions](functions.md) — full reference for `oxphp_http_request()`, `oxphp_superglobals_enabled()`, and all other built-in functions
- [Worker Mode](../features/worker-mode.md) — persistent PHP processes and the request lifecycle
- [Configuration Reference](../operations/configuration.md) — `SUPERGLOBALS_ENABLED` and other environment variables
