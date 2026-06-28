---
title: Yii3 на OxPHP
description: Запуск приложения Yii3 (yiisoft/app) на OxPHP в режиме framework без базы данных — самый компактный пример. Dockerfile, файл Compose и заметки, специфичные для OxPHP.
---

# Yii3 на OxPHP

Yii3 — самый лёгкий рецепт из всех здесь представленных. Шаблон `yiisoft/app` содержит front controller `public/index.php`, не нуждается в базе данных и — поскольку большая часть работы со строками реализована через полифилы — практически не требует дополнительных расширений PHP. Он ложится на [режим маршрутизации framework](../../features/routing.md) в OxPHP и запускается как единственный сервис.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0` (PHP 8.5)
- **Режим маршрутизации:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT` оставлен на значении по умолчанию `/var/www/html/public`)
- **Добавленные расширения:** `mbstring` (нативное, ради скорости — иначе среда выполнения использует полифил)
- **Сервисы:** только OxPHP
- **URL:** `http://localhost:8094` · внутренний `http://localhost:9095/health`

> `yiisoft/app` объявляет `"php": "8.2 - 8.5"`, поэтому образ PHP 8.5 по умолчанию подходит.

## Структура проекта

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

# ── База PHP: нативный mbstring ───────────────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache oniguruma \
    && apk add --no-cache --virtual .build-deps oniguruma-dev \
    && docker-php-ext-install -j"$(nproc)" mbstring \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── Артефакты OxPHP (образ PHP 8.5 по умолчанию) ──────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

# ── dev: сервер OxPHP + PHP CLI + Composer ────────────────────
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

> Шаблон `yiisoft/app` поставляется со своим `.dockerignore` в стиле whitelist; сохраните его — при примонтированном через bind `src/` директива `COPY` остаётся лишь запасным вариантом.

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
      # LISTEN_ADDR (0.0.0.0:80) и DOCUMENT_ROOT (/var/www/html/public) —
      # это значения OxPHP по умолчанию, поэтому оба опущены.
      ENTRY_FILE: index.php          # режим framework
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

## Установка и первый запуск

```bash
docker compose up -d --build
docker compose exec app php yii          # консоль Yii
```

## Заметки по OxPHP

- **Никакой базы данных, никаких лишних сервисов** — минимально возможное развёртывание OxPHP: один образ, один процесс.
- **`yiisoft/assets` публикует статику в `public/assets/<hash>/`** — реальные файлы на диске, которые режим framework отдаёт напрямую. Никаких ухищрений с rewrite URL не требуется (в отличие от некоторых пайплайнов ассетов, встраивающих сегмент версии в URL).

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8094/              # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8094/nonexistent   # 404 (роутер Yii)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9095/health         # 200
```

## См. также

- [Маршрутизация](../../features/routing.md) · [Руководство по Docker](../../getting-started/docker.md)
- [Laravel](laravel.md) и [Symfony](symfony.md) — другие рецепты для режима framework
