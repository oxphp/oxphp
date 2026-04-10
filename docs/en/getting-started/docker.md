---
title: Docker Guide
description: Run OxPHP with Docker. Covers minimal and multi-stage Dockerfiles, Compose configuration, PHP ini mounts, health checks, and port reference.
---

# Docker Guide

OxPHP is designed to run as a container. This guide covers everything you need to build, configure, and operate OxPHP with Docker — from a minimal single-stage image to a full multi-stage setup with separate development and production targets.

## Minimal Dockerfile

The simplest way to containerize your application:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.2.0

COPY --chown=www-data:www-data . /var/www/html
```

This copies your application into the container and serves it from `/var/www/html/public`. The server listens on port `80` by default.

## Multi-Stage Dockerfile

For real-world applications, use a multi-stage Dockerfile with separate `dev` and `prod` targets. The `dev` target includes PHP CLI, Composer, and Xdebug. The `prod` target builds on the minimal OxPHP image with only what's needed in production.

> **Tip:** A ready-to-use version of this Dockerfile is available at [`Dockerfile.best.example`](../../../Dockerfile.best.example) in the repository root. Copy it into your project and adjust the extensions to match your needs.

```dockerfile
# ── Stage: php-base — shared PHP extensions ──────────────────
FROM php:8.4-zts-alpine3.23 AS php-base

RUN apk add --no-cache \
        icu-dev \
        icu-libs \
        postgresql-dev \
        libpq \
    && docker-php-ext-install \
        pdo \
        pdo_mysql \
        pdo_pgsql \
        intl \
    && apk del icu-dev postgresql-dev

# ── Stage: php-dev — add Xdebug on top of base ───────────────
FROM php-base AS php-dev

RUN apk add --no-cache $PHPIZE_DEPS linux-headers \
    && pecl install xdebug \
    && docker-php-ext-enable xdebug \
    && apk del $PHPIZE_DEPS linux-headers

# ── Stage: composer ───────────────────────────────────────────
FROM composer:2 AS composer

# ── Stage: oxphp — pull OxPHP artifacts ──────────────────────
FROM ghcr.io/oxphp/oxphp:0.2.0 AS oxphp

# ── Target: dev ──────────────────────────────────────────────
# Includes: PHP CLI, Composer, Xdebug, OxPHP binary + extension
FROM php-dev AS dev

RUN apk add --no-cache libgcc

# Composer
COPY --from=composer /usr/bin/composer /usr/local/bin/composer

# OxPHP binary
COPY --from=oxphp /usr/local/bin/oxphp /usr/local/bin/oxphp

# Bridge library
COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/

# OxPHP PHP extension
RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    echo "$EXT_DIR" > /tmp/ext_dir
COPY --from=oxphp /usr/local/lib/php/extensions/ /tmp/oxphp-ext/
RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(cat /tmp/ext_dir)/" && \
    rm -rf /tmp/oxphp-ext /tmp/ext_dir

# PHP config
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini

# Dev-friendly OPcache (validates timestamps)
RUN { \
        echo "[opcache]"; \
        echo "opcache.enable=1"; \
        echo "opcache.enable_cli=1"; \
        echo "opcache.validate_timestamps=1"; \
        echo "opcache.revalidate_freq=0"; \
    } > /usr/local/etc/php/conf.d/opcache-dev.ini

# Xdebug — connect back to host
RUN { \
        echo "[xdebug]"; \
        echo "xdebug.mode=debug"; \
        echo "xdebug.start_with_request=trigger"; \
        echo "xdebug.client_host=host.docker.internal"; \
        echo "xdebug.client_port=9003"; \
    } > /usr/local/etc/php/conf.d/xdebug-config.ini

RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data 2>/dev/null || true
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

ENV LD_LIBRARY_PATH=/usr/local/lib

COPY --chown=www-data:www-data . /var/www/html

EXPOSE 80 443

CMD ["oxphp"]

# ── Stage: prod-extensions — compile extensions for prod ─────
FROM php-base AS prod-extensions

RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    mkdir -p /ext-out && \
    cp "$EXT_DIR"/pdo.so \
       "$EXT_DIR"/pdo_mysql.so \
       "$EXT_DIR"/pdo_pgsql.so \
       "$EXT_DIR"/intl.so \
       /ext-out/

# ── Target: prod — minimal, based on OxPHP image ─────────────
FROM oxphp AS prod

USER root
RUN apk add --no-cache icu-libs libpq

COPY --from=prod-extensions /ext-out/*.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN { \
        echo "extension=pdo_mysql.so"; \
        echo "extension=pdo_pgsql.so"; \
        echo "extension=intl.so"; \
    } > /usr/local/etc/php/conf.d/app-extensions.ini

COPY --chown=www-data:www-data . /var/www/html

USER www-data

EXPOSE 80 443

CMD ["oxphp"]
```

Build each target:

```bash
# Development image (includes PHP CLI, Composer, Xdebug)
docker build --target dev -t myapp:dev .

# Production image (minimal)
docker build --target prod -t myapp:prod .
```

> **Note:** The `dev` target is based on `php:8.4-zts-alpine` with OxPHP copied in, giving you full access to PHP CLI and Composer. The `prod` target is based on the OxPHP image directly, keeping the production image small.

## Docker Compose

### Production

```yaml
services:
  oxphp:
    build:
      context: .
      target: prod
    ports:
      - "80:80"
      - "443:443"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - INDEX_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=info
      - ACCESS_LOG=error
      - PHP_WORKERS=4
      - REQUEST_TIMEOUT_SECONDS=120
      - DRAIN_TIMEOUT_SECONDS=30
      - COMPRESSION_LEVEL=4
    restart: unless-stopped
```

### Development

Mount your source directory as a volume so file changes are reflected without rebuilding. The `dev` target has OPcache timestamp validation enabled, so PHP picks up changes automatically.

```yaml
services:
  oxphp:
    build:
      context: .
      target: dev
    ports:
      - "80:80"
      - "9090:9090"
    volumes:
      - ./src:/var/www/html:ro
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - INDEX_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=debug
      - ACCESS_LOG=all
```

## Volume Mounts

| Host Path | Container Path | Purpose |
|-----------|---------------|---------|
| `./src` | `/var/www/html` | Application files (PHP scripts, static assets). Use `:ro` in production |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | PHP runtime configuration (OPcache, sessions, JIT). Use `:ro` |
| `./certs` | `/etc/ssl/oxphp` | TLS certificate and private key. Use `:ro` |

## Port Reference

| Port | Environment Variable | Purpose |
|------|---------------------|---------|
| `80` | `LISTEN_ADDR` | Main HTTP server |
| `443` | `LISTEN_ADDR` | Main HTTPS server (when TLS is configured) |
| `9090` | `INTERNAL_ADDR` | Internal server: `/health`, `/metrics`, `/config` |

> **Note:** The internal server is disabled by default. Set `INTERNAL_ADDR` to enable it. In production, keep the internal port reachable only by your orchestrator or monitoring system — do not expose it publicly.

## PHP Configuration

Customize PHP settings by creating an `oxphp.ini` file and mounting it into the container. This is the recommended way to configure OPcache, JIT, sessions, and other PHP runtime settings.

```ini
zend_extension=opcache

[opcache]
opcache.enable = 1
opcache.enable_cli = 1
opcache.memory_consumption = 128
opcache.interned_strings_buffer = 16
opcache.max_accelerated_files = 10000
opcache.validate_timestamps = 0
opcache.jit_buffer_size = 64M
opcache.jit = tracing

[Session]
session.save_path = /tmp
session.use_cookies = 1
session.use_only_cookies = 1
```

In development, set `opcache.validate_timestamps = 1` and `opcache.revalidate_freq = 0` so PHP picks up file changes without a container restart.

See [OPcache](../php/opcache.md) for recommended settings and JIT configuration.

## Health Checks

Add a Docker health check to let Docker or your orchestrator monitor container health. This requires `INTERNAL_ADDR` to be set.

In `compose.yaml`:

```yaml
services:
  oxphp:
    environment:
      - INTERNAL_ADDR=0.0.0.0:9090
    healthcheck:
      test: ["CMD", "wget", "--quiet", "--tries=1", "--spider", "http://localhost:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

In a `Dockerfile`:

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=5s \
    CMD wget --quiet --tries=1 --spider http://localhost:9090/health || exit 1
```

The `/health` endpoint returns `200` when the server is healthy and `503` when degraded. The response JSON includes uptime, total request count, and active connection count. For Kubernetes, use the same endpoint as both a liveness and readiness probe.

## What's Next

- [Configuration](../operations/configuration.md) — full environment variable reference
- [Routing](../features/routing.md) — Traditional, Framework, SPA, and Worker routing modes
- [Worker Mode](../features/worker-mode.md) — persistent PHP processes for framework applications
- [TLS](../features/tls.md) — HTTPS with built-in TLS termination
- [Health Checks](../operations/health-checks.md) — health endpoint details and Kubernetes integration
- [Graceful Shutdown](../operations/graceful-shutdown.md) — drain behavior and shutdown sequence
