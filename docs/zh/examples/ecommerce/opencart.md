---
title: 在 OxPHP 上运行 OpenCart
description: 以 traditional 路由模式在 OxPHP 上运行 OpenCart 4，搭配 MySQL 与 PHP_DENY_PATHS——包含 Dockerfile、Compose 文件、CLI 安装以及 OxPHP 专属说明。
---

# 在 OxPHP 上运行 OpenCart

OpenCart 是一个 **traditional 模式**应用，拥有两个物理 front controller：`index.php`（店面）和 `admin/index.php`（管理后台），另有位于 `image/` 和 `catalog/view/` 下的静态资源。它的 webroot 就是项目根目录本身。OxPHP 的 [traditional 路由模式](../../features/routing.md)——即未设置 `ENTRY_FILE` 时的默认模式——将两个入口都作为真实文件提供，并从磁盘提供静态资源；`PHP_DENY_PATHS` 则阻止直接执行框架内部脚本。

## 技术栈速览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23`（PHP 8.4）
- **路由模式：** Traditional（无 `ENTRY_FILE`；`DOCUMENT_ROOT` = 项目根目录，覆盖默认的 `…/public`）
- **新增扩展：** `gd`、`mysqli`、`zip`、`mbstring`
- **服务：** OxPHP + MySQL
- **URL：** `http://localhost:8095` · 管理后台 `/admin/` · 内部 `http://localhost:9096/health`

> **PHP 版本：** OpenCart 4 要求 PHP 8.0+，但其代码库早于 8.5；请固定使用 **PHP 8.4** 镜像。OpenCart 使用 **`mysqli`** 驱动，而非 PDO。（注意：OpenCart 3.x 无法在 PHP 8.4+ 上运行——请使用 OpenCart 4.x。）

## 项目结构

OpenCart 以发行版压缩包的形式分发，而非 Composer 包。webroot 即压缩包中 `upload/` 目录的内容：

```bash
mkdir -p opencart-oxphp/src
curl -sL https://github.com/opencart/opencart/releases/download/4.1.0.3/opencart-4.1.0.3.zip -o /tmp/oc.zip
unzip -q /tmp/oc.zip -d /tmp/oc
cp -a /tmp/oc/upload/. opencart-oxphp/src/
# 安装程序需要这些文件存在且可写：
: > opencart-oxphp/src/config.php
: > opencart-oxphp/src/admin/config.php
```

## Dockerfile

`src/Dockerfile.oxphp`（如此命名是为了避免与 OpenCart 自带的 Dockerfile 冲突）：

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP base：OpenCart 运行时扩展 ─────────────────────
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

# ── OxPHP 构件（PHP 8.4 镜像）───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev：OxPHP 服务器 + PHP CLI + Composer ────────────────────
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
      # LISTEN_ADDR 默认为 0.0.0.0:80，因此此处省略。
      DOCUMENT_ROOT: /var/www/html         # webroot 就是项目根目录（覆盖默认的 /…/public）
      INTERNAL_ADDR: 0.0.0.0:9090          # （无 ENTRY_FILE → traditional 模式）
      ACCESS_LOG: all
      # 阻止通过 HTTP 直接执行框架内部脚本和 web 安装程序。
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

## 安装与首次运行

OpenCart 4 自带一个 CLI 安装程序：

```bash
docker compose up -d db
docker compose run --rm app php install/cli_install.php install \
    --username admin --password 'admin123' --email admin@example.com \
    --http_server 'http://localhost:8095/' --language en-gb \
    --db_driver mysqli --db_hostname db --db_username opencart \
    --db_password opencart --db_database opencart --db_port 3306 --db_prefix oc_
# OpenCart 建议在安装完成后移除安装程序：
rm -rf opencart-oxphp/src/install
docker compose up -d app
```

## OxPHP 说明

- **Traditional 模式提供两个 front controller。** `index.php`（店面）和 `admin/index.php`（管理后台）都是物理入口点；traditional 模式将每个都作为真实文件运行。Framework 模式会把所有请求汇集到单个 `index.php`，从而破坏管理后台。
- **`PHP_DENY_PATHS` 保护内部脚本。** 由于 webroot 位于项目根目录，`system/` 和 `install/` 都处于 `DOCUMENT_ROOT` 内部。`PHP_DENY_PATHS=/system/**,/install/**` 会阻止通过 HTTP 直接执行这些脚本——匹配发生在磁盘 I/O 之前，因此不存在存在性探测（existence oracle）。CLI 安装程序通过 shell 运行，不受影响。参见 [PHP 执行拒绝列表](../../security/php-deny.md)。
- **点文件本就安全。** `.env` 风格以及包含点段的路径会通过[点路径拦截](../../security/dot-path-blocking.md)返回 `404`；无需额外规则。
- **SEO URL 默认关闭。** 默认情况下 OpenCart 使用 `index.php?route=…` 查询路由，traditional 模式可直接处理，无需任何 rewrite 规则。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/                       # 200 店面
curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:8095/index.php?route=common/home"  # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/admin/                 # 200 管理后台登录
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8095/system/startup.php     # 404（已拒绝）
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9096/health                  # 200
```

## 另见

- [路由](../../features/routing.md) · [PHP 执行拒绝列表](../../security/php-deny.md) · [点路径拦截](../../security/dot-path-blocking.md)
- [WordPress](../cms/wordpress.md) —— 另一个 traditional 模式范例
