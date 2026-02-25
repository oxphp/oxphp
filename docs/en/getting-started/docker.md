---
title: Docker
description: Docker image usage, compose.yml reference, and deployment tips
---

OxPHP is distributed as a pre-built Docker image at `ghcr.io/oxphp/oxphp:nightly`. This page covers how to use the image, configure it with `compose.yml`, and common deployment considerations.

## Using the Image

The simplest way to run OxPHP is to extend the base image with your application files:

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data . /var/www/html
```

The image includes:

- The `oxphp` binary
- PHP 8.4 ZTS runtime (`libphp.so`)
- Bridge library (`liboxphp_bridge.so`)
- PHP extension (`oxphp_sapi.so`) with `oxphp_request_id()`, `oxphp_server_info()`, and other functions
- Alpine Linux base with minimal runtime dependencies
- `www-data` user (UID 82, GID 82) for non-root execution

The default document root is `/var/www/html/public`. The server listens on port 8080. The `CMD` is `["oxphp"]`.

## compose.yml Reference

```yaml
services:
  oxphp:
    build: .
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
      - DOCUMENT_ROOT=/var/www/html/public
      # - INDEX_FILE=index.php       # Enables Framework routing mode
      - EXECUTOR=sapi                # "sapi" or "stub"
      # - PHP_WORKERS=0              # Static: 0 = CPU/2, or fixed N
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

For development, you can mount your source directory as a volume instead of copying files into the image:

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:nightly
    ports:
      - "8080:8080"
    volumes:
      - ./www:/var/www/html:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html/public
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Address and port for the main HTTP server |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Root directory for serving files |
| `INDEX_FILE` | _(unset)_ | Set to `index.php` for Framework mode or `index.html` for SPA mode |
| `EXECUTOR` | `sapi` | PHP executor type: `sapi` (real PHP) or `stub` (placeholder) |
| `PHP_WORKERS` | `0` (CPU / 2, min 1) | Worker pool mode. `N` = fixed pool, `MIN:MAX` = dynamic scaling |
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
| `TOKIO_WORKERS` | `0` (CPU / 2, min 1) | Tokio async runtime threads (0 = auto) |
| `ACCESS_LOG` | *(off)* | Per-request JSON access log: `all`, `error` (4xx/5xx only), empty = off |
| `SLOT_POOL_SIZE` | `QUEUE_CAPACITY + PHP_WORKERS*2` | Pre-allocated response slot pool size |

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

## PHP Configuration

To customize PHP settings (OPcache, JIT, sessions, etc.), create an `oxphp.ini` file and mount it into the container:

```ini
[opcache]
opcache.enable=1
opcache.jit=1255
opcache.jit_buffer_size=64M
```

```yaml
volumes:
  - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
```

See [OPcache](../php/opcache.md) for recommended settings.

## Alpine www-data User

The image runs as `www-data` (UID 82, GID 82) for compatibility with nginx and Apache conventions. If your application needs to write to specific directories (sessions, cache, uploads), ensure those directories are writable by UID 82.

## Building from Source

If you need to build OxPHP from source (for example, to enable custom Cargo features or modify the server), refer to the [Installation](installation.md) guide for source build instructions. The OxPHP repository includes a multi-stage Dockerfile that compiles the bridge library, PHP extension, and Rust binary from source.

## See Also

- [Installation](installation.md) -- source build prerequisites and instructions
- [Quick Start](quick-start.md) -- get OxPHP running in under 5 minutes
- [Configuration](../operations/configuration.md) -- full environment variable reference
- [Graceful Shutdown](../operations/graceful-shutdown.md) -- drain behavior and timeout settings
