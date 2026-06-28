---
title: 在 OxPHP 上运行 WordPress
description: 以 traditional 路由模式在 OxPHP 上运行 WordPress，配合 MySQL 和 WP-CLI sidecar —— 扩展 OxPHP 运行时的 Dockerfile、Compose 文件以及 OxPHP 专属说明。
---

# 在 OxPHP 上运行 WordPress

WordPress 是一个 **traditional 模式** 应用：它有许多物理入口（`index.php`、`wp-login.php`、`wp-cron.php`，以及整个 `wp-admin/` 目录），每一个都意在作为真实文件被访问。OxPHP 的 [traditional 路由模式](../../features/routing.md) —— 当 `ENTRY_FILE` 未设置时的默认模式 —— 正是以这种方式提供服务。**不要**设置 `ENTRY_FILE`：framework 模式会把每个请求都汇集到单一 front controller，从而破坏 `wp-admin`。

本方案还演示了第二种构建形态：它不是把 OxPHP 复制进 PHP 基础镜像，而是**直接扩展 OxPHP 运行时镜像**，在一个 builder 阶段编译 WordPress 所需的扩展，然后把 `.so` 文件放进去。

## 技术栈一览

- **OxPHP 镜像：** `ghcr.io/oxphp/oxphp:0.9.0`（PHP 8.5）—— 就地扩展
- **路由模式：** Traditional（无 `ENTRY_FILE`）
- **新增扩展：** `mysqli`、`pdo_mysql`、`gd`、`zip`、`intl`、`exif`、`bcmath`
- **服务：** OxPHP + MySQL + 一个 WP-CLI sidecar（`cli` profile）
- **加固：** `PHP_DENY_PATHS` 阻止在 `wp-content/uploads/` 下直接执行 `.php`
- **URL：** `http://localhost:8090` · 内部 `http://localhost:9091/health`

## 项目布局

```
wp-oxphp/
├── Dockerfile
├── docker-compose.yml
└── wordpress/                # WordPress 目录树（从 wordpress.org 下载）
    └── wp-config.php         # 读取 WORDPRESS_* 环境变量
```

`wordpress/wp-config.php` 从容器环境中读取配置：

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

这个 Dockerfile 针对一个匹配的 `php:8.5-zts-alpine`（与 OxPHP 镜像同一 ABI，`no-debug-zts-20250925`）编译扩展，并把 `.so` 文件复制进 OxPHP 运行时：

```dockerfile
# ── 阶段 1：针对 PHP 8.5 编译 WordPress 的 PHP 扩展 ─────
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

# ── 阶段 2：带有 WordPress 扩展的 OxPHP 运行时 ─────────────
FROM ghcr.io/oxphp/oxphp:0.9.0 AS runtime
USER root
RUN apk add --no-cache icu-libs libzip libpng libjpeg-turbo freetype oniguruma
# 把编译好的扩展放进 OxPHP 的 PHP 8.5 扩展目录
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

> 扩展目录（`no-debug-zts-20250925`）是 PHP 8.5 ZTS 的 ABI 标签。如果你基于 PHP 8.4 的 OxPHP 镜像构建，它会变成 `no-debug-zts-20240924` —— 请在构建时用 `php -r 'echo ini_get("extension_dir");'` 推导出来，而不要硬编码。

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
      - ./wordpress:/var/www/html/public   # WordPress 目录树，可实时编辑
    environment:
      # Traditional 路由 —— 无 ENTRY_FILE，因此 wp-admin/*.php、wp-login.php、
      # wp-cron.php 会按照 WordPress 期望的方式作为物理文件提供。
      # LISTEN_ADDR (0.0.0.0:80) 和 DOCUMENT_ROOT (/var/www/html/public) 是
      # OxPHP 的默认值，因此两者都省略。
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      # 在用户内容落地处阻止直接执行 PHP —— 这能挫败被有漏洞的插件上传到
      # wp-content/uploads 中的 shell。不要添加 /wp-content/plugins/** 或
      # /wp-content/themes/** —— 有些插件会在那里暴露可直接调用的 .php 端点。
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

  # 按需使用的 WP-CLI —— OxPHP 运行时镜像不附带 `wp` 二进制文件。
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

## 安装与首次运行

```bash
docker compose up -d --build
docker compose run --rm wpcli wp core install \
    --url=http://localhost:8090 --title="OxPHP WordPress" \
    --admin_user=admin --admin_password=admin --admin_email=admin@example.com
```

## OxPHP 说明

- **Traditional 模式是必需的。** WordPress 需要 `wp-admin/`、`wp-login.php`、`wp-cron.php` 等作为物理文件执行。请保持 `ENTRY_FILE` 未设置。
- **OxPHP 运行时镜像不附带 `wp` 二进制文件**（它是一个精简的服务镜像）。CLI 工作通过 `cli` Compose profile 下的 `wpcli` sidecar 完成，它共享同一个 `wordpress/` 卷和数据库。
- **`wp-config.php` 通过 `getenv()` 读取容器环境**，因此数据库凭据和站点 URL 都放在 Compose 中，而不是硬编码在文件里。
- **`PHP_DENY_PATHS` 对上传目录进行加固。** 由于 traditional 模式会执行物理 `.php` 文件，若不加防护，被有漏洞的插件上传到 `wp-content/uploads/` 中的 shell 就会被执行。`PHP_DENY_PATHS` 阻止在这些路径下执行 `.php` —— 它在任何磁盘 I/O *之前* 就针对 URI 进行匹配，因此不存在存在性预言（existence oracle）。它仅在 traditional 和 SPA 模式下生效；在 framework 模式下它是个空操作（front controller 已经阻止了直接执行 `.php`），这也是为什么本处的 framework 模式方案不需要它。参见 [PHP 执行拒绝列表](../../security/php-deny.md)。

## 验证

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/            # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/wp-login.php # 200（物理文件）
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/wp-content/uploads/x.php # 404（PHP_DENY_PATHS）
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9091/health       # 200
```

## 另见

- [路由](../../features/routing.md) · [PHP 执行拒绝列表](../../security/php-deny.md) · [Docker 指南](../../getting-started/docker.md)
- [OpenCart](../ecommerce/opencart.md) —— 另一个 traditional 模式方案
