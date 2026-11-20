---
title: Superglobals
description: How OxPHP populates $_SERVER, $_GET, $_POST, $_COOKIE, $_FILES, and php://input for each request.
---

# Superglobals

OxPHP populates all standard PHP superglobals before your script executes, matching the behavior PHP developers expect from traditional server setups. Every value is available from the first line of your code — no initialization required. One setting narrows that, and only that one: see [Disabling Superglobals](#disabling-superglobals).

## $_SERVER

OxPHP builds `$_SERVER` from the incoming request following the CGI/1.1 specification, on top of the process environment. Every key it sets, how request headers map to `HTTP_*` keys, the values behind a reverse proxy, `PATH_INFO`, and the differences from PHP-FPM are listed in [Server Variables](server-variables.md).

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

> **What `filter_input(INPUT_GET, …)` sees.** `filter_input()`, `filter_input_array()` and `filter_has_var()` do not read `$_GET`, `$_POST` or `$_COOKIE`. The filter extension keeps a copy of the parsed input of its own, and in a persistent worker that copy is filled by every request — so it has to be given back by every request too, or one client's query values, session cookies and body fields stay readable to every request the worker serves afterwards. OxPHP gives it back at the start of each request, so these three functions answer for the request asking and for no other.
>
> Worker mode adds one limit. If your request pauses — `sleep()`, an `await`, a hooked call — and another request runs on that worker in the window, the storage becomes that request's; reads after the resume answer `null` rather than someone else's input, and the copy your request had is gone by then. It is enough for the other request to start or to resume there — it need not finish. `$_GET`, `$_POST` and `$_COOKIE` travel with the request across a suspension and are unaffected, so read those, or read the filter functions before you suspend.
>
> One call is not allowed to pause at all. `filter_input_array()` given a per-field definition array reads the storage once per field, so a `FILTER_CALLBACK` that does I/O runs with the worker held for its duration instead of handing it to another request, and an explicit `Fiber::suspend()` inside such a callback throws. The wait still happens — it is the worker thread that waits rather than the request — so a slow callback shows up as latency for everything queued behind it, and past their wait budget those queued requests are shed. How long it can last is set by whatever the callback is talking to and not by the server: a stream wrapper gives up after `default_socket_timeout`, 60 seconds out of the box, while mysqlnd waits `mysqlnd.net_read_timeout`, a day out of the box. Give such a call an explicit timeout — or, better, run validation that reaches a database or a cache on the value after `filter_input_array()` has returned it.

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

> **`filter_input(INPUT_POST, …)`** reads the filter extension's own copy of the body rather than `$_POST` — see the note under [`$_GET`](#_get) for what that copy is and how long it lasts.

---

## $_COOKIE

Cookies are parsed from the `Cookie` request header.

```php
<?php
// Request with: Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

> **Note:** Cookies with the `__oxp_` prefix are reserved for internal OxPHP plugins. They are stripped from the `Cookie` header before it reaches PHP and will not appear in `$_COOKIE`. So are cookies whose name PHP would register under that prefix, such as `__oxp.x`: PHP turns a space, a dot or an unmatched `[` in a cookie name into `_`. Every `[` counts here, so a name PHP would make an array, such as `__oxp[a]` (`$_COOKIE['__oxp']['a']`) or `_[oxp_t]`, is stripped as well.

> **Note:** A `Cookie` header holding a byte outside ASCII is dropped as a whole, with no log message about it. `$_COOKIE` is empty, `$_SERVER['HTTP_COOKIE']` is not set, and the request object finds no cookies and no `cookie` header. Plugins see no cookies on such a request either.

> **`filter_input(INPUT_COOKIE, …)`** reads the filter extension's own copy of the cookies rather than `$_COOKIE` — see the note under [`$_GET`](#_get) for what that copy is and how long it lasts.

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

`$_REQUEST` is a merged array of `$_GET`, `$_POST` and `$_COOKIE`, built by PHP according to the `request_order` INI directive, or `variables_order` when `request_order` is not set. The merge itself follows PHP's rules unchanged: the sources are merged in the order the directive lists them, and a later source overwrites a key an earlier one set.

**Cookies are included, and they win.** The OxPHP images load no `php.ini` — `php.ini-production` and `php.ini-development` sit in `/usr/local/etc/php` unused (see [Installation](../getting-started/installation.md)) — so `request_order` is unset and PHP falls back to its built-in `variables_order`, `"EGPCS"`: GET, then POST, then COOKIE. A cookie therefore replaces a query or body value of the same name; when both sides are arrays (`k[a]` in the body, `k[b]` in a cookie) they are merged key by key under the same rule. The `request_order = "GP"` you may know from PHP-FPM setups comes from the `php.ini-production` and `php.ini-development` samples, not from PHP itself. To leave cookies out, set it in an ini file loaded at startup, for example `/usr/local/etc/php/conf.d/zz-request-order.ini` containing `request_order = "GP"`. `ini_set()` cannot change this directive at runtime.

```php
<?php
// POST /form?action=preview with body action=submit
$action = $_REQUEST['action'];  // "submit" — POST overrides GET

// Same request, also carrying the header "Cookie: action=delete"
$action = $_REQUEST['action'];  // "delete" — the cookie overrides both
```

> **Worker mode:** `$_REQUEST` is rebuilt for every request. PHP normally builds it lazily, once, when a script that mentions it is first loaded — which in a persistent worker would mean every later request reading the first one's parameters. OxPHP forces the rebuild instead, so the merged array always describes the request being served.

---

## $_ENV

`$_ENV` holds the process environment. In traditional mode PHP repopulates it from the environment on every request, subject to `variables_order`, as under PHP-FPM — but the environment is the whole server process's, which under PHP-FPM it is not; see `putenv()` at the end of this section.

**Worker mode pins it.** A worker bootstraps once, and `.env` loaders (vlucas/phpdotenv, symfony/dotenv, Laravel's `Env`) write their values straight into `$_ENV`; by default only Laravel's also writes them into the process environment, with `putenv()`. Repopulating `$_ENV` per request would therefore erase the application's configuration from its second request onward, so in worker mode the array is kept for the life of the worker once it exists — the writes your bootstrap makes there stay visible to every request that worker serves. So does any other write into it — the note below the example has what that means inside a request handler.

```php
<?php
// bootstrap, before oxphp_worker()
Dotenv\Dotenv::createImmutable(__DIR__)->load();   // writes into $_ENV

oxphp_worker(function () {
    echo $_ENV['DATABASE_URL'];  // still there on request 10_000
});
```

> **In worker mode `$_ENV` is worker state, not request state.** Pinning the array is the whole of the mechanism, so it cannot tell a bootstrap write from a handler write: `$_ENV['tenant_id'] = $request->tenant();` inside a request handler stays in `$_ENV` for the rest of that worker's life. Every later request that worker serves reads it — and so does a request the worker takes on while the writer is parked at one of the [suspension points](../features/fiber-multiplexing.md#suspension-points), because `$_ENV` is deliberately left out of the state a suspended request carries with it and the two then read the same array. Neither is reported: no warning, no log line, only a request reading a value some other request set. Write to `$_ENV` from your bootstrap and nowhere else. A value that belongs to one request belongs somewhere rebuilt per request — `$_SERVER` is, and it travels with its own request across a suspension. Anything your framework scopes to the request serves too, but check that it is scoped to the request rather than to the worker: a container binding or a request stack set up once during bootstrap is worker state, with the same reach as `$_ENV`. The attributes container on OxPHP's own [request object](request-api.md#attributes) serves as well, for as long as you pass that object along: `oxphp_http_request()` builds a fresh `Request` on every call, and a fresh, empty container with it.

> **What `filter_input(INPUT_ENV, …)` sees.** `filter_input(INPUT_ENV, …)`, `filter_input_array(INPUT_ENV)` and `filter_has_var(INPUT_ENV, …)` read the process environment in worker mode. They do not read `$_ENV`, so values written there — by your bootstrap or by anything else — are not among them. That is how PHP behaves everywhere, because writing to `$_ENV` gives the array its own copy and the filter extension goes on reading the environment snapshot the engine took. Worker mode adds one wrinkle to that: the snapshot is the one taken when `$_ENV` was first built on the worker, so a `putenv()` call made afterwards is not in it either. `getenv()` reads the live environment and is unaffected; `$_ENV` holds the process values plus whatever PHP on that worker has written into it. All of this is about `INPUT_ENV` alone: `INPUT_GET`, `INPUT_POST` and `INPUT_COOKIE` read storage of their own inside the filter extension, described in the note under `$_GET` above.

The pinning relies on PHP's default `auto_globals_jit=1`. With `auto_globals_jit=0`, PHP repopulates `$_ENV` from the process environment on every request before any extension can intervene, and a `.env` loader's values will not survive in worker mode.

**`putenv()` writes the process environment, not `$_ENV`.** It sets the variable for the whole server process — what `getenv()` reads, and what a child process started without an `$env` array of its own inherits — and leaves `$_ENV` as it is. What a request sets with it is put back when that request ends, in both modes: traditional mode runs PHP's request shutdown, which does it, and worker mode does it itself, keeping what the bootstrap set before `oxphp_worker()` as the value to put back to (see [Worker Mode](../features/worker-state.md#what-gets-reset-between-requests)). An [`oxphp_async()`](../features/async-promises.md) task is the exception in both: it runs on an async worker thread, and what it sets stays for the life of that thread. Until the request ends, though, the value is the process's. Requests running at the same time on other workers read it with `getenv()` and their child processes inherit it, where under PHP-FPM, with every worker a process of its own, they would not; in worker mode, so do the requests a worker runs while the request that set it is paused. `$_ENV` is built from the same environment, so it picks such a value up if it is built while the value is set — for the rest of that request in traditional mode, and in worker mode, where with the default `auto_globals_jit=1` it is built once, for the life of the worker. Nor does the environment keep two threads that set the same name apart: when the one that set it first puts it back first — at the end of its request, or as its worker exits, for what the worker's bootstrap set — it puts back the value from before both, for the whole process, while the other still has the name set. Keep a value that belongs to one request out of the environment: pass it in the `$env` argument of `proc_open()` to a child process that needs it, and in an ordinary variable everywhere else.

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

Set `SUPERGLOBALS_ENABLED=false` to stop OxPHP describing the request to PHP: the CGI and `HTTP_*` variables are not registered, and the query string is not handed over to be parsed. Use the [HTTP Request API](request-api.md) (`oxphp_http_request()`) to access request data instead.

```bash
SUPERGLOBALS_ENABLED=false   # the $_SERVER request keys and $_GET are not built
```

It reaches fewer of these arrays than the name suggests. The request body and the `Cookie` header are handed to PHP either way, so everything PHP builds out of them is there as usual:

| What | With `SUPERGLOBALS_ENABLED=false` |
|------|-----|
| `$_GET` | Empty |
| `$_SERVER` | During a request, four keys and no more: `REQUEST_TIME`, `REQUEST_TIME_FLOAT`, `argc` and `argv`, every one of them registered by PHP itself rather than by the server. No `REQUEST_METHOD`, no `REQUEST_URI`, no `HTTP_*`, no process environment. **Worker mode bootstraps differently:** the code above `oxphp_worker()` runs before any request and sees a `$_SERVER` built as though the setting were on — the whole process environment included, plus a placeholder `REQUEST_URI` of `/` |
| `$_POST`, `$_FILES`, `$_COOKIE` | Populated as usual |
| `$_REQUEST` | Populated — merged by PHP as always (see [`$_REQUEST`](#_request)); only what `$_GET` would have contributed is missing |
| `$_SESSION` | Filled by PHP's session module when a request starts a session, not by the SAPI |
| `php://input` | A stream, not a superglobal — readable as usual |
| `header()`, `headers_list()`, etc. | SAPI functions, not superglobals |
| `session_start()` and other `session_*()` functions | Native PHP functions |
| `oxphp_http_request()` | Available — every method answers, including `query()` and `payload()` |

Code that routes on `$_SERVER['REQUEST_URI']` finds nothing there under this setting and has to read `oxphp_http_request()->path()` instead. In a worker entry script that applies to the handler; the bootstrap above it sees the placeholder `/` described in the table, which is not a route either.

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
