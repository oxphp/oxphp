---
title: 在 OxPHP 上运行 Yii3
description: 在 OxPHP 的 framework 路由模式下运行无数据库的 Yii3（yiisoft/app）应用——本文是最精简的示例。包含 Dockerfile、Compose 文件以及 OxPHP 特定的注意事项。
---

# 在 OxPHP 上运行 Yii3

Yii3 是这里最精简的方案。`yiisoft/app` 模板带有一个 `public/index.php` front controller，无需数据库，而且——由于大多数字符串处理都由 polyfill 提供——基本不需要额外的 PHP 扩展。它能直接映射到 OxPHP 的 [framework 路由模式](../../features/routing.md)，并作为单个服务运行。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.9.0`（PHP 8.5）
- **路由模式：** Framework（`ENTRY_FILE=index.php`；`DOCUMENT_ROOT` 保持默认值 `/var/www/html/public`）
- **新增扩展：** `mbstring`（原生扩展，为了性能——否则运行时会用 polyfill 替代）
- **服务：** 仅 OxPHP
- **URL：** `http://localhost:8094` · 内部 `http://localhost:9095/health`

> `yiisoft/app` 声明了 `"php": "8.2 - 8.5"`，所以默认的 PHP 8.5 镜像完全适用。

## 项目布局

```bash
mkdir -p yii3-oxphp/src
docker run --rm -v "$PWD/yii3-oxphp/src":/app -w /app \
    composer:2 create-project yiisoft/app . --prefer-dist
cp yii3-oxphp/src/.env.example yii3-oxphp/src/.env
```

## Dockerfile

`src/Dockerfile`：

```dockerfile
ARG OXPHP_VERSION=0.9.0
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23

# ── PHP 基础镜像：原生 mbstring ─────────────────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache oniguruma \
    && apk add --no-cache --virtual .build-deps oniguruma-dev \
    && docker-php-ext-install -j"$(nproc)" mbstring \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP 构件（PHP 8.5 默认镜像） ───────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION} AS oxphp

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
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html
ENV LD_LIBRARY_PATH=/usr/local/lib
WORKDIR /var/www/html
COPY --chown=www-data:www-data . /var/www/html
EXPOSE 80 9090
CMD ["oxphp"]
```

> `yiisoft/app` 模板自带白名单式的 `.dockerignore`；保留它——在绑定挂载 `src/` 的情况下，`COPY` 只是一个兜底措施。

## docker-compose.yml

```yaml
services:
  app:
    build:
      context: ./src
      target: dev
    image: yii3-oxphp/app:dev
    container_name: yii3-oxphp
    ports:
      - "8094:80"
      - "9095:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR (0.0.0.0:80) 和 DOCUMENT_ROOT (/var/www/html/public)
      # 都是 OxPHP 的默认值，所以两者都省略。
      ENTRY_FILE: index.php          # framework 模式
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
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
docker compose exec app php yii          # Yii 控制台
```

## OxPHP 注意事项

- **无数据库、无额外服务**——这是规模最小的 OxPHP 部署：一个镜像，一个进程。
- **`yiisoft/assets` 会将静态资源发布到 `public/assets/<hash>/`**——是磁盘上的真实文件，由 framework 模式直接提供。无需做任何 URL 重写的花活（不像某些在 URL 中嵌入版本段的资源管线）。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8094/              # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8094/nonexistent   # 404（Yii 路由器）
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9095/health         # 200
```

## 另请参阅

- [路由](../../features/routing.md) · [Docker 指南](../../getting-started/docker.md)
- [Laravel](laravel.md) 和 [Symfony](symfony.md)——其他 framework 模式的方案
