---
title: Docker Guide
description: Run OxPHP with Docker. Covers minimal and multi-stage Dockerfiles, Compose configuration, PHP ini mounts, health checks, and port reference.
---

# Docker Guide

OxPHP is designed to run as a container. This guide covers everything you need to build, configure, and operate OxPHP with Docker — from a minimal single-stage image to a full multi-stage setup with separate development and production targets.

## Minimal Dockerfile

The simplest way to containerize your application:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.6.0

COPY --chown=www-data:www-data . /var/www/html/public
```

This copies your application into the container. The default `DOCUMENT_ROOT` is `/var/www/html/public` — for Laravel, Symfony, or any project that already ships a `public/` subdirectory, use `COPY --chown=www-data:www-data . /var/www/html` instead so the framework's own `public/` aligns with the default. The server listens on port `80` by default.

## Multi-Stage Dockerfile

For real-world applications, use a multi-stage Dockerfile with separate `dev` and `prod` targets. The `dev` target includes PHP CLI, Composer, and Xdebug. The `prod` target builds on the minimal OxPHP image with only what's needed in production.

> **Tip:** A ready-to-use version of this Dockerfile is available at [`examples/dockerfile/Dockerfile`](../../../examples/dockerfile/Dockerfile) in the repository. Copy it into your project and adjust the extensions to match your needs.

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
FROM ghcr.io/oxphp/oxphp:0.6.0 AS oxphp

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

# Framework layout: project ships a public/ subdir (Laravel/Symfony/Slim).
# For a bare index.php at the project root, copy into /var/www/html/public instead.
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

# Framework layout: project ships a public/ subdir (Laravel/Symfony/Slim).
# For a bare index.php at the project root, copy into /var/www/html/public instead.
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

> **Note:** The `dev` target is based on `php:8.4-zts-alpine` (swap `8.4` for `8.5` to match an OxPHP `:*-php8.5*` tag) with OxPHP copied in, giving you full access to PHP CLI and Composer. The `prod` target is based on the OxPHP image directly, keeping the production image small.

## Installing PHP Extensions in Production

Starting with OxPHP 0.3.0, the production image ships the full PHP toolchain (`php`, `docker-php-ext-install`, `phpize`) inherited from `php:8.4-zts-alpine` (or `php:8.5-zts-alpine` for the `:*-php8.5*` variants) and does **not** set a `USER` directive. Downstream Dockerfiles can install PHP extensions directly — no `USER` toggle required.

### Quick-start pattern (single stage)

The shortest useful example:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.6.0

RUN docker-php-ext-install mysqli pdo_mysql

COPY --chown=www-data:www-data . /var/www/html/public

CMD ["oxphp"]
```

`--chown=www-data:www-data` on the `COPY` is important: files are owned by `www-data` (uid 82) inside the image, so an orchestrator-level `--user www-data` drop lands on a webroot the unprivileged process can read and (where needed) write to.

The container starts as root. In production, drop privileges at the orchestrator level (see the security note below).

### Best practice pattern (two stages, smaller image)

For the smallest possible final image, compile extensions in a dedicated builder stage and copy only the compiled `.so` files into the runtime stage. The [Multi-Stage Dockerfile](#multi-stage-dockerfile) walkthrough above uses `FROM ghcr.io/oxphp/oxphp:0.6.0 AS prod` — simple, portable, and recommended as a starting point.

`examples/dockerfile/Dockerfile` in the repository goes further: its `prod` target is based on bare `alpine` with an explicit `apk` dependency list, copying only the `oxphp` binary, `libphp.so`, compiled PHP extensions, and the required shared libraries. This cuts the base image from ~188 MB down to ~76 MB (~60% reduction, excluding your app code) at the cost of tracking PHP/Alpine version bumps in the `apk` list. The same file also ships a `prod-cli` target — a short-lived image for `php artisan migrate`, Composer, and other maintenance commands that should stay out of the serving path.

Note: the walkthrough above still shows explicit `USER root` / `USER www-data` toggles for defense-in-depth — with v0.3.0 they are optional since the base image no longer sets `USER`.

### Running CLI tools and migrations

The same prod image can run `php` CLI commands for migrations, Composer, or ad-hoc inspection. `docker run` replaces the default CMD with the command you pass — the container runs the command and exits, it does not also start the OxPHP server.

```bash
# Run Laravel migrations against the prod image.
# Container runs as root by default — the CLI has write access to
# root-owned mounted volumes.
docker run --rm \
    -v "$(pwd):/var/www/html" \
    ghcr.io/oxphp/oxphp:0.6.0 \
    php artisan migrate

# If the mounted volume is owned by www-data, pass Docker's --user:
docker run --rm --user www-data \
    -v "$(pwd):/var/www/html" \
    ghcr.io/oxphp/oxphp:0.6.0 \
    php artisan migrate
```

`docker exec <container> docker-php-ext-install <ext>` also works on a running container without any additional flags — useful for debugging a live container. For production, persist the extension in your Dockerfile so it survives restarts.

### Security note

The prod image has no `USER` directive, so the container runs as root by default. This is intentional and matches `nginx:alpine` / `php:*-fpm-alpine` / `frankenphp:alpine` conventions. In production **you must** drop privileges at the orchestrator level:

- **Docker:** `docker run --user www-data ghcr.io/oxphp/oxphp:0.6.0`
- **Compose:**
  ```yaml
  services:
    oxphp:
      image: ghcr.io/oxphp/oxphp:0.6.0
      user: www-data
  ```
- **Kubernetes:**
  ```yaml
  securityContext:
    runAsNonRoot: true
    runAsUser: 82
    runAsGroup: 82
  ```
  `runAsNonRoot: true` is defense-in-depth: if `runAsUser` is ever removed or overridden to `0`, the kubelet rejects the pod instead of silently running as root.

The `www-data` user (uid 82, gid 82) is pre-created by the base image, and `/var/www/html` is chowned to it at build time, so any of these drop paths lands on a readable webroot.

### Run as non-root on port 80 (`serve --user`)

Dropping privileges at the orchestrator level (above) has one limitation: a process that starts as `www-data` cannot bind a privileged port (below 1024). To serve on `:80`/`:443` you would otherwise need `CAP_NET_BIND_SERVICE`, a `su-exec`-style entrypoint, or a high port (such as `:8080`) behind a port mapping.

`oxphp serve --user=<spec>` collapses this into a single process: OxPHP binds the listeners **as root**, then permanently drops to the target user **before** any connection is accepted or any PHP worker runs. You get a privileged port *and* non-root request handling without extra capabilities.

Start the container **as root** — do not also set `user:`, because the bind needs root — and pass the flag via `command`:

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.6.0
    command: ["oxphp", "serve", "--user=www-data"]
    ports:
      - "80:80"
      - "443:443"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
```

`<spec>` accepts a user name, `name:group`, a numeric `uid`, or `uid:gid`. The drop runs `initgroups → setgid → setuid`, verifies that root cannot be regained, and is irreversible; on Linux it also sets `no_new_privs`. If the process is **not** started as root, `serve --user` exits with an error rather than silently continuing as root.

> Choose **one** model, not both. Use the orchestrator drop (`user:` / `runAsUser`) when a high port or an external load balancer terminates `:80`. Use `serve --user` when you want OxPHP itself to own the privileged bind. Setting `user:` **and** `serve --user` makes the bind fail — the container is no longer root.

**File-permission checklist for the dropped user.** After the drop, everything OxPHP touches at runtime must be accessible to `<spec>`:

| Resource | Requirement |
|----------|-------------|
| `DOCUMENT_ROOT` | Readable. `/var/www/html` is chowned to `www-data` at image build, so this is satisfied by default. |
| Session save path (`session.save_path`, default `/tmp`) | Writable. |
| Upload tmp dir (`upload_tmp_dir`) | Writable, when file uploads are used. |
| OPcache file cache (`opcache.file_cache`) | Writable, when a secondary file cache is enabled. |
| File-based access log | Writable, when logging to a file rather than stdout. |
| TLS private key (`TLS_KEY`) | **Readable by the dropped user** — group- or world-readable, not root-only `0600`. The key is read *after* the drop, so a root-only key makes TLS startup fail. |

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
      - ENTRY_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=info
      - ACCESS_LOG=error
      - PHP_WORKERS=4
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
      - ./custom.ini:/usr/local/etc/php/conf.d/custom.ini:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - ENTRY_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=debug
      - ACCESS_LOG=all
```

## Volume Mounts

| Host Path | Container Path | Purpose |
|-----------|---------------|---------|
| `./src` | `/var/www/html` | Application files (PHP scripts, static assets). Use `:ro` in production |
| `./custom.ini` | `/usr/local/etc/php/conf.d/custom.ini` | PHP runtime configuration (OPcache, sessions, JIT). Use `:ro` |
| `./certs` | `/etc/ssl/oxphp` | TLS certificate and private key. Use `:ro` |

## Port Reference

| Port | Environment Variable | Purpose |
|------|---------------------|---------|
| `80` | `LISTEN_ADDR` | Main HTTP server |
| `443` | `LISTEN_ADDR` | Main HTTPS server (when TLS is configured) |
| `9090` | `INTERNAL_ADDR` | Internal server: `/health`, `/metrics`, `/config` |

> **Note:** The internal server is disabled by default. Set `INTERNAL_ADDR` to enable it. In production, keep the internal port reachable only by your orchestrator or monitoring system — do not expose it publicly.

## PHP Configuration

Customize PHP settings by creating an `custom.ini` file and mounting it into the container. This is the recommended way to configure OPcache, JIT, sessions, and other PHP runtime settings.

```ini
; Do NOT add zend_extension=opcache — OPcache is already compiled into the
; PHP ZTS base image. The [opcache] section below configures it directly.

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

> **Note:** Do not add `zend_extension=opcache` to this file. OPcache is already built into the PHP ZTS image used by OxPHP. Adding a `zend_extension` line will produce a warning on every request startup.

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
