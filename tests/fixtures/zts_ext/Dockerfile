# Test image: OxPHP dev image + parallel, the one live extension that requires a
# ZTS build of PHP. It is not installed in the dev image because nothing else
# needs it and it costs a compile; this profile is the only consumer.
#
# pthreads is deliberately absent. PECL serves it as "Deprecated! use
# channel:///parallel" and its last release predates PHP 8, so there is nothing
# to install and nothing to assert about it.
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

# parallel refuses to build against a non-ZTS PHP, so the grep at the end is a
# build-time assert that the base image is still the ZTS one: a silent switch to
# an NTS base would otherwise surface as every test in this profile failing for
# reasons that look unrelated. $PHPIZE_DEPS comes from the PHP base image.
RUN set -eux; \
    apk add --no-cache --virtual .zts-ext-build $PHPIZE_DEPS; \
    printf '\n' | pecl install parallel; \
    docker-php-ext-enable parallel; \
    pecl clear-cache; \
    apk del .zts-ext-build; \
    php -m | grep -E '^parallel$'; \
    php -r 'exit(PHP_ZTS ? 0 : 1);'

USER www-data
