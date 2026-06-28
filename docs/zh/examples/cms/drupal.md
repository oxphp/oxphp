---
title: 在 OxPHP 上运行 Drupal
description: 在 OxPHP 的 framework 路由模式下，配合 MySQL 与 drush 运行 Drupal 11 —— 包含 Dockerfile、Compose 文件、安装步骤以及 OxPHP 专属说明。
---

# 在 OxPHP 上运行 Drupal

Drupal 11 使用 `web/index.php` 作为 front controller，并启用 clean URL（每一个非文件路径都重写到 `index.php`），这正好就是 OxPHP 的 [framework 路由模式](../../features/routing.md)。聚合后的 CSS/JS 会落到 `web/sites/default/files/`，并像其他静态资源一样从磁盘直接提供。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23`（PHP 8.4）
- **路由模式：** Framework（`ENTRY_FILE=index.php`；`DOCUMENT_ROOT=/var/www/html/web` 覆盖默认的 `…/public`）
- **新增扩展：** `gd`、`pdo_mysql`、`zip`、`mbstring`
- **服务：** OxPHP + MySQL
- **URL：** `http://localhost:8097` · 内部 `http://localhost:9098/health`

> **PHP 版本：** Drupal 11 需要 PHP 8.3+，并官方支持 8.4。它依赖的 Symfony 组件栈早于 PHP 8.5，因此本方案锁定 **PHP 8.4** 的 OxPHP 镜像。Drupal 使用 **PDO**（`pdo_mysql`），而非 `mysqli`。

## 项目结构

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

> 这里需要 `--ignore-platform-reqs`，因为原版 `composer:2` 镜像不带 `gd`；该扩展仅在运行时才需要，而 OxPHP 镜像已经提供了它。

## Dockerfile

`src/Dockerfile`：

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP 基础镜像：Drupal 运行时扩展 ───────────────────────
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

# ── OxPHP 构件（PHP 8.4 镜像） ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev：OxPHP 服务器 + PHP CLI + Composer + drush ────────────
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
      # LISTEN_ADDR 默认为 0.0.0.0:80，因此此处省略。
      DOCUMENT_ROOT: /var/www/html/web   # 覆盖默认的 /var/www/html/public
      ENTRY_FILE: index.php              # framework 模式
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

## 安装与首次运行

`drush` 是最简洁的非交互式安装器：

```bash
docker compose up -d --build
docker compose exec app vendor/bin/drush site:install standard \
    --db-url=mysql://drupal:drupal@db:3306/drupal \
    --account-name=admin --account-pass=admin123 \
    --site-name="Drupal OxPHP" -y
```

## OxPHP 说明

- **Clean URL 就是 framework 模式。** Drupal 的 `RewriteCond !-f` + `RewriteRule ^ index.php` 正是 framework 模式所做的事 —— 已存在的文件直接提供，其余一切都派发到 `web/index.php`。
- **聚合资源从磁盘提供。** Drupal 把合并后的 CSS/JS 写入 `web/sites/default/files/css` 和 `…/js`；OxPHP 将它们作为静态文件提供。
- **首次请求慢，随后变快。** 冷启动的首次请求会预热 Twig 编译、资源聚合以及各类缓存（约 15 秒）。后续请求只需个位数毫秒。这是 Drupal 的预热过程，并非 OxPHP 的开销。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8097/           # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8097/user/login # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9098/health      # 200
```

## 另请参阅

- [路由](../../features/routing.md) · [Docker 指南](../../getting-started/docker.md)
- [Craft CMS](craft.md) 和 [October CMS](october.md) —— 另外两份 framework 模式的 CMS 方案
