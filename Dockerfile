# ══════════════════════════════════════════════════════════════
# Build arguments (overridable from compose.yml or CI)
# ══════════════════════════════════════════════════════════════
ARG PHP_VERSION=8.5
ARG ALPINE_VERSION=3.23
ARG BASE_IMAGE=php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION}

# ══════════════════════════════════════════════════════════════
# Stage 1: Build bridge library (needs PHP headers for zval accessors)
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS bridge-builder

RUN apk add --no-cache gcc musl-dev make

WORKDIR /build
COPY ext/bridge/ ./

RUN make && make install

# ══════════════════════════════════════════════════════════════
# Stage 2: Build PHP extension (needs phpize + bridge headers)
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS ext-builder

RUN apk add --no-cache gcc musl-dev make autoconf

# Install bridge library (needed for linking)
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/

WORKDIR /build/ext
COPY ext/config.m4 ext/php_oxphp_sapi.h ext/oxphp_sapi.c ext/oxphp_fiber.h ext/oxphp_fiber.c ./
COPY ext/bridge/oxphp_bridge.h ./bridge/

RUN phpize && \
    ./configure --enable-oxphp-sapi && \
    make && \
    make install

# ══════════════════════════════════════════════════════════════
# Stage 3: Build Rust binary in PHP ZTS image
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS builder

# Install Rust + build dependencies
RUN apk add --no-cache \
    rust \
    cargo \
    gcc \
    musl-dev \
    pkgconfig \
    readline-dev \
    ncurses-dev \
    curl-dev \
    oniguruma-dev \
    sqlite-dev \
    argon2-dev \
    libxml2-dev \
    zlib-dev \
    openssl-dev \
    gnu-libiconv-dev

# Install bridge library (needed for Rust linking)
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/

WORKDIR /build

# Cargo feature set. Default includes all production plugins. Override
# via compose build-args or --build-arg on the command line.
ARG CARGO_FEATURES="plugin-apm,plugin-async"

# Version injected from CI when building a tagged release. Empty default
# leaves Cargo.toml version untouched.
ARG OXPHP_VERSION=""

# Copy dependency files first (layer caching)
COPY Cargo.toml Cargo.lock ./

# Create dummy source to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    touch src/lib.rs && \
    if [ -n "${CARGO_FEATURES}" ]; then \
        cargo build --release --features "${CARGO_FEATURES}"; \
    else \
        cargo build --release; \
    fi && \
    rm -rf src target/release/oxphp target/release/deps/oxphp-* target/release/.fingerprint/oxphp-*

# Copy real source and build script
COPY src ./src
COPY build.rs ./

# Patch version from build arg (only when OXPHP_VERSION is explicitly set)
RUN if [ -n "${OXPHP_VERSION}" ]; then \
        sed -i "s/^version = \".*\"/version = \"${OXPHP_VERSION}\"/" Cargo.toml; \
    fi

# Build release binary
RUN if [ -n "${CARGO_FEATURES}" ]; then \
        cargo build --release --features "${CARGO_FEATURES}"; \
    else \
        cargo build --release; \
    fi

# ══════════════════════════════════════════════════════════════
# Stage 4: Runtime — PHP ZTS base provides php CLI, phpize,
# docker-php-ext-install, libphp.so, extensions dir, www-data user,
# and all PHP runtime dependencies (libxml2, libcurl, etc.).
# libgcc must be added explicitly: the Rust-compiled oxphp binary
# dynamically links libgcc_s.so.1, which the PHP base image does not
# ship (it's a gcc runtime dep, not a PHP dep).
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE}

RUN apk add --no-cache libgcc

# Copy bridge library from bridge-builder
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/

# Copy oxphp SAPI extension from ext-builder (merges into the existing
# extensions directory provided by the PHP base image).
COPY --from=ext-builder /usr/local/lib/php/extensions/ /usr/local/lib/php/extensions/

# PHP configuration — dev image ships oxphp.ini (OPcache + JIT + preloading)
COPY oxphp.ini /usr/local/etc/php/conf.d/oxphp.ini
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/extension.ini

# Copy binary from builder
COPY --from=builder /build/target/release/oxphp /usr/local/bin/oxphp

# Create web root (www-data already exists in the PHP base image)
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

# Dev image ships the full www/ including preload.php, worker.php, app/,
# fixtures/, and public/ with index.php + assets.
COPY --chown=www-data:www-data www/ /var/www/html/

# Ensure libphp.so and liboxphp_bridge.so are found at runtime
ENV LD_LIBRARY_PATH=/usr/local/lib

USER www-data

EXPOSE 80 443

CMD ["oxphp"]
