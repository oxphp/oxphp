#!/bin/sh
# scripts/php-base-image.sh
#
# Print the official PHP ZTS image OxPHP builds on for a PHP minor and a base
# tag. Up to 8.5 the ZTS variant carries a `-zts` infix (php:8.5-zts-trixie);
# from 8.6 every official image is ZTS and the infix is gone (php:8.6-trixie),
# while php:8.5-trixie is an NTS image libphp cannot be linked from.
#
# Usage:
#   scripts/php-base-image.sh <php_minor> <base_tag>
#
# Example:
#   scripts/php-base-image.sh 8.5 alpine3.24   # php:8.5-zts-alpine3.24
#   scripts/php-base-image.sh 8.6 trixie       # php:8.6-trixie

set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 <php_minor> <base_tag>" >&2
    exit 2
fi

php_minor=$1
base_tag=$2

case "$php_minor" in
    8.4|8.5) echo "php:${php_minor}-zts-${base_tag}" ;;
    8.[6-9]|9.*) echo "php:${php_minor}-${base_tag}" ;;
    *)
        echo "error: unsupported PHP minor: $php_minor" >&2
        exit 1
        ;;
esac
