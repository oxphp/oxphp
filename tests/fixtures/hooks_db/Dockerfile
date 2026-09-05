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
# max_execution_time and default_socket_timeout. Left alone both stand at the
# engine's defaults for a server SAPI — measured in this image, 30 and 60 — which
# would put that bound at half a minute, so default_socket_timeout is set to two
# seconds and that is what keeps the test reaching the give-up branch to a few
# seconds. Shipped as a startup value rather than set from a test, because the
# server reads the value the process started with: a request that lowers it for an
# fsockopen() of its own must not shorten the bound for everyone else.
#
# max_execution_time is set to 0 for a different reason, and it leaves the bound
# where it was: the bound is the smaller of the two limits that impose one, and
# with max_execution_time imposing none it is default_socket_timeout's two seconds
# either way. It does cost something, and not the bound: with 30 and 2, a test
# whose timing assertions read the bound was separating the smaller of the two
# limits from the larger, since taking the larger would have waited half a minute
# and failed them. With 0 and 2 both answers are two seconds, so a regression that
# started taking the larger would pass unseen. The two tests that read the bound
# that way — test_shared_redis_wait_gives_up and
# test_shared_mysqli_close_under_parked_reader — call set_time_limit(30)
# themselves for that reason, which puts the two limits back at 30 and 2 for the
# request making the assertion and leaves this startup value alone for everyone
# else.
#
# What 0 removes is a clock this profile's results would otherwise be read
# against. An async worker thread arms its execution timer once, in the
# php_request_startup() it does when the thread starts — the pool runs one
# long-lived request for the lifetime of the thread — and never re-arms or
# disarms it: the reset between tasks zeroes the limit the engine compares
# against but leaves the timer running. So roughly max_execution_time seconds
# into the process, whichever task is running then is killed, reported as
# "Maximum execution time of 0 seconds exceeded" because the limit the message
# names has since been zeroed. All but three of the tests here dispatch async
# tasks, so which one is holding a task at that moment decides which one turns
# red, and adding a test anywhere earlier in the suite moves the target. At 0 the
# timer is never armed.
RUN printf 'default_socket_timeout=2\nmax_execution_time=0\n' > /usr/local/etc/php/conf.d/zz-hooksdb.ini
