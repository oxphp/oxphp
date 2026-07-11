---
title: Compression
description: OxPHP compresses responses with Brotli by default, reducing transfer sizes for text, JSON, SVG, and other compressible content types.
---

# Compression

OxPHP compresses HTTP responses using Brotli encoding by default. Compression applies automatically to text-based content types when the client supports it, reducing transfer sizes without any application code changes.

## How It Works

1. **Accept-Encoding check** — the client's `Accept-Encoding` header is parsed for `br` (Brotli) support. Requests without `br` in this header are never compressed.
2. **Content type check** — the response MIME type is verified against the list of compressible types.
3. **Already encoded check** — responses with an existing `Content-Encoding` header are skipped to avoid double-compression.
4. **Size range check** — only responses between 256 bytes and 3 MB are compressed. Smaller responses see little benefit; larger responses are streamed without buffering.
5. **Compression** — Brotli encoding is applied. If the compressed output is not smaller than the original, the uncompressed response is sent instead.

Compression happens after PHP execution and after static file serving. The entire compressed body is held in memory briefly, which is why responses above 3 MB are excluded.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `COMPRESSION_LEVEL` | `4` | Brotli quality level (0–11). Higher values produce smaller output at the cost of more CPU time. Set to `0` to disable compression entirely |

The default level of `4` provides a good balance between compression ratio and CPU usage for web serving. Levels 9–11 are better suited for offline or build-time compression.

## Compressible Content Types

Compression applies to the following MIME types:

**Text types:**
- `text/html`
- `text/css`
- `text/plain`
- `text/xml`
- `text/javascript`

**Application types:**
- `application/javascript`
- `application/json`
- `application/xml`
- `application/xhtml+xml`
- `application/rss+xml`
- `application/atom+xml`
- `application/manifest+json`
- `application/ld+json`
- `application/wasm`

**Other types:**
- `image/svg+xml`
- `font/ttf`
- `font/otf`
- `application/x-font-ttf`
- `application/x-font-opentype`
- `application/vnd.ms-fontobject`

## Not Compressed

Responses are sent without compression when any of the following conditions are met:

- The client does not advertise `br` in the `Accept-Encoding` header
- The response already has a `Content-Encoding` header (e.g. pre-compressed content)
- The response body is smaller than 256 bytes or larger than 3 MB
- The content type is not in the compressible list (e.g. `image/png`, `image/jpeg`, `font/woff2`, `application/zip` — these formats already use internal compression)
- The response is streamed — its length is unknown when headers are sent (PHP scripts using `oxphp_stream_flush()`, Server-Sent Events). Compressing a stream would require buffering it entirely in memory, destroying time-to-first-byte, so streamed responses always pass through uncompressed

## Response Headers

When compression is applied, OxPHP sets the following headers:

| Header | Value |
|--------|-------|
| `Content-Encoding` | `br` |
| `Content-Length` | Updated to the compressed body size |
| `Vary` | `Accept-Encoding` is appended, ensuring HTTP caches store separate versions for clients with and without Brotli support |

## Troubleshooting

### Responses are not compressed

Verify that the client sends `Accept-Encoding: br`. Most modern browsers do, but some HTTP testing tools do not include it by default.

**Check** with curl:

```bash
curl -H "Accept-Encoding: br" -I http://localhost/
```

Look for `Content-Encoding: br` in the response headers. If it is absent, check that:

1. `COMPRESSION_LEVEL` is not set to `0`
2. The response body is at least 256 bytes
3. The response `Content-Type` is in the compressible list above

### Compression is making responses larger

For very small responses (under a few hundred bytes), Brotli overhead occasionally produces a larger output than the original. OxPHP detects this and sends the uncompressed response automatically — no configuration change is needed.

### High CPU usage from compression

Higher quality levels (8–11) compress significantly better but use much more CPU. If you observe high CPU consumption from compression:

**Fix:** Lower `COMPRESSION_LEVEL` to `4` or `5`. These levels provide 80–90% of the size reduction of maximum quality at a fraction of the CPU cost.

### Pre-compressed assets are being compressed again

If your build pipeline generates `.br` files and sets the `Content-Encoding: br` header on those files, OxPHP skips re-compression automatically. If your pre-compressed content is being compressed again, verify that the `Content-Encoding` header is present in the original response before compression runs.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - ENTRY_FILE=index.php
      - COMPRESSION_LEVEL=6
```

## See Also

- [Static Files](static-files.md) — file serving, MIME detection, and HTTP caching
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
