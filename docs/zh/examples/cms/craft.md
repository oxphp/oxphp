---
title: 在 OxPHP 上运行 Craft CMS
description: 在 OxPHP 的 framework 路由模式下，搭配 MySQL 运行 Craft CMS 5 —— 包含 Dockerfile、Compose 文件、通过控制台完成安装，以及 OxPHP 特有的注意事项。
---

# 在 OxPHP 上运行 Craft CMS

Craft CMS 使用 `web/index.php` 作为前端控制器（front controller），并从 `web/cpresources/` 提供控制面板资源。它运行在 OxPHP 的 [framework 路由模式](../../features/routing.md)下：已存在的静态文件（包括控制面板资源）从磁盘直接提供，其余所有请求都分发到 `index.php`。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.9.0`（PHP 8.5）
- **路由模式：** Framework（`ENTRY_FILE=index.php`；`DOCUMENT_ROOT=/var/www/html/web` 覆盖默认的 `…/public`）
- **新增扩展：** `bcmath`、`gd`、`intl`、`pdo_mysql`、`mbstring`、`zip`
- **服务：** OxPHP + MySQL（8.4 LTS）
- **URL：** `http://localhost:8092` · 控制面板 `/admin` · 内部端点 `http://localhost:9093/health`

## 项目结构

```bash
mkdir -p craft-oxphp/src
docker run --rm -v "$PWD/craft-oxphp/src":/app -w /app \
    composer:2 create-project craftcms/craft . \
    --ignore-platform-reqs --no-scripts --no-interaction --prefer-dist
```

> 之所以需要 `--ignore-platform-reqs --no-scripts`，是因为现成的 `composer:2` 镜像缺少 `bcmath`/`gd`/`intl`；这些扩展只在运行时才需要，而 Craft 的创建后安装向导会稍后在构建好的容器内运行。

Craft 从环境变量读取数据库连接，因此除了控制台写入的那些键之外，无需再编辑 `.env`。

## Dockerfile

`src/Dockerfile`：

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── PHP 基础镜像：Craft 运行时扩展 ────────────────────────
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

# ── OxPHP 制品 ───────────────────────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

# ── dev：OxPHP 服务器 + PHP CLI + Composer + Craft 扩展 ──
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
      # LISTEN_ADDR 默认为 0.0.0.0:80，因此省略。
      DOCUMENT_ROOT: /var/www/html/web   # 覆盖默认的 /var/www/html/public
      ENTRY_FILE: index.php              # framework 模式
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

## 安装与首次运行

Craft 通过其控制台进行安装，该控制台位于同一个容器内：

```bash
docker compose up -d --build
docker compose exec app php craft setup/security-key   # 写入 CRAFT_SECURITY_KEY
docker compose exec app php craft setup/app-id         # 写入 CRAFT_APP_ID
docker compose exec app php craft install/craft \
    --username=admin --email=admin@example.com --password=password \
    --site-name="Craft OxPHP" --site-url='http://localhost:8092' --language=en-US
```

## OxPHP 注意事项

- **控制面板资源是静态文件。** `web/cpresources/<hash>/…` 存在于磁盘上，由 framework 模式提供 —— 请确认它返回 `200`，否则控制面板会显示为无样式状态。
- **请使用 MySQL 8.4 LTS，而非 MySQL 9。** Craft 已针对 MySQL 8.x 验证；8.4 LTS 镜像是稳妥之选。（MySQL 8.4 移除了 `--default-authentication-plugin` 服务器标志 —— 请勿传入该标志。）
- **数据库设置来自容器环境变量**（`CRAFT_DB_*`），因此它们存放在 Compose 中。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8092/        # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8092/admin   # 302 → 登录
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9093/health   # 200
```

## 另见

- [路由](../../features/routing.md) · [Docker 指南](../../getting-started/docker.md)
- [Drupal](drupal.md) 和 [October CMS](october.md) —— 其他 framework 模式的 CMS 教程
