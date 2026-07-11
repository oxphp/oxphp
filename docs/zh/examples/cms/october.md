---
title: 在 OxPHP 上运行 October CMS
description: 以 framework 路由模式在 OxPHP 上运行 October CMS 4，使用 public/ 镜像目录和 SYMLINK_ALLOW_PATHS——Dockerfile、Compose 文件、安装步骤以及 OxPHP 专属说明。
---

# 在 OxPHP 上运行 October CMS

October CMS（基于 Laravel）在项目根目录提供 `index.php`，与 `config/`、`.env` 和 `vendor/` 并列。它的 nginx 配置将所有请求重写到 `index.php`，并且**仅**从白名单资源路径（`*/assets`）提供静态文件。要在 OxPHP 上获得相同的部署形态——同时不暴露主题源码或配置——本方案使用 `php artisan october:mirror public` 构建一个 `public/` document root，并让 framework 模式指向它。

这正是用来演示 OxPHP [`SYMLINK_ALLOW_PATHS`](../../security/symlink-allow-paths.md) 的方案。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.10.0-php8.4-alpine3.23`（PHP 8.4）
- **路由模式：** Framework（`ENTRY_FILE=index.php`；`DOCUMENT_ROOT` 保持默认 `/var/www/html/public`）
- **新增扩展：** `gd`、`pdo_mysql`、`mbstring`、`zip`
- **服务：** OxPHP + MySQL
- **URL：** `http://localhost:8098` · 后台 `/admin` · 内部 `http://localhost:9099/health`

> **无需许可证。** `october/october` 可直接从 Packagist 安装。Laravel-11 组件栈早于 PHP 8.5，因此本方案固定使用 **PHP 8.4** 镜像。

## 项目布局

```bash
mkdir -p october-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/october-oxphp/src":/app -w /app \
    composer:2 create-project october/october . --no-interaction --prefer-dist
```

然后将 `src/.env` 指向数据库服务（`DB_HOST=db`、`DB_DATABASE=october`、……），并设置 `APP_URL=http://localhost:8098`。

## Dockerfile

`src/Dockerfile.oxphp`（如此命名以避免与 October 自带的资源冲突）：

```dockerfile
ARG OXPHP_VERSION=0.10.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP 基础镜像：October 运行时扩展 ──────────────────────
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

# ── OxPHP 制品（PHP 8.4 镜像） ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev：OxPHP 服务器 + PHP CLI + Composer ────────────────────
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
      # LISTEN_ADDR（0.0.0.0:80）和 DOCUMENT_ROOT（/var/www/html/public）都是
      # OxPHP 默认值，因此两者均省略。
      ENTRY_FILE: index.php          # framework 模式
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      # public/ 镜像由指向项目树的符号链接构建而成；
      # 允许 OxPHP 跟随它们跳出 document root。
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

## 安装与首次运行

October CMS 4 没有 `october:install` 向导命令，也不会预置默认管理员，因此流程为：migrate → 构建内容表 → mirror → 创建管理员：

```bash
docker compose up -d db

# 1. 数据库结构与模块种子
docker compose run --rm app php artisan october:migrate

# 2. 动态内容表（演示主题使用 Tailor 内容）
docker compose run --rm app php artisan tailor:migrate

# 3. 构建 public/ document root（index.php + */assets 符号链接）
docker compose run --rm app sh -c 'mkdir -p public && php artisan october:mirror public --relative'

# 4. 创建后台超级用户（不存在安装向导）
docker compose run --rm app php artisan tinker --execute '
  $u = Backend\Models\User::firstOrNew(["login" => "admin"]);
  $u->email = "admin@example.com"; $u->password = "admin123";
  $u->password_confirmation = "admin123"; $u->is_superuser = true;
  $u->is_activated = true; $u->save();'

docker compose up -d app
```

## OxPHP 说明

- **`october:mirror public` 复刻了 October 的 nginx 资源白名单。** 它会创建一个 `public/` 目录，其中仅包含 `index.php`（一个符号链接）以及指向 `*/assets` 目录的符号链接——主题 `.htm` 页面、`config/`、`.env`、`vendor/` 和 `storage/logs` 都留在 document root **之外**。当 `DOCUMENT_ROOT=…/public` 时，它们都无法通过 HTTP 访问。
- **`SYMLINK_ALLOW_PATHS=/var/www/html` 是必需的。** 默认情况下，OxPHP 会阻止解析到 `DOCUMENT_ROOT` 之外的符号链接。镜像的符号链接指回项目树（`/var/www/html/modules/…`、`/var/www/html/themes/…`），因此需要将项目根目录加入白名单。`/var` 被精确黑名单，但不是前缀黑名单，所以 `/var/www/html` 是被允许的。参见 [Symlink Allow Paths](../../security/symlink-allow-paths.md)。
- **`tailor:migrate` 很容易被遗忘。** 如果不执行它，演示主题会抛出 `SQLSTATE 1146: table 'xc_…' doesn't exist`，因为其内容集合是 Tailor 蓝图，其数据表独立于 `october:migrate` 单独构建。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/                       # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/admin                  # 302 → 登录
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/themes/demo/assets/css/theme.css  # 200
# document-root 镜像将内部文件挡在外面：
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/.env                   # 404
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8098/themes/demo/pages/home.htm  # 404
```

## 另请参阅

- [Routing](../../features/routing.md) · [Symlink Allow Paths](../../security/symlink-allow-paths.md) · [Dot-Path Blocking](../../security/dot-path-blocking.md)
- [Drupal](drupal.md) 和 [Craft CMS](craft.md) —— 其他 framework 模式的 CMS 方案
