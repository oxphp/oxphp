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

# pdo_mysql and mysqli both compile against the bundled mysqlnd, so neither needs
# a client library. Both are installed because they reach mysqlnd by different
# routes and the tests need both: pdo_mysql through PDO's methods, mysqli through
# the procedural functions WordPress calls. phpredis asks four yes/no questions
# about optional serializers (igbinary, lzf, zstd, msgpack); answering "no" to all
# keeps the build to the default protocol handling this profile exercises.
# $PHPIZE_DEPS comes from the PHP base image.
RUN set -eux; \
    apk add --no-cache --virtual .hooks-db-build $PHPIZE_DEPS; \
    docker-php-ext-install pdo_mysql mysqli; \
    printf 'no\nno\nno\nno\n' | pecl install redis; \
    docker-php-ext-enable redis; \
    pecl clear-cache; \
    apk del .hooks-db-build; \
    php -m | grep -E '^(pdo_mysql|mysqli|redis)$'

# The bound on waiting for a connection another fiber holds is the smaller of
# max_execution_time and default_socket_timeout. This image carries no php.ini and
# sets neither, so both stand at the engine's defaults for a server SAPI — measured
# in this image, max_execution_time=30 and default_socket_timeout=60 — and two
# seconds here is what keeps the test that reaches the give-up branch to a few
# seconds instead of half a minute. Anyone raising it past 30 should know the other
# one becomes the bound. Shipped as a startup value rather than set from a test,
# because the server reads the value the process started with: a request that lowers
# it for an fsockopen() of its own must not shorten the bound for everyone else.
RUN printf 'default_socket_timeout=2\n' > /usr/local/etc/php/conf.d/zz-hooksdb.ini
