---
title: Superglobals
description: How OxPHP populates $_SERVER, $_GET, $_POST, $_COOKIE, $_FILES, and php://input for each request.
---

# Superglobals

OxPHP populates all standard PHP superglobals before your script executes, matching the behavior PHP developers expect from traditional server setups. Every value is available from the first line of your code — no initialization required.

## $_SERVER

OxPHP builds `$_SERVER` from the incoming HTTP request following the CGI/1.1 specification. Process environment variables are imported first; CGI variables are set afterward, so request-specific values always override any colliding environment keys.

### Standard Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `SCRIPT_FILENAME` | Absolute filesystem path to the PHP script being executed | `/var/www/html/public/index.php` |
| `DOCUMENT_ROOT` | Web root directory configured via `DOCUMENT_ROOT` env var | `/var/www/html/public` |
| `SERVER_SOFTWARE` | Server identifier | `OxPHP/0.1.0` |
| `SERVER_PROTOCOL` | Always `HTTP/1.1` | `HTTP/1.1` |
| `REQUEST_METHOD` | HTTP method | `GET` |
| `REQUEST_URI` | Full URI with query string | `/app?page=2` |
| `SCRIPT_NAME` | URI path without query string | `/app` |
| `DOCUMENT_URI` | Alias for `SCRIPT_NAME`, for nginx/PHP-FPM compatibility | `/app` |
| `PHP_SELF` | Same as `SCRIPT_NAME` | `/app` |
| `QUERY_STRING` | Query portion of the URI (empty string when absent) | `page=2` |
| `SERVER_NAME` | Hostname from the `Host` header | `example.com` |
| `SERVER_PORT` | Port from the `Host` header | `8080` |
| `REMOTE_ADDR` | Client IP address | `172.17.0.1` |
| `REMOTE_PORT` | Client port number | `54321` |
| `HTTPS` | Set to `"on"` when the connection uses TLS; absent otherwise | `on` |
| `REQUEST_SCHEME` | `"https"` for TLS connections, `"http"` otherwise | `https` |
| `CONTENT_TYPE` | Value of the `Content-Type` header (no `HTTP_` prefix) | `application/json` |
| `CONTENT_LENGTH` | Value of the `Content-Length` header (no `HTTP_` prefix) | `128` |
| `REQUEST_TIME` | Unix timestamp (integer) when the request started | `1738800000` |
| `REQUEST_TIME_FLOAT` | Unix timestamp with microsecond precision | `1738800000.123456` |
| `GATEWAY_INTERFACE` | CGI version string | `CGI/1.1` |

When the `Host` header is absent, `SERVER_NAME` defaults to `localhost` and `SERVER_PORT` defaults to `80` (or `443` for TLS).

### HTTP Request Headers

All HTTP request headers are added to `$_SERVER` with an `HTTP_` prefix. Header names are converted to uppercase with dashes replaced by underscores, following CGI/1.1 conventions:

```text
Accept: text/html            -> HTTP_ACCEPT
X-Forwarded-For: 1.2.3.4    -> HTTP_X_FORWARDED_FOR
Authorization: Bearer abc    -> HTTP_AUTHORIZATION
Cookie: session=xyz          -> HTTP_COOKIE
```

> **Note:** `Content-Type` and `Content-Length` appear without the `HTTP_` prefix — as `CONTENT_TYPE` and `CONTENT_LENGTH` — as required by the CGI specification.

### Trace Context Variables

When distributed tracing is enabled, OxPHP adds trace context variables to `$_SERVER`:

| Variable | Description | Example |
|----------|-------------|---------|
| `OXPHP_TRACE_ID` | W3C trace ID for the current request | `4bf92f3577b34da6a3ce929d0e0e4736` |
| `OXPHP_SPAN_ID` | Span ID for the OxPHP server span | `00f067aa0ba902b7` |
| `OXPHP_PARENT_SPAN_ID` | Parent span ID from the upstream service (empty if root) | `b9c7c989f97918e1` |

These variables are only present when a valid `traceparent` header arrives or when OxPHP generates a new trace. If tracing is not configured, these keys are absent.

### Differences from PHP-FPM

The following variables behave differently compared to a standard PHP-FPM setup:

| Variable | Behavior |
|----------|----------|
| `SERVER_ADDR` | Not set. OxPHP does not populate the local server IP address. |
| `PATH_INFO` | Set automatically — see [PATH_INFO Behavior](#path_info-behavior) below. |
| `PATH_TRANSLATED` | Not set. |
| `PHP_AUTH_USER` / `PHP_AUTH_PW` / `AUTH_TYPE` | Not extracted from the `Authorization` header. Read `$_SERVER['HTTP_AUTHORIZATION']` directly. |
| `REDIRECT_STATUS` | Not set. OxPHP does not use an internal redirect mechanism. |

### Example

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$scheme  = $_SERVER['REQUEST_SCHEME'];  // "http" or "https"

// Read a custom header
$token = $_SERVER['HTTP_AUTHORIZATION'] ?? '';

// Prefer X-Forwarded-For when behind a trusted reverse proxy
$xff  = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'];

// Check TLS without checking the port
if (isset($_SERVER['HTTPS']) && $_SERVER['HTTPS'] === 'on') {
    // Secure connection
}
```

### PATH_INFO Behavior

`$_SERVER['PATH_INFO']` is populated automatically based on the active routing mode. There is no feature flag — the previous `SPLIT_PATH_INFO_ENABLED` env variable has been removed.

| Routing mode | When set | Value |
|---|---|---|
| **Traditional** (`INDEX_FILE` unset) | Only when the URI contains `.php/` and the script prefix exists on disk | Tail after the script segment |
| **Framework** (`INDEX_FILE=index.php`) | **Always** — every request is rewritten onto the front controller | Full original URI |
| **SPA** (`INDEX_FILE=index.html`) | Never — PHP only runs for exact `.php` files, no PATH_INFO |  — |

#### Traditional mode examples

OxPHP scans the URI left-to-right for the first `.php` segment that corresponds to an actual file on disk. Everything after it becomes `PATH_INFO`:

| Request URI | File on disk | `SCRIPT_NAME` | `PATH_INFO` | `PHP_SELF` |
|---|---|---|---|---|
| `/app.php/user/42` | `app.php` exists | `/app.php` | `/user/42` | `/app.php/user/42` |
| `/index.php/api/v2/users` | `index.php` exists | `/index.php` | `/api/v2/users` | `/index.php/api/v2/users` |
| `/app.php` | `app.php` exists | `/app.php` | *(absent)* | `/app.php` |
| `/missing.php/foo` | file not found | falls back to `/index.php` | — | depends on fallback |

#### Framework mode examples

Every non-static request is rewritten onto `index.php`, with `PATH_INFO` carrying the original URI. Your front controller reads `PATH_INFO` to dispatch routes.

| Request URI | `SCRIPT_NAME` | `PATH_INFO` |
|---|---|---|
| `/api/users` | `/index.php` | `/api/users` |
| `/about.php` | `/index.php` | `/about.php` |
| `/api.php/v1/users` | `/index.php` | `/api.php/v1/users` |
| `/` | `/index.php` | `/` |

> **Note:** `PATH_TRANSLATED` is not populated. It is rarely used in practice and is not set by nginx or PHP-FPM by default.

---

## $_GET

Query string parameters are parsed automatically from the request URI.

```php
<?php
// Request: GET /search?q=oxphp&page=2
$query = $_GET['q'];     // "oxphp"
$page  = $_GET['page'];  // "2"
```

Array syntax works as expected:

```php
<?php
// Request: GET /filter?tags[]=php&tags[]=async
$tags = $_GET['tags'];  // ["php", "async"]
```

---

## $_POST

OxPHP supports the two standard content types for form submissions:

- `application/x-www-form-urlencoded` — standard HTML form data
- `multipart/form-data` — file uploads combined with form fields

```php
<?php
// Request: POST /login
// Content-Type: application/x-www-form-urlencoded
// Body: username=admin&password=secret

$username = $_POST['username'];  // "admin"
$password = $_POST['password'];  // "secret"
```

For JSON or other content types, use `php://input` instead:

```php
<?php
// Request: POST /api/users
// Content-Type: application/json
// Body: {"name":"Alice","email":"alice@example.com"}

$data  = json_decode(file_get_contents('php://input'), true);
$name  = $data['name'];   // "Alice"
$email = $data['email'];  // "alice@example.com"
```

---

## $_COOKIE

Cookies are parsed from the `Cookie` request header.

```php
<?php
// Request with: Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

> **Note:** Cookies with the `__oxp_` prefix are reserved for internal OxPHP plugins. They are stripped from the `Cookie` header before it reaches PHP and will not appear in `$_COOKIE`.

---

## $_FILES

File uploads sent via `multipart/form-data` populate the `$_FILES` array with the standard PHP structure:

```php
<?php
// $_FILES['avatar'] structure:
// [
//     'name'     => 'photo.jpg',       // Original filename sent by the client
//     'type'     => 'image/jpeg',      // MIME type declared by the client
//     'tmp_name' => '/tmp/phpAb12Cd',  // Temporary file path on the server
//     'error'    => 0,                 // UPLOAD_ERR_OK (0 means no error)
//     'size'     => 204800,            // File size in bytes
// ]

if ($_FILES['avatar']['error'] === UPLOAD_ERR_OK) {
    $tmp  = $_FILES['avatar']['tmp_name'];
    $name = basename($_FILES['avatar']['name']);
    move_uploaded_file($tmp, "/uploads/$name");
}
```

---

## $_REQUEST

`$_REQUEST` is a merged array of `$_GET`, `$_POST`, and optionally `$_COOKIE`, built by PHP according to the `request_order` INI directive (default: `"GP"` — GET, then POST). OxPHP does not modify this behavior.

```php
<?php
// GET /form?action=preview with POST body: action=submit
$action = $_REQUEST['action'];  // "submit" (POST overrides GET with default order)
```

---

## php://input

The raw request body is available through the `php://input` stream. This is the standard way to read JSON payloads, XML, or any content type other than form submissions.

```php
<?php
$body = file_get_contents('php://input');
$data = json_decode($body, true);
```

`php://input` is rewindable and can be read multiple times within the same request.

> **Note:** `php://input` is empty for `multipart/form-data` requests. Use `$_POST` and `$_FILES` for those.

---

## Disabling Superglobals

Set `SUPERGLOBALS_ENABLED=false` to disable population of `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, and `$_SERVER`. When disabled, these arrays are empty. Use the [HTTP Request API](request-api.md) (`oxphp_http_request()`) to access request data instead.

```bash
SUPERGLOBALS_ENABLED=false   # superglobals are empty arrays
```

The following remain available regardless of this setting:

| What | Why |
|------|-----|
| `$_SESSION` | Managed by PHP's session module, not by the SAPI |
| `php://input` | A stream, not a superglobal |
| `header()`, `headers_list()`, etc. | SAPI functions, not superglobals |
| `session_start()` and other `session_*()` functions | Native PHP functions |
| `oxphp_http_request()` | Always available — the recommended alternative |

You can check the current setting at runtime:

```php
if (!oxphp_superglobals_enabled()) {
    $request = oxphp_http_request();
    $page = $request->query('page', 1);
}
```

---

## See Also

- [HTTP Request API](request-api.md) -- typed, lazy-loading request object as an alternative to superglobals
- [PHP Functions](functions.md) -- `oxphp_request_id()`, `oxphp_worker_id()`, and other extension functions
- [Worker Mode](../features/worker-mode.md) -- how superglobals are refreshed between worker requests
- [Configuration Reference](../operations/configuration.md) -- `DOCUMENT_ROOT` and other server configuration variables
