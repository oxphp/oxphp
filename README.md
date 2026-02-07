# OxPHP

Async HTTP server built with Rust. Serves static files over HTTP/1.1 and HTTP/2 with automatic protocol detection.

## Features

- **HTTP/1.1 + HTTP/2** auto-detection (h2c) via hyper
- **LRU file cache** to reduce filesystem syscalls
- **JSON structured logging** via tracing
- **Path traversal protection** with percent-decoding and sanitization
- **Non-root container** execution as www-data (UID 82)

## Quick Start

```bash
docker compose up -d
curl http://localhost:8080/
```

## Configuration

All settings are via environment variables:

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Address and port to bind |
| `DOCUMENT_ROOT` | `/var/www/html` | Filesystem path to serve files from |
| `INDEX_FILE` | *(unset)* | Routing mode selector |
| `LOG_LEVEL` | `info` | Tracing verbosity: `error`, `warn`, `info`, `debug`, `trace` |

## Build

```bash
cargo build --release
```

### Run locally

```bash
DOCUMENT_ROOT=./www ./target/release/oxphp
```

### Docker

```bash
docker compose up -d
```

## Development

```bash
# Full verification
cargo fmt -- --check && cargo clippy -- -D warnings && cargo test
```

## License

[AGPL-3.0](LICENSE)
