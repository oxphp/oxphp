---
title: October CMS на OxPHP
description: Запуск October CMS 4 на OxPHP в режиме framework-маршрутизации с зеркалом public/ и SYMLINK_ALLOW_PATHS — Dockerfile, Compose-файл, шаги установки и заметки, специфичные для OxPHP.
---

# October CMS на OxPHP

October CMS (на базе Laravel) поставляется с `index.php` в корне проекта, рядом с `config/`, `.env` и `vendor/`. Его конфигурация nginx переписывает все запросы на `index.php` и отдаёт статику **только** из путей к ассетам из белого списка (`*/assets`). Чтобы получить такую же модель на OxPHP — не раскрывая исходники тем или конфигурацию — этот рецепт собирает document root `public/` командой `php artisan october:mirror public` и направляет на него режим framework.

Это рецепт, который задействует [`SYMLINK_ALLOW_PATHS`](../../security/symlink-allow-paths.md) OxPHP.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23` (PHP 8.4)
- **Режим маршрутизации:** Framework (`ENTRY_FILE=index.php`; `DOCUMENT_ROOT` оставлен по умолчанию `/var/www/html/public`)
- **Добавленные расширения:** `gd`, `pdo_mysql`, `mbstring`, `zip`
- **Сервисы:** OxPHP + MySQL
- **URL:** `http://localhost:8098` · бэкенд `/admin` · внутренний `http://localhost:9099/health`

> **Лицензия не нужна.** `october/october` устанавливается напрямую из Packagist. Стек компонентов на Laravel 11 предшествует PHP 8.5, поэтому здесь зафиксирован образ **PHP 8.4**.

## Структура проекта

```bash
mkdir -p october-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/october-oxphp/src":/app -w /app \
    composer:2 create-project october/october . --no-interaction --prefer-dist
```

Затем направьте `src/.env` на сервис базы данных (`DB_HOST=db`, `DB_DATABASE=october`, …) и задайте `APP_URL=http://localhost:8098`.

## Dockerfile

`src/Dockerfile.oxphp` (назван так, чтобы не конфликтовать с собственными ассетами October):

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── База PHP: runtime-расширения для October ──────────────────
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

# ── Артефакты OxPHP (образ PHP 8.4) ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

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
      # LISTEN_ADDR (0.0.0.0:80) и DOCUMENT_ROOT (/var/www/html/public) —
      # значения по умолчанию для OxPHP, поэтому оба опущены.
      ENTRY_FILE: index.php          # режим framework
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      # Зеркало public/ собрано из символьных ссылок в дерево проекта;
      # разрешаем OxPHP переходить по ним за пределы document root.
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

## Установка и первый запуск

В October CMS 4 нет команды-мастера `october:install` и не создаётся администратор по умолчанию, поэтому последовательность такова: migrate → построение таблиц контента → mirror → создание администратора:

```bash
docker compose up -d db

# 1. Схема и сиды модулей
docker compose run --rm app php artisan october:migrate

# 2. Таблицы динамического контента (демо-тема использует контент Tailor)
docker compose run --rm app php artisan tailor:migrate

# 3. Сборка document root public/ (index.php + символьные ссылки */assets)
docker compose run --rm app sh -c 'mkdir -p public && php artisan october:mirror public --relative'

# 4. Создание супер-пользователя бэкенда (мастера установки не существует)
docker compose run --rm app php artisan tinker --execute '
  $u = Backend\Models\User::firstOrNew(["login" => "admin"]);
  $u->email = "admin@example.com"; $u->password = "admin123";
  $u->password_confirmation = "admin123"; $u->is_superuser = true;
  $u->is_activated = true; $u->save();'

docker compose up -d app
```

## Заметки по OxPHP

- **`october:mirror public` воспроизводит белый список ассетов nginx из October.** Она создаёт каталог `public/`, содержащий только `index.php` (символьную ссылку) и символьные ссылки на каталоги `*/assets` — страницы тем `.htm`, `config/`, `.env`, `vendor/` и `storage/logs` остаются **вне** document root. При `DOCUMENT_ROOT=…/public` ни один из них недоступен по HTTP.
- **`SYMLINK_ALLOW_PATHS=/var/www/html` обязателен.** По умолчанию OxPHP блокирует символьные ссылки, которые разрешаются за пределами `DOCUMENT_ROOT`. Символьные ссылки зеркала указывают обратно в дерево проекта (`/var/www/html/modules/…`, `/var/www/html/themes/…`), поэтому корень проекта добавлен в белый список. `/var` находится в точном чёрном списке, но не в префиксном, поэтому `/var/www/html` разрешён. См. [Symlink Allow Paths](../../security/symlink-allow-paths.md).
- **`tailor:migrate` легко забыть.** Без неё демо-тема выбрасывает `SQLSTATE 1146: table 'xc_…' doesn't exist`, потому что её коллекции контента — это blueprint-ы Tailor, чьи таблицы создаются отдельно от `october:migrate`.

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/                       # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/admin                  # 302 → login
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/themes/demo/assets/css/theme.css  # 200
# зеркало document root держит внутренности недоступными:
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/.env                   # 404
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/themes/demo/pages/home.htm  # 404
```

## См. также

- [Routing](../../features/routing.md) · [Symlink Allow Paths](../../security/symlink-allow-paths.md) · [Dot-Path Blocking](../../security/dot-path-blocking.md)
- [Drupal](drupal.md) и [Craft CMS](craft.md) — другие рецепты CMS в режиме framework
