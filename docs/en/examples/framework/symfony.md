---
title: Symfony on OxPHP
description: Run a Symfony application on OxPHP in framework routing mode with no database — Dockerfile, Compose file, install steps, and OxPHP-specific notes.
---

# Symfony on OxPHP

Symfony's `public/index.php` front controller fits OxPHP's [framework routing mode](../../features/routing.md) directly. The `symfony/skeleton` needs no database, so this is a single-service deployment — add Doctrine and a `db` service later if you wire persistence.

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.10.0` (PHP 8.5)
- **Routing mode:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT` left at the default `/var/www/html/public`)
- **Extensions added:** `intl`, `mbstring` (Symfony's recommended baseline; the runtime strictly needs only core extensions)
- **Services:** OxPHP only
- **URL:** `http://localhost:8096` · internal `http://localhost:9097/health`

> **PHP version:** Symfony 8 declares `"php": ">=8.4"`, which the default PHP 8.5 image satisfies. Confirm a dependency does not cap the upper bound with `composer check-platform-reqs --no-dev`.

## Project layout

```bash
mkdir -p symfony-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/symfony-oxphp/src":/app -w /app \
    composer:2 create-project symfony/skeleton . --prefer-dist
# optional, for a rendered page:
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/symfony-oxphp/src":/app -w /app \
    composer:2 require twig symfony/asset
```

## Dockerfile

`src/Dockerfile`:

```dockerfile
ARG OXPHP_VERSION=0.10.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── PHP base: intl + mbstring ─────────────────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache icu-libs oniguruma \
    && apk add --no-cache --virtual .build-deps icu-dev oniguruma-dev \
    && docker-php-ext-install -j"$(nproc)" intl mbstring \
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

## docker-compose.yml

```yaml
services:
  app:
    build:
      context: ./src
      target: dev
    image: symfony-oxphp/app:dev
    container_name: symfony-oxphp
    ports:
      - "8096:80"
      - "9097:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR (0.0.0.0:80) and DOCUMENT_ROOT (/var/www/html/public) are
      # the OxPHP defaults, so both are omitted.
      ENTRY_FILE: index.php          # framework mode
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      APP_ENV: dev
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
docker compose exec app php bin/console about
```

## OxPHP notes

- **Minimal extension footprint.** The Symfony skeleton's runtime needs only core extensions (`ctype`, `iconv`, `xml`), all present in the base image — `mbstring` is otherwise polyfilled. Native `intl` + `mbstring` are added because any real Symfony app uses them and the native versions are faster than the polyfill.
- **`APP_ENV` from the container environment.** Symfony reads it from the real environment over `.env`, so set it in Compose.
- **Static assets are served from `public/` by framework mode** — `{{ asset('css/app.css') }}` resolves to a real file on disk and is served directly, falling through to `index.php` only on a miss.

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8096/             # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8096/no-such-route # 404 (Symfony router)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9097/health        # 200
```

## See also

- [Routing](../../features/routing.md) · [Docker Guide](../../getting-started/docker.md)
- [Laravel](laravel.md) and [Yii3](yii3.md) — the other framework-mode recipes
