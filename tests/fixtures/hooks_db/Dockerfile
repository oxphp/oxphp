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

# mysqli and redis are enabled from files that sort after the dev image's
# extension.ini, the way a hand-written mysqli.ini or redis.ini does, while
# pdo_mysql keeps the docker-php-ext-* name that sorts before it. Extensions load
# in the order their ini files are read, and under RUNTIME_HOOKS=streams the
# methods among the database entry points are guarded from oxphp_sapi's own module
# startup, on the classes that exist by then — so this is the layout where a
# client's class would appear after oxphp_sapi started and never be guarded,
# unless oxphp_sapi is started after it regardless. Every mysqli and redis test in
# this profile runs in that layout, and test_late_loaded_clients_start_before_oxphp
# checks that the layout is still this one.
RUN set -eux; \
    cd /usr/local/etc/php/conf.d; \
    mv docker-php-ext-mysqli.ini mysqli.ini; \
    mv docker-php-ext-redis.ini redis.ini; \
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
# max_execution_time is deliberately left at the engine's default here. The two
# tests that read the bound — test_shared_redis_wait_gives_up and
# test_shared_mysqli_close_under_parked_reader — are asserting that the smaller
# of the two limits is the one taken, so the two have to stay apart: were both
# two seconds, a regression that started taking the larger would read the same
# and pass unseen. Those two call set_time_limit(30) themselves as well, so the
# separation holds for them whatever this file says.
RUN printf 'default_socket_timeout=2\n' > /usr/local/etc/php/conf.d/zz-hooksdb.ini

# A DSN alias, which PDO resolves from php.ini alone (pdo.dsn.<name> is read from
# the startup configuration, not from ini_set), naming the profile's MySQL server
# exactly as the tests spell it out in full. Only
# test_persistent_ctor_gate_spellings names it: two constructors reaching one
# data source through different spellings must still wait for each other.
RUN printf 'pdo.dsn.hooksdb="mysql:host=hooksdb-mysql;port=3306;dbname=appdb"\n' > /usr/local/etc/php/conf.d/zz-hooksdb-dsn.ini
