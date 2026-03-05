---
title: Static Files
description: File serving with MIME detection, LRU caching, HTTP caching, and streaming for large files
---

OxPHP serves static files directly from the document root with automatic MIME type detection, a three-tier LRU cache, HTTP caching (ETag / Last-Modified / 304), and streaming for large files.

## MIME type detection

MIME types are determined from the file extension using the `mime_guess` crate. If no type can be determined, the server falls back to `application/octet-stream`. Common mappings include:

| Extension | Content-Type |
|-----------|-------------|
| `.html` | `text/html` |
| `.css` | `text/css` |
| `.js` | `application/javascript` |
| `.json` | `application/json` |
| `.png` | `image/png` |
| `.svg` | `image/svg+xml` |
| `.woff2` | `font/woff2` |

## File cache

The file cache reduces filesystem syscalls during routing and serving. It uses an `RwLock<FileCacheInner>` wrapping three separate HashMaps (metadata, content, and canonical path caches) with counter-based LRU eviction and operates on three tiers:

### Metadata cache

Stores whether a path refers to a file, a directory, or does not exist. The router checks this cache on every request to decide whether to serve, execute, or return 404 without hitting the filesystem.

- Capacity: 200 entries (configurable at compile time)
- Eviction: LRU by access counter
- Cache miss triggers an async `tokio::fs::metadata()` call

### Content cache

Stores the full file contents for small files so repeated requests are served from memory without disk I/O.

- Per-file maximum: 1 MB
- Total cache budget: 64 MB
- Eviction: LRU when total bytes exceed the budget
- Data stored as `Bytes` (reference-counted, zero-copy clone)
- MIME type stored alongside content as `Arc<str>`

Files larger than 1 MB are never cached and are always streamed from disk.

### Canonical path cache

Stores the result of `canonicalize()` calls used for symlink escape protection. This avoids repeated `realpath(3)` syscalls on the same paths.

- Shares the same 200-entry capacity as the metadata cache
- Stores `Option<PathBuf>` -- `None` means the file did not exist at cache time
- Eviction: LRU by access counter

## HTTP caching

OxPHP supports HTTP caching for static files with ETag validation and conditional requests, controlled by the `STATIC_CACHE_TTL` environment variable.

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `STATIC_CACHE_TTL` | `30d` | Cache TTL for static files. Supports flexible duration formats or `off` to disable |

Supported duration formats:

| Format | Example | Seconds |
|--------|---------|---------|
| Seconds | `30s` | 30 |
| Minutes | `5m` | 300 |
| Hours | `2h` | 7,200 |
| Days | `30d` | 2,592,000 |
| Weeks | `1w` | 604,800 |
| Years | `1y` | 31,536,000 |
| Bare number | `3600` | 3,600 |
| Disabled | `off` | *(no caching headers)* |

### Response headers

When caching is enabled, every static file response includes:

| Header | Value | Example |
|--------|-------|---------|
| `Cache-Control` | `public, max-age={ttl}` | `public, max-age=2592000` |
| `ETag` | Weak ETag from file size and mtime | `W/"1024-65a1b2c3"` |
| `Last-Modified` | RFC 7231 HTTP date | `Tue, 14 Nov 2023 18:13:20 GMT` |

### Conditional requests (304 Not Modified)

When a client sends `If-None-Match` or `If-Modified-Since` headers, OxPHP validates them against the file's current metadata:

1. **If-None-Match** takes priority (per RFC 7232 §3.3). The ETag is compared against a comma-separated list of tags. A `*` value always matches.
2. **If-Modified-Since** is checked as a fallback. The file's modification time (truncated to second precision) is compared against the header value.

If the file has not changed, OxPHP returns a `304 Not Modified` response with `ETag`, `Last-Modified`, and `Cache-Control` headers but no body. This check runs **before** any file I/O:

- For **cached files**: the conditional check runs under a read lock without cloning any data
- For **uncached files**: the conditional check runs after `fs::metadata()` but before `fs::read()`, avoiding disk reads for unchanged files

### ETag format

ETags use the weak format `W/"<size>-<mtime_hex>"` where:

- `<size>` is the file size in bytes (decimal)
- `<mtime_hex>` is the Unix timestamp of the last modification (lowercase hex)

Weak ETags indicate semantic equivalence — the response may differ at the byte level (e.g., after Brotli compression) but represents the same content.

### Disabling caching

Set `STATIC_CACHE_TTL=off` to disable all caching headers. No `Cache-Control`, `ETag`, or `Last-Modified` headers will be sent, and conditional requests will not be evaluated.

## Serving behavior

When a static file request arrives:

1. **Conditional check (cached)** -- if the file is in the content cache and has matching conditional headers, return 304 immediately
2. **Content cache check** -- if the file is cached, return it with the stored MIME type and caching headers
3. **MIME type lookup** -- compute the content type from the file extension
4. **TOCTOU re-validation** -- if symlink protection is enabled, re-canonicalize the path before reading
5. **Metadata + conditional check (uncached)** -- read file metadata; if conditional headers match, return 304 before reading the file
6. **Size check** -- determine the serving strategy based on file size

### Small files (up to 1 MB)

The entire file is read into memory with `tokio::fs::read()`, inserted into the content cache, and returned as a buffered response body. The `Content-Length` header is set to the exact byte count.

### Large files (over 1 MB)

The file is opened with `tokio::fs::File::open()` and streamed to the client using `ReaderStream`. The `Content-Length` header is set from the file metadata so the client knows the total size.

## Response headers

Every static file response includes:

| Header | Value |
|--------|-------|
| `Content-Type` | Detected MIME type |
| `Content-Length` | File size in bytes |
| `Cache-Control` | `public, max-age={ttl}` *(when caching enabled)* |
| `ETag` | `W/"<size>-<mtime_hex>"` *(when caching enabled)* |
| `Last-Modified` | RFC 7231 HTTP date *(when caching enabled)* |

## Error handling

- **File not found**: returns 404 with a plain text body
- **Permission errors**: propagated as a 500 error
- **Read failures after metadata check**: returns 404 (file may have been deleted between check and read)

## See Also

- [Routing](routing.md) -- how URL paths are resolved to files on disk
- [Compression](compression.md) -- Brotli compression for compressible static file responses; `Vary: Accept-Encoding` is added by the compression layer, not by static file serving
- [Error Pages](error-pages.md) -- custom HTML pages for error responses
