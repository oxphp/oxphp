---
title: Installation
description: Install OxPHP via Docker image or build from source. Covers prerequisites, verification, and platform notes.
---

# Installation

OxPHP is distributed as a Docker image — the fastest and recommended way to start serving PHP applications. The image bundles the server binary, PHP 8.4 ZTS, the OxPHP extension, and all runtime dependencies on Alpine Linux.

## Docker (Recommended)

Pull the official image from the GitHub Container Registry:

```bash
docker pull ghcr.io/oxphp/oxphp:0.1.0
```

The image includes:

- **OxPHP server binary** — the async HTTP server
- **PHP 8.4 ZTS** — thread-safe PHP runtime for multi-worker execution
- **OxPHP PHP extension** (`oxphp_sapi.so`) — provides `oxphp_request_id()`, `oxphp_server_info()`, `oxphp_worker()`, and other built-in functions
- **Bridge library** (`liboxphp_bridge.so`) — connects the Rust server to the PHP runtime
- **Alpine Linux** base — minimal runtime footprint
- Runs as **www-data** (UID 82, GID 82) for non-root container execution

### Image Structure

File layout of the runtime image:

```
/usr/local/
├── bin/
│   └── oxphp                                        # server binary
├── lib/
│   ├── libphp.so                                    # PHP 8.4 ZTS runtime
│   ├── liboxphp_bridge.so                           # C bridge (TLS slot between Rust and PHP)
│   └── php/extensions/no-debug-zts-20240924/
│       ├── oxphp_sapi.so                            # OxPHP PHP extension
│       └── opcache.so                               # OPcache (from base PHP)
├── etc/php/
│   └── conf.d/
│       ├── oxphp.ini                                # PHP settings for OxPHP
│       └── extension.ini                            # extension=oxphp_sapi.so
```

The three OxPHP components and their purpose:

| Component | Size | Purpose |
|-----------|------|---------|
| `oxphp` | ~8 MB | HTTP server, routing, plugins, metrics |
| `liboxphp_bridge.so` | ~50 KB | Shared `__thread` TLS context between Rust and PHP |
| `oxphp_sapi.so` | ~200 KB | PHP functions (`oxphp_request_id()`, `OxPHP\Http\Request`, etc.) |

Dependency chain:

```
oxphp ──► libphp.so ──► libxml2, libcurl, libsqlite3, libonig, ...
  │
  └──► liboxphp_bridge.so ◄── oxphp_sapi.so
```

The `oxphp` binary dynamically links to `libphp.so` and `liboxphp_bridge.so`. The PHP extension `oxphp_sapi.so` also links to the bridge library — this is the only way to share per-request state between Rust and PHP through a common `__thread` slot in a single `.so`.

### Minimal Dockerfile

The base image `php:8.4-zts-alpine3.23` already contains `libphp.so` and all its dependencies. You only need to copy the three OxPHP artifacts:

```dockerfile
FROM php:8.4-zts-alpine3.23

COPY --from=ghcr.io/oxphp/oxphp:0.1.0 /usr/local/bin/oxphp /usr/local/bin/oxphp
COPY --from=ghcr.io/oxphp/oxphp:0.1.0 /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ghcr.io/oxphp/oxphp:0.1.0 /usr/local/lib/php/extensions/no-debug-zts-20240924/oxphp_sapi.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp.ini

COPY --chown=www-data:www-data . /var/www/html

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
FROM ghcr.io/oxphp/oxphp:0.1.0

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

COPY --chown=www-data:www-data . /var/www/html
```

If your application does not need additional extensions, this is sufficient:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

Build and run:

```bash
docker build -t my-app .
docker run -p 80:80 my-app
```

The server listens on port `80` by default. The document root is `/var/www/html/public`. Laravel and Symfony projects already contain a `public/` directory, so copying the project into `/var/www/html/` is sufficient. If your project structure differs, override the document root with the `DOCUMENT_ROOT` environment variable.

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
- PHP 8.4 with ZTS (Zend Thread Safety) enabled
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

> **Note:** When deploying to Alpine Linux, build inside the same `php:8.4-zts-alpine` image used for the PHP runtime. Mixing glibc and musl builds causes runtime errors. The official Docker image handles this correctly.

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
