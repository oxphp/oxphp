---
title: Craft CMS on OxPHP
description: Run Craft CMS 5 on OxPHP in framework routing mode with MySQL — Dockerfile, Compose file, console-driven install, and OxPHP-specific notes.
---

# Craft CMS on OxPHP

Craft CMS uses `web/index.php` as its front controller and serves control-panel resources from `web/cpresources/`. It runs in OxPHP's [framework routing mode](../../features/routing.md): existing static files (including the control-panel assets) are served from disk, everything else is dispatched to `index.php`.

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.10.0` (PHP 8.5)
- **Routing mode:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT=/var/www/html/web` overrides the default `…/public`)
- **Extensions added:** `bcmath`, `gd`, `intl`, `pdo_mysql`, `mbstring`, `zip`
- **Services:** OxPHP + MySQL (8.4 LTS)
- **URL:** `http://localhost:8092` · control panel `/admin` · internal `http://localhost:9093/health`

## Project layout

```bash
mkdir -p craft-oxphp/src
docker run --rm -v "$PWD/craft-oxphp/src":/app -w /app \
    composer:2 create-project craftcms/craft . \
    --ignore-platform-reqs --no-scripts --no-interaction --prefer-dist
```

> `--ignore-platform-reqs --no-scripts` is needed because the stock `composer:2` image lacks `bcmath`/`gd`/`intl`; those are only required at runtime, and Craft's post-create setup wizard runs later, inside the built container.

Craft reads its database connection from the environment, so no `.env` editing is required beyond the keys the console writes.

## Dockerfile

`src/Dockerfile`:

```dockerfile
ARG OXPHP_VERSION=0.10.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── PHP base: Craft runtime extensions ────────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache \
        icu-libs libpng libjpeg-turbo freetype oniguruma libzip \
    && apk add --no-cache --virtual .build-deps \
        icu-dev libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev libzip-dev \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" bcmath gd intl pdo_mysql mbstring zip \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP artifacts ───────────────────────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

# ── dev: OxPHP server + PHP CLI + Composer + Craft extensions ──
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
        echo "post_max_size=64M"; echo "max_execution_time=120"; \
    } > /usr/local/etc/php/conf.d/craft.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/web && chown -R www-data:www-data /var/www/html
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
    image: craft-oxphp/app:dev
    container_name: craft-oxphp
    ports:
      - "8092:80"
      - "9093:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR defaults to 0.0.0.0:80, so it is omitted.
      DOCUMENT_ROOT: /var/www/html/web   # overrides the default /var/www/html/public
      ENTRY_FILE: index.php              # framework mode
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      CRAFT_DB_DRIVER: mysql
      CRAFT_DB_SERVER: db
      CRAFT_DB_PORT: "3306"
      CRAFT_DB_DATABASE: craft
      CRAFT_DB_USER: craft
      CRAFT_DB_PASSWORD: craft
      PRIMARY_SITE_URL: http://localhost:8092
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
    image: mysql:8.4
    container_name: craft-oxphp-db
    ports:
      - "3309:3306"
    environment:
      MYSQL_DATABASE: craft
      MYSQL_USER: craft
      MYSQL_PASSWORD: craft
      MYSQL_ROOT_PASSWORD: root
    volumes:
      - db_data:/var/lib/mysql
    healthcheck:
      test: ["CMD", "mysqladmin", "ping", "-h", "127.0.0.1", "-uroot", "-proot"]
      interval: 5s
      timeout: 5s
      retries: 20
      start_period: 15s
    restart: unless-stopped

volumes:
  db_data:
```

## Install and first run

Craft is installed through its console, which lives in the same container:

```bash
docker compose up -d --build
docker compose exec app php craft setup/security-key   # writes CRAFT_SECURITY_KEY
docker compose exec app php craft setup/app-id         # writes CRAFT_APP_ID
docker compose exec app php craft install/craft \
    --username=admin --email=admin@example.com --password=password \
    --site-name="Craft OxPHP" --site-url='http://localhost:8092' --language=en-US
```

## OxPHP notes

- **Control-panel resources are static files.** `web/cpresources/<hash>/…` exists on disk and is served by framework mode — verify it returns `200`, otherwise the control panel would render unstyled.
- **Use MySQL 8.4 LTS, not MySQL 9.** Craft is verified against MySQL 8.x; the 8.4 LTS image is the safe choice. (MySQL 8.4 removed the `--default-authentication-plugin` server flag — do not pass it.)
- **Database settings come from the container environment** (`CRAFT_DB_*`), so they live in Compose.

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8092/        # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8092/admin   # 302 → login
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9093/health   # 200
```

## See also

- [Routing](../../features/routing.md) · [Docker Guide](../../getting-started/docker.md)
- [Drupal](drupal.md) and [October CMS](october.md) — the other framework-mode CMS recipes
