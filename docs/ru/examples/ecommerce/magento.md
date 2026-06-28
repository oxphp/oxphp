---
title: Magento на OxPHP
description: Запуск Magento Open Source 2.4 на OxPHP в режиме framework routing с MySQL и OpenSearch — Dockerfile, Compose-файл, шаги установки, симлинк версии для статических ресурсов и замечания, специфичные для OxPHP.
---

# Magento на OxPHP

Magento — самый тяжёлый рецепт здесь: он требует поисковый движок (OpenSearch), длинный список расширений и шаг развёртывания статического контента. Он работает в [режиме framework routing](../../features/routing.md) OxPHP с `pub/` в качестве document root, но его версионированные URL статических ресурсов требуют одного дополнительного шага, описанного ниже.

## Стек в двух словах

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23` (PHP 8.4)
- **Режим маршрутизации:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT=/var/www/html/pub` переопределяет умолчание `…/public`)
- **Добавленные расширения:** `bcmath`, `gd`, `intl`, `pdo_mysql`, `soap`, `xsl`, `zip`, `mbstring`, `ftp`, `pcntl`, `sockets`
- **Сервисы:** OxPHP + MySQL 8.0 + OpenSearch 2.x
- **URL:** `http://localhost:8093` · админка `/admin` · внутренний `http://localhost:9094/health`

> **Версия PHP:** Magento 2.4.8 объявляет `"php": "~8.2 || ~8.3 || ~8.4"` и отвергает PHP 8.5 — фиксируйте образ с **PHP 8.4**. `ext-sockets` является транзитивным требованием (`php-amqplib`) и для компиляции нуждается в `linux-headers`.

## Структура проекта

Magento Open Source из официального дистрибутивного канала требует ключи аутентификации Adobe Marketplace. Чтобы установить без ключей, клонируйте репозиторий с открытым исходным кодом (его модули поставляются прямо в дереве через `replace`, поэтому `composer install` подтягивает только зависимости с Packagist):

```bash
mkdir -p magento-oxphp
git clone --branch 2.4.8 --depth 1 https://github.com/magento/magento2.git magento-oxphp/src
# composer install запускается позже, внутри контейнера PHP 8.4 (composer:2 — это PHP 8.5,
# а Magento его отвергает):
docker compose run --rm --no-deps -e COMPOSER_MEMORY_LIMIT=-1 app \
    composer install --no-interaction --prefer-dist
```

## Dockerfile

`src/Dockerfile.oxphp` (назван так, чтобы не конфликтовать с собственными docker-ресурсами Magento):

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── База PHP: runtime-расширения Magento ──────────────────────
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

# ── Артефакты OxPHP (образ PHP 8.4) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev: сервер OxPHP + PHP CLI + Composer + расширения Magento ─
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
      # LISTEN_ADDR по умолчанию 0.0.0.0:80, поэтому опущен.
      DOCUMENT_ROOT: /var/www/html/pub   # переопределяет умолчание /var/www/html/public
      ENTRY_FILE: index.php              # режим framework
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
      - --log_bin_trust_function_creators=1   # Magento создаёт триггеры/функции
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

## Установка и первый запуск

```bash
docker compose up -d db opensearch
# composer install (см. «Структура проекта»), затем:
docker compose run --rm app php bin/magento setup:install \
    --base-url=http://localhost:8093/ \
    --db-host=db --db-name=magento --db-user=magento --db-password=magento \
    --admin-firstname=Admin --admin-lastname=User \
    --admin-email=admin@example.com --admin-user=admin --admin-password='Admin123!' \
    --language=en_US --currency=USD --timezone=America/New_York \
    --search-engine=opensearch --opensearch-host=opensearch --opensearch-port=9200 \
    --opensearch-index-prefix=magento --opensearch-enable-auth=0

# production-режим компилирует DI и развёртывает статический контент
docker compose run --rm app php bin/magento deploy:mode:set production

# КРИТИЧНО для OxPHP: заставляет версионированные статические URL резолвиться (см. замечания ниже)
docker compose run --rm app sh -c \
    'ln -sfn . "pub/static/version$(cat pub/static/deployed_version.txt)"'

docker compose up -d app
```

## Замечания по OxPHP

- **Версионированным статическим URL нужен симлинк.** Magento выдаёт URL ресурсов вида `/static/version<timestamp>/frontend/…`, тогда как файлы лежат в `pub/static/frontend/…`. nginx срезает сегмент `version<N>/` правилом rewrite; в режиме framework у OxPHP такого rewrite нет, поэтому каждый версионированный ресурс отдавал бы `404`, и витрина отображалась бы без стилей. Решение — самоссылающийся симлинк, который заставляет версионированный путь резолвиться в реальные файлы:

  ```bash
  ln -sfn . "pub/static/version$(cat pub/static/deployed_version.txt)"
  ```

  Поскольку симлинк остаётся внутри `DOCUMENT_ROOT` (`pub/`), запись `SYMLINK_ALLOW_PATHS` не требуется.

- **Запускайте production-режим.** Пул воркеров OxPHP многопоточный (PHP ZTS). Developer-режим Magento генерирует DI-классы и статические ресурсы на лету (через `pub/static.php`, который режим framework не маршрутизирует), создавая риск гонок между потоками-воркерами. `deploy:mode:set production` заранее компилирует DI и развёртывает статический контент, так что воркеры никогда не генерируют код во время запроса.
- **MySQL: `--log_bin_trust_function_creators=1`.** Magento создаёт триггеры и хранимые функции во время установки; при включённом бинарном логировании (умолчание MySQL 8.0) пользователь `magento`, не имеющий `SUPER`, иначе натыкается на ошибку 1419.
- **`composer install` запускается в контейнере PHP 8.4.** Штатный образ `composer:2` — это PHP 8.5, который Magento отвергает; запускайте его через собранный образ `app`.

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8093/         # 200 витрина
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8093/admin    # 200 вход в админку
curl -s    http://localhost:8093/ | grep -oE '/static/version[0-9]+/[^"]+\.css' | head -1 | \
    xargs -I{} curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:8093{}"  # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9094/health    # 200
```

## Смотрите также

- [Маршрутизация](../../features/routing.md) · [Руководство по Docker](../../getting-started/docker.md) · [Справочник по конфигурации](../../operations/configuration.md)
- [OpenCart](opencart.md) — другой рецепт для электронной коммерции
