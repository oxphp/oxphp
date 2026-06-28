---
title: OpenCart на OxPHP
description: Запуск OpenCart 4 на OxPHP в режиме traditional routing с MySQL и PHP_DENY_PATHS — Dockerfile, Compose-файл, установка через CLI и заметки, специфичные для OxPHP.
---

# OpenCart на OxPHP

OpenCart — это приложение для **traditional mode** с двумя физическими front controller'ами: `index.php` (витрина) и `admin/index.php` (панель администратора), плюс статические ресурсы в `image/` и `catalog/view/`. Его webroot — это сам корень проекта. [Режим traditional routing](../../features/routing.md) в OxPHP — используемый по умолчанию, когда `ENTRY_FILE` не задан — обслуживает обе точки входа как реальные файлы и отдаёт статические ресурсы с диска; `PHP_DENY_PATHS` блокирует прямое выполнение внутренних компонентов фреймворка.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23` (PHP 8.4)
- **Режим маршрутизации:** Traditional (без `ENTRY_FILE`; `DOCUMENT_ROOT` = корень проекта, переопределяет значение по умолчанию `…/public`)
- **Добавленные расширения:** `gd`, `mysqli`, `zip`, `mbstring`
- **Сервисы:** OxPHP + MySQL
- **URL:** `http://localhost:8095` · админка `/admin/` · внутренний `http://localhost:9096/health`

> **Версия PHP:** OpenCart 4 требует PHP 8.0+, но его кодовая база предшествует 8.5; зафиксируйте образ **PHP 8.4**. OpenCart использует драйвер **`mysqli`**, а не PDO. (Примечание: OpenCart 3.x не запустится на PHP 8.4+ — используйте OpenCart 4.x.)

## Структура проекта

OpenCart поставляется как релизный архив, а не как Composer-пакет. Webroot — это содержимое каталога `upload/` из архива:

```bash
mkdir -p opencart-oxphp/src
curl -sL https://github.com/opencart/opencart/releases/download/4.1.0.3/opencart-4.1.0.3.zip -o /tmp/oc.zip
unzip -q /tmp/oc.zip -d /tmp/oc
cp -a /tmp/oc/upload/. opencart-oxphp/src/
# установщику требуется, чтобы эти файлы существовали и были доступны для записи:
: > opencart-oxphp/src/config.php
: > opencart-oxphp/src/admin/config.php
```

## Dockerfile

`src/Dockerfile.oxphp` (назван так, чтобы не конфликтовать с входящим в комплект OpenCart Dockerfile):

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── База PHP: runtime-расширения OpenCart ─────────────────────
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

# ── Артефакты OxPHP (образ PHP 8.4) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev: сервер OxPHP + PHP CLI + Composer ────────────────────
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
      # LISTEN_ADDR по умолчанию 0.0.0.0:80, поэтому он опущен.
      DOCUMENT_ROOT: /var/www/html         # webroot ЕСТЬ корень проекта (переопределяет значение по умолчанию /…/public)
      INTERNAL_ADDR: 0.0.0.0:9090          # (нет ENTRY_FILE → режим traditional)
      ACCESS_LOG: all
      # Блокируем прямое HTTP-выполнение внутренних компонентов фреймворка и веб-установщика.
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

## Установка и первый запуск

OpenCart 4 поставляется с CLI-установщиком:

```bash
docker compose up -d db
docker compose run --rm app php install/cli_install.php install \
    --username admin --password 'admin123' --email admin@example.com \
    --http_server 'http://localhost:8095/' --language en-gb \
    --db_driver mysqli --db_hostname db --db_username opencart \
    --db_password opencart --db_database opencart --db_port 3306 --db_prefix oc_
# OpenCart рекомендует удалить установщик после установки:
rm -rf opencart-oxphp/src/install
docker compose up -d app
```

## Заметки по OxPHP

- **Traditional mode обслуживает два front controller'а.** `index.php` (витрина) и `admin/index.php` (админка) — оба являются физическими точками входа; traditional mode запускает каждый как реальный файл. Framework mode направлял бы всё в один `index.php` и сломал бы админку.
- **`PHP_DENY_PATHS` защищает внутренние компоненты.** Когда webroot находится в корне проекта, `system/` и `install/` располагаются внутри `DOCUMENT_ROOT`. `PHP_DENY_PATHS=/system/**,/install/**` блокирует прямое HTTP-выполнение этих скриптов — сопоставление происходит до дисковых операций, поэтому нет утечки информации о существовании файлов (existence oracle). CLI-установщик работает через shell и не затрагивается. См. [Deny-список выполнения PHP](../../security/php-deny.md).
- **Dotfiles уже защищены.** Пути в стиле `.env` и пути с dot-сегментами возвращают `404` благодаря [блокировке dot-путей](../../security/dot-path-blocking.md); никакого правила не требуется.
- **SEO-URL выключены по умолчанию.** По умолчанию OpenCart использует query-маршрутизацию вида `index.php?route=…`, которую traditional mode обрабатывает напрямую без правил перезаписи.

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/                       # 200 витрина
curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:8095/index.php?route=common/home"  # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/admin/                 # 200 вход в админку
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/system/startup.php     # 404 (запрещено)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9096/health                  # 200
```

## Смотрите также

- [Маршрутизация](../../features/routing.md) · [Deny-список выполнения PHP](../../security/php-deny.md) · [Блокировка dot-путей](../../security/dot-path-blocking.md)
- [WordPress](../cms/wordpress.md) — другой рецепт для traditional mode
