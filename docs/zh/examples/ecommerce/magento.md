---
title: 在 OxPHP 上运行 Magento
description: 以 framework 路由模式在 OxPHP 上运行 Magento Open Source 2.4，搭配 MySQL 与 OpenSearch —— 包含 Dockerfile、Compose 文件、安装步骤、静态资源版本符号链接，以及 OxPHP 专属注意事项。
---

# 在 OxPHP 上运行 Magento

Magento 是本系列中最重的方案：它强制要求一个搜索引擎（OpenSearch）、一长串扩展，以及一个静态内容部署步骤。它运行在 OxPHP 的 [framework 路由模式](../../features/routing.md) 下，以 `pub/` 作为 document root，但其带版本号的静态资源 URL 需要下文所述的一个额外步骤。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.10.0-php8.4-alpine3.23`（PHP 8.4）
- **路由模式：** Framework（`ENTRY_FILE=index.php`；`DOCUMENT_ROOT=/var/www/html/pub` 覆盖默认的 `…/public`）
- **新增扩展：** `bcmath`、`gd`、`intl`、`pdo_mysql`、`soap`、`xsl`、`zip`、`mbstring`、`ftp`、`pcntl`、`sockets`
- **服务：** OxPHP + MySQL 8.0 + OpenSearch 2.x
- **URL：** `http://localhost:8093` · 后台 `/admin` · 内部 `http://localhost:9094/health`

> **PHP 版本：** Magento 2.4.8 声明 `"php": "~8.2 || ~8.3 || ~8.4"`，并拒绝 PHP 8.5 —— 请锁定 **PHP 8.4** 镜像。`ext-sockets` 是一个传递依赖（来自 `php-amqplib`），编译时需要 `linux-headers`。

## 项目布局

来自官方分发渠道的 Magento Open Source 需要 Adobe Marketplace 的鉴权密钥。要在没有密钥的情况下安装，请克隆开源仓库（其模块通过 `replace` 在源码树内提供，因此 `composer install` 只会拉取 Packagist 依赖）：

```bash
mkdir -p magento-oxphp
git clone --branch 2.4.8 --depth 1 https://github.com/magento/magento2.git magento-oxphp/src
# composer install 稍后在 PHP 8.4 容器内运行（composer:2 是 PHP 8.5，
# 而 Magento 会拒绝它）：
docker compose run --rm --no-deps -e COMPOSER_MEMORY_LIMIT=-1 app \
    composer install --no-interaction --prefer-dist
```

## Dockerfile

`src/Dockerfile.oxphp`（这样命名是为了避免与 Magento 自带的 docker 资源冲突）：

```dockerfile
ARG OXPHP_VERSION=0.10.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ── PHP 基础镜像：Magento 运行时扩展 ──────────────────────
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base
RUN apk add --no-cache \
        icu-libs libpng libjpeg-turbo freetype oniguruma libzip libxslt \
    && apk add --no-cache --virtual .build-deps \
        icu-dev libpng-dev libjpeg-turbo-dev freetype-dev oniguruma-dev \
        libzip-dev libxslt-dev libxml2-dev linux-headers \
    && docker-php-ext-configure gd --with-jpeg --with-freetype \
    && docker-php-ext-install -j"$(nproc)" \
        bcmath gd intl pdo_mysql soap xsl zip mbstring ftp pcntl sockets \
    && apk del .build-deps

# ── Composer ──────────────────────────────────────────────────
FROM composer:2 AS composer

# ── OxPHP 构件（PHP 8.4 镜像） ───────────────────────────
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ── dev：OxPHP 服务器 + PHP CLI + Composer + Magento 扩展 ─
FROM php-base AS dev
RUN apk add --no-cache libgcc git patch
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
        echo "memory_limit=4G"; echo "max_execution_time=1800"; \
        echo "realpath_cache_size=10M"; echo "realpath_cache_ttl=86400"; \
        echo "upload_max_filesize=64M"; echo "post_max_size=64M"; \
    } > /usr/local/etc/php/conf.d/magento.ini
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data
RUN mkdir -p /var/www/html/pub && chown -R www-data:www-data /var/www/html
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
    image: magento-oxphp/app:dev
    container_name: magento-oxphp
    ports:
      - "8093:80"
      - "9094:9090"
    volumes:
      - ./src:/var/www/html
    environment:
      # LISTEN_ADDR 默认为 0.0.0.0:80，因此此处省略。
      DOCUMENT_ROOT: /var/www/html/pub   # 覆盖默认的 /var/www/html/public
      ENTRY_FILE: index.php              # framework 模式
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
    depends_on:
      db:
        condition: service_healthy
      opensearch:
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
    container_name: magento-oxphp-db
    ports:
      - "3310:3306"
    command:
      - --max_allowed_packet=64M
      - --innodb-buffer-pool-size=1G
      - --log_bin_trust_function_creators=1   # Magento 会创建触发器/函数
    environment:
      MYSQL_DATABASE: magento
      MYSQL_USER: magento
      MYSQL_PASSWORD: magento
      MYSQL_ROOT_PASSWORD: root
    volumes:
      - db_data:/var/lib/mysql
    healthcheck:
      test: ["CMD", "mysqladmin", "ping", "-h", "127.0.0.1", "-uroot", "-proot"]
      interval: 5s
      timeout: 5s
      retries: 30
      start_period: 20s
    restart: unless-stopped

  opensearch:
    image: opensearchproject/opensearch:2.19.1
    container_name: magento-oxphp-opensearch
    environment:
      - discovery.type=single-node
      - DISABLE_SECURITY_PLUGIN=true
      - DISABLE_INSTALL_DEMO_CONFIG=true
      - bootstrap.memory_lock=true
      - "OPENSEARCH_JAVA_OPTS=-Xms1g -Xmx1g"
    ulimits:
      memlock: { soft: -1, hard: -1 }
      nofile:  { soft: 65536, hard: 65536 }
    ports:
      - "9201:9200"
    volumes:
      - opensearch_data:/usr/share/opensearch/data
    healthcheck:
      test: ["CMD-SHELL", "curl -sf http://localhost:9200/_cluster/health || exit 1"]
      interval: 10s
      timeout: 5s
      retries: 30
      start_period: 20s
    restart: unless-stopped

volumes:
  db_data:
  opensearch_data:
```

## 安装与首次运行

```bash
docker compose up -d db opensearch
# composer install（参见“项目布局”），然后：
docker compose run --rm app php bin/magento setup:install \
    --base-url=http://localhost:8093/ \
    --db-host=db --db-name=magento --db-user=magento --db-password=magento \
    --admin-firstname=Admin --admin-lastname=User \
    --admin-email=admin@example.com --admin-user=admin --admin-password='Admin123!' \
    --language=en_US --currency=USD --timezone=America/New_York \
    --search-engine=opensearch --opensearch-host=opensearch --opensearch-port=9200 \
    --opensearch-index-prefix=magento --opensearch-enable-auth=0

# production 模式会编译 DI 并部署静态内容
docker compose run --rm app php bin/magento deploy:mode:set production

# 对 OxPHP 至关重要：让带版本号的静态 URL 能够解析（参见下方注意事项）
docker compose run --rm app sh -c \
    'ln -sfn . "pub/static/version$(cat pub/static/deployed_version.txt)"'

docker compose up -d app
```

## OxPHP 注意事项

- **带版本号的静态 URL 需要一个符号链接。** Magento 生成的资源 URL 形如 `/static/version<timestamp>/frontend/…`，而文件实际位于 `pub/static/frontend/…`。nginx 通过一条 rewrite 规则去掉 `version<N>/` 段；OxPHP 的 framework 模式没有这样的 rewrite，因此每个带版本号的资源都会返回 `404`，店面也会因为缺少样式而显示异常。解决办法是创建一个自引用符号链接，让带版本号的路径解析到真实文件：

  ```bash
  ln -sfn . "pub/static/version$(cat pub/static/deployed_version.txt)"
  ```

  由于该符号链接位于 `DOCUMENT_ROOT`（`pub/`）之内，无需配置任何 `SYMLINK_ALLOW_PATHS` 条目。

- **运行 production 模式。** OxPHP 的 worker 池是多线程的（PHP ZTS）。Magento 的 developer 模式会即时生成 DI 类和静态资源（通过 `pub/static.php`，而 framework 模式并不路由它），这有可能在多个 worker 线程之间产生竞态。`deploy:mode:set production` 会预先编译 DI 并提前部署静态内容，因此 worker 在请求时绝不会生成代码。
- **MySQL：`--log_bin_trust_function_creators=1`。** Magento 在安装过程中会创建触发器和存储函数；在启用了二进制日志的情况下（MySQL 8.0 的默认设置），非 `SUPER` 权限的 `magento` 用户否则会遇到错误 1419。
- **`composer install` 在 PHP 8.4 容器中运行。** 自带的 `composer:2` 镜像是 PHP 8.5，会被 Magento 拒绝；请改用构建好的 `app` 镜像来运行它。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8093/         # 200 店面
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8093/admin    # 200 后台登录
curl -s    http://localhost:8093/ | grep -oE '/static/version[0-9]+/[^"]+\.css' | head -1 | \
    xargs -I{} curl -s -o /dev/null -w "%{http_code}\n" "http://localhost:8093{}"  # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9094/health    # 200
```

## 另请参阅

- [Routing](../../features/routing.md) · [Docker 指南](../../getting-started/docker.md) · [配置参考](../../operations/configuration.md)
- [OpenCart](opencart.md) —— 另一个电商方案
