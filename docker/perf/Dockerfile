# ══════════════════════════════════════════════════════════════
# Profiling image — same build as docker/dev/Dockerfile but with
# debug symbols + frame pointers retained, perf + FlameGraph tooling
# installed, and NO `USER www-data` (perf and apk need root).
#
# NOT a production artifact. Used to capture on-CPU flamegraphs of the
# Channel recv waker path (shared-issue 19/21). Run privileged:
#
#   docker build -f docker/perf/Dockerfile -t oxphp-perf .
#   docker run --rm -it --privileged \
#     -e WORKER_MODE_ENABLED=true -e ENTRY_FILE=/var/www/html/bench_worker.php \
#     -e PHP_WORKERS=2 -e ASYNC_WORKERS=4 \
#     -v "$PWD/www:/var/www/html:ro" -p 8099:80 -p 9099:9090 \
#     oxphp-perf
# ══════════════════════════════════════════════════════════════
ARG PHP_VERSION=8.4
ARG ALPINE_VERSION=3.23
ARG BASE_IMAGE=php:${PHP_VERSION}-zts-alpine${ALPINE_VERSION}
ARG OXPHP_WITH_PROFILER=1

# ── Stage 1: bridge library (with -g for symbols) ──────────────
FROM ${BASE_IMAGE} AS bridge-builder
ARG OXPHP_WITH_PROFILER
RUN apk add --no-cache gcc musl-dev make
WORKDIR /build
COPY ext/bridge/ ./
RUN export EXTRA_CFLAGS="-g $([ "${OXPHP_WITH_PROFILER}" = "1" ] && echo '-DOXPHP_WITH_PROFILER=1')" && \
    make && make install

# ── Stage 2: PHP extension (with -g) ───────────────────────────
FROM ${BASE_IMAGE} AS ext-builder
ARG OXPHP_WITH_PROFILER
RUN apk add --no-cache gcc musl-dev make autoconf
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/
WORKDIR /build/ext
COPY ext/config.m4 ext/php_oxphp_sapi.h ext/oxphp_sapi.c ext/oxphp_fiber.h ext/oxphp_fiber.c ./
COPY ext/bridge/oxphp_bridge.h ./bridge/
RUN phpize && \
    CPPFLAGS="-g $([ "${OXPHP_WITH_PROFILER}" = "1" ] && echo '-DOXPHP_WITH_PROFILER=1')" \
        ./configure --enable-oxphp-sapi && \
    make && make install

# ── Stage 3: Rust binary WITH debug symbols + frame pointers ───
FROM ${BASE_IMAGE} AS builder
RUN apk add --no-cache \
    rust cargo gcc musl-dev pkgconfig readline-dev ncurses-dev curl-dev \
    oniguruma-dev sqlite-dev argon2-dev libxml2-dev zlib-dev openssl-dev \
    gnu-libiconv-dev protobuf-dev
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/
WORKDIR /build
ARG CARGO_FEATURES="plugin-apm,plugin-async,plugin-shared"

# Keep symbols + line info + frame pointers in the release build so perf
# resolves Rust frames and can unwind cheaply.
ENV CARGO_PROFILE_RELEASE_STRIP=false
ENV CARGO_PROFILE_RELEASE_DEBUG=2
ENV RUSTFLAGS="-C force-frame-pointers=yes"

COPY Cargo.toml Cargo.lock ./
RUN mkdir src benches && \
    echo "fn main() {}" > src/main.rs && touch src/lib.rs && \
    echo "fn main() {}" > benches/synthetic_promise.rs && \
    cargo build --release --features "${CARGO_FEATURES}" && \
    rm -rf src benches target/release/oxphp target/release/deps/oxphp-* target/release/.fingerprint/oxphp-*
COPY src ./src
COPY benches ./benches
COPY build.rs ./
COPY proto ./proto
RUN cargo build --release --features "${CARGO_FEATURES}"

# ── Stage 4: runtime + perf + FlameGraph (root, no USER drop) ──
FROM ${BASE_IMAGE}
RUN apk add --no-cache libgcc perf perl bash
# Brendan Gregg's FlameGraph scripts (perf script → folded → svg).
ADD https://raw.githubusercontent.com/brendangregg/FlameGraph/master/stackcollapse-perf.pl /opt/fg/stackcollapse-perf.pl
ADD https://raw.githubusercontent.com/brendangregg/FlameGraph/master/flamegraph.pl /opt/fg/flamegraph.pl
RUN chmod +x /opt/fg/*.pl

COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ext-builder /usr/local/lib/php/extensions/ /usr/local/lib/php/extensions/
COPY docker/custom.ini /usr/local/etc/php/conf.d/custom.ini
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/extension.ini
ENV LD_LIBRARY_PATH=/usr/local/lib
COPY --from=builder /build/target/release/oxphp /usr/local/bin/oxphp
RUN mkdir -p /var/www/html/public
COPY www/ /var/www/html/

# NB: no `USER www-data` — perf_event_open + apk need root.
EXPOSE 80 443
CMD ["oxphp"]
