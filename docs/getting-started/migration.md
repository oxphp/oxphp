---
title: Migration
description: Move an existing PHP application to OxPHP from nginx + PHP-FPM, FrankenPHP or RoadRunner — routing modes, configuration mapping, behaviour differences, and what does not carry over.
---

# Migration

This guide moves an existing PHP application to OxPHP. Start with the section for what you run today — nginx + PHP-FPM, or FrankenPHP and RoadRunner — and read [What does not carry over](#what-does-not-carry-over) before you commit to the move.

## From nginx + PHP-FPM

Most applications move without code changes. The default mode runs each request from a clean PHP state, the same lifecycle PHP-FPM gives you, so the work is translating configuration. The [example deployments](../examples/index.md) are complete recipes for Laravel, Symfony, WordPress, Drupal, Magento and others.

**1. Pick the routing mode that matches your `try_files`.**

| nginx | OxPHP |
|---|---|
| `try_files $uri /index.php?$query_string` — Laravel, Symfony, Yii | `ENTRY_FILE=index.php` (Framework) |
| `try_files $uri $uri/ /index.php` with several PHP entry points — WordPress, OpenCart | `ENTRY_FILE` unset (Traditional) |
| `try_files $uri /index.html` — single-page app | `ENTRY_FILE=index.html` (SPA) |

**2. Build the image.** Start from the two-line Dockerfile in [Quick Start](quick-start.md). Extensions must be built for thread-safe PHP, in a `php:*-zts-alpine` stage with the same PHP minor as the OxPHP image — see [Installing PHP extensions](docker.md#installing-php-extensions-in-production). Check that every PECL extension you use supports ZTS.

**3. Translate the configuration.** The image loads no `php.ini`. Keep your PHP settings (`memory_limit`, `upload_max_filesize`, OPcache, …) in a file of your own anywhere in the project, for example `docker/php.ini`, and copy it into the image's PHP config directory — a path inside the container, not in your project:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.12.0

COPY docker/php.ini /usr/local/etc/php/conf.d/zz-app.ini
COPY --chown=www-data:www-data . /var/www/html/public
```

PHP reads every `.ini` file in that directory in alphabetical order and the last value wins, so the `zz-` prefix makes yours apply after the image's own files. Mounting the file as a volume works too — see [PHP configuration](docker.md#php-configuration). The rest maps onto environment variables:

| nginx / PHP-FPM | OxPHP |
|---|---|
| `listen 443 ssl`, `ssl_certificate` | `TLS_CERT`, `TLS_KEY`, `TLS_MIN_VERSION` — [TLS](../features/tls.md) |
| `pm = static` / `pm = dynamic`, `pm.max_children` | `PHP_WORKERS=N` / `PHP_WORKERS=MIN:MAX` |
| `php_admin_value[...]`, `php.ini` | your own `.ini` file copied to `/usr/local/etc/php/conf.d/` in the image (see above) |
| `gzip on`, `brotli on` | on by default; `COMPRESSION_ENCODINGS` — [Compression](../features/compression.md) |
| `expires 30d` for static files | `STATIC_MAX_AGE` (default `30d`) |
| `limit_req` | `RATE_LIMIT`, `RATE_WINDOW_SECONDS` (fixed window per IP) — [Rate limiting](../features/rate-limiting.md) |
| `set_real_ip_from`, `real_ip_header` | `TRUSTED_PROXIES` — [Trusted proxies](../security/trusted-proxies.md) |
| `error_page 404 /404.html` | `ERROR_PAGES_DIR` with `{status}.html` — [Error pages](../features/error-pages.md) |
| `access_log` | `ACCESS_LOG=all` or `error`, JSON lines — [Access logging](../features/access-logging.md) |
| `location ~ ^/uploads/.*\.php$ { deny all; }` | `PHP_DENY_PATHS=/uploads/**` — [PHP deny-list](../security/php-deny.md) |
| `pm.status_path`, `ping.path` | `INTERNAL_ADDR` with `/health` and `/metrics` — [Health checks](../operations/health-checks.md) |
| `fastcgi_finish_request()` | `oxphp_finish_request()` — [Early response](../features/early-response.md) |

**4. Check the behaviour differences.**

- **`max_execution_time` counts wall-clock time.** Under PHP-FPM time spent waiting on a database or an HTTP call is not charged; under thread-safe PHP it is. A slow endpoint that never hit the limit before can start returning `504` — see [Timeouts](../features/timeouts.md#wall-clock-not-cpu-time).
- **`php_sapi_name()` returns `cli-server`.** Code that branches on `fpm-fcgi` needs another check: `function_exists('oxphp_request_id')`.
- **A few `$_SERVER` keys differ.** `SERVER_ADDR`, `PATH_TRANSLATED` and `REDIRECT_STATUS` are not set, and `PHP_AUTH_USER` / `PHP_AUTH_PW` are not extracted — read `HTTP_AUTHORIZATION` — see [Differences from PHP-FPM](../php/superglobals.md#differences-from-php-fpm).
- **Framing is restricted by default.** Responses carry `X-Frame-Options: SAMEORIGIN` and a matching `frame-ancestors` policy; set `FRAME_OPTIONS=off` if you manage framing elsewhere or embed the app in another origin.

**5. Roll out beside the old stack.** Run OxPHP next to nginx + PHP-FPM, point a share of traffic at it through your existing load balancer, use `/health/readiness` for probes, and compare error rates and latency on `/metrics` before moving the rest.

**6. Turn on worker mode last.** Migrate in the default mode first; then set `WORKER_MODE_ENABLED=true` and `ENTRY_FILE` to a worker script. State that lives in static properties and long-lived connections now survives between requests — read [What gets reset between requests](../features/worker-mode.md#what-gets-reset-between-requests) before enabling it.

## From FrankenPHP or RoadRunner

Worker scripts keep their shape: bootstrap once, then serve requests. Replace the `frankenphp_handle_request()` loop with a single `oxphp_worker($handler)` call — OxPHP runs the loop and calls the handler for each request — and read the request from the superglobals or `oxphp_http_request()`. RoadRunner's PSR-7 `waitRequest()` / `respond()` loop has no counterpart: move the application's handler into the same callback. See [`oxphp_worker()`](../php/functions.md#oxphp_worker) and [Worker mode](../features/worker-mode.md).

**Drop the `MAX_REQUESTS` counter.** There is no equivalent and you do not need one: a worker is not meant to be restarted every N requests, and doing so only hides a leak. If you suspect one, track it instead:

- chart `oxphp_worker_memory_bytes` per worker and watch for a steady climb;
- set `WORKER_MAX_MEMORY_MIB` as a safety net — a worker past the limit is replaced before the container runs out of memory, and each such recycle is counted in `oxphp_worker_recycles_by_reason_total{reason="max_memory"}`;
- remember that this limit measures only PHP's own heap. With extension-heavy stacks, compare `Worker::current()->rss()` against your own threshold and call `Worker::scheduleExit()`.

## What does not carry over

- **No reverse proxy or upstream.** There is no `proxy_pass` and no WebSocket server. If nginx also fronts other services, keep a proxy in front of OxPHP for those routes.
- **One port, one protocol.** An instance serves plain HTTP or HTTPS, not both, so the port-80 redirect to HTTPS needs a proxy or a second instance. The certificate is read at startup, so a renewal needs a restart — see [TLS](../features/tls.md).
- **Not yet available:** HTTP/3 and `103 Early Hints` — see the [Roadmap](https://github.com/oxphp/oxphp#roadmap).
- **Platform.** Linux only, distributed as a Docker image; no apt or brew packages. PHP 8.4 and 8.5 are supported.

## What's Next

- [Routing](../features/routing.md) — Traditional, Framework, SPA, and Worker routing modes in detail
- [Docker Guide](docker.md) — development and production Dockerfiles, Compose configuration, PHP ini mounts
- [Configuration](../operations/configuration.md) — full environment variable reference
- [Worker Mode](../features/worker-mode.md) — persistent PHP workers that bootstrap once and handle multiple requests
- [Example Deployments](../examples/index.md) — complete recipes for Laravel, Symfony, WordPress, Drupal, Magento and others
