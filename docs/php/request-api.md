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

`oxphp_http_request()` returns a read-only proxy to the HTTP request data stored in the current worker thread. Data is fetched lazily — a single method call like `$request->header('Accept')` goes directly to the Rust-side data structure and returns just that value. Of the full-array calls, `query()` and `payload()` keep their parsed result on the object they were called on; `headers()`, `cookies()` and `files()` build theirs afresh every time. Nothing is kept for the request: each call to `oxphp_http_request()` returns a new object, with a new set of those caches.

**Why use it instead of superglobals?**

- **JSON body parsing is built in.** `$request->payload()` hands back a decoded `application/json` body whatever the request method is, and the fields of a `POST` form body — `application/x-www-form-urlencoded` or `multipart/form-data` — without extra code. The exact returns are in [Parsed Body](#parsed-body).
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

Call `oxphp_http_request()` anywhere in a script executing inside an active HTTP request, including inside the `oxphp_worker()` callback:

```php
<?php
oxphp_worker(function () {
    $request = oxphp_http_request();
    $method = $request->method();
    // ...
});
```

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

> **Behind a reverse proxy:** `scheme()`, `isSecure()`, `host()`, and `port()` honor `X-Forwarded-Proto` and `X-Forwarded-Host` when `TRUSTED_PROXIES` includes the peer. Without trusted proxies they reflect the direct connection.

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
| `$request->query()` | All parameters as a flat array of name-value pairs |
| `$request->query('page')` | The value of `page`, or `null` if absent |
| `$request->query('page', 1)` | The value of `page`, or `1` if absent |

> **Known gap:** bracket notation is not expanded into nested arrays yet. `?tags[]=php&tags[]=async` currently yields a flat entry named `tags[]` rather than a `tags` array, and a repeated name resolves to its first occurrence through `query('name')` while `query()` keeps the last. Use `$_GET` for array parameters until this is fixed.

```php
// Request: GET /search?q=oxphp&tags[]=php&tags[]=async
$q    = $request->query('q');      // "oxphp"
$tags = $_GET['tags'];             // ["php", "async"]
```

Found values are always strings. `$default` is returned as-is when the key is absent. A parameter sent without a value (`?flag`) and one sent empty (`?flag=`) both read as `''` — only an absent parameter gives `null`.

#### Decoding

Names and values are percent-decoded, and `+` is read as a space — the same rules PHP applies when building `$_GET`:

```php
// Request: GET /search?q=%D0%9F%D1%80%D0%B8%D0%B2%D0%B5%D1%82&tag=a+b
$request->query('q');    // "Привет"
$request->query('tag');  // "a b"
```

Values are byte strings, so an escape encoding something that is not valid UTF-8 survives intact — a signed token or a binary id can still be verified against the bytes as sent. `queryString()` remains the way to read the query string exactly as it arrived, undecoded.

Names are reported as the client sent them, where `$_GET` reports them as PHP files them. PHP rewrites `' '`, `'.'` and `'['` in superglobal names, so `?a.b=1` lands in `$_GET` as `a_b`:

```php
// Request: GET /?a.b=1
$_GET['a_b'];            // "1"
$request->query('a.b');  // "1"
```

A name is also kept whole where PHP would cut it — PHP handles superglobal names as C strings, so `?a%00b=1` becomes `$_GET['a']` while `query()` keeps `a\0b`. Do not read that as a reason to validate through one API and read through the other: the truncation lets one parameter land on another's key, so `?a=1&a%00x=2` gives `$_GET['a'] === '2'` where `query('a') === '1'`, and the two disagree on the value rather than merely on the set of names.

At most 1000 parameters are parsed. PHP applies its own limit, `max_input_vars`, which defaults to the same 1000 but is configurable — raise it and `$_GET` will hold parameters past the point where `query()` stops.

---

### Parsed Body

```php
$request->payload(?string $key = null, mixed $default = null): mixed
```

Returns the parsed request body. The body is parsed according to the `Content-Type` header:

| Content-Type | Returns |
|---|---|
| `application/x-www-form-urlencoded` | Associative array of field values, on a POST request; an empty array on any other method |
| `multipart/form-data` | Associative array of text field values, on a POST request; an empty array on any other method |
| `application/json` | The decoded value — an array for an object or an array, otherwise the scalar itself (`string`, `int`, `float`, `bool`); `null` for invalid JSON and for a literal `null` body |
| Any other value | `null`. Matching is by prefix, so anything beginning `application/json` — `application/json-patch+json`, for one — takes the JSON row above, while `text/json` and `application/vnd.api+json` land here |

A JSON body is decoded whatever the request method is. A form body is not: the two form rows read what PHP parsed the body into, and PHP does that for POST alone, so `payload()` on a `PUT` or `PATCH` carrying form fields returns an empty array. Send such bodies as JSON, or read them from [`body()`](#raw-body) and parse them yourself. Both form rows also give you the body as it arrived rather than as your code left it: writing to `$_POST` later separates a copy for the script, and `payload()` goes on reading the array PHP parsed out of the body.

The parsed result is cached on the first call and reused by later calls on the same object. A second `oxphp_http_request()` is a second object, with a cache of its own — it builds the result again.

| Call | Returns |
|------|---------|
| `$request->payload()` | The entire parsed body |
| `$request->payload(null, [])` | The entire parsed body, or `[]` when there is none — an empty body, a `Content-Type` from the last row above, invalid JSON, or a literal JSON `null` |
| `$request->payload('email')` | A single field value, or `null` if absent |
| `$request->payload('email', '')` | A single field value, or `''` if absent |

A key lookup only reaches into an array payload. On a JSON body that decoded to a scalar there are no keys, so `payload('anything')` returns the default. A body of `false` or `0` is a value the default does not stand in for.

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

Returns the value of a single cookie, or `$default` if the cookie is not present. A cookie sent empty (`Cookie: a=`) reads as `''`; only an absent cookie gives `$default`.

```php
$request->cookies(): array
```

Returns all cookies as an associative array of name-value pairs.

#### Decoding

Cookies decode differently from query parameters, following what PHP does when building `$_COOKIE`: values are percent-decoded, but `+` stays a literal `+`, and names are not decoded at all.

```php
// Cookie: token=a%20b; sep=a+b; %D0%BA=1
$request->cookie('token');  // "a b"   — percent-escapes decoded
$request->cookie('sep');    // "a+b"   — unlike query(), "+" is not a space
$request->cookie('%D0%BA'); // "1"     — look up by the name as sent
```

Names are reported as sent here too. PHP rewrites `' '`, `'.'` and `'['` in every superglobal name, cookies included, so `Cookie: a.b=1` lands in `$_COOKIE` as `a_b` while `cookie('a.b')` and `cookies()['a.b']` use the name the client sent.

Values are byte strings, so a signed cookie or `setcookie('sid', random_bytes(16))` round-trips as the exact bytes it was set with.

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

Returns the client IP address. When `TRUSTED_PROXIES` is configured and the request peer is in the trusted set, this is the rightmost untrusted address from `X-Forwarded-For` or RFC 7239 `Forwarded`. Otherwise it is the direct peer IP — typically your load balancer, not the end client.

The raw `X-Forwarded-For` header remains available via `$request->header('X-Forwarded-For')` for advanced cases, but parsing it manually is rarely correct (leftmost vs rightmost, no CIDR trust check). Configure `TRUSTED_PROXIES` instead — see [Trusted Proxies](../security/trusted-proxies.md).

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

Returns the mutable attributes container held by this `Request` object. Use attributes to hand data from middleware to route handlers and to whatever else receives the same object, instead of reaching for a global variable.

```php
<?php
// In authentication middleware
$request->attributes()->set('user', $authenticatedUser);

// In the route handler
$user = $request->attributes()->get('user');
```

The container belongs to the object, not to the request. `oxphp_http_request()` returns a new `Request` on every call, and the container you take from it is a new, empty one, so a write made through one call is not readable through another — the default comes back instead, with no warning and no log line:

```php
<?php
oxphp_http_request()->attributes()->set('tenant_id', $id);
oxphp_http_request()->attributes()->get('tenant_id');   // null — a second Request, a second container
```

Pass the object along, as `$request` is in the first of the two examples above, and everything that holds it reads and writes the one container — including a Fiber you hand it to. A Fiber that calls `oxphp_http_request()` for itself gets a container of its own, like any other caller. Because PHP Fibers are cooperative, no two of them are ever inside a container at the same moment.

Nothing is reset between requests, because there is nothing to reset: the container goes when the object holding it does, and the next request's handler calls `oxphp_http_request()` and gets one of its own.

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
    public function moveTo(string $destination): bool;
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

`AttributesInterface` is the only mutable part of the request object. It is intended for metadata your own code derives — authenticated user, resolved route parameters, locale, feature flags — and reads again further along the same call chain. How far it reaches is how far the `Request` object holding it is passed; see [Attributes](#attributes).

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
SUPERGLOBALS_ENABLED=false   # the $_SERVER request keys and $_GET are not built
```

By default, OxPHP populates `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, and `$_SERVER` as usual. The HTTP Object API is available alongside superglobals in this mode.

Setting `SUPERGLOBALS_ENABLED=false` skips the work of describing the request to PHP: the CGI and `HTTP_*` variables are not registered, and the query string is not handed over for PHP to parse. What that leaves is narrower than the name suggests — the request body and the `Cookie` header reach PHP either way, and PHP builds the arrays it makes out of them regardless of this setting:

| Feature | Behavior with `SUPERGLOBALS_ENABLED=false` |
|---------|-------------------------------------------|
| `oxphp_http_request()` | Available — every method answers, including `query()` and `payload()` |
| `php://input` | Available (it is a stream, not a superglobal) |
| `$_SESSION` | Available (managed by PHP's session module) |
| `header()`, `headers_list()` | Available (SAPI output functions) |
| `session_start()`, `session_*()` | Available (native PHP functions) |
| `$_POST`, `$_FILES`, `$_COOKIE` | **Populated as usual** — they are built from the body and the `Cookie` header, which this setting does not touch |
| `$_REQUEST` | Populated — PHP merges it per `request_order` as always; only its `$_GET` half is missing |
| `$_GET` | Empty |
| `$_SERVER` | During a request, four keys and no more: `REQUEST_TIME`, `REQUEST_TIME_FLOAT`, `argc` and `argv`, every one of them registered by PHP itself rather than by the server. No `REQUEST_METHOD`, no `REQUEST_URI`, no `HTTP_*`, no process environment. **Worker mode bootstraps differently:** the code above `oxphp_worker()` runs before any request and sees a `$_SERVER` built as though the setting were on — the whole process environment included, plus a placeholder `REQUEST_URI` of `/` |

So this setting is not a way to keep request data out of PHP's globals, and it is not a way to stop a form body being parsed. The work it skips is the per-request `$_SERVER` fold — the CGI and `HTTP_*` variables plus the process environment — and PHP's parse of the query string into `$_GET`.

Code that routes on `$_SERVER['REQUEST_URI']` finds nothing there under this setting and has to read `oxphp_http_request()->path()` instead. In a worker entry script that applies to the handler; the bootstrap above it sees the placeholder `/` described in the table, which is not a route either.

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

In worker mode, `oxphp_http_request()` builds a new `Request` every time it is called — once per call, not once per request. Call it inside the handler and pass the object where it is needed, as the example below does.

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

Do not keep an object past the handler that built it. One held across requests is not invalidated, and nothing reports it as stale: `method()`, `path()`, `header()` and the other uncached readers go on answering out of whatever request the worker thread is serving at the moment you call them, while what the object cached — the `query()` array, the parsed `payload()`, the attributes container — is whatever each of them took on its own first call, which for one never made before is this request's. Nothing on a `Request` is cleared when the next request begins — what the soft reset between requests does cover is listed under [What Gets Reset Between Requests](../features/worker-mode.md#what-gets-reset-between-requests).

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

oxphp_worker(function () {
    $request = oxphp_http_request();

    if ($request->path() === '/login' && $request->isMethod('POST')) {
        $username = $request->payload('username');
        $password = $request->payload('password');

        if (verify_credentials($username, $password)) {
            session_start();
            $_SESSION['user'] = $username;
            $_SESSION['authenticated'] = true;
            session_write_close();
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
            session_write_close();
            header('Location: /login');
            return;
        }

        echo 'Welcome, ' . htmlspecialchars($session->get('user'));
        session_write_close();
    }
});
```

`session_write_close()` on every path out is what makes the session a request's own rather than the worker's. A worker has no end-of-request to close a session at, so one left open is closed only when that worker takes its next request — and until then the save handler is still holding whatever it locked. Closing it yourself also puts the write where your application put it. It is not a separation between requests that **overlap**, though: a worker admits new requests whenever the one it is running suspends, and a session still open at a suspension point is handed to whatever is admitted in that window. See [Worker Mode](../features/worker-mode.md#what-gets-reset-between-requests).

---

## See Also

- [Superglobals](superglobals.md) — how OxPHP populates `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, and `$_FILES`
- [PHP Functions](functions.md) — full reference for `oxphp_http_request()`, `oxphp_superglobals_enabled()`, and all other built-in functions
- [Worker Mode](../features/worker-mode.md) — persistent PHP processes and the request lifecycle
- [Configuration Reference](../operations/configuration.md) — `SUPERGLOBALS_ENABLED` and other environment variables
