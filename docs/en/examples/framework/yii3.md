---
title: Yii3 on OxPHP
description: Run a Yii3 (yiisoft/app) application on OxPHP in framework routing mode with no database — the leanest example. Dockerfile, Compose file, and OxPHP-specific notes.
---

# Yii3 on OxPHP

Yii3 is the leanest recipe here. The `yiisoft/app` template has a `public/index.php` front controller, needs no database, and — because most string handling is polyfilled — needs essentially no extra PHP extensions. It maps onto OxPHP's [framework routing mode](../../features/routing.md) and runs as a single service.

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.9.0` (PHP 8.5)
- **Routing mode:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT` left at the default `/var/www/html/public`)
- **Extensions added:** `mbstring` (native, for speed — the runtime otherwise polyfills it)
- **Services:** OxPHP only
- **URL:** `http://localhost:8094` · internal `http://localhost:9095/health`

> `yiisoft/app` declares `"php": "8.2 - 8.5"`, so the default PHP 8.5 image fits.

## Project layout

```bash
mkdir -p yii3-oxphp/src
docker run --rm -v "$PWD/yii3-oxphp/src":/app -w /app \
    composer:2 create-project yiisoft/app . --prefer-dist
cp yii3-oxphp/src/.env.example yii3-oxphp/src/.env
```

## Dockerfile

`src/Dockerfile`:

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── PHP base: native mbstring ─────────────────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache oniguruma \
    && apk add --no-cache --virtual .build-deps oniguruma-dev \
    && docker-php-ext-install -j"$(nproc)" mbstring \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP artifacts (PHP 8.5 default image) ───────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

# ── dev: OxPHP server + PHP CLI + Composer ────────────────────
FROM php-base AS dev
RUN apk add --no-cache libgcc git
COPY --from=composer /usr/bin/composer /usr/local/bin/composer
COPY --from=oxphp /usr/local/bin/oxphp              /usr/local/bin/oxphp
COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=oxphp /usr/local/lib/php/extensions/ /tmp/oxphp-ext/
RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(php -r 'echo ini_get("extension_dir");')/" \
    && rm -rf /tmp/oxphp-ext \
    && echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini
RUN { \
        echo "[opcache]"; echo "opcache.enable=1"; echo "opcache.enable_cli=1"; \
        echo "opcache.validate_timestamps=1"; echo "opcache.revalidate_freq=0"; \
    } > /usr/local/etc/php/conf.d/opcache-dev.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html
ENV LD_LIBRARY_PATH=/usr/local/lib
WORKDIR /var/www/html
COPY --chown=www-data:www-data . /var/www/html
EXPOSE 80 9090
CMD ["oxphp"]
```

> The `yiisoft/app` template ships its own whitelist-style `.dockerignore`; keep it — with a bind-mounted `src/`, the `COPY` is only a fallback.

## docker-compose.yml

```yaml
services:
  app:
    build:
      context: ./src
      target: dev
    image: yii3-oxphp/app:dev
    container_name: yii3-oxphp
    ports:
      - "8094:80"
      - "9095:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR (0.0.0.0:80) and DOCUMENT_ROOT (/var/www/html/public) are
      # the OxPHP defaults, so both are omitted.
      ENTRY_FILE: index.php          # framework mode
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "wget", "-q", "--spider", "http://0.0.0.0:9090/health"]
      interval: 10s
      timeout: 3s
      retries: 5
      start_period: 5s
```

## Install and first run

```bash
docker compose up -d --build
docker compose exec app php yii          # the Yii console
```

## OxPHP notes

- **No database, no extra services** — the smallest possible OxPHP deployment: one image, one process.
- **`yiisoft/assets` publishes static into `public/assets/<hash>/`** — real files on disk, served directly by framework mode. No URL-rewrite gymnastics are needed (unlike some asset pipelines that embed a version segment in the URL).

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8094/              # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8094/nonexistent   # 404 (Yii router)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9095/health         # 200
```

## See also

- [Routing](../../features/routing.md) · [Docker Guide](../../getting-started/docker.md)
- [Laravel](laravel.md) and [Symfony](symfony.md) — the other framework-mode recipes
