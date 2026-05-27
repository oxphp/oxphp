---
title: Static Files
description: OxPHP serves static files with automatic MIME detection, in-memory caching, HTTP cache headers, and conditional 304 responses.
---

# Static Files

OxPHP serves static files directly from the document root without invoking PHP. Files are served with automatic MIME type detection, an in-memory cache for fast repeated access, and full HTTP caching support including ETags and conditional requests.

## How It Works

When a request matches a static file:

1. **File matched** — the routing layer resolves the URL path to a file on disk
2. **MIME detection** — the content type is determined from the file extension
3. **Cache check** — the file cache is checked before touching the filesystem
4. **Conditional check** — if the request carries `If-None-Match` or `If-Modified-Since`, OxPHP evaluates the condition and may return `304 Not Modified` without sending a body
5. **Response** — files up to 1 MiB are served from the in-memory cache; larger files are streamed directly from disk

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `STATIC_MAX_AGE` | `30d` | `Cache-Control: max-age` for static files. Accepts `30s`, `5m`, `2h`, `30d`, `1w`, `1y`, a bare number of seconds (e.g. `3600`), or `off` to disable caching headers entirely. Replaces deprecated `STATIC_CACHE_TTL`. |
| `STATIC_REVALIDATE` | `off` | Set to `on` to enable mtime revalidation on the in-memory content cache. Replaces deprecated `STATIC_CACHE` (where `off` had the inverse meaning). |

## MIME Detection

MIME types are determined automatically from the file extension. If no type can be determined, the server falls back to `application/octet-stream`. Common mappings include:

| Extension | Content-Type |
|-----------|-------------|
| `.html` | `text/html` |
| `.css` | `text/css` |
| `.js` | `text/javascript` |
| `.json` | `application/json` |
| `.png` | `image/png` |
| `.svg` | `image/svg+xml` |
| `.woff2` | `font/woff2` |

## File Caching

OxPHP uses an in-memory cache to reduce disk I/O for frequently requested files:

- Files **up to 1 MiB** (1,048,576 bytes) are read into memory and cached. The total cache budget is 64 MiB (67,108,864 bytes). When the budget is exceeded, the least recently used entries are evicted to make room.
- Files **larger than 1 MiB** are always streamed directly from disk. The `Content-Length` header is set from file metadata so the client knows the total size upfront.

The file cache is populated on the first request to each file and retained across subsequent requests. By default, cache entries persist until evicted by the LRU policy.

### Content Revalidation

Set `STATIC_REVALIDATE=on` to enable mtime-based revalidation. In this mode, each cache hit performs a `stat()` syscall to check the file's modification time. If the file has changed on disk, the stale entry is evicted and the file is re-read automatically. **Enable this in development** — you see file changes immediately without restarting the server. Leave it off in production.

In production, leave `STATIC_REVALIDATE` unset (the default `off`) for maximum throughput with zero per-request syscall overhead.

## HTTP Caching

### Cache-Control

When `STATIC_MAX_AGE` is set (the default is `30d`), every static file response includes a `Cache-Control` header:

```http
Cache-Control: public, max-age=2592000
```

The `max-age` value is the TTL converted to seconds. Set `STATIC_MAX_AGE=off` to omit this header entirely.

### ETag and Last-Modified

Every static file response includes:

- **ETag** — a weak ETag in the format `W/"<size>-<mtime_hex>"`, derived from the file size and last modification time
- **Last-Modified** — an RFC 7231 HTTP date based on the file's modification time

These headers allow browsers and CDNs to validate cached copies without re-downloading the file.

### Conditional Requests (304)

OxPHP evaluates conditional request headers to avoid sending unchanged file content:

- **If-None-Match** — the client sends the ETag it has cached. If it matches the current file, OxPHP returns `304 Not Modified` with no body.
- **If-Modified-Since** — the client sends a timestamp. If the file has not been modified since that time, OxPHP returns 304.

`If-None-Match` takes priority over `If-Modified-Since` per RFC 7232. For files already in the in-memory cache, the conditional check runs without any disk I/O.

### Disabling Caching

There are two independent cache layers and a variable for each:

| Variable | Controls | Effect of `off` |
|----------|----------|-----------------|
| `STATIC_MAX_AGE=off` | **Browser cache** (HTTP headers) | No `Cache-Control`, `ETag`, or `Last-Modified` headers sent |
| `STATIC_REVALIDATE=on` | **Server in-memory cache** | Each hit validates file mtime; stale entries evicted automatically |

For development, set `STATIC_REVALIDATE=on` so the server always serves fresh content. Optionally also set `STATIC_MAX_AGE=off` to prevent browser caching entirely.

## Troubleshooting

### Server keeps serving stale files

By default, the in-memory content cache does not check whether files have changed on disk. Set `STATIC_REVALIDATE=on` during development to enable mtime revalidation — the server will detect file changes automatically.

### Browser keeps serving stale files

If the server is returning fresh content but the browser still shows the old version, the browser's own cache is the culprit. Set `STATIC_MAX_AGE=off` to stop sending caching headers, or use your browser's hard reload (Shift+F5 or Cmd+Shift+R).

### Files are served with `application/octet-stream`

OxPHP uses the file extension to determine the MIME type. If an extension is missing or not recognized, it falls back to `application/octet-stream`. Add the correct extension to your file, or ensure your framework sets the `Content-Type` header explicitly in PHP responses.

### Large files seem slow

Files larger than 1 MiB are streamed from disk on every request and are not cached in memory. For very large files, place a CDN in front of OxPHP to cache them at the edge. Alternatively, restructure your assets so frequently served files stay under 1 MiB.

### 304 responses are returned when you expect 200

A 304 means the client already has the current version. This is correct behavior. If you need to force a fresh response during development, set `STATIC_MAX_AGE=off` to stop sending `ETag` and `Last-Modified` headers.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.6.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - ENTRY_FILE=index.php
      - STATIC_MAX_AGE=1y
```

## Best Practices

- **Use long TTLs with cache-busting filenames** in production (e.g. `app.a1b2c3.js`). Set `STATIC_MAX_AGE=1y` for maximum browser and CDN caching.
- **Set `STATIC_REVALIDATE=on` during development** so the server detects file changes automatically. Optionally also set `STATIC_MAX_AGE=off` to bypass browser caching.
- **Place a CDN in front of OxPHP** for high-traffic sites. The `ETag`, `Last-Modified`, and `Cache-Control` headers work with all major CDN providers.
- **Let your build tool handle asset hashing.** Frameworks like Vite and Laravel Mix generate hashed filenames automatically, making long cache TTLs safe.

## See Also

- [Compression](compression.md) — Brotli compression for compressible static file responses
- [Routing](routing.md) — how URL paths are resolved to files on disk
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
