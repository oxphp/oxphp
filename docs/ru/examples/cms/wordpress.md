---
title: WordPress на OxPHP
description: Запуск WordPress на OxPHP в traditional-режиме маршрутизации с MySQL и сайдкаром WP-CLI — Dockerfile, расширяющий runtime-образ OxPHP, файл Compose и заметки, специфичные для OxPHP.
---

# WordPress на OxPHP

WordPress — это приложение **traditional-режима**: у него много физических точек входа (`index.php`, `wp-login.php`, `wp-cron.php` и весь каталог `wp-admin/`), и к каждой нужно обращаться как к реальному файлу. [Traditional-режим маршрутизации](../../features/routing.md) OxPHP — режим по умолчанию, когда `ENTRY_FILE` не задан — обслуживает их именно так. **Не** задавайте `ENTRY_FILE`: framework-режим направил бы каждый запрос на единственный front controller и сломал бы `wp-admin`.

Этот рецепт также демонстрирует второй вариант сборки: вместо копирования OxPHP в базовый PHP-образ он **расширяет runtime-образ OxPHP напрямую**, компилируя расширения, нужные WordPress, в стадии-сборщике и подкладывая `.so`-файлы.

## Стек вкратце

- **Образ OxPHP:** `ghcr.io/oxphp/oxphp:0.9.0` (PHP 8.5) — расширяется на месте
- **Режим маршрутизации:** Traditional (без `ENTRY_FILE`)
- **Добавленные расширения:** `mysqli`, `pdo_mysql`, `gd`, `zip`, `intl`, `exif`, `bcmath`
- **Сервисы:** OxPHP + MySQL + сайдкар WP-CLI (профиль `cli`)
- **Усиление безопасности:** `PHP_DENY_PATHS` блокирует прямое выполнение `.php` внутри `wp-content/uploads/`
- **URL:** `http://localhost:8090` · внутренний `http://localhost:9091/health`

## Структура проекта

```
wp-oxphp/
├── Dockerfile
├── docker-compose.yml
└── wordpress/                # дерево WordPress (скачайте с wordpress.org)
    └── wp-config.php         # читает переменные окружения WORDPRESS_*
```

`wordpress/wp-config.php` читает свои настройки из окружения контейнера:

```php
define( 'DB_NAME',     getenv( 'WORDPRESS_DB_NAME' )     ?: 'wordpress' );
define( 'DB_USER',     getenv( 'WORDPRESS_DB_USER' )     ?: 'wordpress' );
define( 'DB_PASSWORD', getenv( 'WORDPRESS_DB_PASSWORD' ) ?: 'wordpress' );
define( 'DB_HOST',     getenv( 'WORDPRESS_DB_HOST' )     ?: 'db:3306' );
$__site_url = getenv( 'WORDPRESS_SITE_URL' ) ?: 'http://localhost:8090';
define( 'WP_HOME',    $__site_url );
define( 'WP_SITEURL', $__site_url );
```

## Dockerfile

Этот Dockerfile компилирует расширения против совпадающего `php:8.5-zts-alpine` (тот же ABI, что и у образа OxPHP, `no-debug-zts-20250925`) и копирует `.so`-файлы в runtime OxPHP:

```dockerfile
# ── Стадия 1: компиляция PHP-расширений WordPress против PHP 8.5 ─────
FROM php:8.5-zts-alpine3.23 AS ext-builder
RUN apk add --no-cache \
        icu-dev libzip-dev libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" \
        mysqli pdo_mysql gd zip intl exif bcmath
RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && mkdir -p /ext-out \
    && cp "$EXT_DIR"/mysqli.so   "$EXT_DIR"/pdo_mysql.so "$EXT_DIR"/gd.so \
          "$EXT_DIR"/zip.so      "$EXT_DIR"/intl.so      "$EXT_DIR"/exif.so \
          "$EXT_DIR"/bcmath.so   /ext-out/

# ── Стадия 2: runtime OxPHP с расширениями WordPress ─────────────
FROM ghcr.io/oxphp/oxphp:0.9.0 AS runtime
USER root
RUN apk add --no-cache icu-libs libzip libpng libjpeg-turbo freetype oniguruma
# Подкладываем скомпилированные расширения в каталог расширений PHP 8.5 OxPHP
COPY --from=ext-builder /ext-out/*.so \
    /usr/local/lib/php/extensions/no-debug-zts-20250925/
RUN { \
        echo "extension=mysqli.so";   echo "extension=pdo_mysql.so"; \
        echo "extension=gd.so";       echo "extension=zip.so"; \
        echo "extension=intl.so";     echo "extension=exif.so"; \
        echo "extension=bcmath.so"; \
    } > /usr/local/etc/php/conf.d/wordpress-extensions.ini
RUN { \
        echo "upload_max_filesize=64M"; echo "post_max_size=64M"; \
        echo "memory_limit=256M";       echo "max_execution_time=300"; \
        echo "max_input_vars=3000"; \
    } > /usr/local/etc/php/conf.d/wordpress.ini
EXPOSE 80
```

> Каталог расширений (`no-debug-zts-20250925`) — это ABI-тег PHP 8.5 ZTS. Если вы собираете на образе OxPHP с PHP 8.4, он становится `no-debug-zts-20240924` — вычисляйте его во время сборки через `php -r 'echo ini_get("extension_dir");'`, а не зашивайте жёстко.

## docker-compose.yml

```yaml
services:
  wp:
    build:
      context: .
    image: wp-oxphp/oxphp-wordpress:dev
    container_name: wp-oxphp
    ports:
      - "8090:80"
      - "9091:9090"
    volumes:
      - ./wordpress:/var/www/html/public   # дерево WordPress, редактируемое на лету
    environment:
      # Traditional-маршрутизация — БЕЗ ENTRY_FILE, чтобы wp-admin/*.php, wp-login.php,
      # wp-cron.php обслуживались как физические файлы, как этого ждёт WordPress.
      # LISTEN_ADDR (0.0.0.0:80) и DOCUMENT_ROOT (/var/www/html/public) —
      # значения по умолчанию OxPHP, поэтому оба опущены.
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      # Блокируем прямое выполнение PHP там, где оседает пользовательский контент — это
      # нейтрализует шелл, загруженный в wp-content/uploads уязвимым плагином. НЕ добавляйте
      # /wp-content/plugins/** или /wp-content/themes/** — некоторые плагины предоставляют
      # там напрямую вызываемые .php-эндпоинты.
      PHP_DENY_PATHS: "/wp-content/uploads/**,/wp-content/cache/**,/wp-content/upgrade/**"
      WORDPRESS_DB_HOST: db:3306
      WORDPRESS_DB_NAME: wordpress
      WORDPRESS_DB_USER: wordpress
      WORDPRESS_DB_PASSWORD: wordpress
      WORDPRESS_SITE_URL: http://localhost:8090
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
    container_name: wp-oxphp-db
    ports:
      - "3307:3306"
    environment:
      MYSQL_DATABASE: wordpress
      MYSQL_USER: wordpress
      MYSQL_PASSWORD: wordpress
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

  # WP-CLI по требованию — runtime-образ OxPHP не содержит бинарника `wp`.
  wpcli:
    image: wordpress:cli-php8.4
    container_name: wp-oxphp-wpcli
    profiles: ["cli"]
    user: "0:0"
    volumes:
      - ./wordpress:/var/www/html
    environment:
      WORDPRESS_DB_HOST: db:3306
      WORDPRESS_DB_NAME: wordpress
      WORDPRESS_DB_USER: wordpress
      WORDPRESS_DB_PASSWORD: wordpress
      WORDPRESS_SITE_URL: http://localhost:8090
    depends_on:
      db:
        condition: service_healthy

volumes:
  db_data:
```

## Установка и первый запуск

```bash
docker compose up -d --build
docker compose run --rm wpcli wp core install \
    --url=http://localhost:8090 --title="OxPHP WordPress" \
    --admin_user=admin --admin_password=admin --admin_email=admin@example.com
```

## Заметки по OxPHP

- **Traditional-режим обязателен.** WordPress требует, чтобы `wp-admin/`, `wp-login.php`, `wp-cron.php` и т.д. выполнялись как физические файлы. Оставьте `ENTRY_FILE` незаданным.
- **Runtime-образ OxPHP не содержит бинарника `wp`** (это минимальный образ для обслуживания). Работа из CLI идёт через сайдкар `wpcli` в профиле Compose `cli`, который разделяет тот же том `wordpress/` и ту же базу данных.
- **`wp-config.php` читает окружение контейнера** через `getenv()`, поэтому учётные данные базы и URL сайта живут в Compose, а не зашиты в файле.
- **`PHP_DENY_PATHS` усиливает защиту каталогов загрузок.** Поскольку traditional-режим выполняет физические `.php`-файлы, шелл, загруженный в `wp-content/uploads/` уязвимым плагином, иначе бы запустился. `PHP_DENY_PATHS` блокирует выполнение `.php` внутри этих путей — сопоставление с URI происходит *до* любого обращения к диску, поэтому оракула существования нет. Это работает только в traditional- и SPA-режимах; в framework-режиме это no-op (front controller уже препятствует прямому выполнению `.php`), поэтому рецептам framework-режима здесь это не нужно. См. [Список запрета выполнения PHP](../../security/php-deny.md).

## Проверка

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/            # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/wp-login.php # 200 (физический файл)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/wp-content/uploads/x.php # 404 (PHP_DENY_PATHS)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9091/health       # 200
```

## См. также

- [Маршрутизация](../../features/routing.md) · [Список запрета выполнения PHP](../../security/php-deny.md) · [Руководство по Docker](../../getting-started/docker.md)
- [OpenCart](../ecommerce/opencart.md) — другой рецепт traditional-режима
