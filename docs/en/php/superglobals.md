---
title: Superglobals
description: How OxPHP populates PHP superglobal arrays
---

OxPHP populates all standard PHP superglobals so that existing PHP code works without modification. Every value is set **before** `php_request_startup()` runs, so superglobals are available from the first line of your script --- including inside extensions like OPcache that read them during request initialization.

## `$_SERVER`

The custom SAPI registers a full set of CGI/1.1 variables through the `register_server_variables` callback. These variables are built from the HTTP request and injected into `$_SERVER` during PHP request startup.

### Standard CGI Variables

| Variable | Source | Example |
|----------|--------|---------|
| `REQUEST_METHOD` | HTTP method | `GET` |
| `REQUEST_URI` | Full URI with query string | `/app?page=2` |
| `QUERY_STRING` | Query portion of the URI | `page=2` |
| `SERVER_PROTOCOL` | Always `HTTP/1.1` | `HTTP/1.1` |
| `SCRIPT_NAME` | URI path without query string | `/app` |
| `PHP_SELF` | Same as `SCRIPT_NAME` | `/app` |
| `SCRIPT_FILENAME` | Absolute filesystem path to the script | `/var/www/html/public/index.php` |
| `DOCUMENT_ROOT` | Web root directory | `/var/www/html/public` |
| `SERVER_SOFTWARE` | Server identifier | `OxPHP/0.1.0` |
| `GATEWAY_INTERFACE` | CGI version | `CGI/1.1` |
| `REMOTE_ADDR` | Client IP address | `172.17.0.1` |
| `REMOTE_PORT` | Client port | `54321` |
| `SERVER_NAME` | From `Host` header (host part) | `example.com` |
| `SERVER_PORT` | From `Host` header (port part) | `8080` |
| `CONTENT_TYPE` | From `Content-Type` header | `application/json` |
| `CONTENT_LENGTH` | From `Content-Length` header | `128` |

When the `Host` header is missing, `SERVER_NAME` defaults to `localhost` and `SERVER_PORT` defaults to `80`.

### HTTP Request Headers

All HTTP request headers are added to `$_SERVER` with the `HTTP_` prefix and dashes converted to underscores, following standard CGI conventions:

```
Accept: text/html         → HTTP_ACCEPT = "text/html"
X-Forwarded-For: 1.2.3.4 → HTTP_X_FORWARDED_FOR = "1.2.3.4"
Authorization: Bearer ... → HTTP_AUTHORIZATION = "Bearer ..."
```

`Content-Type` and `Content-Length` are **not** prefixed with `HTTP_` --- they appear as `CONTENT_TYPE` and `CONTENT_LENGTH` directly, as required by the CGI specification.

### Usage Example

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$docroot = $_SERVER['DOCUMENT_ROOT'];

// Access custom headers
$token   = $_SERVER['HTTP_AUTHORIZATION'] ?? '';
$xff     = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'];
```

## `$_GET`

Query string parameters are parsed by PHP's standard engine from the `QUERY_STRING` server variable. OxPHP sets `QUERY_STRING` from the request URI, and PHP populates `$_GET` automatically during request startup.

```php
<?php
// Request: GET /search?q=oxphp&page=2
$query = $_GET['q'];     // "oxphp"
$page  = $_GET['page'];  // "2"
```

## `$_POST`

The SAPI provides a `read_post` callback that feeds the request body to PHP's standard POST parser. PHP handles both `application/x-www-form-urlencoded` and `multipart/form-data` content types. The body is read incrementally --- PHP calls the callback repeatedly until it returns 0 bytes.

```php
<?php
// Request: POST /login with application/x-www-form-urlencoded body
$username = $_POST['username'];
$password = $_POST['password'];
```

The raw request body is also available through `php://input`:

```php
<?php
$json = json_decode(file_get_contents('php://input'), true);
```

## `$_COOKIE`

The SAPI provides a `read_cookies` callback that returns the raw `Cookie` header string. PHP's engine parses it into the `$_COOKIE` array during request startup.

```php
<?php
// Request with Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

## `$_REQUEST`

`$_REQUEST` is populated by PHP based on the `request_order` INI directive (default: `"GP"` --- GET then POST). It merges `$_GET` and `$_POST` (and optionally `$_COOKIE`) in the configured order. OxPHP does not override this behavior.

## `$_FILES`

File uploads sent via `multipart/form-data` are parsed by PHP's standard `read_post` mechanism. The `$_FILES` array is populated automatically, including `name`, `type`, `tmp_name`, `error`, and `size` for each uploaded file.

```php
<?php
if ($_FILES['avatar']['error'] === UPLOAD_ERR_OK) {
    $tmp  = $_FILES['avatar']['tmp_name'];
    $name = $_FILES['avatar']['name'];
    move_uploaded_file($tmp, "/uploads/$name");
}
```

## Implementation Details

### Batch FFI Pattern

OxPHP builds all `$_SERVER` key-value pairs in a single Rust `Vec<(CString, CString)>` and stores them in a thread-local before PHP starts the request. During the `register_server_variables` callback, the entire vector is iterated in one pass, calling `php_register_variable_safe()` for each entry. This avoids per-variable FFI calls and keeps the overhead constant regardless of the number of headers.

### Data Lifetime Requirements

All per-request data --- server variables, the cookie string, and the request body --- must be stored in thread-local storage **before** `php_request_startup()` is called. These values must remain valid through `php_request_shutdown()` because PHP's engine holds raw pointers into them. OxPHP stores them in a `RequestData` struct inside a `thread_local! { RefCell }` and clears them only after the request is fully shut down.

### Thread-Local Bridge

The C bridge library (`liboxphp_bridge.so`) uses `__thread` TLS to share per-request context (request ID, worker ID, request time) between Rust and the PHP extension. Both the Rust binary and the PHP extension link against the same shared library, giving them access to the same thread-local storage. This is the only reliable mechanism for sharing state across `dlopen` boundaries.

## See Also

- [PHP Extension Functions](functions.md) --- built-in functions for accessing request IDs, worker info, and server metadata
- [OPcache Compatibility](opcache.md) --- how request time enables OPcache's `file_update_protection`
- [SAPI Bridge](/architecture/sapi-bridge.md) --- the C bridge library and thread-local storage mechanism
- [Request Lifecycle](/architecture/request-lifecycle.md) --- how request data flows from Rust to PHP
