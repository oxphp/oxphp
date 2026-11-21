---
title: Server Variables
description: Every $_SERVER key OxPHP sets: standard CGI variables, request headers, proxy and trace-context values, PATH_INFO rules, and differences from PHP-FPM.
---

# Server Variables

OxPHP builds `$_SERVER` from the incoming HTTP request following the CGI/1.1 specification. Process environment variables are imported first; CGI variables are set afterward, so request-specific values always override any colliding environment keys.

## Standard Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `SCRIPT_FILENAME` | Absolute filesystem path to the PHP script being executed | `/var/www/html/public/index.php` |
| `DOCUMENT_ROOT` | Web root directory configured via `DOCUMENT_ROOT` env var. With `DOCUMENT_ROOT=false`, the entry file's directory — `getenv('DOCUMENT_ROOT')` still returns the variable, `false` | `/var/www/html/public` |
| `SERVER_SOFTWARE` | Server identifier (carries the running OxPHP version) | `OxPHP/0.16.0` |
| `SERVER_PROTOCOL` | Negotiated HTTP protocol version | `HTTP/2` |
| `REQUEST_METHOD` | HTTP method | `GET` |
| `REQUEST_URI` | Full URI with query string | `/app?page=2` |
| `SCRIPT_NAME` | Path of the executed script relative to `DOCUMENT_ROOT` — the front controller in Framework mode, **not** the request URI | `/index.php` |
| `DOCUMENT_URI` | Alias for `SCRIPT_NAME`, for nginx/PHP-FPM compatibility | `/index.php` |
| `PHP_SELF` | `SCRIPT_NAME` plus `PATH_INFO` when present, otherwise equal to `SCRIPT_NAME` | `/index.php/user/42` |
| `QUERY_STRING` | Query portion of the URI (empty string when absent) | `page=2` |
| `SERVER_NAME` | Hostname from the `Host` header | `example.com` |
| `SERVER_PORT` | Port from the `Host` header | `8080` |
| `REMOTE_ADDR` | Client IP address | `172.17.0.1` |
| `REMOTE_PORT` | Client port number | `54321` |
| `HTTPS` | Set to `"on"` when the connection uses TLS; absent otherwise | `on` |
| `REQUEST_SCHEME` | `"https"` for TLS connections, `"http"` otherwise | `https` |
| `CONTENT_TYPE` | Value of the `Content-Type` header (no `HTTP_` prefix) | `application/json` |
| `CONTENT_LENGTH` | Value of the `Content-Length` header (no `HTTP_` prefix) | `128` |
| `REQUEST_TIME` | Unix timestamp (integer) of when a PHP worker took the request — time spent waiting for a free worker is not included | `1738800000` |
| `REQUEST_TIME_FLOAT` | The same moment as `REQUEST_TIME`, with microsecond precision | `1738800000.123456` |
| `GATEWAY_INTERFACE` | CGI version string | `CGI/1.1` |

A request without a `Host` header reaches PHP only over HTTP/1.0, and then `SERVER_NAME` defaults to `localhost` and `SERVER_PORT` defaults to `80` (or `443` for TLS). RFC 9112 §3.2 has a server answer `400 Bad Request` to an HTTP/1.1 request without one, and to any request with more than one `Host` line or a `Host` that is not valid, and OxPHP does so without running PHP. An HTTP/2 request that names its host in `:authority` needs no `Host`; one without an `:authority` is held to the HTTP/1.1 rules, since RFC 9110 §7.2 lets a client leave `Host` out only when it sends `:authority`. A valid `Host` is a host and an optional `:port`, as RFC 9110 §7.2 has it: a name made of letters, digits, `-._~`, `!$&'()*+,;=` and `%` escapes such as `%41`, or an IPv6 address in brackets (`[::1]`, or the IPvFuture form RFC 3986 keeps for address formats to come, such as `[v1.x]`), and a port from 0 to 65535 written in digits. So `Host: café.example` sent as raw UTF-8, `Host: example.com:abc`, `Host: :80` and an IPv6 zone such as `Host: [fe80::1%25eth0]` are each refused, while a name in another script sent as punycode (`xn--caf-dma.example`) is valid. A port left empty, as in `Host: example.com:`, is valid and reads as the scheme's default, which is what RFC 9110 §4.2.3 makes of `http://example.com:/`: `SERVER_PORT` is `80` and `HTTP_HOST` is `example.com:`. nginx answers each of these cases the same way; of the values tried against it, it differs on three: it accepts a `%` that starts no escape (`Host: a%zz`) and brackets around anything but an IPv6 address or an IPvFuture (`Host: []`, `Host: [1.2.3.4]`, `Host: [a.example]`), which OxPHP refuses, and refuses an empty label (`Host: a..b`), which OxPHP accepts.

Over HTTP/2 a client sends the host in the `:authority` pseudo-header instead of a `Host` header, and OxPHP hands it to PHP as the request's `Host` header: `HTTP_HOST`, `SERVER_NAME`, `SERVER_PORT` and the [request object](request-api.md)'s host all come from it, except that behind a [trusted proxy](#behind-a-reverse-proxy) `SERVER_NAME`, `SERVER_PORT` and the host come from the host the proxy forwards, as over HTTP/1.1. nginx also builds a `Host` header from `:authority`, which is where PHP-FPM behind it gets its `HTTP_HOST`. A `Host` header sent next to it is replaced by it, since RFC 9113 §8.3.1 forbids taking the host from `Host` when `:authority` is present, and a request whose `Host` header names something else is answered with `400 Bad Request` without reaching PHP. Letter case does not count as a difference there, but a port does: `example.com` and `example.com:443` differ.

Over HTTP/1.1 the host comes from the request target instead when the target is in absolute form, as in `GET http://example.com/path HTTP/1.1`: RFC 9112 §3.2.2 has an origin server ignore the `Host` header of such a request, so the host and port the target names replace it, whatever that header said once it passes the checks [above](#standard-variables) (an HTTP/1.1 request still has to send one), and `REQUEST_URI` is the target's path and query. A target with an empty path, as in `GET http://example.com?page=2 HTTP/1.1`, has `/` for one, the path RFC 9112 §3.2.1 has a client send for an empty one in origin form: `REQUEST_URI` is `/?page=2`, as for the same request in origin form. nginx passes PHP-FPM `?page=2` there: its `$request_uri`, which the stock `fastcgi_params` hands over as `REQUEST_URI`, is the target from the `?` on. The same holds for HTTP/1.0. The target's scheme is not taken: `REQUEST_SCHEME`, `HTTPS` and the port of a host named without one are what they would be for the same request in origin form. nginx routes such a request by the target's host but passes PHP-FPM the `Host` header it received as `HTTP_HOST`. Over either protocol, a `user@` or `user:password@` in front of the host, which `http` and `https` URIs are not to carry, is not part of the host: the `Host` header PHP gets leaves it out, and over HTTP/2 a `Host` header is compared with the `:authority` without it. An absolute-form target or an `:authority` whose host is empty, as in `GET http://:80/path HTTP/1.1`, names no host, and the request is answered with `400 Bad Request` without reaching PHP: RFC 9110 §4.2.1 has a recipient reject an `http` URI like that. So is one whose host or port is not valid by the `Host` rule above, as in `GET http://example.com:abc/ HTTP/1.1`, which nginx refuses too; an empty port, as in `http://example.com:/`, is the scheme's default.

A request target in none of the forms HTTP gives it is answered with `400 Bad Request` without reaching PHP: over HTTP/1.1 and HTTP/1.0 a target with neither a leading `/` nor a `scheme://` in front, as in `GET index.php HTTP/1.1`, read as the form RFC 9112 §3.2 keeps for `CONNECT`, which does not reach PHP either; over HTTP/2 a `:path` that does not start with `/`, which RFC 9113 §8.3.1 rules out, or none at all in a request with no `:authority` either; and over either a target of `*` with a method other than `OPTIONS`, the one method it is for. RFC 9112 §3 has a server answer an invalid request line with `400` or a redirect rather than correct it and serve the request. `OPTIONS *` is routed as a request for the path `*`, and where that runs a script, its `REQUEST_URI` is `*`. nginx refuses all of these with `400`, `OPTIONS *` included. An HTTP/2 request with an `:authority` and no `:path` never reaches OxPHP: the HTTP/2 layer resets its stream, where nginx answers `400`. A target with no `//` after its scheme, as in `GET mailto:user@example.com HTTP/1.1`, is an absolute URI, but one with no host, which every `http` and `https` URI has (RFC 9110 §4.2.1), and it is refused the same way, as nginx refuses it. A `:path` holding only a fragment, as in `#top`, arrives empty, the fragment dropped before the server sees it: without an `:authority` it is refused like a missing one, and with one it is served as a request for `/`, which nginx refuses.

## HTTP Request Headers

HTTP request headers are added to `$_SERVER` with an `HTTP_` prefix. Header names are converted to uppercase with dashes replaced by underscores, following CGI/1.1 conventions:

```text
Accept: text/html            -> HTTP_ACCEPT
X-Forwarded-For: 1.2.3.4    -> HTTP_X_FORWARDED_FOR
Authorization: Bearer abc    -> HTTP_AUTHORIZATION
Cookie: session=xyz          -> HTTP_COOKIE
```

> **Note:** `Content-Type` and `Content-Length` appear without the `HTTP_` prefix — as `CONTENT_TYPE` and `CONTENT_LENGTH` — as required by the CGI specification.

A header whose name contains an underscore (`_`) or a dot (`.`) is not added to `$_SERVER`. PHP turns a dot in a `$_SERVER` key into an underscore, so `X_Forwarded_For` and `X.Forwarded.For` would both arrive as `HTTP_X_FORWARDED_FOR`, the key of the `X-Forwarded-For` header — and a client could set a key that the application expects only from a proxy in front of it, such as `HTTP_X_AUTHENTICATED_USER` from an authentication proxy that sets `X-Authenticated-User`. nginx drops such headers by default as well (`underscores_in_headers off`, `ignore_invalid_headers on`). To read one, use the [request object](request-api.md#headers) with the name as it was sent: `$request->header('X_Api_Key')`.

A header sent as several field lines with the same name — an `X-Forwarded-For` line from each of two proxies, say — is one key, its lines joined with `, ` in the order they arrived, as HTTP lets a recipient combine them (RFC 9110 §5.3), and the [request object](request-api.md#headers) holds the same value. `Cookie` lines are joined with `; `, the separator of a cookie list. A header with a byte outside ASCII on any of its lines is left out of `$_SERVER` and of the request object, the same as a header sent as one such line. `Content-Type` and `Content-Length` hold one value only and are the exception to both: `CONTENT_TYPE` and `CONTENT_LENGTH`, and the request object's values for them, are their first line, whatever the later lines hold. `Host` holds one value too, and a request that sends it on more than one line is refused, except over HTTP/2 with an `:authority`, which replaces every `Host` line that names the same host. Over HTTP/2, a request that carries an `:authority`, and over HTTP/1.1 or HTTP/1.0 one whose target is in absolute form, has that host as its `Host`, as described [above](#standard-variables).

## Behind a Reverse Proxy

When `TRUSTED_PROXIES` is configured and the request peer is in the trusted set, OxPHP rewrites the following `$_SERVER` keys from forwarded headers (`X-Forwarded-*` or RFC 7239 `Forwarded`):

| Variable | Value when peer is trusted | Value otherwise |
|----------|----------------------------|------------------|
| `REMOTE_ADDR` | Rightmost untrusted address from `X-Forwarded-For` / `Forwarded`; `CF-Connecting-IP` for a request whose forwarding chain shows it arrived from a Cloudflare edge when `TRUSTED_PROXIES` names `cloudflare`; the direct peer when the forwarding headers yield no address | Direct peer IP |
| `REMOTE_PORT` | Client source port from an `ip:port` entry in `Forwarded` or `X-Forwarded-For`, otherwise `0`; the peer's port when the address is not rewritten | Direct peer port |
| `HTTPS` | `"on"` when `X-Forwarded-Proto: https` | Set only when the peer connection is TLS |
| `REQUEST_SCHEME` | `"https"` / `"http"` from `X-Forwarded-Proto` | Based on actual TLS state |
| `SERVER_NAME` | Host portion of `X-Forwarded-Host` | Host portion of the `Host` header |
| `SERVER_PORT` | `X-Forwarded-Port`, else port portion of `X-Forwarded-Host`, else 443/80 by scheme | Port portion of `Host`, or 443/80 |

The raw `HTTP_X_FORWARDED_FOR`, `HTTP_X_FORWARDED_PROTO`, `HTTP_X_FORWARDED_HOST`, `HTTP_X_FORWARDED_PORT`, and `HTTP_FORWARDED` keys remain in `$_SERVER` unchanged — both the rewritten values and the original headers are available.

When the client address is rewritten, `REMOTE_PORT` is the client's source port if the entry the address comes from carries one — an RFC 7239 `Forwarded: for="ip:port"` node, or an `ip:port` entry in `X-Forwarded-For` — and `"0"` otherwise: the value is zeroed rather than guessed. An address taken from `CF-Connecting-IP` carries no port either, so `REMOTE_PORT` is `"0"` then even when `Forwarded` has one. When the forwarding headers yield no client address (see [Trusted Proxies](../security/trusted-proxies.md#client-ip-extraction)), neither `REMOTE_ADDR` nor `REMOTE_PORT` is rewritten: both stay those of the direct peer.

When `TRUSTED_PROXIES` is **not** set, no rewriting occurs and `REMOTE_ADDR` is always the direct peer — typically your load balancer, not the end client. Parsing `X-Forwarded-For` manually is error-prone (leftmost vs rightmost, no CIDR trust check); prefer configuring `TRUSTED_PROXIES`. See [Trusted Proxies](../security/trusted-proxies.md) for the trust algorithm and configuration syntax.

## Trace Context Variables

When distributed tracing is enabled, OxPHP adds trace context variables to `$_SERVER`:

| Variable | Description | Example |
|----------|-------------|---------|
| `OXPHP_TRACE_ID` | W3C trace ID for the current request | `4bf92f3577b34da6a3ce929d0e0e4736` |
| `OXPHP_SPAN_ID` | Span ID for the OxPHP server span | `00f067aa0ba902b7` |
| `OXPHP_PARENT_SPAN_ID` | Parent span ID from the upstream service (empty if root) | `b9c7c989f97918e1` |

These variables are only present when a valid `traceparent` header arrives or when OxPHP generates a new trace. If tracing is not configured, these keys are absent.

## Differences from PHP-FPM

The following variables behave differently compared to a standard PHP-FPM setup:

| Variable | Behavior |
|----------|----------|
| `SERVER_ADDR` | Not set. OxPHP does not populate the local server IP address. |
| `PATH_INFO` | Set automatically — see [PATH_INFO Behavior](#path_info-behavior) below. |
| `PATH_TRANSLATED` | Not set. |
| `PHP_AUTH_USER` / `PHP_AUTH_PW` / `AUTH_TYPE` | Not extracted from the `Authorization` header. Read `$_SERVER['HTTP_AUTHORIZATION']` directly. |
| `REDIRECT_STATUS` | Not set. OxPHP does not use an internal redirect mechanism. |

## Example

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$scheme  = $_SERVER['REQUEST_SCHEME'];  // "http" or "https"

// Read a custom header
$token = $_SERVER['HTTP_AUTHORIZATION'] ?? '';

// REMOTE_ADDR is already the real client IP when TRUSTED_PROXIES is configured.
// Without it, REMOTE_ADDR is the direct peer (usually a load balancer).
$clientIp = $_SERVER['REMOTE_ADDR'];

// Check TLS without checking the port
if (isset($_SERVER['HTTPS']) && $_SERVER['HTTPS'] === 'on') {
    // Secure connection
}
```

## PATH_INFO Behavior

`$_SERVER['PATH_INFO']` is populated automatically based on the active routing mode. There is no feature flag — the previous `SPLIT_PATH_INFO_ENABLED` env variable has been removed.

| Routing mode | When set | Value |
|---|---|---|
| **Traditional** (`ENTRY_FILE` unset) | Only when the URI contains `.php/` and the script prefix exists on disk | Tail after the script segment |
| **Framework** (`ENTRY_FILE=index.php`) | Only when the request explicitly names the entry file with a trailing segment (`/index.php/extra`) | Tail after the entry file, e.g. `/news` |
| **SPA** (`ENTRY_FILE=index.html`) | Never — PHP only runs for exact `.php` files, no PATH_INFO |  — |

`SCRIPT_NAME` always identifies the executed script (the resolved file relative to the document root), so in normal routing `PATH_INFO` is present only when `SCRIPT_NAME` is a literal prefix of the request path. When a request is rewritten to a front controller it does not name (an application route, a directory index, a static-miss fallback), `PATH_INFO` is absent and the original path lives in `REQUEST_URI`. (The `PHP_DENY_PATHS` fallback is a deliberate exception: it sets `PATH_INFO` to the original sanitized URI so the fallback script can route on it.)

### Traditional mode examples

OxPHP scans the URI left-to-right for the first `.php` segment that corresponds to an actual file on disk. Everything after it becomes `PATH_INFO`:

| Request URI | File on disk | `SCRIPT_NAME` | `PATH_INFO` | `PHP_SELF` |
|---|---|---|---|---|
| `/app.php/user/42` | `app.php` exists | `/app.php` | `/user/42` | `/app.php/user/42` |
| `/index.php/api/v2/users` | `index.php` exists | `/index.php` | `/api/v2/users` | `/index.php/api/v2/users` |
| `/app.php` | `app.php` exists | `/app.php` | *(absent)* | `/app.php` |
| `/missing.php/foo` | file not found | falls back to `/index.php` | — | depends on fallback |

### Framework mode examples

Every non-static request is rewritten onto `index.php`. `PATH_INFO` is set only when the request explicitly names the entry file with a trailing segment; for application routes the original path is read from `REQUEST_URI`.

| Request URI | `SCRIPT_NAME` | `PATH_INFO` |
|---|---|---|
| `/api/users` | `/index.php` | *(absent)* |
| `/about.php` | `/index.php` | *(absent)* |
| `/index.php/news/local` | `/index.php` | `/news/local` |
| `/index.php` | `/index.php` | *(absent)* |

> **Note:** `PATH_TRANSLATED` is not populated. It is rarely used in practice and is not set by nginx or PHP-FPM by default.

## See Also

- [Superglobals](superglobals.md) — the other request superglobals and how to disable them
- [HTTP Request API](request-api.md) — typed access to the same request data
- [Trusted Proxies](../security/trusted-proxies.md) — which `$_SERVER` keys a trusted proxy's headers override
