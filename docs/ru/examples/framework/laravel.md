---
title: Laravel на OxPHP
description: Запуск приложения Laravel на OxPHP в режиме framework routing с базой данных MySQL — Dockerfile, Compose-файл, шаги установки и специфичная для OxPHP конфигурация.
---

# Laravel на OxPHP

Laravel — это каноническое приложение для framework-режима: единый front controller в `public/index.php`, а статические ресурсы отдаются напрямую из `public/`. [Framework routing mode](../../features/routing.md) в OxPHP ложится на эту схему точь-в-точь — существующие файлы отдаются с диска, всё остальное направляется в `index.php`.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0` (PHP 8.5)
- **Режим маршрутизации:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT` оставлен по умолчанию `/var/www/html/public`)
- **Добавленные расширения:** `pdo_mysql`, `intl`
- **Сервисы:** OxPHP + MySQL
- **URL:** `http://localhost:8091` · внутренний `http://localhost:9092/health`

## Структура проекта

```
laravel-oxphp/
├── docker-compose.yml
└── src/                     # composer create-project laravel/laravel src
    ├── Dockerfile
    └── …                    # приложение Laravel
```

Сначала сгенерируйте приложение:

```bash
mkdir -p laravel-oxphp/src
docker run --rm -v "$PWD/laravel-oxphp/src":/app -w /app \
    composer:2 create-project laravel/laravel . --prefer-dist
```

Затем направьте `src/.env` на сервис базы данных:

```dotenv
APP_URL=http://localhost:8091
DB_CONNECTION=mysql
DB_HOST=db
DB_PORT=3306
DB_DATABASE=laravel
DB_USERNAME=laravel
DB_PASSWORD=laravel
```

## Dockerfile

`src/Dockerfile` — копирует OxPHP в базовый образ `php:8.5-zts-alpine`, который несёт runtime-расширения Laravel:

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── База PHP: runtime-расширения Laravel ──────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache icu-libs \
    && apk add --no-cache --virtual .build-deps icu-dev \
    && docker-php-ext-install pdo_mysql intl \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── Артефакты OxPHP ───────────────────────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

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
    image: laravel-oxphp/app:dev
    container_name: laravel-oxphp
    ports:
      - "8091:80"
      - "9092:9090"
    volumes:
      - ./src:/var/www/html          # правка на лету без пересборки
    environment:
      # LISTEN_ADDR (0.0.0.0:80) и DOCUMENT_ROOT (/var/www/html/public) —
      # значения OxPHP по умолчанию, поэтому оба опущены.
      ENTRY_FILE: index.php          # framework-режим
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
    image: mysql:9
    container_name: laravel-oxphp-db
    ports:
      - "3308:3306"
    environment:
      MYSQL_DATABASE: laravel
      MYSQL_USER: laravel
      MYSQL_PASSWORD: laravel
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

## Установка и первый запуск

Образ `dev` несёт PHP CLI и Composer, поэтому `artisan` запускается в том же контейнере:

```bash
docker compose up -d --build
docker compose exec app php artisan key:generate
docker compose exec app php artisan migrate
```

## Заметки по OxPHP

- **`ENTRY_FILE=index.php` выбирает framework-режим.** Каталог `public/` Laravel становится `DOCUMENT_ROOT`; собственные `.env`, `app/`, `vendor/` фреймворка остаются за пределами document root и никогда недоступны по HTTP.
- **Окружение контейнера переопределяет `.env`.** OxPHP передаёт окружение контейнера в PHP, а загрузчик dotenv в Laravel не переопределяет реальные переменные окружения — поэтому `DB_HOST`, `APP_ENV` и т. д., заданные в Compose, имеют приоритет.
- **OPcache перепроверяет файлы на каждый запрос** в target `dev`, поэтому правки в примонтированном `src/` вступают в силу немедленно. Для production задайте `opcache.validate_timestamps=0` и уберите bind mount.

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8091/       # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8091/up     # 200 (health-маршрут)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9092/health # 200
```

## Смотрите также

- [Routing](../../features/routing.md) · [Руководство по Docker](../../getting-started/docker.md) · [Справочник по конфигурации](../../operations/configuration.md)
- [Symfony](symfony.md) и [Yii3](yii3.md) — другие рецепты для framework-режима
