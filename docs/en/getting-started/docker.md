---
title: Docker
description: Dockerfile stages, docker-compose.yml reference, and deployment tips
---

OxPHP ships with a multi-stage Dockerfile that produces a minimal Alpine runtime image. This page explains each build stage, the `docker-compose.yml` configuration, and common deployment considerations.

## Dockerfile Stages

The Dockerfile has four stages. Each stage builds one component and passes artifacts forward.

### Stage 1: bridge-builder

```dockerfile
FROM alpine:3.21 AS bridge-builder
RUN apk add --no-cache gcc musl-dev make
COPY ext/bridge/ ./
RUN make && make install
```

Compiles `liboxphp_bridge.so`, a small C shared library that provides `__thread` TLS variables shared between Rust and the PHP extension. This is built on plain Alpine with just gcc -- it has no PHP dependency.

**Artifacts:** `/usr/local/lib/liboxphp_bridge.so`, `/usr/local/include/oxphp_bridge.h`

### Stage 2: ext-builder

```dockerfile
FROM php:8.4-zts-alpine AS ext-builder
RUN apk add --no-cache gcc musl-dev make autoconf
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/
COPY ext/config.m4 ext/php_oxphp_sapi.h ext/oxphp_sapi.c ./
COPY ext/bridge/oxphp_bridge.h ./bridge/
RUN phpize && ./configure --enable-oxphp-sapi && make && make install
```

Builds the PHP extension (`oxphp_sapi.so`) using `phpize` from the PHP 8.4 ZTS image. The extension links against the bridge library and exposes functions like `oxphp_request_id()` and `oxphp_server_info()` to PHP userland.

**Artifacts:** PHP extension `.so` file in `/usr/local/lib/php/extensions/`

### Stage 3: builder

```dockerfile
FROM php:8.4-zts-alpine AS builder
RUN apk add --no-cache rust cargo musl-dev pkgconfig ...
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY Cargo.toml Cargo.lock ./

ARG CARGO_FEATURES=""

RUN mkdir src && echo "fn main() {}" > src/main.rs && touch src/lib.rs && \
    cargo build --release && \
    rm -rf src target/release/oxphp target/release/deps/oxphp-* target/release/.fingerprint/oxphp-*
COPY src ./src
COPY build.rs ./
RUN if [ -n "${CARGO_FEATURES}" ]; then \
        cargo build --release --features "${CARGO_FEATURES}"; \
    else \
        cargo build --release; \
    fi
```

Builds the Rust binary inside the same `php:8.4-zts-alpine` image. This is required because the binary links against `libphp.so` and `liboxphp_bridge.so` -- building in a separate image with a different musl version causes TLS corruption at runtime.

The stage uses a dependency caching trick: it first builds with a dummy `main.rs` to cache all dependency crates, then removes only the OxPHP-specific artifacts (`target/release/oxphp`, `deps/oxphp-*`, `.fingerprint/oxphp-*`) before copying the real source. This way, only the final binary is rebuilt on source changes.

The `CARGO_FEATURES` build argument allows enabling optional Cargo features (such as `plugin-example`) at build time without modifying the Dockerfile.

**Artifacts:** `/build/target/release/oxphp`

### Stage 4: runtime

```dockerfile
FROM alpine:3.21
RUN apk add --no-cache libgcc libxml2 sqlite-libs libcurl oniguruma argon2-libs zlib ...
COPY --from=builder /usr/local/lib/libphp.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ext-builder /usr/local/lib/php/extensions/ /usr/local/lib/php/extensions/
COPY --from=builder /build/target/release/oxphp /usr/local/bin/oxphp
ENV LD_LIBRARY_PATH=/usr/local/lib
USER www-data
EXPOSE 8080
CMD ["oxphp"]
```

The final runtime image is based on `alpine:3.21`. It copies only what is needed:

- `libphp.so` -- the PHP runtime library
- `liboxphp_bridge.so` -- the C bridge library
- PHP extension files
- The `oxphp` binary
- PHP configuration (`oxphp.ini`, extension loading)
- Default web root contents (`/var/www/html/`)

The `www-data` user (UID 82, GID 82) runs the server process. Alpine 3.21 already has the `www-data` group pre-created, so the Dockerfile adds only the user.

`LD_LIBRARY_PATH=/usr/local/lib` is set so the dynamic linker can find `libphp.so` and `liboxphp_bridge.so` at runtime.

## docker-compose.yml Reference

```yaml
services:
  oxphp:
    build:
      context: .
      args:
        # Extra Cargo features (space-separated), e.g. "plugin-example"
        CARGO_FEATURES: ""
    ports:
      - "8080:8080"   # Main HTTP server
      - "9090:9090"   # Internal server (health/metrics/config)
    volumes:
      - ./www:/var/www/html:ro
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
      - ./certs:/etc/ssl/oxphp:ro
    environment:
      # Server
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html
      # - INDEX_FILE=index.php       # Enables Framework routing mode
      - EXECUTOR=sapi                # "sapi" or "stub"
      # - PHP_WORKERS=0              # Static: 0 = CPU*2, or fixed N
      # - PHP_WORKERS=2:16           # Dynamic: scale between 2 and 16
      # - PHP_WORKERS_IDLE_SEC=30    # Idle timeout for dynamic scale-down
      # - QUEUE_CAPACITY=512         # Default: PHP_WORKERS * 128

      # Logging
      - LOG_LEVEL=info

      # Internal server
      - INTERNAL_ADDR=0.0.0.0:9090

      # Timeouts (seconds)
      - HEADER_TIMEOUT_SECS=5
      - IDLE_TIMEOUT_SECS=60
      - REQUEST_TIMEOUT_SECS=120
      - DRAIN_TIMEOUT_SECS=30

      # Rate limiting (0 = disabled)
      # - RATE_LIMIT=100
      # - RATE_WINDOW=60

      # TLS
      # - TLS_CERT=/etc/ssl/oxphp/server.pem
      # - TLS_KEY=/etc/ssl/oxphp/server.key

      # Error pages
      # - ERROR_PAGES_DIR=/var/www/errors

      # Compression (default: true)
      # - COMPRESSION=true
    restart: unless-stopped
```

### Build Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `CARGO_FEATURES` | `""` | Space-separated list of additional Cargo features to enable (e.g. `plugin-example`) |

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Address and port for the main HTTP server |
| `DOCUMENT_ROOT` | `/var/www/html` | Root directory for serving files |
| `INDEX_FILE` | _(unset)_ | Set to `index.php` for Framework mode or `index.html` for SPA mode |
| `EXECUTOR` | `sapi` | PHP executor type: `sapi` (real PHP) or `stub` (placeholder) |
| `PHP_WORKERS` | `0` (CPU * 2, static) | Worker pool mode. `N` = fixed pool, `MIN:MAX` = dynamic scaling |
| `PHP_WORKERS_IDLE_SEC` | `30` | Idle timeout before a dynamic worker is retired |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Bounded request queue size. 503 returned when full |
| `LOG_LEVEL` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent connections |
| `INTERNAL_ADDR` | _(unset)_ | Address for internal server. Unset disables it |
| `HEADER_TIMEOUT_SECS` | `5` | Timeout for reading request headers |
| `IDLE_TIMEOUT_SECS` | `60` | Keep-alive idle timeout |
| `REQUEST_TIMEOUT_SECS` | `120` | Maximum request processing time. 0 disables the timeout |
| `DRAIN_TIMEOUT_SECS` | `30` | Grace period for in-flight connections during shutdown |
| `RATE_LIMIT` | `0` | Max requests per IP per window. 0 disables rate limiting |
| `RATE_WINDOW` | `60` | Rate limiting window in seconds |
| `TLS_CERT` | _(unset)_ | Path to TLS certificate PEM file |
| `TLS_KEY` | _(unset)_ | Path to TLS private key PEM file |
| `ERROR_PAGES_DIR` | _(unset)_ | Directory containing `{status}.html` error page files |
| `COMPRESSION` | `true` | Enable Brotli compression. Set to `false`, `0`, or `off` to disable |

### Ports

| Port | Purpose |
|------|---------|
| `8080` | Main HTTP server (or HTTPS if TLS is configured) |
| `9090` | Internal server: `/health`, `/metrics`, `/config` |

### Volume Mounts

| Host Path | Container Path | Purpose |
|-----------|---------------|---------|
| `./www` | `/var/www/html` | Application files (PHP scripts, static assets). Mount as `:ro` |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | PHP configuration (OPcache, sessions). Mount as `:ro` |
| `./certs` | `/etc/ssl/oxphp` | TLS certificate and key files. Mount as `:ro` |

## Alpine www-data User

The runtime image runs as `www-data` (UID 82, GID 82) for compatibility with nginx and Apache conventions. Alpine 3.21 has the `www-data` group pre-created at GID 82 but does not include the user, so the Dockerfile creates it:

```dockerfile
RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data 2>/dev/null || true
```

If your application needs to write to specific directories (sessions, cache, uploads), ensure those directories are writable by UID 82.

## See Also

- [Installation](/getting-started/installation/) -- build prerequisites and source build instructions
- [Quick Start](/getting-started/quick-start/) -- get OxPHP running in under 5 minutes
- [Configuration](/operations/configuration/) -- full environment variable reference
- [Graceful Shutdown](/operations/graceful-shutdown/) -- drain behavior and timeout settings
