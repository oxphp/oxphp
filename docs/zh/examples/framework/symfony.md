---
title: 在 OxPHP 上运行 Symfony
description: 以 framework 路由模式在 OxPHP 上运行无数据库的 Symfony 应用 —— 包含 Dockerfile、Compose 文件、安装步骤以及 OxPHP 专属注意事项。
---

# 在 OxPHP 上运行 Symfony

Symfony 的 `public/index.php` front controller 可直接套用 OxPHP 的 [framework 路由模式](../../features/routing.md)。`symfony/skeleton` 不需要数据库，因此这是一个单服务部署 —— 如果你后续要接入持久化，再加上 Doctrine 和一个 `db` 服务即可。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.9.0`（PHP 8.5）
- **路由模式：** Framework（`ENTRY_FILE=index.php`；`DOCUMENT_ROOT` 保持默认的 `/var/www/html/public`）
- **新增扩展：** `intl`、`mbstring`（Symfony 推荐的基线；运行时严格来说只需要核心扩展）
- **服务：** 仅 OxPHP
- **URL：** `http://localhost:8096` · 内部 `http://localhost:9097/health`

> **PHP 版本：** Symfony 8 声明 `"php": ">=8.4"`，默认的 PHP 8.5 镜像满足该要求。用 `composer check-platform-reqs --no-dev` 确认没有依赖给上限设了天花板。

## 项目结构

```bash
mkdir -p symfony-oxphp/src
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/symfony-oxphp/src":/app -w /app \
    composer:2 create-project symfony/skeleton . --prefer-dist
# 可选，用于渲染页面：
docker run --rm -e COMPOSER_ALLOW_SUPERUSER=1 \
    -v "$PWD/symfony-oxphp/src":/app -w /app \
    composer:2 require twig symfony/asset
```

## Dockerfile

`src/Dockerfile`：

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── PHP base：intl + mbstring ─────────────────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache icu-libs oniguruma \
    && apk add --no-cache --virtual .build-deps icu-dev oniguruma-dev \
    && docker-php-ext-install -j"$(nproc)" intl mbstring \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP 构件（PHP 8.5 默认镜像） ───────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

# ── dev：OxPHP server + PHP CLI + Composer ────────────────────
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
      # LISTEN_ADDR (0.0.0.0:80) 和 DOCUMENT_ROOT (/var/www/html/public) 都是
      # OxPHP 的默认值，因此两者都省略。
      ENTRY_FILE: index.php          # framework 模式
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

## 安装与首次运行

```bash
docker compose up -d --build
docker compose exec app php bin/console about
```

## OxPHP 注意事项

- **最小化扩展占用。** Symfony skeleton 的运行时只需要核心扩展（`ctype`、`iconv`、`xml`），它们都已存在于基础镜像中 —— 否则 `mbstring` 会以 polyfill 形式提供。之所以加入原生的 `intl` + `mbstring`，是因为任何真实的 Symfony 应用都会用到它们，而原生版本比 polyfill 更快。
- **`APP_ENV` 来自容器环境。** Symfony 会优先从真实环境而非 `.env` 读取它，因此请在 Compose 中设置。
- **静态资源由 framework 模式从 `public/` 提供** —— `{{ asset('css/app.css') }}` 会解析到磁盘上的真实文件并直接提供，只有在未命中时才回退到 `index.php`。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8096/             # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8096/no-such-route # 404（Symfony 路由器）
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9097/health        # 200
```

## 另请参阅

- [路由](../../features/routing.md) · [Docker 指南](../../getting-started/docker.md)
- [Laravel](laravel.md) 与 [Yii3](yii3.md) —— 另外两份 framework 模式配方
