---
title: OpenCart on OxPHP
description: Run OpenCart 4 on OxPHP in traditional routing mode with MySQL and PHP_DENY_PATHS — Dockerfile, Compose file, CLI install, and OxPHP-specific notes.
---

# OpenCart on OxPHP

OpenCart is a **traditional-mode** application with two physical front controllers: `index.php` (storefront) and `admin/index.php` (admin panel), plus static assets under `image/` and `catalog/view/`. Its webroot is the project root itself. OxPHP's [traditional routing mode](../../features/routing.md) — the default with no `ENTRY_FILE` — serves both entry points as real files and serves static assets from disk; `PHP_DENY_PATHS` blocks direct execution of the framework internals.

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.10.0-php8.4-alpine3.23` (PHP 8.4)
- **Routing mode:** Traditional (no `ENTRY_FILE`; `DOCUMENT_ROOT` = project root, overrides the default `…/public`)
- **Extensions added:** `gd`, `mysqli`, `zip`, `mbstring`
- **Services:** OxPHP + MySQL
- **URL:** `http://localhost:8095` · admin `/admin/` · internal `http://localhost:9096/health`

> **PHP version:** OpenCart 4 requires PHP 8.0+ but its codebase predates 8.5; pin the **PHP 8.4** image. OpenCart uses the **`mysqli`** driver, not PDO. (Note: OpenCart 3.x will not run on PHP 8.4+ — use OpenCart 4.x.)

## Project layout

OpenCart ships as a release archive, not a Composer package. The webroot is the contents of the archive's `upload/` directory:

```bash
mkdir -p opencart-oxphp/src
curl -sL https://github.com/opencart/opencart/releases/download/4.1.0.3/opencart-4.1.0.3.zip -o /tmp/oc.zip
unzip -q /tmp/oc.zip -d /tmp/oc
cp -a /tmp/oc/upload/. opencart-oxphp/src/
# the installer needs these to exist and be writable:
: > opencart-oxphp/src/config.php
: > opencart-oxphp/src/admin/config.php
```

## Dockerfile

`src/Dockerfile.oxphp` (named to avoid colliding with OpenCart's bundled Dockerfile):

```dockerfile
ARG OXPHP_VERSION=0.10.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP base: OpenCart runtime extensions ─────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache \
        libpng libjpeg-turbo freetype oniguruma libzip \
    && apk add --no-cache --virtual .build-deps \
        libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev libzip-dev \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" gd mysqli zip mbstring \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP artifacts (PHP 8.4 image) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev: OxPHP server + PHP CLI + Composer ────────────────────
FROM php-base AS dev
RUN apk add --no-cache libgcc
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
    } > /usr/local/etc/php/conf.d/opencart.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html && chown -R www-data:www-data /var/www/html
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
      dockerfile: Dockerfile.oxphp
      target: dev
    image: opencart-oxphp/app:dev
    container_name: opencart-oxphp
    ports:
      - "8095:80"
      - "9096:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR defaults to 0.0.0.0:80, so it is omitted.
      DOCUMENT_ROOT: /var/www/html         # webroot IS the project root (overrides default /…/public)
      INTERNAL_ADDR: 0.0.0.0:9090          # (no ENTRY_FILE → traditional mode)
      ACCESS_LOG: all
      # Block direct HTTP execution of framework internals and the web installer.
      PHP_DENY_PATHS: "/system/**,/install/**"
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
    container_name: opencart-oxphp-db
    ports:
      - "3311:3306"
    environment:
      MYSQL_DATABASE: opencart
      MYSQL_USER: opencart
      MYSQL_PASSWORD: opencart
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

OpenCart 4 ships a CLI installer:

```bash
docker compose up -d db
docker compose run --rm app php install/cli_install.php install \
    --username admin --password 'admin123' --email admin@example.com \
    --http_server 'http://localhost:8095/' --language en-gb \
    --db_driver mysqli --db_hostname db --db_username opencart \
    --db_password opencart --db_database opencart --db_port 3306 --db_prefix oc_
# OpenCart recommends removing the installer afterwards:
rm -rf opencart-oxphp/src/install
docker compose up -d app
```

## OxPHP notes

- **Traditional mode serves two front controllers.** `index.php` (storefront) and `admin/index.php` (admin) are both physical entry points; traditional mode runs each as a real file. Framework mode would funnel everything to one `index.php` and break the admin.
- **`PHP_DENY_PATHS` protects the internals.** With the webroot at the project root, `system/` and `install/` sit inside `DOCUMENT_ROOT`. `PHP_DENY_PATHS=/system/**,/install/**` blocks direct HTTP execution of those scripts — matched before disk I/O, so there is no existence oracle. The CLI installer runs over the shell and is unaffected. See [PHP Execution Deny-List](../../security/php-deny.md).
- **Dotfiles are already safe.** `.env`-style and dot-segment paths return `404` via [dot-path blocking](../../security/dot-path-blocking.md); no rule needed.
- **SEO URLs are off by default.** Default OpenCart uses `index.php?route=…` query routing, which traditional mode handles directly with no rewrite rules.

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/                       # 200 storefront
curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:8095/index.php?route=common/home"  # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/admin/                 # 200 admin login
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/system/startup.php     # 404 (denied)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9096/health                  # 200
```

## See also

- [Routing](../../features/routing.md) · [PHP Execution Deny-List](../../security/php-deny.md) · [Dot-Path Blocking](../../security/dot-path-blocking.md)
- [WordPress](../cms/wordpress.md) — the other traditional-mode recipe
