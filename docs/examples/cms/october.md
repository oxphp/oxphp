---
title: October CMS on OxPHP
description: Run October CMS 4 on OxPHP in framework routing mode with a public/ mirror and SYMLINK_ALLOW_PATHS — Dockerfile, Compose file, install steps, and OxPHP-specific notes.
---

# October CMS on OxPHP

October CMS (Laravel-based) ships with `index.php` at the project root, next to `config/`, `.env`, and `vendor/`. Its nginx config rewrites everything to `index.php` and serves static **only** from whitelisted asset paths (`*/assets`). To get the same posture on OxPHP — without exposing theme source or config — this recipe builds a `public/` document root with `php artisan october:mirror public` and points framework mode at it.

This is the recipe that exercises OxPHP's [`SYMLINK_ALLOW_PATHS`](../../security/symlink-allow-paths.md).

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.11.0-php8.4-alpine3.23` (PHP 8.4)
- **Routing mode:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT` left at the default `/var/www/html/public`)
- **Extensions added:** `gd`, `pdo_mysql`, `mbstring`, `zip`
- **Services:** OxPHP + MySQL
- **URL:** `http://localhost:8098` · backend `/admin` · internal `http://localhost:9099/health`

> **No license needed.** `october/october` installs straight from Packagist. The Laravel-11 component stack predates PHP 8.5, so this pins the **PHP 8.4** image.

## Project layout

```bash
mkdir -p october-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/october-oxphp/src":/app -w /app \
    composer:2 create-project october/october . --no-interaction --prefer-dist
```

Then point `src/.env` at the database service (`DB_HOST=db`, `DB_DATABASE=october`, …) and set `APP_URL=http://localhost:8098`.

## Dockerfile

`src/Dockerfile.oxphp` (named to avoid colliding with October's own assets):

```dockerfile
ARG OXPHP_VERSION=0.11.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP base: October runtime extensions ──────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache \
        libpng libjpeg-turbo freetype oniguruma libzip \
    && apk add --no-cache --virtual .build-deps \
        libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev libzip-dev \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" gd pdo_mysql mbstring zip \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP artifacts (PHP 8.4 image) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

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
RUN { \
        echo "memory_limit=256M"; echo "upload_max_filesize=64M"; \
        echo "post_max_size=64M"; echo "max_execution_time=300"; \
    } > /usr/local/etc/php/conf.d/october.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html
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
    image: october-oxphp/app:dev
    container_name: october-oxphp
    ports:
      - "8098:80"
      - "9099:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR (0.0.0.0:80) and DOCUMENT_ROOT (/var/www/html/public) are
      # the OxPHP defaults, so both are omitted.
      ENTRY_FILE: index.php          # framework mode
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      # The public/ mirror is built from symlinks into the project tree;
      # allow OxPHP to follow them out of the document root.
      SYMLINK_ALLOW_PATHS: /var/www/html
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
    container_name: october-oxphp-db
    ports:
      - "3313:3306"
    command:
      - --max_allowed_packet=64M
    environment:
      MYSQL_DATABASE: october
      MYSQL_USER: october
      MYSQL_PASSWORD: october
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

October CMS 4 has no `october:install` wizard command and seeds no default admin, so the sequence is migrate → build content tables → mirror → create admin:

```bash
docker compose up -d db

# 1. Schema and module seeds
docker compose run --rm app php artisan october:migrate

# 2. Dynamic content tables (the demo theme uses Tailor content)
docker compose run --rm app php artisan tailor:migrate

# 3. Build the public/ document root (index.php + */assets symlinks)
docker compose run --rm app sh -c 'mkdir -p public && php artisan october:mirror public --relative'

# 4. Create a backend super-user (no install wizard exists)
docker compose run --rm app php artisan tinker --execute '
  $u = Backend\Models\User::firstOrNew(["login" => "admin"]);
  $u->email = "admin@example.com"; $u->password = "admin123";
  $u->password_confirmation = "admin123"; $u->is_superuser = true;
  $u->is_activated = true; $u->save();'

docker compose up -d app
```

## OxPHP notes

- **`october:mirror public` reproduces October's nginx asset whitelist.** It creates a `public/` directory holding only `index.php` (a symlink) and symlinks to the `*/assets` directories — theme `.htm` pages, `config/`, `.env`, `vendor/`, and `storage/logs` stay **outside** the document root. With `DOCUMENT_ROOT=…/public`, none of them are reachable over HTTP.
- **`SYMLINK_ALLOW_PATHS=/var/www/html` is required.** OxPHP blocks symlinks that resolve outside `DOCUMENT_ROOT` by default. The mirror's symlinks point back into the project tree (`/var/www/html/modules/…`, `/var/www/html/themes/…`), so the project root is whitelisted. `/var` is exact-blacklisted but not prefix-blacklisted, so `/var/www/html` is allowed. See [Symlink Allow Paths](../../security/symlink-allow-paths.md).
- **`tailor:migrate` is easy to forget.** Without it, the demo theme throws `SQLSTATE 1146: table 'xc_…' doesn't exist`, because its content collections are Tailor blueprints whose tables are built separately from `october:migrate`.

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/                       # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/admin                  # 302 → login
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/themes/demo/assets/css/theme.css  # 200
# the document-root mirror keeps internals out:
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/.env                   # 404
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/themes/demo/pages/home.htm  # 404
```

## See also

- [Routing](../../features/routing.md) · [Symlink Allow Paths](../../security/symlink-allow-paths.md) · [Dot-Path Blocking](../../security/dot-path-blocking.md)
- [Drupal](drupal.md) and [Craft CMS](craft.md) — the other framework-mode CMS recipes
