---
title: Symfony на OxPHP
description: Запуск приложения Symfony на OxPHP в режиме framework без базы данных — Dockerfile, файл Compose, шаги установки и заметки, специфичные для OxPHP.
---

# Symfony на OxPHP

Front controller Symfony `public/index.php` напрямую укладывается в [режим маршрутизации framework](../../features/routing.md) OxPHP. `symfony/skeleton` не требует базы данных, поэтому это развёртывание с единственным сервисом — добавьте Doctrine и сервис `db` позже, если будете подключать персистентность.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.10.0` (PHP 8.5)
- **Режим маршрутизации:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT` оставлен по умолчанию `/var/www/html/public`)
- **Добавленные расширения:** `intl`, `mbstring` (рекомендуемый базовый набор Symfony; во время выполнения строго необходимы только core-расширения)
- **Сервисы:** только OxPHP
- **URL:** `http://localhost:8096` · внутренний `http://localhost:9097/health`

> **Версия PHP:** Symfony 8 объявляет `"php": ">=8.4"`, чему образ PHP 8.5 по умолчанию удовлетворяет. Убедитесь, что зависимость не ограничивает верхнюю границу, с помощью `composer check-platform-reqs --no-dev`.

## Структура проекта

```bash
mkdir -p symfony-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/symfony-oxphp/src":/app -w /app \
    composer:2 create-project symfony/skeleton . --prefer-dist
# опционально, для отрисованной страницы:
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

# ── артефакты OxPHP (образ PHP 8.5 по умолчанию) ───────────────
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
      # LISTEN_ADDR (0.0.0.0:80) и DOCUMENT_ROOT (/var/www/html/public) —
      # значения OxPHP по умолчанию, поэтому оба опущены.
      ENTRY_FILE: index.php          # режим framework
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

## Установка и первый запуск

```bash
docker compose up -d --build
docker compose exec app php bin/console about
```

## Заметки по OxPHP

- **Минимальный набор расширений.** Во время выполнения скелету Symfony нужны только core-расширения (`ctype`, `iconv`, `xml`), все они присутствуют в базовом образе — `mbstring` в остальных случаях покрывается полифилом. Нативные `intl` + `mbstring` добавлены потому, что их использует любое реальное приложение Symfony, а нативные версии быстрее полифила.
- **`APP_ENV` из окружения контейнера.** Symfony читает её из реального окружения в приоритете над `.env`, поэтому задавайте её в Compose.
- **Статические ресурсы обслуживаются из `public/` режимом framework** — `{{ asset('css/app.css') }}` разрешается в реальный файл на диске и обслуживается напрямую, проваливаясь в `index.php` только при промахе.

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8096/             # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8096/no-such-route # 404 (роутер Symfony)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9097/health        # 200
```

## См. также

- [Маршрутизация](../../features/routing.md) · [Руководство по Docker](../../getting-started/docker.md)
- [Laravel](laravel.md) и [Yii3](yii3.md) — другие рецепты для режима framework
