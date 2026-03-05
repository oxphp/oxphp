---
title: Configuration
description: Complete environment variable reference for OxPHP
---

OxPHP is configured entirely through environment variables. There are no configuration files. Every variable has a sensible default, so a zero-configuration deployment works out of the box for development.

## Environment Variable Reference

### Server

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Address and port for the main HTTP server |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Root directory for serving files and PHP scripts |
| `INDEX_FILE` | *(empty)* | Controls routing mode. See [Routing Modes](#routing-modes) |
| `TOKIO_WORKERS` | `0` (CPU / 2, min 1) | Tokio async I/O threads. `0` = auto (CPU/2), `1` = single-threaded runtime, `N` = multi-threaded runtime with N worker threads |
| `MAX_CONNECTIONS` | `10000` | Maximum concurrent TCP connections. New connections beyond this limit wait for a semaphore permit |

### PHP Execution

| Variable | Default | Description |
|----------|---------|-------------|
| `EXECUTOR` | `sapi` | PHP executor type. `sapi` for real PHP execution, `stub` for a placeholder response (benchmarking) |
| `PHP_WORKERS` | `0` (CPU / 2, min 1) | Worker pool mode. Set `N` for a fixed pool, or `MIN:MAX` for dynamic scaling. See [Worker Modes](#worker-modes) |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Seconds a dynamic worker must be idle before it is retired. Only applies in dynamic mode |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Maximum requests waiting in the PHP queue. When full, new PHP requests receive a `503 Service Unavailable` response. Uses initial worker count for dynamic mode |

### Logging

| Variable | Default | Description |
|----------|---------|-------------|
| `LOG_LEVEL` | `info` | Log verbosity. One of: `trace`, `debug`, `info`, `warn`, `error` |
| `ACCESS_LOG` | *(off)* | Per-request JSON access log. Values: `all` (every request), `error` (4xx/5xx only), empty/unset = off |

### Timeouts

| Variable | Default | Description |
|----------|---------|-------------|
| `HEADER_TIMEOUT_SECONDS` | `5` | Maximum seconds to wait for request headers after TCP connection |
| `IDLE_TIMEOUT_SECONDS` | `60` | Keep-alive idle timeout. Connections with no activity for this duration are closed |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Maximum seconds for the entire request-response cycle. Set to `0` to disable |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Maximum seconds to wait for in-flight connections during graceful shutdown |

### Rate Limiting

| Variable | Default | Description |
|----------|---------|-------------|
| `RATE_LIMIT` | `0` | Maximum requests per IP address per time window. `0` disables rate limiting |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window duration in seconds |

### TLS

| Variable | Default | Description |
|----------|---------|-------------|
| `TLS_CERT` | *(none)* | Path to the TLS certificate PEM file. Both `TLS_CERT` and `TLS_KEY` must be set to enable TLS |
| `TLS_KEY` | *(none)* | Path to the TLS private key PEM file |

### Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `INTERNAL_ADDR` | *(none)* | Address for the internal server (health checks, metrics, config). Not started when unset |
| `ERROR_PAGES_DIR` | *(none)* | Directory containing custom error page HTML files named `{status}.html` (e.g., `404.html`, `503.html`) |

### Worker Mode

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_FILE` | *(none)* | Path to the PHP worker script (relative to `DOCUMENT_ROOT`). When set, enables persistent worker mode where PHP processes stay alive across requests |
| `WORKER_MAX_REQUESTS` | `0` | Maximum requests a worker handles before recycling. `0` disables the limit |
| `WORKER_MAX_MEMORY_MIB` | `0` | Maximum memory (in megabytes) a worker may use before recycling. `0` disables the limit |

### Static File Caching

| Variable | Default | Description |
|----------|---------|-------------|
| `STATIC_CACHE_TTL` | `30d` | Cache TTL for static file responses. Controls `Cache-Control`, `ETag`, and `Last-Modified` headers. Supports: `30s`, `5m`, `2h`, `30d`, `1w`, `1y`, bare seconds (`3600`), or `off` to disable |

### Compression

| Variable | Default | Description |
|----------|---------|-------------|
| `COMPRESSION_ENABLED` | `true` | Enable Brotli compression for compressible response types. Disable with `false`, `0`, or `off` |

## Worker Modes

The `PHP_WORKERS` variable controls whether the PHP worker pool uses a fixed size or scales dynamically.

### Static Mode (default)

Set `PHP_WORKERS` to a number (or leave unset/`0` for auto-detection):

```bash
PHP_WORKERS=8      # Fixed 8 workers
PHP_WORKERS=0      # Auto-detect: CPU / 2 (min 1)
```

Workers are spawned at startup and never change. Each worker uses a blocking `recv()` with zero CPU overhead when idle.

### Dynamic Mode

Set `PHP_WORKERS` to `MIN:MAX` to enable auto-scaling:

```bash
PHP_WORKERS=2:16       # Scale between 2 and 16 workers
PHP_WORKERS=4:0        # 4 minimum, auto-detect maximum (CPU * 2)
PHP_WORKERS=0:0        # Auto-detect both (CPU/4 min (min 1), CPU*2 max)
```

The ScaleManager runs every 500ms and:
- **Scales up** when all workers are busy and the pool is below MAX (500ms cooldown)
- **Scales down** when a worker has been idle longer than `PHP_WORKERS_IDLE_SECONDS` and the pool is above MIN (5s cooldown)

Dynamic workers use `recv_timeout(200ms)` to allow periodic shutdown-flag checks.

## Routing Modes

The `INDEX_FILE` variable controls how OxPHP routes incoming requests. There are three modes:

| Mode | `INDEX_FILE` value | Behavior |
|------|-------------------|----------|
| Traditional | *(empty / unset)* | Direct file mapping. `/about.php` serves `about.php`. `/` serves `index.php` or `index.html` if present |
| Framework | `index.php` | All non-file requests route to `index.php` (front controller). Direct `.php` access is blocked |
| SPA | `index.html` | Missing files fall back to `index.html`. `.php` files still execute normally |

### Traditional Mode (default)

URLs map directly to files on disk. This is the standard behavior for classic PHP applications.

```bash
# No INDEX_FILE set — traditional mode is the default
DOCUMENT_ROOT=/var/www/html/public
```

### Framework Mode

All requests pass through a single entry point. This is the standard pattern for Laravel, Symfony, and similar frameworks.

```bash
INDEX_FILE=index.php
DOCUMENT_ROOT=/var/www/html/public
```

### SPA Mode

Static assets are served directly. All other requests receive `index.html`, allowing the JavaScript router to handle navigation.

```bash
INDEX_FILE=index.html
DOCUMENT_ROOT=/var/www/html/dist
```

## Example Configurations

### Development

```bash
LISTEN_ADDR=127.0.0.1:8080
DOCUMENT_ROOT=./www
LOG_LEVEL=debug
PHP_WORKERS=1
INTERNAL_ADDR=127.0.0.1:9090
```

### Laravel Production (static pool)

```bash
LISTEN_ADDR=0.0.0.0:8080
DOCUMENT_ROOT=/var/www/html/public
INDEX_FILE=index.php
PHP_WORKERS=8
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
MAX_CONNECTIONS=10000
INTERNAL_ADDR=127.0.0.1:9090
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
HEADER_TIMEOUT_SECONDS=5
IDLE_TIMEOUT_SECONDS=30
REQUEST_TIMEOUT_SECONDS=60
DRAIN_TIMEOUT_SECONDS=30
COMPRESSION_ENABLED=true
STATIC_CACHE_TTL=30d
```

### Laravel Production (dynamic pool)

```bash
LISTEN_ADDR=0.0.0.0:8080
DOCUMENT_ROOT=/var/www/html/public
INDEX_FILE=index.php
PHP_WORKERS=4:32
PHP_WORKERS_IDLE_SECONDS=60
QUEUE_CAPACITY=512
LOG_LEVEL=warn
MAX_CONNECTIONS=10000
INTERNAL_ADDR=127.0.0.1:9090
RATE_LIMIT=100
RATE_WINDOW_SECONDS=60
HEADER_TIMEOUT_SECONDS=5
IDLE_TIMEOUT_SECONDS=30
REQUEST_TIMEOUT_SECONDS=60
DRAIN_TIMEOUT_SECONDS=30
COMPRESSION_ENABLED=true
STATIC_CACHE_TTL=30d
```

### Docker Compose

```yaml
services:
  oxphp:
    image: oxphp:latest
    ports:
      - "8080:8080"
    environment:
      LISTEN_ADDR: "0.0.0.0:8080"
      DOCUMENT_ROOT: "/var/www/html/public"
      INDEX_FILE: "index.php"
      PHP_WORKERS: "4"             # Or "2:16" for dynamic scaling
      # PHP_WORKERS_IDLE_SECONDS: "30" # Idle timeout (dynamic mode only)
      QUEUE_CAPACITY: "512"
      LOG_LEVEL: "info"
      INTERNAL_ADDR: "127.0.0.1:9090"
      COMPRESSION_ENABLED: "true"
      # STATIC_CACHE_TTL: "30d"       # Static file cache TTL (default: 30d)
    volumes:
      - ./src:/var/www/html
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
```

### Worker Mode (persistent PHP)

```bash
LISTEN_ADDR=0.0.0.0:8080
DOCUMENT_ROOT=/var/www/html/public
WORKER_FILE=../worker.php
PHP_WORKERS=8
WORKER_MAX_REQUESTS=10000
WORKER_MAX_MEMORY_MIB=128
QUEUE_CAPACITY=1024
LOG_LEVEL=warn
INTERNAL_ADDR=127.0.0.1:9090
```

Worker mode keeps PHP processes alive across requests. The worker script calls `oxphp_worker()` with a handler callback. Workers are automatically recycled when they hit `WORKER_MAX_REQUESTS` or `WORKER_MAX_MEMORY_MIB`. Set both to `0` to disable recycling.

### TLS Termination

```bash
LISTEN_ADDR=0.0.0.0:443
TLS_CERT=/etc/oxphp/tls/cert.pem
TLS_KEY=/etc/oxphp/tls/key.pem
```

OxPHP uses rustls for TLS, so there is no OpenSSL dependency. The certificate and key must be in PEM format.

## Inspecting Active Configuration

When the internal server is running, you can view the resolved configuration:

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:8080",
  "document_root": "/var/www/html/public",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "idle_timeout_seconds": 60,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": "/etc/oxphp/error-pages",
  "compression_enabled": true,
  "static_cache_ttl": 2592000,
  "access_log": true,
  "plugins": {}
}
```

TLS key and certificate paths are not included in the output. The `tls_enabled` boolean indicates whether TLS is active. The `plugins` object contains configuration from any loaded plugins.

## See Also

- [Routing](/features/routing.md) --- detailed explanation of the three routing modes
- [Health Checks](health-checks.md) --- the internal server's `/health`, `/metrics`, and `/config` endpoints
- [Metrics](metrics.md) --- Prometheus-compatible metrics reference
- [Graceful Shutdown](graceful-shutdown.md) --- how `DRAIN_TIMEOUT_SECONDS` affects the shutdown sequence
- [TLS](/features/tls.md) --- TLS configuration and certificate requirements
- [Rate Limiting](/features/rate-limiting.md) --- per-IP rate limiting details
- [Worker Pool](/architecture/worker-pool.md) --- static and dynamic worker pool architecture
