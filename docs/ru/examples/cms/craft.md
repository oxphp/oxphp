---
title: Craft CMS на OxPHP
description: Запуск Craft CMS 5 на OxPHP в режиме framework-маршрутизации с MySQL — Dockerfile, Compose-файл, установка через консоль и заметки, специфичные для OxPHP.
---

# Craft CMS на OxPHP

Craft CMS использует `web/index.php` в качестве front controller и отдаёт ресурсы панели управления из `web/cpresources/`. Он работает в [режиме framework-маршрутизации](../../features/routing.md) OxPHP: существующие статические файлы (включая ассеты панели управления) отдаются с диска, всё остальное направляется в `index.php`.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0` (PHP 8.5)
- **Режим маршрутизации:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT=/var/www/html/web` переопределяет значение по умолчанию `…/public`)
- **Добавленные расширения:** `bcmath`, `gd`, `intl`, `pdo_mysql`, `mbstring`, `zip`
- **Сервисы:** OxPHP + MySQL (8.4 LTS)
- **URL:** `http://localhost:8092` · панель управления `/admin` · внутренний `http://localhost:9093/health`

## Структура проекта

```bash
mkdir -p craft-oxphp/src
docker run --rm -v "$PWD/craft-oxphp/src":/app -w /app \
    composer:2 create-project craftcms/craft . \
    --ignore-platform-reqs --no-scripts --no-interaction --prefer-dist
```

> `--ignore-platform-reqs --no-scripts` необходимы, потому что в стандартном образе `composer:2` отсутствуют `bcmath`/`gd`/`intl`; они нужны только во время выполнения, а мастер настройки Craft, запускаемый после создания, отрабатывает позже, внутри собранного контейнера.

Craft считывает параметры подключения к базе данных из окружения, поэтому редактирование `.env` не требуется сверх тех ключей, которые записывает консоль.

## Dockerfile

`src/Dockerfile`:

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── База PHP: runtime-расширения Craft ────────────────────────
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

# ── Артефакты OxPHP ───────────────────────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

# ── dev: сервер OxPHP + PHP CLI + Composer + расширения Craft ──
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
      # LISTEN_ADDR по умолчанию 0.0.0.0:80, поэтому опущен.
      DOCUMENT_ROOT: /var/www/html/web   # переопределяет значение по умолчанию /var/www/html/public
      ENTRY_FILE: index.php              # режим framework
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

## Установка и первый запуск

Craft устанавливается через свою консоль, которая находится в том же контейнере:

```bash
docker compose up -d --build
docker compose exec app php craft setup/security-key   # записывает CRAFT_SECURITY_KEY
docker compose exec app php craft setup/app-id         # записывает CRAFT_APP_ID
docker compose exec app php craft install/craft \
    --username=admin --email=admin@example.com --password=password \
    --site-name="Craft OxPHP" --site-url='http://localhost:8092' --language=en-US
```

## Заметки по OxPHP

- **Ресурсы панели управления — это статические файлы.** `web/cpresources/<hash>/…` существует на диске и отдаётся режимом framework — убедитесь, что возвращается `200`, иначе панель управления отрисуется без стилей.
- **Используйте MySQL 8.4 LTS, а не MySQL 9.** Craft проверен на MySQL 8.x; образ 8.4 LTS — безопасный выбор. (В MySQL 8.4 удалён серверный флаг `--default-authentication-plugin` — не передавайте его.)
- **Настройки базы данных берутся из окружения контейнера** (`CRAFT_DB_*`), поэтому они находятся в Compose.

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8092/        # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8092/admin   # 302 → login
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9093/health   # 200
```

## См. также

- [Маршрутизация](../../features/routing.md) · [Руководство по Docker](../../getting-started/docker.md)
- [Drupal](drupal.md) и [October CMS](october.md) — другие рецепты CMS в режиме framework
