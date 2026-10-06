# ══════════════════════════════════════════════════════════════
# Debian (glibc) variant of the production image.
#
# Same four stages, same feature set and same runtime layout as
# docker/release/alpine/Dockerfile; the distribution and its C library
# differ. On musl, PHP's own build leaves the thread-local `_tsrm_ls_cache`
# on the default TLS model (TSRM/TSRM.h opts musl out of `initial-exec`), so
# EG()/CG() in libphp.so reach it through a call into the TLS runtime; on
# glibc it is `initial-exec`, a load relative to the thread pointer. musl's
# malloc, which libraries such as libpq use for every query result, costs
# more as well. Together this makes the Alpine image spend more CPU per
# request than this one.
#
# Differences a downstream Dockerfile sees against the Alpine image:
#   - packages come from apt, not apk;
#   - `www-data` is uid/gid 33 (Debian), not 82 (Alpine);
#   - there is no `wget` (BusyBox provides it on Alpine); `curl` is there;
#   - the base image is several times larger.
# ══════════════════════════════════════════════════════════════
ARG PHP_VERSION=8.4
ARG DEBIAN_VERSION=trixie
ARG BASE_IMAGE=php:${PHP_VERSION}-zts-${DEBIAN_VERSION}

# Toggles -DOXPHP_WITH_PROFILER=1 in both C build stages, as in the Alpine
# release Dockerfile.
ARG OXPHP_WITH_PROFILER=1

# ══════════════════════════════════════════════════════════════
# Stage 1: Build bridge library
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS bridge-builder

ARG OXPHP_WITH_PROFILER

RUN apt-get update && \
    apt-get install -y --no-install-recommends gcc libc6-dev make && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY ext/bridge/ ./

RUN export EXTRA_CFLAGS="$([ "${OXPHP_WITH_PROFILER}" = "1" ] && echo '-DOXPHP_WITH_PROFILER=1')" && \
    make && make install

# ══════════════════════════════════════════════════════════════
# Stage 2: Build PHP extension (oxphp_sapi.so)
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS ext-builder

ARG OXPHP_WITH_PROFILER

RUN apt-get update && \
    apt-get install -y --no-install-recommends gcc libc6-dev make autoconf && \
    rm -rf /var/lib/apt/lists/*

COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/

WORKDIR /build/ext
COPY ext/config.m4 ext/php_oxphp_sapi.h ext/oxphp_sapi.c ext/oxphp_fiber.h ext/oxphp_fiber.c ./
COPY ext/bridge/oxphp_bridge.h ./bridge/

RUN phpize && \
    CPPFLAGS="$([ "${OXPHP_WITH_PROFILER}" = "1" ] && echo '-DOXPHP_WITH_PROFILER=1')" \
        ./configure --enable-oxphp-sapi && \
    make && \
    make install

# ══════════════════════════════════════════════════════════════
# Stage 3: Build Rust binary with the fixed production feature set
# (the same one as the Alpine release Dockerfile).
#
# Debian's own rustc is older than the crate's MSRV, so the compiler comes
# from rustup (Debian package), pinned to the channel in rust-toolchain.toml
# — the same version the Alpine image is built with, read from the one file
# that names it.
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        rustup \
        ca-certificates \
        gcc \
        libc6-dev \
        pkg-config \
        libreadline-dev \
        libncurses-dev \
        libcurl4-openssl-dev \
        libonig-dev \
        libsqlite3-dev \
        libargon2-dev \
        libxml2-dev \
        zlib1g-dev \
        libssl-dev \
        protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' /tmp/rust-toolchain.toml) && \
    [ -n "$channel" ] && \
    rustup toolchain install "$channel" --profile minimal && \
    rustup default "$channel"

COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/

WORKDIR /build

# Version injected from CI, as in the Alpine release Dockerfile.
ARG OXPHP_VERSION="0.0.0-dev"

COPY Cargo.toml Cargo.lock ./
# build.rs runs prost_build::compile_protos unconditionally, so proto/
# must be present even for the dep-cache build below.
COPY proto ./proto
COPY build.rs ./

# Cache dependency compilation with production feature set baked in.
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    touch src/lib.rs && \
    cargo build --release --features plugin-apm,plugin-async,plugin-profiler,plugin-shared && \
    rm -rf src target/release/oxphp target/release/deps/oxphp-* target/release/.fingerprint/oxphp-*

COPY src ./src

# Patch Cargo.toml version from the build arg (always applied in prod builds)
RUN sed -i "s/^version = \".*\"/version = \"${OXPHP_VERSION}\"/" Cargo.toml

# Production build — hardcoded feature set, no ARG override.
RUN cargo build --release --features plugin-apm,plugin-async,plugin-profiler,plugin-shared

# ══════════════════════════════════════════════════════════════
# Stage 4: Runtime. The PHP base image already carries libgcc_s and the
# shared libraries libphp.so links; ca-certificates is pinned for the
# OTLP exporter's TLS verification, as in the Alpine image.
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE}

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/

# Merges into the extensions directory provided by the PHP base image.
COPY --from=ext-builder /usr/local/lib/php/extensions/ /usr/local/lib/php/extensions/

# Production intentionally does NOT ship a custom.ini.
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/extension.ini

# Build-time assert that the bare `php` CLI can load the extension — see the
# Alpine release Dockerfile for why this is a build failure, not a smoke-test one.
ENV LD_LIBRARY_PATH=/usr/local/lib
RUN out=$(php -n -d extension=oxphp_sapi.so -m 2>&1) && \
    echo "$out" | grep -qx oxphp_sapi || { \
        echo "FATAL: oxphp_sapi failed to load. php -m output:"; \
        echo "$out"; \
        exit 1; \
    }

COPY --from=builder /build/target/release/oxphp /usr/local/bin/oxphp

# Every shared library the binary links must resolve in this stage; the
# builder had -dev packages this stage does not.
RUN missing=$(ldd /usr/local/bin/oxphp | grep 'not found' || true) && \
    [ -z "$missing" ] || { echo "FATAL: unresolved libraries:"; echo "$missing"; exit 1; }

# Web root (www-data, uid 33, already exists in the PHP base image)
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

COPY --chown=www-data:www-data www/public/welcome.php /var/www/html/public/index.php

COPY LICENSE THIRD_PARTY_LICENSES.html /usr/share/licenses/oxphp/

LABEL org.opencontainers.image.title="OxPHP" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/oxphp/oxphp" \
      org.opencontainers.image.documentation="https://github.com/oxphp/oxphp#readme"

EXPOSE 80 443

CMD ["oxphp"]
