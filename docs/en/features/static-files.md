---
title: Static Files
description: File serving with MIME detection, LRU caching, and streaming for large files
---

OxPHP serves static files directly from the document root with automatic MIME type detection, a three-tier LRU cache, and streaming for large files.

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

The file cache reduces filesystem syscalls during routing and serving. It uses a `Mutex<HashMap>` with counter-based LRU eviction and operates on three tiers:

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

## Serving behavior

When a static file request arrives:

1. **Content cache check** -- if the file is cached, return it immediately with the stored MIME type
2. **MIME type lookup** -- compute the content type from the file extension
3. **TOCTOU re-validation** -- if symlink protection is enabled, re-canonicalize the path before reading
4. **Size check** -- read file metadata to determine the serving strategy

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

## Error handling

- **File not found**: returns 404 with a plain text body
- **Permission errors**: propagated as a 500 error
- **Read failures after metadata check**: returns 404 (file may have been deleted between check and read)

## See Also

- [Routing](routing.md) -- how URL paths are resolved to files on disk
- [Compression](compression.md) -- Brotli compression for compressible static file responses
- [Error Pages](error-pages.md) -- custom HTML pages for error responses
