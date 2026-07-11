---
title: 在 OxPHP 上运行 Laravel
description: 在 framework 路由模式下、配合 MySQL 数据库在 OxPHP 上运行 Laravel 应用 —— 包含 Dockerfile、Compose 文件、安装步骤以及 OxPHP 专属配置。
---

# 在 OxPHP 上运行 Laravel

Laravel 是 framework 模式应用的典型代表：位于 `public/index.php` 的单一 front controller，静态资源直接从 `public/` 提供。OxPHP 的 [framework 路由模式](../../features/routing.md)与之完全契合 —— 已存在的文件从磁盘提供，其余请求一律分发到 `index.php`。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.10.0`（PHP 8.5）
- **路由模式：** Framework（`ENTRY_FILE=index.php`；`DOCUMENT_ROOT` 保留默认值 `/var/www/html/public`）
- **新增扩展：** `pdo_mysql`、`intl`
- **服务：** OxPHP + MySQL
- **URL：** `http://localhost:8091` · 内部 `http://localhost:9092/health`

## 项目结构

```
laravel-oxphp/
├── docker-compose.yml
└── src/                     # composer create-project laravel/laravel src
    ├── Dockerfile
    └── …                    # the Laravel application
```

首先生成应用：

```bash
mkdir -p laravel-oxphp/src
docker run --rm -v "$PWD/laravel-oxphp/src":/app -w /app \
    composer:2 create-project laravel/laravel . --prefer-dist
```

然后让 `src/.env` 指向数据库服务：

```dotenv
APP_URL=http://localhost:8091
DB_CONNECTION=mysql
DB_HOST=db
DB_PORT=3306
DB_DATABASE=laravel
DB_USERNAME=laravel
DB_PASSWORD=laravel
```

## Dockerfile

`src/Dockerfile` —— 将 OxPHP 拷贝进一个携带 Laravel 运行时扩展的 `php:8.5-zts-alpine` 基础镜像：

```dockerfile
ARG OXPHP_VERSION=0.10.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── PHP base: Laravel 运行时扩展 ──────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache icu-libs \
    && apk add --no-cache --virtual .build-deps icu-dev \
    && docker-php-ext-install pdo_mysql intl \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP 构建产物 ───────────────────────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

# ── dev: OxPHP 服务器 + PHP CLI + Composer ────────────────────
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
    image: laravel-oxphp/app:dev
    container_name: laravel-oxphp
    ports:
      - "8091:80"
      - "9092:9090"
    volumes:
      - ./src:/var/www/html          # 无需重新构建即可实时编辑
    environment:
      # LISTEN_ADDR (0.0.0.0:80) 和 DOCUMENT_ROOT (/var/www/html/public) 都是
      # OxPHP 的默认值，因此两者均省略。
      ENTRY_FILE: index.php          # framework 模式
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
    image: mysql:9
    container_name: laravel-oxphp-db
    ports:
      - "3308:3306"
    environment:
      MYSQL_DATABASE: laravel
      MYSQL_USER: laravel
      MYSQL_PASSWORD: laravel
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

`dev` 镜像自带 PHP CLI 和 Composer，因此 `artisan` 可在同一容器中运行：

```bash
docker compose up -d --build
docker compose exec app php artisan key:generate
docker compose exec app php artisan migrate
```

## OxPHP 说明

- **`ENTRY_FILE=index.php` 启用 framework 模式。** Laravel 的 `public/` 成为 `DOCUMENT_ROOT`；框架自身的 `.env`、`app/`、`vendor/` 都位于 document root 之外，永远无法通过 HTTP 访问。
- **容器环境变量覆盖 `.env`。** OxPHP 将容器环境传递给 PHP，而 Laravel 的 dotenv 加载器不会覆盖真实的环境变量 —— 因此在 Compose 中设置的 `DB_HOST`、`APP_ENV` 等会生效。
- **`dev` 目标中 OPcache 每个请求都会重新校验**，因此对绑定挂载的 `src/` 所做的编辑会立即生效。生产环境下，请设置 `opcache.validate_timestamps=0` 并去掉绑定挂载。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8091/       # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8091/up     # 200（health 路由）
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9092/health # 200
```

## 参见

- [路由](../../features/routing.md) · [Docker 指南](../../getting-started/docker.md) · [配置参考](../../operations/configuration.md)
- [Symfony](symfony.md) 与 [Yii3](yii3.md) —— 其他 framework 模式的示例方案
