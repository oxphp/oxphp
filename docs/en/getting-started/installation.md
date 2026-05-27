---
title: Installation
description: Install OxPHP via Docker image or build from source. Covers prerequisites, verification, and platform notes.
---

# Installation

OxPHP is distributed as a Docker image — the fastest and recommended way to start serving PHP applications. The image bundles the server binary, PHP 8.4 or 8.5 ZTS, the OxPHP extension, and all runtime dependencies on Alpine Linux. The default `:0.6.0` and `:latest` tags ship PHP 8.5; pull PHP 8.4 with the `:0.6.0-php8.4`, `:php8.4`, or any `*-php8.4*` tag variant.

## Docker (Recommended)

Pull the official image from the GitHub Container Registry:

```bash
docker pull ghcr.io/oxphp/oxphp:0.6.0
```

The image includes:

- **OxPHP server binary** — the async HTTP server
- **PHP ZTS runtime** — 8.4 or 8.5, depending on the tag pulled; thread-safe PHP for multi-worker execution
- **OxPHP PHP extension** (`oxphp_sapi.so`) — provides `oxphp_request_id()`, `oxphp_server_info()`, `oxphp_worker()`, and other built-in functions
- **Bridge library** (`liboxphp_bridge.so`) — connects the Rust server to the PHP runtime
- **Alpine Linux** base — minimal runtime footprint
- **No `USER` directive** — the image runs as **root** by default, matching `nginx:alpine` / `php-fpm:alpine` / `frankenphp:alpine`. The `www-data` user (UID 82, GID 82) is pre-created and `/var/www/html` is chowned to it at build time; drop privileges at the orchestrator level when you deploy:
  - `docker run --user www-data ghcr.io/oxphp/oxphp:0.6.0`
  - Compose: `services.app.user: www-data`
  - Kubernetes: `securityContext.runAsUser: 82`

### Image Structure

File layout of the runtime image:

```
/usr/local/
├── bin/
│   └── oxphp                                        # server binary
├── lib/
│   ├── libphp.so                                    # PHP ZTS runtime (8.4 or 8.5, matches the image tag)
│   ├── liboxphp_bridge.so                           # C bridge library
│   └── php/extensions/no-debug-zts-<ABI>/
│       └── oxphp_sapi.so                            # OxPHP PHP extension
├── etc/php/
│   └── conf.d/
│       ├── oxphp.ini                                # PHP settings for OxPHP
│       └── extension.ini                            # extension=oxphp_sapi.so
```

> **The `<ABI>` value depends on the PHP minor.** PHP 8.4 uses `20240924`, PHP 8.5 uses a different date stamp. The examples below pin `20240924` because their `FROM` line targets `php:8.4-zts-alpine3.23` — switch the FROM and you must switch the date too. To derive it portably inside the build:
>
> ```bash
> php -r 'echo ini_get("extension_dir");'
> # /usr/local/lib/php/extensions/no-debug-zts-20240924
> ```
>
> Use `$(php -r 'echo ini_get("extension_dir");')` in shell commands to avoid hardcoding.

The three OxPHP components and their purpose:

| Component | Size | Purpose |
|-----------|------|---------|
| `oxphp` | ~8 MB | HTTP server, routing, plugins, metrics |
| `liboxphp_bridge.so` | ~50 KB | Shared bridge library that links the server to the PHP runtime |
| `oxphp_sapi.so` | ~200 KB | PHP functions (`oxphp_request_id()`, `OxPHP\Http\Request`, etc.) |

Dependency chain:

```
oxphp ──► libphp.so ──► libxml2, libcurl, libsqlite3, libonig, ...
  │
  └──► liboxphp_bridge.so ◄── oxphp_sapi.so
```

The `oxphp` binary links to `libphp.so` and `liboxphp_bridge.so`. The PHP extension `oxphp_sapi.so` also links to the bridge library so that per-request state is available to your PHP code.

### Minimal Dockerfile

The base image `php:8.4-zts-alpine3.23` (or `php:8.5-zts-alpine3.23`) already contains `libphp.so` and all its dependencies. Match the PHP minor in your `FROM` to the OxPHP tag you copy from. You only need to copy the three OxPHP artifacts:

```dockerfile
FROM php:8.4-zts-alpine3.23

COPY --from=ghcr.io/oxphp/oxphp:0.6.0 /usr/local/bin/oxphp /usr/local/bin/oxphp
COPY --from=ghcr.io/oxphp/oxphp:0.6.0 /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ghcr.io/oxphp/oxphp:0.6.0 /usr/local/lib/php/extensions/no-debug-zts-20240924/oxphp_sapi.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp.ini

COPY --chown=www-data:www-data . /var/www/html/public

EXPOSE 80 443 9090

CMD ["oxphp"]
```

This approach is convenient for development — PHP CLI, `composer`, `docker-php-ext-install`, and `xdebug` are all available. See the [Docker Guide](docker.md) for details.

### Production Dockerfile

The official OxPHP image is minimal — it does not include PHP CLI or extension build tools. If your application needs additional extensions (pdo_mysql, intl, etc.), build them in a separate stage and copy into the final image:

```dockerfile
# Extension build stage
FROM php:8.4-zts-alpine3.23 AS extensions

RUN apk add --no-cache icu-dev postgresql-dev \
    && docker-php-ext-install pdo pdo_mysql pdo_pgsql intl

# Production
FROM ghcr.io/oxphp/oxphp:0.6.0

# Runtime dependencies for extensions
USER root
RUN apk add --no-cache icu-libs libpq

# Copy compiled extensions
COPY --from=extensions /usr/local/lib/php/extensions/no-debug-zts-20240924/*.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

# Enable extensions
RUN { \
        echo "extension=pdo.so"; \
        echo "extension=pdo_mysql.so"; \
        echo "extension=pdo_pgsql.so"; \
        echo "extension=intl.so"; \
    } > /usr/local/etc/php/conf.d/app-extensions.ini

USER www-data

COPY --chown=www-data:www-data . /var/www/html/public
```

If your application does not need additional extensions, this is sufficient:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.6.0

COPY --chown=www-data:www-data . /var/www/html/public
```

Build and run:

```bash
docker build -t my-app .
docker run -p 80:80 my-app
```

The server listens on port `80` by default. The document root is `/var/www/html/public` — the snippets above copy the project directly into it. For Laravel, Symfony, or other frameworks that already ship a `public/` subdirectory, use `COPY --chown=www-data:www-data . /var/www/html` instead so the framework's own `public/` lines up with the default. If your structure differs further, override the document root with the `DOCUMENT_ROOT` environment variable.

## Source Build (Without PHP)

Build OxPHP from source with the PHP feature disabled to serve static files only:

```bash
cargo build --release --no-default-features
```

The binary is at `target/release/oxphp`. It uses a stub executor that returns a placeholder response for PHP requests while serving static files normally. This mode is useful for testing the server without a PHP runtime present.

## Source Build (With PHP)

Building OxPHP with full PHP support requires the bridge library and PHP extension to be compiled and installed first.

### Prerequisites

- Rust toolchain (1.91.1 or later)
- PHP 8.4 or 8.5 with ZTS (Zend Thread Safety) enabled
- C compiler (gcc or clang)
- `phpize` and PHP development headers

### Build Steps

```bash
# 1. Build and install the bridge library
cd ext/bridge
make && sudo make install

# 2. Build and install the PHP extension
cd ../
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# 3. Build OxPHP (default features include php)
cargo build --release
```

The binary requires the shared libraries in the library search path at runtime:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

> **Note:** When deploying to Alpine Linux, build inside the same `php:{8.4,8.5}-zts-alpine` image used for the PHP runtime — match the minor of the OxPHP image you ship. Mixing glibc and musl builds causes runtime errors. The official Docker image handles this correctly.

## Verifying Installation

After starting OxPHP, structured JSON log output confirms the server is running:

```text
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:80",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:80"}
```

Test that the server responds:

```bash
curl http://localhost/
```

If you enabled the internal server with `INTERNAL_ADDR`, verify the health endpoint:

```bash
curl http://localhost:9090/health
```

A healthy server returns `200` with JSON status. A degraded server returns `503`.

## What's Next

- [Quick Start](quick-start.md) — create a project, run OxPHP with Docker Compose, and make your first request
- [Docker Guide](docker.md) — Dockerfiles for development and production, Compose configuration, and volume mounts
- [Configuration](../operations/configuration.md) — full environment variable reference
