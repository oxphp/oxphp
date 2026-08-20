---
title: WordPress on OxPHP
description: Run WordPress on OxPHP in traditional routing mode with MySQL and a WP-CLI sidecar — Dockerfile that extends the OxPHP runtime, Compose file, and OxPHP-specific notes.
---

# WordPress on OxPHP

WordPress is a **traditional-mode** application: it has many physical entry points (`index.php`, `wp-login.php`, `wp-cron.php`, and the whole `wp-admin/` directory), each meant to be reached as a real file. OxPHP's [traditional routing mode](../../features/routing.md) — the default when `ENTRY_FILE` is unset — serves them exactly that way. Do **not** set `ENTRY_FILE`: framework mode would funnel every request to one front controller and break `wp-admin`.

This recipe also demonstrates the second build shape: instead of copying OxPHP into a PHP base, it **extends the OxPHP runtime image directly**, compiling WordPress's extensions in a builder stage and dropping the `.so` files in.

## Stack at a glance

- **OxPHP image:** `ghcr.io/oxphp/oxphp:0.11.0` (PHP 8.5) — extended in place
- **Routing mode:** Traditional (no `ENTRY_FILE`)
- **Extensions added:** `mysqli`, `pdo_mysql`, `gd`, `zip`, `intl`, `exif`, `bcmath`
- **Services:** OxPHP + MySQL + a WP-CLI sidecar (`cli` profile)
- **Hardening:** `PHP_DENY_PATHS` blocks direct `.php` execution under `wp-content/uploads/`
- **URL:** `http://localhost:8090` · internal `http://localhost:9091/health`

## Project layout

```
wp-oxphp/
├── Dockerfile
├── docker-compose.yml
└── wordpress/                # the WordPress tree (download from wordpress.org)
    └── wp-config.php         # reads WORDPRESS_* environment variables
```

`wordpress/wp-config.php` reads its settings from the container environment:

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

This Dockerfile compiles the extensions against a matching `php:8.5-zts-alpine` (same ABI as the OxPHP image, `no-debug-zts-20250925`) and copies the `.so` files into the OxPHP runtime:

```dockerfile
# ── Stage 1: compile WordPress PHP extensions against PHP 8.5 ─────
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

# ── Stage 2: OxPHP runtime with WordPress extensions ─────────────
FROM ghcr.io/oxphp/oxphp:0.11.0 AS runtime
USER root
RUN apk add --no-cache icu-libs libzip libpng libjpeg-turbo freetype oniguruma
# Drop the compiled extensions into OxPHP's PHP 8.5 extension dir
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

> The extension directory (`no-debug-zts-20250925`) is the PHP 8.5 ZTS ABI tag. If you build on the PHP 8.4 OxPHP image, this becomes `no-debug-zts-20240924` — derive it at build time with `php -r 'echo ini_get("extension_dir");'` rather than hard-coding it.

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
      - ./wordpress:/var/www/html/public   # WordPress tree, live-editable
    environment:
      # Traditional routing — NO ENTRY_FILE, so wp-admin/*.php, wp-login.php,
      # wp-cron.php are served as physical files the way WordPress expects.
      # LISTEN_ADDR (0.0.0.0:80) and DOCUMENT_ROOT (/var/www/html/public) are
      # the OxPHP defaults, so both are omitted.
      INTERNAL_ADDR: 0.0.0.0:9090
      ACCESS_LOG: all
      # Block direct PHP execution where user content lands — defeats a shell
      # uploaded into wp-content/uploads by a vulnerable plugin. Do NOT add
      # /wp-content/plugins/** or /wp-content/themes/** — some plugins expose
      # directly-callable .php endpoints there.
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

  # On-demand WP-CLI — the OxPHP runtime image ships no `wp` binary.
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

## Install and first run

```bash
docker compose up -d --build
docker compose run --rm wpcli wp core install \
    --url=http://localhost:8090 --title="OxPHP WordPress" \
    --admin_user=admin --admin_password=admin --admin_email=admin@example.com
```

## OxPHP notes

- **Traditional mode is mandatory.** WordPress needs `wp-admin/`, `wp-login.php`, `wp-cron.php`, etc. to execute as physical files. Leave `ENTRY_FILE` unset.
- **The OxPHP runtime image ships no `wp` binary** (it is a minimal serving image). CLI work goes through the `wpcli` sidecar under the `cli` Compose profile, sharing the same `wordpress/` volume and database.
- **`wp-config.php` reads the container environment** via `getenv()`, so database credentials and the site URL live in Compose, not hard-coded in the file.
- **`PHP_DENY_PATHS` hardens the upload directories.** Because traditional mode executes physical `.php` files, a shell uploaded into `wp-content/uploads/` by a vulnerable plugin would otherwise run. `PHP_DENY_PATHS` blocks `.php` execution under those paths — matched against the URI *before* any disk I/O, so there is no existence oracle. It applies only in traditional and SPA modes; in framework mode it is a no-op (the front controller already prevents direct `.php` execution), which is why the framework-mode recipes here do not need it. See [PHP Execution Deny-List](../../security/php-deny.md).

## Verify

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/            # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/wp-login.php # 200 (physical file)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8090/wp-content/uploads/x.php # 404 (PHP_DENY_PATHS)
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:9091/health       # 200
```

## See also

- [Routing](../../features/routing.md) · [PHP Execution Deny-List](../../security/php-deny.md) · [Docker Guide](../../getting-started/docker.md)
- [OpenCart](../ecommerce/opencart.md) — the other traditional-mode recipe
