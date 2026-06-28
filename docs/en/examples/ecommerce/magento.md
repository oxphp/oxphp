---
title: Magento on OxPHP
description: Run Magento Open Source 2.4 on OxPHP in framework routing mode with MySQL and OpenSearch — Dockerfile, Compose file, install steps, the static-asset version symlink, and OxPHP-specific notes.
---

# Magento on OxPHP

Magento is the heaviest recipe here: it mandates a search engine (OpenSearch), a long extension list, and a static-content deployment step. It runs in OxPHP's [framework routing mode](../../features/routing.md) with `pub/` as the document root, but its versioned static-asset URLs need one extra step described below.

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23` (PHP 8.4)
- **Routing mode:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT=/var/www/html/pub` overrides the default `…/public`)
- **Extensions added:** `bcmath`, `gd`, `intl`, `pdo_mysql`, `soap`, `xsl`, `zip`, `mbstring`, `ftp`, `pcntl`, `sockets`
- **Services:** OxPHP + MySQL 8.0 + OpenSearch 2.x
- **URL:** `http://localhost:8093` · admin `/admin` · internal `http://localhost:9094/health`

> **PHP version:** Magento 2.4.8 declares `"php": "~8.2 || ~8.3 || ~8.4"` and rejects PHP 8.5 — pin the **PHP 8.4** image. `ext-sockets` is a transitive requirement (`php-amqplib`) and needs `linux-headers` to compile.

## Project layout

Magento Open Source from the official distribution channel needs Adobe Marketplace auth keys. To install without keys, clone the open-source repository (its modules are provided in-tree via `replace`, so `composer install` pulls only Packagist dependencies):

```bash
mkdir -p magento-oxphp
git clone --branch 2.4.8 --depth 1 https://github.com/magento/magento2.git magento-oxphp/src
# composer install runs later, inside the PHP 8.4 container (composer:2 is PHP 8.5
# and Magento rejects it):
docker compose run --rm --no-deps -e COMPOSER_MEMORY_LIMIT=-1 app \
    composer install --no-interaction --prefer-dist
```

## Dockerfile

`src/Dockerfile.oxphp` (named to avoid colliding with Magento's own docker assets):

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP base: Magento runtime extensions ──────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache \
        icu-libs libpng libjpeg-turbo freetype oniguruma libzip libxslt \
    && apk add --no-cache --virtual .build-deps \
        icu-dev libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev \
        libzip-dev libxslt-dev libxml2-dev linux-headers \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" \
        bcmath gd intl pdo_mysql soap xsl zip mbstring ftp pcntl sockets \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP artifacts (PHP 8.4 image) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev: OxPHP server + PHP CLI + Composer + Magento extensions ─
FROM php-base AS dev
RUN apk add --no-cache libgcc git patch
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
        echo "memory_limit=4G"; echo "max_execution_time=1800"; \
        echo "realpath_cache_size=10M"; echo "realpath_cache_ttl=86400"; \
        echo "upload_max_filesize=64M"; echo "post_max_size=64M"; \
    } > /usr/local/etc/php/conf.d/magento.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/pub && chown -R www-data:www-data /var/www/html
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
    image: magento-oxphp/app:dev
    container_name: magento-oxphp
    ports:
      - "8093:80"
      - "9094:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR defaults to 0.0.0.0:80, so it is omitted.
      DOCUMENT_ROOT: /var/www/html/pub   # overrides the default /var/www/html/public
      ENTRY_FILE: index.php              # framework mode
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
    depends_on:
      db:
        condition: service_healthy
      opensearch:
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
    container_name: magento-oxphp-db
    ports:
      - "3310:3306"
    command:
      - --max_allowed_packet=64M
      - --innodb-buffer-pool-size=1G
      - --log_bin_trust_function_creators=1   # Magento creates triggers/functions
    environment:
      MYSQL_DATABASE: magento
      MYSQL_USER: magento
      MYSQL_PASSWORD: magento
      MYSQL_ROOT_PASSWORD: root
    volumes:
      - db_data:/var/lib/mysql
    healthcheck:
      test: ["CMD", "mysqladmin", "ping", "-h", "127.0.0.1", "-uroot", "-proot"]
      interval: 5s
      timeout: 5s
      retries: 30
      start_period: 20s
    restart: unless-stopped

  opensearch:
    image: opensearchproject/opensearch:2.19.1
    container_name: magento-oxphp-opensearch
    environment:
      - discovery.type=single-node
      - DISABLE_SECURITY_PLUGIN=true
      - DISABLE_INSTALL_DEMO_CONFIG=true
      - bootstrap.memory_lock=true
      - "OPENSEARCH_JAVA_OPTS=-Xms1g -Xmx1g"
    ulimits:
      memlock: { soft: -1, hard: -1 }
      nofile:  { soft: 65536, hard: 65536 }
    ports:
      - "9201:9200"
    volumes:
      - opensearch_data:/usr/share/opensearch/data
    healthcheck:
      test: ["CMD-SHELL", "curl -sf http://localhost:9200/_cluster/health || exit 1"]
      interval: 10s
      timeout: 5s
      retries: 30
      start_period: 20s
    restart: unless-stopped

volumes:
  db_data:
  opensearch_data:
```

## Install and first run

```bash
docker compose up -d db opensearch
# composer install (see Project layout), then:
docker compose run --rm app php bin/magento setup:install \
    --base-url=http://localhost:8093/ \
    --db-host=db --db-name=magento --db-user=magento --db-password=magento \
    --admin-firstname=Admin --admin-lastname=User \
    --admin-email=admin@example.com --admin-user=admin --admin-password='Admin123!' \
    --language=en_US --currency=USD --timezone=America/New_York \
    --search-engine=opensearch --opensearch-host=opensearch --opensearch-port=9200 \
    --opensearch-index-prefix=magento --opensearch-enable-auth=0

# production mode compiles DI and deploys static content
docker compose run --rm app php bin/magento deploy:mode:set production

# CRITICAL for OxPHP: make versioned static URLs resolve (see notes below)
docker compose run --rm app sh -c \
    'ln -sfn . "pub/static/version$(cat pub/static/deployed_version.txt)"'

docker compose up -d app
```

## OxPHP notes

- **Versioned static URLs need a symlink.** Magento emits asset URLs like `/static/version<timestamp>/frontend/…`, while the files live at `pub/static/frontend/…`. nginx strips the `version<N>/` segment with a rewrite rule; OxPHP's framework mode has no such rewrite, so every versioned asset would `404` and the storefront would render unstyled. The fix is a self-referential symlink that makes the versioned path resolve to the real files:

  ```bash
  ln -sfn . "pub/static/version$(cat pub/static/deployed_version.txt)"
  ```

  Because the symlink stays inside `DOCUMENT_ROOT` (`pub/`), no `SYMLINK_ALLOW_PATHS` entry is required.

- **Run production mode.** OxPHP's worker pool is multi-threaded (PHP ZTS). Magento developer mode generates DI classes and static assets on the fly (via `pub/static.php`, which framework mode does not route), risking races across worker threads. `deploy:mode:set production` pre-compiles DI and deploys static content up front, so workers never generate code at request time.
- **MySQL: `--log_bin_trust_function_creators=1`.** Magento creates triggers and stored functions during install; with binary logging enabled (the MySQL 8.0 default) the non-`SUPER` `magento` user otherwise hits error 1419.
- **`composer install` runs in the PHP 8.4 container.** The stock `composer:2` image is PHP 8.5, which Magento rejects; run it through the built `app` image instead.

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8093/         # 200 storefront
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8093/admin    # 200 admin login
curl -s    http://localhost:8093/ | grep -oE '/static/version[0-9]+/[^"]+\.css' | head -1 | \
    xargs -I{} curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:8093{}"  # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9094/health    # 200
```

## See also

- [Routing](../../features/routing.md) · [Docker Guide](../../getting-started/docker.md) · [Configuration Reference](../../operations/configuration.md)
- [OpenCart](opencart.md) — the other e-commerce recipe
