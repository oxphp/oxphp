# Test image: OxPHP dev image + intl. intl's NumberFormatter::parse() calls
# libc's setlocale() itself, on the process locale, which no other extension in
# the dev image does; the locale_intl profile is the only consumer.
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

# icu-libs stays installed for the extension to load; icu-dev and $PHPIZE_DEPS
# (from the PHP base image) are only needed to compile it.
RUN set -eux; \
    apk add --no-cache icu-libs; \
    apk add --no-cache --virtual .locale-intl-build $PHPIZE_DEPS icu-dev; \
    docker-php-ext-install intl; \
    apk del .locale-intl-build; \
    php -m | grep -E '^intl$'

USER www-data
