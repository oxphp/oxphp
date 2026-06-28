---
title: Drupal on OxPHP
description: Run Drupal 11 on OxPHP in framework routing mode with MySQL and drush — Dockerfile, Compose file, install steps, and OxPHP-specific notes.
---

# Drupal on OxPHP

Drupal 11 uses `web/index.php` as a front controller with clean URLs (every non-file path rewrites to `index.php`), which is exactly OxPHP's [framework routing mode](../../features/routing.md). Aggregated CSS/JS land in `web/sites/default/files/` and are served from disk like any other static asset.

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23` (PHP 8.4)
- **Routing mode:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT=/var/www/html/web` overrides the default `…/public`)
- **Extensions added:** `gd`, `pdo_mysql`, `zip`, `mbstring`
- **Services:** OxPHP + MySQL
- **URL:** `http://localhost:8097` · internal `http://localhost:9098/health`

> **PHP version:** Drupal 11 requires PHP 8.3+ and officially supports 8.4. Its Symfony-component stack predates PHP 8.5, so this recipe pins the **PHP 8.4** OxPHP image. Drupal uses **PDO** (`pdo_mysql`), not `mysqli`.

## Project layout

```bash
mkdir -p drupal-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/drupal-oxphp/src":/app -w /app \
    composer:2 create-project drupal/recommended-project . \
    --ignore-platform-reqs --no-interaction --prefer-dist
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/drupal-oxphp/src":/app -w /app \
    composer:2 require drush/drush --ignore-platform-reqs --no-interaction
```

> `--ignore-platform-reqs` is needed because the stock `composer:2` image has no `gd`; the extension is only required at runtime, where the OxPHP image provides it.

## Dockerfile

`src/Dockerfile`:

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP base: Drupal runtime extensions ───────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache \
        libpng libjpeg-turbo freetype oniguruma libzip \
    && apk add --no-cache --virtual .build-deps \
        libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev libzip-dev \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" gd pdo_mysql zip mbstring \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP artifacts (PHP 8.4 image) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev: OxPHP server + PHP CLI + Composer + drush ────────────
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
RUN { \
        echo "memory_limit=256M"; echo "upload_max_filesize=64M"; \
        echo "post_max_size=64M"; echo "max_execution_time=300"; \
    } > /usr/local/etc/php/conf.d/drupal.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/web && chown -R www-data:www-data /var/www/html
ENV LD_LIBRARY_PATH=/usr/local/lib
WORKDIR /var/www/html
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
    image: drupal-oxphp/app:dev
    container_name: drupal-oxphp
    ports:
      - "8097:80"
      - "9098:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR defaults to 0.0.0.0:80, so it is omitted.
      DOCUMENT_ROOT: /var/www/html/web   # overrides the default /var/www/html/public
      ENTRY_FILE: index.php              # framework mode
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
    depends_on:
      db:
        condition: service_healthy
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "wget", "-q", "--spider", "http://0.0.0.0:9090/health"]
      interval: 10s
      timeout: 3s
      retries: 5
      start_period: 5s

  db:
    image: mysql:8.0
    container_name: drupal-oxphp-db
    ports:
      - "3312:3306"
    command:
      - --max_allowed_packet=64M
    environment:
      MYSQL_DATABASE: drupal
      MYSQL_USER: drupal
      MYSQL_PASSWORD: drupal
      MYSQL_ROOT_PASSWORD: root
    volumes:
      - db_data:/var/lib/mysql
    healthcheck:
      test: ["CMD", "mysqladmin", "ping", "-h", "127.0.0.1", "-uroot", "-proot"]
      interval: 5s
      timeout: 5s
      retries: 30
      start_period: 15s
    restart: unless-stopped

volumes:
  db_data:
```

## Install and first run

`drush` is the cleanest non-interactive installer:

```bash
docker compose up -d --build
docker compose exec app vendor/bin/drush site:install standard \
    --db-url=mysql://drupal:drupal@db:3306/drupal \
    --account-name=admin --account-pass=admin123 \
    --site-name="Drupal OxPHP" -y
```

## OxPHP notes

- **Clean URLs are framework mode.** Drupal's `RewriteCond !-f` + `RewriteRule ^ index.php` is precisely what framework mode does — serve existing files, dispatch everything else to `web/index.php`.
- **Aggregated assets are served from disk.** Drupal writes combined CSS/JS to `web/sites/default/files/css` and `…/js`; OxPHP serves them as static files.
- **First request is slow, then fast.** The cold first request warms Twig compilation, asset aggregation, and caches (≈15 s). Subsequent requests are single-digit milliseconds. This is Drupal warmup, not OxPHP overhead.

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8097/           # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8097/user/login # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9098/health      # 200
```

## See also

- [Routing](../../features/routing.md) · [Docker Guide](../../getting-started/docker.md)
- [Craft CMS](craft.md) and [October CMS](october.md) — the other framework-mode CMS recipes
