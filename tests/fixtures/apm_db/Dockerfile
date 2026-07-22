# Test image: OxPHP dev image + the DB client extensions the base image does not
# ship (it carries only pdo_sqlite). Used by tests/compose.apm-db.yml to exercise
# the APM database auto-instrumentation against real MySQL / PostgreSQL servers.
#
# Requires the dev image to be built first:
#   docker build -f docker/dev/Dockerfile -t oxphp-oxphp:latest .
FROM oxphp-oxphp:latest

USER root

# pdo_mysql + mysqli compile against the bundled mysqlnd (no external client
# lib). pdo_pgsql needs libpq at runtime and postgresql-dev to build. $PHPIZE_DEPS
# (autoconf/gcc/make/…) comes from the official PHP base image.
RUN set -eux; \
    apk add --no-cache libpq; \
    apk add --no-cache --virtual .apm-db-build $PHPIZE_DEPS postgresql-dev; \
    docker-php-ext-install pdo_mysql mysqli pdo_pgsql; \
    apk del .apm-db-build; \
    php -m | grep -E '^(pdo_mysql|mysqli|pdo_pgsql|pdo_sqlite)$'
