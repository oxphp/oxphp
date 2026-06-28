---
title: Drupal на OxPHP
description: Запуск Drupal 11 на OxPHP в режиме маршрутизации framework с MySQL и drush — Dockerfile, Compose-файл, шаги установки и заметки, специфичные для OxPHP.
---

# Drupal на OxPHP

Drupal 11 использует `web/index.php` в качестве front controller с чистыми URL (любой путь, не указывающий на файл, переписывается на `index.php`), что в точности соответствует [режиму маршрутизации framework](../../features/routing.md) в OxPHP. Агрегированные CSS/JS попадают в `web/sites/default/files/` и отдаются с диска как любой другой статический ресурс.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23` (PHP 8.4)
- **Режим маршрутизации:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT=/var/www/html/web` переопределяет значение по умолчанию `…/public`)
- **Добавленные расширения:** `gd`, `pdo_mysql`, `zip`, `mbstring`
- **Сервисы:** OxPHP + MySQL
- **URL:** `http://localhost:8097` · внутренний `http://localhost:9098/health`

> **Версия PHP:** Drupal 11 требует PHP 8.3+ и официально поддерживает 8.4. Его стек на компонентах Symfony появился раньше PHP 8.5, поэтому в этом рецепте закреплён образ OxPHP с **PHP 8.4**. Drupal использует **PDO** (`pdo_mysql`), а не `mysqli`.

## Структура проекта

```bash
mkdir -p drupal-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/drupal-oxphp/src":/app -w /app \
    composer:2 create-project drupal/recommended-project . \
    --ignore-platform-reqs --no-interaction --prefer-dist
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/drupal-oxphp/src":/app -w /app \
    composer:2 require drush/drush --ignore-platform-reqs --no-interaction
```

> `--ignore-platform-reqs` нужен потому, что в стандартном образе `composer:2` нет `gd`; расширение требуется только во время выполнения, где его предоставляет образ OxPHP.

## Dockerfile

`src/Dockerfile`:

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── База PHP: расширения времени выполнения для Drupal ─────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache \
        libpng libjpeg-turbo freetype oniguruma libzip \
    && apk add --no-cache --virtual .build-deps \
        libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev libzip-dev \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" gd pdo_mysql zip mbstring \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── Артефакты OxPHP (образ PHP 8.4) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev: сервер OxPHP + PHP CLI + Composer + drush ────────────
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
    } > /usr/local/etc/php/conf.d/drupal.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/web && chown -R www-data:www-data /var/www/html
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
      target: dev
    image: drupal-oxphp/app:dev
    container_name: drupal-oxphp
    ports:
      - "8097:80"
      - "9098:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR по умолчанию 0.0.0.0:80, поэтому опущен.
      DOCUMENT_ROOT: /var/www/html/web   # переопределяет значение по умолчанию /var/www/html/public
      ENTRY_FILE: index.php              # режим framework
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
    image: mysql:8.0
    container_name: drupal-oxphp-db
    ports:
      - "3312:3306"
    command:
      - --max_allowed_packet=64M
    environment:
      MYSQL_DATABASE: drupal
      MYSQL_USER: drupal
      MYSQL_PASSWORD: drupal
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

`drush` — самый чистый неинтерактивный установщик:

```bash
docker compose up -d --build
docker compose exec app vendor/bin/drush site:install standard \
    --db-url=mysql://drupal:drupal@db:3306/drupal \
    --account-name=admin --account-pass=admin123 \
    --site-name="Drupal OxPHP" -y
```

## Заметки по OxPHP

- **Чистые URL — это режим framework.** Связка `RewriteCond !-f` + `RewriteRule ^ index.php` в Drupal делает ровно то же, что и режим framework — отдаёт существующие файлы и направляет всё остальное в `web/index.php`.
- **Агрегированные ресурсы отдаются с диска.** Drupal записывает объединённые CSS/JS в `web/sites/default/files/css` и `…/js`; OxPHP отдаёт их как статические файлы.
- **Первый запрос медленный, дальше — быстро.** Холодный первый запрос прогревает компиляцию Twig, агрегацию ресурсов и кэши (≈15 с). Последующие запросы укладываются в единицы миллисекунд. Это прогрев Drupal, а не накладные расходы OxPHP.

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8097/           # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8097/user/login # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9098/health      # 200
```

## Смотрите также

- [Маршрутизация](../../features/routing.md) · [Руководство по Docker](../../getting-started/docker.md)
- [Craft CMS](craft.md) и [October CMS](october.md) — другие рецепты CMS в режиме framework
