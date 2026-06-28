---
title: Example Deployments
description: Real-world recipes for running popular PHP frameworks and CMSs on OxPHP — Laravel, Symfony, Yii3, WordPress, Drupal, Craft, Magento, OpenCart, and October CMS — each with a Dockerfile, Compose file, install steps, and the OxPHP-specific notes that matter.
---

# Example Deployments

These guides show how to run nine popular PHP applications on OxPHP, each as a self-contained Docker Compose project. Every recipe was built and verified end-to-end: storefront, admin panel, static assets, and the OxPHP internal health endpoint all answer `200`.

Each page is a complete, copy-pasteable recipe — a `Dockerfile`, a `docker-compose.yml`, the install commands, and the OxPHP-specific details that the application's stock documentation (written for nginx + PHP-FPM) does not cover.

## The applications

| Application | Type | Routing mode | PHP | Extra services | Install method |
|-------------|------|--------------|-----|----------------|----------------|
| [Laravel](framework/laravel.md) | Framework | Framework | 8.5 | MySQL | `composer create-project` |
| [Symfony](framework/symfony.md) | Framework | Framework | 8.5 | — | `composer create-project` |
| [Yii3](framework/yii3.md) | Framework | Framework | 8.5 | — | `composer create-project` |
| [WordPress](cms/wordpress.md) | CMS | Traditional | 8.5 | MySQL | WP-CLI |
| [Drupal](cms/drupal.md) | CMS | Framework | 8.4 | MySQL | `drush site:install` |
| [Craft CMS](cms/craft.md) | CMS | Framework | 8.5 | MySQL | `craft install` |
| [October CMS](cms/october.md) | CMS | Framework + mirror | 8.4 | MySQL | `october:migrate` + mirror |
| [Magento](ecommerce/magento.md) | E-commerce | Framework | 8.4 | MySQL + OpenSearch | `bin/magento setup:install` |
| [OpenCart](ecommerce/opencart.md) | E-commerce | Traditional | 8.4 | MySQL | CLI installer |

## What every recipe has in common

### Build on the published OxPHP image

OxPHP ships a ready PHP runtime as `ghcr.io/oxphp/oxphp` (PHP 8.5 by default; a PHP 8.4 variant is published as `ghcr.io/oxphp/oxphp:<ver>-php8.4-alpine<X>`). The published image already contains the `oxphp` binary, `libphp.so`, the OxPHP SAPI extension, the PHP CLI, and Composer-friendly tooling. The recipes extend it in one of two shapes:

1. **Copy OxPHP into a `php:*-zts-alpine` base** (used by Laravel, Symfony, Yii3, Craft, Magento, OpenCart, Drupal, October). A multi-stage build assembles a `dev` image from four stages:

   ```dockerfile
   FROM php:8.4-zts-alpine3.23 AS php-base       # your app's PHP extensions
   FROM composer:2            AS composer        # the Composer binary
   FROM ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23 AS oxphp   # OxPHP artifacts
   FROM php-base AS dev                          # final image
   # ... copy the oxphp binary, bridge library, and SAPI extension across:
   COPY --from=oxphp /usr/local/bin/oxphp              /usr/local/bin/oxphp
   COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
   COPY --from=oxphp /usr/local/lib/php/extensions/    /tmp/oxphp-ext/
   RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(php -r 'echo ini_get("extension_dir");')/" \
       && echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini
   ```

2. **Extend the OxPHP runtime directly** (used by WordPress). A builder stage compiles the extensions against a matching `php:*-zts-alpine` and drops the `.so` files into the OxPHP image.

Either way, **the PHP ABI must match**. The OxPHP image's `libphp.so` and `oxphp_sapi.so` are compiled for one PHP version (e.g. 8.4 → `no-debug-zts-20240924`); the `php-base`/builder stage must use the same `php:<X.Y>-zts-alpine<Z>` so the extensions you compile are ABI-compatible. Mixing versions makes `oxphp_sapi.so` refuse to load or corrupts musl TLS at startup.

See the [Docker Guide](../getting-started/docker.md) for the canonical multi-stage template, and [`examples/dockerfile/`](../../../examples/dockerfile/) in the repository for a ready-to-copy version.

### Pick the PHP version deliberately

The default `ghcr.io/oxphp/oxphp:<ver>` image is **PHP 8.5**. That is fine for modern frameworks (Laravel, Symfony, Yii3, Craft). Older or more conservative codebases — Magento, OpenCart, Drupal, October CMS — pin **PHP 8.4** via the `…-php8.4-alpine…` tag, because their component stacks predate 8.5 and emit deprecations on it. Each recipe states which it uses and why.

### Choose the routing mode from the app's shape

OxPHP's [routing mode](../features/routing.md) maps directly onto how the application is laid out:

- **Framework mode** (`ENTRY_FILE=index.php`) — one front controller under a `public/` (or `web/`, `pub/`) directory; existing static files are served from disk, everything else is dispatched to `index.php`. Use for Laravel, Symfony, Yii3, Craft, Magento, Drupal, October.
- **Traditional mode** (no `ENTRY_FILE`) — multiple physical PHP entry points (e.g. `index.php` plus an `admin/` directory) served as real files. Use for WordPress and OpenCart.

### Install through the same container

The `dev` image carries the PHP CLI and Composer (and `drush`, `wp`, `bin/magento`, `php yii`, `php craft`, `php artisan` as appropriate), so every install and maintenance command runs inside the running container — no separate toolchain:

```bash
docker compose exec app php artisan migrate      # Laravel
docker compose exec app vendor/bin/drush cr      # Drupal
docker compose run  --rm app composer install    # any
```

### Security defaults that apply everywhere

OxPHP gives you, for free, several protections that nginx + PHP-FPM require explicit config for:

- [Dot-path blocking](../security/dot-path-blocking.md) — `.env`, `.git/`, `.htaccess` and any other dot-segment path return `404` without configuration. This is why running an app from a directory that also contains `.env` does not leak it.
- [PHP execution deny-list](../security/php-deny.md) (`PHP_DENY_PATHS`) — used by the traditional-mode recipes: OpenCart blocks `system/` and `install/` scripts; WordPress blocks `.php` execution under `wp-content/uploads/`. (A no-op in framework mode, where arbitrary `.php` is never executed directly.)
- [Symlink allow-paths](../security/symlink-allow-paths.md) (`SYMLINK_ALLOW_PATHS`) — used by October CMS so OxPHP follows the asset symlinks produced by `october:mirror public`, while still blocking symlink escapes everywhere else.

## The recipes

The pages are grouped by application type, mirroring the directory layout:

```
examples/
├── framework/   # Laravel, Symfony, Yii3
├── cms/         # WordPress, Drupal, Craft, October
└── ecommerce/   # Magento, OpenCart
```

### Framework — `framework/`

- [Laravel](framework/laravel.md) — the canonical framework-mode app
- [Symfony](framework/symfony.md) — minimal skeleton, no database
- [Yii3](framework/yii3.md) — leanest of all; core extensions only

### CMS — `cms/`

- [WordPress](cms/wordpress.md) — traditional mode, runtime-extension build, WP-CLI sidecar
- [Drupal](cms/drupal.md) — framework mode, PDO + `drush`
- [Craft CMS](cms/craft.md) — framework mode, console-driven install
- [October CMS](cms/october.md) — framework mode with a `public/` mirror and `SYMLINK_ALLOW_PATHS`

### E-commerce — `ecommerce/`

- [Magento](ecommerce/magento.md) — the heaviest: OpenSearch, PHP 8.4, static-asset version symlink
- [OpenCart](ecommerce/opencart.md) — traditional mode with two front controllers and `PHP_DENY_PATHS`
