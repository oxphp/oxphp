# ══════════════════════════════════════════════════════════════
# Build arguments (overridable from compose.yml or CI)
# ══════════════════════════════════════════════════════════════
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23
ARG BASE_IMAGE=php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION}

# Toggles -DOXPHP_WITH_PROFILER=1 in both C build stages. Must match the
# Cargo `plugin-profiler` feature (on by default in Cargo.toml). Set to 0
# when building with --no-default-features or a feature set that excludes
# plugin-profiler — the observer registration in ext/oxphp_sapi.c then
# compiles out and the PHP-side plugin runtime has nothing to hook into.
ARG OXPHP_WITH_PROFILER=1

# ══════════════════════════════════════════════════════════════
# Stage 1: Build bridge library (needs PHP headers for zval accessors)
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS bridge-builder

ARG OXPHP_WITH_PROFILER

RUN apk add --no-cache gcc musl-dev make

WORKDIR /build
COPY ext/bridge/ ./

# Append -DOXPHP_WITH_PROFILER=1 via EXTRA_CFLAGS (see ext/bridge/Makefile)
# so the Makefile's CFLAGS defaults (-O2, -fPIC, -D_GNU_SOURCE, PHP includes)
# are preserved. Exported so both `make` and `make install` see the same
# value (install re-evaluates the all-target's dep graph).
RUN export EXTRA_CFLAGS="$([ "${OXPHP_WITH_PROFILER}" = "1" ] && echo '-DOXPHP_WITH_PROFILER=1')" && \
    make && make install

# ══════════════════════════════════════════════════════════════
# Stage 2: Build PHP extension (needs phpize + bridge headers)
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE} AS ext-builder

ARG OXPHP_WITH_PROFILER

RUN apk add --no-cache gcc musl-dev make autoconf

# Install bridge library (needed for linking)
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/

WORKDIR /build/ext
COPY ext/config.m4 ext/php_oxphp_sapi.h ext/oxphp_sapi.c ext/oxphp_fiber.h ext/oxphp_fiber.c ./
COPY ext/bridge/oxphp_bridge.h ./bridge/

# Propagate -DOXPHP_WITH_PROFILER=1 into phpize's generated Makefile via
# CPPFLAGS (appended by autoconf to every compile line). Using CPPFLAGS
# instead of CFLAGS avoids clobbering the phpize-emitted optimisation and
# PHP ABI flags that land in CFLAGS itself.
RUN phpize && \
    CPPFLAGS="$([ "${OXPHP_WITH_PROFILER}" = "1" ] && echo '-DOXPHP_WITH_PROFILER=1')" \
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
    gnu-libiconv-dev \
    protobuf-dev

# Install bridge library (needed for Rust linking)
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/

WORKDIR /build

# Cargo feature set. Default includes all production plugins. Override
# via compose build-args or --build-arg on the command line.
ARG CARGO_FEATURES="plugin-apm,plugin-async,plugin-shared"

# Version injected from CI when building a tagged release. Empty default
# leaves Cargo.toml version untouched.
ARG OXPHP_VERSION=""

# Copy dependency files first (layer caching)
COPY Cargo.toml Cargo.lock ./

# Create dummy source to cache dependencies
RUN mkdir src benches && \
    echo "fn main() {}" > src/main.rs && \
    touch src/lib.rs && \
    echo "fn main() {}" > benches/synthetic_promise.rs && \
    if [ -n "${CARGO_FEATURES}" ]; then \
        cargo build --release --features "${CARGO_FEATURES}"; \
    else \
        cargo build --release; \
    fi && \
    rm -rf src benches target/release/oxphp target/release/deps/oxphp-* target/release/.fingerprint/oxphp-*

# Copy real source and build script
COPY src ./src
COPY benches ./benches
COPY build.rs ./
COPY proto ./proto

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
# ca-certificates: OxPHP's OTLP exporter verifies the collector's TLS
# certificate against the system trust store, so an https:// endpoint needs a
# CA bundle. The php:*-alpine base already ships it, but pinning it here keeps
# the dependency explicit and survives other bases (e.g. distroless).
# ══════════════════════════════════════════════════════════════
FROM ${BASE_IMAGE}

RUN apk add --no-cache libgcc ca-certificates

# Copy bridge library from bridge-builder
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/

# Copy oxphp SAPI extension from ext-builder (merges into the existing
# extensions directory provided by the PHP base image).
COPY --from=ext-builder /usr/local/lib/php/extensions/ /usr/local/lib/php/extensions/

# PHP configuration — dev image ships custom.ini (OPcache + JIT + preloading)
COPY docker/custom.ini /usr/local/etc/php/conf.d/custom.ini
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/extension.ini

# Build-time assert: bare `php` CLI must dlopen the extension successfully.
# Same rationale as Dockerfile.alpine-release — turns a stale-cache or
# upstream-PHP-bump silent breakage into a build failure with full context.
# `-n` skips all ini scanning (custom.ini's preload.php is not yet copied at
# this point of the build) and `-d extension=` loads only the extension we
# want to verify. We deliberately do NOT add `-d display_startup_errors=1`
# because dlopen errors are emitted unconditionally to stderr; surfacing
# them via `2>&1` is enough.
ENV LD_LIBRARY_PATH=/usr/local/lib
RUN out=$(php -n -d extension=oxphp_sapi.so -m 2>&1) && \
    echo "$out" | grep -qx oxphp_sapi || { \
        echo "FATAL: oxphp_sapi failed to load. php -m output:"; \
        echo "$out"; \
        exit 1; \
    }

# Copy binary from builder
COPY --from=builder /build/target/release/oxphp /usr/local/bin/oxphp

# Create web root (www-data already exists in the PHP base image)
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

# Dev image ships the full www/ including preload.php, worker.php, app/,
# fixtures/, and public/ with index.php + assets.
COPY --chown=www-data:www-data www/ /var/www/html/

# LD_LIBRARY_PATH is set above before the build-time extension load assert
# so that check finds liboxphp_bridge.so; persist it for runtime so `oxphp`
# and any downstream `php` invocations resolve it.

USER www-data

EXPOSE 80 443

CMD ["oxphp"]
