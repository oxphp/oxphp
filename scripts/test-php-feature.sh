#!/bin/sh
# scripts/test-php-feature.sh
#
# Run the Rust tests that only build with the `php` feature. They link
# libphp.so and liboxphp_bridge.so, so they cannot run on a plain host:
# every host test run passes --no-default-features, which compiles
# src/php/sapi.rs, executor::sapi and the other php-gated code out.
#
# Runs inside a php:*-zts-alpine image (CI uses it as the job container):
#
#   docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/tmp/target \
#       php:8.4-zts-alpine3.23 scripts/test-php-feature.sh
#
# Set CARGO_TARGET_DIR when running against a checkout that a host build
# also uses, so the Linux artifacts do not land in the host's target/.

set -eu

if ! command -v apk >/dev/null 2>&1 || ! command -v php-config >/dev/null 2>&1; then
    echo "error: run this inside a php:*-zts-alpine image (needs apk and php-config)" >&2
    exit 1
fi

# Same toolchain and libphp.so link dependencies as the builder stage of
# docker/dev/Dockerfile, plus make for the bridge.
apk add --no-cache \
    rust cargo gcc musl-dev make pkgconfig \
    readline-dev ncurses-dev curl-dev oniguruma-dev sqlite-dev argon2-dev \
    libxml2-dev zlib-dev openssl-dev gnu-libiconv-dev protobuf-dev

# The bridge is built outside the checkout so no object files land in it.
# `clean` first: a local make in ext/bridge leaves its objects there
# (untracked), and make would install that build — possibly from another
# platform or without the profiler — as up to date.
# OXPHP_WITH_PROFILER matches the plugin-profiler feature, which `php`
# always enables.
bridge_dir=$(mktemp -d)
cp -R ext/bridge/. "$bridge_dir"
make -C "$bridge_dir" EXTRA_CFLAGS=-DOXPHP_WITH_PROFILER=1 clean install

export LD_LIBRARY_PATH=/usr/local/lib

# Build first, so a compile error fails here with its own output rather than
# surfacing below as an empty test list.
cargo test --locked --no-run

# A filter over tests that were never built reports "0 passed" and exits 0,
# which is how these tests went unrun in the first place. Fail instead if
# the feature did not bring them in.
sapi_tests=$(cargo test --locked --lib -- --list 2>/dev/null | grep -c '^php::sapi::.*: test$' || true)
if [ "$sapi_tests" -eq 0 ]; then
    echo "error: no php::sapi tests in the build — the php feature is not active" >&2
    exit 1
fi
echo "php::sapi tests in the build: $sapi_tests"

cargo test --locked
