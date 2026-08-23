---
title: Static Files
description: OxPHP serves static files with automatic MIME detection, in-memory caching, HTTP cache headers, conditional 304 responses, and Range requests.
---

# Static Files

OxPHP serves static files directly from the document root without invoking PHP. Files are served with automatic MIME type detection, an in-memory cache for fast repeated access, and full HTTP caching support including ETags, conditional requests, and Range requests for partial downloads.

## How It Works

When a request matches a static file:

1. **File matched** — the routing layer resolves the URL path to a file on disk
2. **MIME detection** — the content type is determined from the file extension
3. **Cache check** — the file cache is checked before touching the filesystem
4. **Conditional check** — if the request carries `If-None-Match` or `If-Modified-Since`, OxPHP evaluates the condition and may return `304 Not Modified` without sending a body
5. **Range check** — if a GET or HEAD request carries a `Range` header, OxPHP responds with `206 Partial Content`: GET receives only the requested byte range, HEAD the same range headers with no body
6. **Response** — files up to 1 MiB are served from the in-memory cache; larger files are streamed directly from disk, unless they are about to be compressed

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `STATIC_MAX_AGE` | `30d` | `Cache-Control: max-age` for static files. Accepts `30s`, `5m`, `2h`, `30d`, `1w`, `1y`, a bare number of seconds (e.g. `3600`), or `off` to disable caching headers entirely. Replaces deprecated `STATIC_CACHE_TTL`. |
| `STATIC_REVALIDATE` | `off` | Set to `on` to enable mtime revalidation on the in-memory content cache (re-checks each file at most once per 3 seconds; changes become visible within that window). Replaces deprecated `STATIC_CACHE` (where `off` had the inverse meaning). |

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
- Files **larger than 1 MiB** are streamed directly from disk instead of being cached. The `Content-Length` header is set from file metadata so the client knows the total size upfront.
- One exception to the streaming: a compressible file that still fits the [compression](compression.md) window (3 MB) is read in full for clients that accept a content coding, because a body the server does not hold whole cannot be encoded. It is compressed for that request and, being over the cache limit, compressed again for the next one — put a CDN in front of assets in this range, or keep them under 1 MiB, and the work is done once.

The file cache is populated on the first request to each file and retained across subsequent requests. By default, cache entries persist until evicted by the LRU policy.

### Content Revalidation

Set `STATIC_REVALIDATE=on` to enable mtime-based revalidation. In this mode the server re-checks a cached file's modification time with a `stat()` syscall **at most once every 3 seconds per file**, not on every request. If the file has changed on disk, the stale entry is evicted and the file is re-read automatically. Within the 3-second window a cached entry is served straight from memory with no syscall, so the cost is amortized rather than paid per request. Changes on disk become visible within 3 seconds. **Enable this in development** — you see file changes without restarting the server. Leave it off in production.

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

- **ETag** — a strong ETag in the format `"<size>-<mtime_hex>"`, derived from the file size and last modification time. A strong validator also satisfies `If-Range`, so interrupted downloads can resume safely. When a response is served compressed — under any of the codings — the tag is weakened to `W/"…"` — the compressed bytes are a different representation, and a weak tag still revalidates (304) but prevents mixing compressed and uncompressed fragments on resume.
- **Last-Modified** — an RFC 7231 HTTP date based on the file's modification time

These headers allow browsers and CDNs to validate cached copies without re-downloading the file.

### Conditional Requests (304)

OxPHP evaluates conditional request headers to avoid sending unchanged file content:

- **If-None-Match** — the client sends the ETag it has cached. If it matches the current file, OxPHP returns `304 Not Modified` with no body.
- **If-Modified-Since** — the client sends a timestamp. If the file has not been modified since that time, OxPHP returns 304.

`If-None-Match` takes priority over `If-Modified-Since` per RFC 7232. For files already in the in-memory cache, the conditional check runs without any disk I/O.

### Range Requests (206)

Static file responses advertise `Accept-Ranges: bytes`, and GET requests with a single-range `Range` header receive only the requested bytes:

```http
GET /videos/intro.mp4 HTTP/1.1
Range: bytes=1048576-

HTTP/1.1 206 Partial Content
Content-Range: bytes 1048576-52428799/52428800
Content-Length: 51380224
```

This enables `<video>`/`<audio>` seeking in browsers, resumable downloads (`wget -c`, download managers), and partial PDF loading. All three range forms from RFC 9110 are supported: `bytes=N-M`, `bytes=N-` (from offset to end), and `bytes=-N` (last N bytes).

- A range that cannot be satisfied (start beyond the end of file) returns `416 Range Not Satisfiable` with `Content-Range: bytes */<size>`.
- **If-Range** is honored: when the client sends the ETag (or `Last-Modified` date) of its partial copy and the file has changed since, OxPHP returns the full `200` response instead of a mismatched fragment. The date form is only accepted once the file's modification second has fully elapsed — a just-written file could change again within the same second without moving the date, so it is not yet a strong validator (RFC 9110).
- Requests with **multiple ranges** (`bytes=0-1,4-5`) receive the full file as `200 OK` — `multipart/byteranges` responses are not generated.
- **HEAD** requests with a `Range` header receive the same `206`/`Content-Range` headers as GET without a body, matching nginx and Apache.
- **Ranges and compression are mutually exclusive.** For clients that accept any content coding, range handling is disabled on representations that would be served compressed, and compressed responses do not advertise `Accept-Ranges` — a resumed download could otherwise splice uncompressed bytes onto a compressed prefix. Files above the compression window (3 MB) are never compressed, so ranges always work for the content that actually needs them: video, archives, and images. Responses for compression-eligible files always carry `Vary: Accept-Encoding` — even when served uncompressed — so shared caches keep the variants apart.
- `206` responses are never compressed, and range handling does not apply to PHP responses — only to static files.

Example: resume an interrupted download with curl:

```bash
curl -C - -O https://example.com/dist/app-installer.dmg
```

### Disabling Caching

There are two independent cache layers and a variable for each:

| Variable | Controls | Effect of `off` |
|----------|----------|-----------------|
| `STATIC_MAX_AGE=off` | **Browser cache** (HTTP headers) | No `Cache-Control`, `ETag`, or `Last-Modified` headers sent |
| `STATIC_REVALIDATE=on` | **Server in-memory cache** | Re-checks file mtime at most once per 3s per file; stale entries evicted automatically |

For development, set `STATIC_REVALIDATE=on` so the server always serves fresh content. Optionally also set `STATIC_MAX_AGE=off` to prevent browser caching entirely.

## Troubleshooting

### Server keeps serving stale files

By default, the in-memory content cache does not check whether files have changed on disk. Set `STATIC_REVALIDATE=on` during development to enable mtime revalidation — the server detects file changes automatically (within 3 seconds).

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
    image: ghcr.io/oxphp/oxphp:0.11.0
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

- [Compression](compression.md) — how compressible static files are compressed and cached per coding
- [Routing](routing.md) — how URL paths are resolved to files on disk
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
