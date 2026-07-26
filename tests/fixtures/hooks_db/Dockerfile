# Test image: OxPHP dev image + the two client extensions this profile needs.
# pdo_mysql speaks the MySQL protocol through the bundled mysqlnd; phpredis is
# an independently maintained PECL extension. Both read their server's replies
# with php_stream_read() on a tcp:// stream, which is the path the socket hooks
# replace — that is what makes them worth testing together.
#
# Requires the dev image to be built first:
#   docker build -f docker/dev/Dockerfile -t oxphp-oxphp:latest .
#
# BASE_IMAGE lets a run point at a privately tagged dev image instead, which
# keeps a measurement from silently picking up someone else's rebuild of the
# shared tag partway through.
ARG BASE_IMAGE=oxphp-oxphp:latest
FROM ${BASE_IMAGE}

USER root

# pdo_mysql compiles against the bundled mysqlnd, so it needs no client library.
# phpredis asks four yes/no questions about optional serializers (igbinary, lzf,
# zstd, msgpack); answering "no" to all keeps the build to the default protocol
# handling this profile exercises. $PHPIZE_DEPS comes from the PHP base image.
RUN set -eux; \
    apk add --no-cache --virtual .hooks-db-build $PHPIZE_DEPS; \
    docker-php-ext-install pdo_mysql; \
    printf 'no\nno\nno\nno\n' | pecl install redis; \
    docker-php-ext-enable redis; \
    pecl clear-cache; \
    apk del .hooks-db-build; \
    php -m | grep -E '^(pdo_mysql|redis)$'
