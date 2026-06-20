# ══════════════════════════════════════════════════════════════
# OxPHP Best Practice Dockerfile
#
# Usage:
#   docker build --target dev      -t myapp:dev      .
#   docker build --target prod     -t myapp:prod     .
#   docker build --target prod-cli -t myapp:prod-cli .
#
# Dev:      PHP CLI, Composer, Xdebug, opcache validates timestamps
# Prod:     Minimal runtime on bare alpine — no PHP CLI, no toolchain,
#           no package manager clutter. Only the oxphp binary,
#           libphp.so, required shared libraries, and your app code.
#           Runs as www-data.
# Prod-CLI: Short-lived image for migrations/artisan/maintenance.
#           Run as `docker run --rm myapp:prod-cli php artisan migrate`.
#           Keep out of the long-running serving path.
# ══════════════════════════════════════════════════════════════

# Pinned versions — single source of truth. The OxPHP image tag below is
# derived from all three (canonical scheme: {ver}-php{minor}-alpine{alpine}),
# so the PHP/Alpine versions of the OxPHP base image, the php-base stage, and
# the prod runtime can never drift apart. That match matters: a mismatched
# musl/PHP version behind the precompiled libphp.so and the Rust-compiled
# oxphp binary can cause musl TLS corruption or undefined-symbol errors at
# startup. Override any of them with --build-arg at build time.
ARG OXPHP_VERSION=0.8.0
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23

# ══════════════════════════════════════════════════════════════
# Stage: php-base — shared PHP extensions (pdo, intl)
# ══════════════════════════════════════════════════════════════
FROM php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION} AS php-base

RUN apk add --no-cache \
        icu-dev \
        icu-libs \
        postgresql-dev \
        libpq \
    && docker-php-ext-install \
        pdo \
        pdo_mysql \
        pdo_pgsql \
        intl \
    && apk del icu-dev postgresql-dev

# ══════════════════════════════════════════════════════════════
# Stage: php-dev — add xdebug on top of base
# ══════════════════════════════════════════════════════════════
FROM php-base AS php-dev

RUN apk add --no-cache $PHPIZE_DEPS linux-headers \
    && pecl install xdebug \
    && docker-php-ext-enable xdebug \
    && apk del $PHPIZE_DEPS linux-headers

# ══════════════════════════════════════════════════════════════
# Stage: composer — official Composer image
# ══════════════════════════════════════════════════════════════
FROM composer:2 AS composer

# ══════════════════════════════════════════════════════════════
# Stage: oxphp — pull OxPHP artifacts
#
# The tag is derived from the ARGs above, so its Alpine/PHP versions match the
# php-base stage by construction. For production hardening, pin by digest
# instead of tag (tags are mutable, digests are not):
#   FROM ghcr.io/oxphp/oxphp:0.8.0-php8.4-alpine3.23@sha256:<digest> AS oxphp
# The same applies to php:*-zts-alpine* and composer:2 above.
# ══════════════════════════════════════════════════════════════
FROM ghcr.io/oxphp/oxphp:${OXPHP_VERSION}-php${PHP_VERSION}-alpine${ALPINE_VERSION} AS oxphp

# ══════════════════════════════════════════════════════════════
# Target: dev
#
# Base: php:${PHP_VERSION}-zts-alpine (PHP CLI available)
# Extensions: pdo, intl, xdebug
# OxPHP copied INTO this image
# ══════════════════════════════════════════════════════════════
FROM php-dev AS dev

RUN apk add --no-cache libgcc

# Composer
COPY --from=composer /usr/bin/composer /usr/local/bin/composer

# OxPHP binary
COPY --from=oxphp /usr/local/bin/oxphp /usr/local/bin/oxphp

# Bridge library
COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/

# OxPHP PHP extension (match base image's extension directory)
RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    echo "$EXT_DIR" > /tmp/ext_dir
COPY --from=oxphp /usr/local/lib/php/extensions/ /tmp/oxphp-ext/
RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(cat /tmp/ext_dir)/" && \
    rm -rf /tmp/oxphp-ext /tmp/ext_dir

# PHP config
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini

# Dev-friendly opcache (validates timestamps, no JIT)
# Note: base image already loads opcache, so only override settings
RUN { \
        echo "[opcache]"; \
        echo "opcache.enable=1"; \
        echo "opcache.enable_cli=1"; \
        echo "opcache.validate_timestamps=1"; \
        echo "opcache.revalidate_freq=0"; \
    } > /usr/local/etc/php/conf.d/opcache-dev.ini

# Xdebug defaults (connect back to host)
RUN { \
        echo "[xdebug]"; \
        echo "xdebug.mode=debug"; \
        echo "xdebug.start_with_request=trigger"; \
        echo "xdebug.client_host=host.docker.internal"; \
        echo "xdebug.client_port=9003"; \
    } > /usr/local/etc/php/conf.d/xdebug-config.ini

# Create www-data user if it doesn't already exist in the base image.
RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data

# Web root
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

ENV LD_LIBRARY_PATH=/usr/local/lib

# Your app code
COPY --chown=www-data:www-data . /var/www/html

EXPOSE 80 443

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -qO- http://127.0.0.1/ >/dev/null || exit 1

CMD ["oxphp"]

# ══════════════════════════════════════════════════════════════
# Target: prod-cli
#
# Short-lived image for running CLI operations in staging/production:
# migrations (`php artisan migrate`), console commands, cache warmup,
# ad-hoc composer invocations. Run as `docker run --rm` or as a
# Kubernetes Job — keep this image out of the serving path.
#
# Base:       php:${PHP_VERSION}-zts-alpine + prod extensions (no OxPHP server)
# Extras:     Composer
# User:       www-data
# Entrypoint: inherited docker-php-entrypoint — pass the command at
#             `docker run` time.
# ══════════════════════════════════════════════════════════════
FROM php-base AS prod-cli

RUN apk add --no-cache icu-libs libpq

COPY --from=composer /usr/bin/composer /usr/local/bin/composer

RUN getent passwd www-data >/dev/null \
    || adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data

RUN mkdir -p /var/www/html && chown -R www-data:www-data /var/www/html

COPY --chown=www-data:www-data . /var/www/html

USER www-data
WORKDIR /var/www/html

# No CMD — pass the command at runtime:
#   docker run --rm myapp:prod-cli php artisan migrate
#   docker run --rm myapp:prod-cli composer install --no-dev

# ══════════════════════════════════════════════════════════════
# Stage: prod-extensions — build PHP extensions into standalone dir
#
# Copies the compiled .so files into /ext-out preserving PHP's
# real extension_dir path (e.g. /usr/local/lib/php/extensions/
# no-debug-zts-20240924/). Copying /ext-out/ into the prod image
# lands them in the right place without resolving extension_dir in
# prod (prod has no PHP CLI).
# ══════════════════════════════════════════════════════════════
FROM php-base AS prod-extensions

RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    mkdir -p "/ext-out$EXT_DIR" && \
    cp "$EXT_DIR"/pdo_mysql.so \
       "$EXT_DIR"/pdo_pgsql.so \
       "$EXT_DIR"/intl.so \
       "/ext-out$EXT_DIR/"

# ══════════════════════════════════════════════════════════════
# Target: prod
#
# Minimal runtime on bare alpine. Contains:
#   - oxphp binary                       (from OxPHP image)
#   - libphp.so                          (from OxPHP image)
#   - liboxphp_bridge.so                 (from OxPHP image)
#   - oxphp_sapi.so extension            (from OxPHP image)
#   - pdo_mysql, pdo_pgsql, intl         (from prod-extensions)
#   - PHP config directory               (from OxPHP image)
#   - Runtime shared libraries           (from apk)
#   - Your app code
# Does NOT contain:
#   - PHP CLI (`/usr/local/bin/php`)
#   - phpize, docker-php-*, pecl, pear
#   - Composer (use prod-cli target for that)
#   - Any build toolchain
#
# Base: alpine:${ALPINE_VERSION} — must match the alpine version
#       baked into the OxPHP base image, otherwise musl/libc version
#       skew can cause TLS corruption at startup.
# User: www-data
#
# Size budget (smoke-tested on linux/arm64, without your app code):
#   alpine rootfs         ~9 MB
#   apk runtime libs     ~33 MB
#   oxphp binary         ~8 MB
#   libphp.so           ~20 MB
#   bridge + ext + conf  ~6 MB
#   ---------------------------
#   base total          ~76 MB
#
# For reference, the full ghcr.io/oxphp/oxphp base image is ~190 MB, so this
# approach roughly halves the image size at the cost of an explicit apk-deps
# list that must track PHP/Alpine version bumps.
# ══════════════════════════════════════════════════════════════
FROM alpine:${ALPINE_VERSION} AS prod

# Runtime shared libraries for the Rust-compiled oxphp binary and for
# libphp.so. The PHP base image statically resolves these at build
# time; we must provide the same set of libraries here.
#
# This list was derived from:
#   ldd /usr/local/bin/oxphp
#   ldd /usr/local/lib/libphp.so
#   ldd /usr/local/lib/php/extensions/no-debug-zts-*/intl.so
#   ldd /usr/local/lib/php/extensions/no-debug-zts-*/pdo_pgsql.so
# inside the ghcr.io/oxphp/oxphp image you pull. Rerun after any PHP
# or Alpine version bump and reconcile.
#
# Transitive curl/xml deps (xz-libs, c-ares, nghttp2, nghttp3, idn2,
# psl, zstd, brotli-libs, libunistring) are pulled automatically by
# `apk add libcurl libxml2`, no need to list them explicitly.
#
# Core oxphp runtime:    libgcc (Rust binary)
# PHP core runtime:      readline, gnu-libiconv, libxml2, libcurl,
#                        openssl (libssl/libcrypto), sqlite-libs,
#                        oniguruma, argon2-libs, zlib, ncurses-libs
# App extensions:        icu-libs + libstdc++ (intl), libpq (pdo_pgsql)
# Base-image extensions: libsodium (sodium — enabled by default in the
#                        PHP base image via docker-php-ext-sodium.ini)
# Runtime niceties:      ca-certificates, tzdata
# pdo_mysql:             not listed — uses mysqlnd (bundled in libphp.so)
RUN apk add --no-cache \
        ca-certificates \
        tzdata \
        libgcc \
        libstdc++ \
        readline \
        ncurses-libs \
        gnu-libiconv \
        libxml2 \
        libcurl \
        openssl \
        oniguruma \
        sqlite-libs \
        argon2-libs \
        zlib \
        libsodium \
        icu-libs \
        libpq

# Create www-data user (uid 82) to match the OxPHP base image.
# The www-data group (gid 82) already exists in alpine by default.
RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data

# Copy everything oxphp needs at runtime from the OxPHP base image.
# Globs on libphp.so pick up any versioned symlinks (e.g. libphp.so.8).
COPY --from=oxphp /usr/local/bin/oxphp              /usr/local/bin/oxphp
COPY --from=oxphp /usr/local/lib/libphp*.so*        /usr/local/lib/
COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=oxphp /usr/local/lib/php/               /usr/local/lib/php/
COPY --from=oxphp /usr/local/etc/php/               /usr/local/etc/php/

# App PHP extensions (pdo_mysql, pdo_pgsql, intl) — prod-extensions
# preserves the extension_dir path, so this COPY lands them in the
# same directory oxphp_sapi.so already lives in.
COPY --from=prod-extensions /ext-out/ /

RUN { \
        echo "extension=pdo_mysql.so"; \
        echo "extension=pdo_pgsql.so"; \
        echo "extension=intl.so"; \
    } > /usr/local/etc/php/conf.d/app-extensions.ini

# Production opcache. OPcache is statically compiled into the PHP ZTS base
# (no zend_extension line needed), so this only tunes it. The image is
# immutable, so timestamp validation is disabled — no per-request stat(2) of
# every script. Bump the buffers for a typical framework codebase. JIT is
# left off (workload-dependent); enable it deliberately if you benchmark a win.
RUN { \
        echo "[opcache]"; \
        echo "opcache.enable=1"; \
        echo "opcache.validate_timestamps=0"; \
        echo "opcache.interned_strings_buffer=16"; \
        echo "opcache.max_accelerated_files=20000"; \
        echo "opcache.memory_consumption=192"; \
    } > /usr/local/etc/php/conf.d/opcache-prod.ini

# Web root
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

ENV LD_LIBRARY_PATH=/usr/local/lib

# Your app code
COPY --chown=www-data:www-data . /var/www/html

USER www-data

EXPOSE 80 443

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -qO- http://127.0.0.1/ >/dev/null || exit 1

CMD ["oxphp"]
