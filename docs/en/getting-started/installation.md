---
title: Installation
description: Install OxPHP via Docker image or build from source. Covers prerequisites, verification, and platform notes.
---

# Installation

OxPHP is distributed as a Docker image — the fastest and recommended way to start serving PHP applications. The image bundles the server binary, PHP 8.4 ZTS, the OxPHP extension, and all runtime dependencies on Alpine Linux.

## Docker (Recommended)

Pull the official image from the GitHub Container Registry:

```bash
docker pull ghcr.io/oxphp/oxphp:0.1.0
```

The image includes:

- **OxPHP server binary** — the async HTTP server
- **PHP 8.4 ZTS** — thread-safe PHP runtime for multi-worker execution
- **OxPHP PHP extension** (`oxphp_sapi.so`) — provides `oxphp_request_id()`, `oxphp_server_info()`, `oxphp_worker()`, and other built-in functions
- **Bridge library** (`liboxphp_bridge.so`) — connects the Rust server to the PHP runtime
- **Alpine Linux** base — minimal runtime footprint
- Runs as **www-data** (UID 82, GID 82) for non-root container execution

To containerize your application, extend the official image:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

Build and run:

```bash
docker build -t my-app .
docker run -p 80:80 my-app
```

The server listens on port `80` by default. The document root is `/var/www/html/public`.

## Source Build (Without PHP)

Build OxPHP from source with the PHP feature disabled to serve static files only:

```bash
cargo build --release --no-default-features
```

The binary is at `target/release/oxphp`. It uses a stub executor that returns a placeholder response for PHP requests while serving static files normally. This mode is useful for testing the server without a PHP runtime present.

## Source Build (With PHP)

Building OxPHP with full PHP support requires the bridge library and PHP extension to be compiled and installed first.

### Prerequisites

- Rust toolchain (1.91.1 or later)
- PHP 8.4 with ZTS (Zend Thread Safety) enabled
- C compiler (gcc or clang)
- `phpize` and PHP development headers

### Build Steps

```bash
# 1. Build and install the bridge library
cd ext/bridge
make && sudo make install

# 2. Build and install the PHP extension
cd ../
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# 3. Build OxPHP (default features include php)
cargo build --release
```

The binary requires the shared libraries in the library search path at runtime:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

> **Note:** When deploying to Alpine Linux, build inside the same `php:8.4-zts-alpine` image used for the PHP runtime. Mixing glibc and musl builds causes runtime errors. The official Docker image handles this correctly.

## Verifying Installation

After starting OxPHP, structured JSON log output confirms the server is running:

```text
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:80",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:80"}
```

Test that the server responds:

```bash
curl http://localhost/
```

If you enabled the internal server with `INTERNAL_ADDR`, verify the health endpoint:

```bash
curl http://localhost:9090/health
```

A healthy server returns `200` with JSON status. A degraded server returns `503`.

## What's Next

- [Quick Start](quick-start.md) — create a project, run OxPHP with Docker Compose, and make your first request
- [Docker Guide](docker.md) — Dockerfiles for development and production, Compose configuration, and volume mounts
- [Configuration](../operations/configuration.md) — full environment variable reference
