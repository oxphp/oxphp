---
title: Installation
description: How to install and run OxPHP
---

## Docker Image (Recommended)

OxPHP is distributed as a pre-built Docker image. Pull the latest nightly build:

```bash
docker pull ghcr.io/oxphp/oxphp:nightly
```

Create a `Dockerfile` in your project root:

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data . /var/www/html
```

Build and run:

```bash
docker build -t my-app .
docker run -p 8080:8080 my-app
```

That's it. The image includes the Rust binary, PHP 8.4 ZTS runtime, bridge library, PHP extension, and all required dependencies. No build tools needed.

## Prerequisites

**Docker (recommended):**

- Docker Engine 20.10+ or Docker Desktop

**Source build (without PHP):**

- Rust toolchain 1.75+ (`rustup` recommended)

**Source build (with PHP):**

- Rust toolchain 1.75+
- PHP 8.4 with ZTS (Zend Thread Safety) enabled
- `libphp.so` available in the library search path
- C compiler (gcc or clang) for the bridge library and PHP extension

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

- [Quick Start](quick-start.md) -- get OxPHP running in under 5 minutes
- [Docker](docker.md) -- compose.yml reference, Dockerfile stages, and deployment tips
- [Configuration](../operations/configuration.md) -- full list of environment variables
