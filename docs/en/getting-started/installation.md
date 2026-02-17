---
title: Installation
description: How to install and build OxPHP
---

## Prerequisites

**Docker (recommended):**

- Docker Engine 20.10+ or Docker Desktop
- Docker Compose v2

**Source build (without PHP):**

- Rust toolchain 1.75+ (`rustup` recommended)

**Source build (with PHP):**

- Rust toolchain 1.75+
- PHP 8.4 with ZTS (Zend Thread Safety) enabled
- `libphp.so` available in the library search path
- C compiler (gcc or clang) for the bridge library and PHP extension

## Docker Build

Docker is the primary build method. It produces a minimal Alpine image with the Rust binary, PHP runtime, bridge library, and PHP extension pre-configured.

```bash
docker compose build
docker compose up -d
```

The multi-stage Dockerfile handles the complete build pipeline:

1. Compiles the C bridge library (`liboxphp_bridge.so`)
2. Builds the PHP extension (`oxphp_sapi.so`) against PHP 8.4 ZTS
3. Builds the Rust binary inside the same `php:8.4-zts-alpine` image
4. Copies only runtime artifacts into a slim Alpine image

To enable optional features like the example plugin, pass `CARGO_FEATURES` as a build argument:

```bash
docker compose build --build-arg CARGO_FEATURES="plugin-example"
```

See the [Docker guide](/getting-started/docker/) for a full walkthrough of the Dockerfile stages and `docker-compose.yml` configuration.

## Source Build (Stub Executor)

To build OxPHP without PHP support (static file serving only, useful for development), use `--no-default-features` to disable the `php` feature:

```bash
cargo build --release --no-default-features
```

The resulting binary is at `target/release/oxphp`. It uses the stub executor, which returns a placeholder response for PHP requests.

**Note:** The `php` feature is enabled by default. Running `cargo build --release` without `--no-default-features` requires `libphp.so` and the bridge library to be available on the host.

## Source Build (With PHP)

Building with PHP requires `libphp.so` (ZTS build) and the bridge library to be installed on the host:

```bash
# Build and install the bridge library
cd ext/bridge
make && sudo make install

# Build and install the PHP extension
cd ext
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# Build OxPHP with PHP support (default features include php)
cargo build --release
```

At runtime, the binary needs `libphp.so` and `liboxphp_bridge.so` in the library search path:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

### Alpine Compatibility

If you are deploying to Alpine Linux, you must build the Rust binary inside the same `php:8.4-zts-alpine` image used for the PHP runtime. Building in a separate image or on a different libc (glibc vs musl) causes TLS corruption at runtime. The provided Dockerfile handles this correctly.

## Running Tests

Run the test suite on a host without PHP by disabling default features:

```bash
# All checks (format, lint, tests)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Unit tests only
cargo test --no-default-features --lib

# All tests (unit + integration)
cargo test --no-default-features

# With example plugin
cargo clippy --no-default-features --features plugin-example -- -D warnings && cargo test --no-default-features --features plugin-example
```

## Verifying the Installation

After starting OxPHP, you should see structured JSON log output:

```
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:8080",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:8080"}
```

Test that the server responds:

```bash
curl http://localhost:8080/
```

If you configured the internal server, check the health endpoint:

```bash
curl http://localhost:9090/health
```

## See Also

- [Quick Start](/getting-started/quick-start/) -- get OxPHP running in under 5 minutes
- [Docker](/getting-started/docker/) -- Dockerfile stages, docker-compose.yml reference, and deployment tips
- [Configuration](/operations/configuration/) -- full list of environment variables
