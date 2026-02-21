---
title: Compression
description: Brotli compression for compressible response types
---

OxPHP compresses HTTP responses using Brotli when the client supports it and the response type is compressible. Compression is enabled by default and can be toggled with an environment variable.

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `COMPRESSION` | Enable Brotli compression | `true` |

To disable compression:

```bash
COMPRESSION=false
```

The values `false`, `0`, and `off` all disable compression. Any other value (or not setting the variable at all) enables it.

## How it works

The compression pipeline runs after the response is built and before it is sent to the client. It is only invoked when the request includes an `Accept-Encoding` header -- requests without this header skip the compression function entirely, avoiding async overhead.

### Decision flow

1. **Accept-Encoding check** -- parse the header, split on `,`, extract the encoding name before any `;` quality parameter, and look for `br`
2. **Content-Type check** -- verify the response MIME type is in the compressible list
3. **Already encoded check** -- skip if the response already has a `Content-Encoding` header
4. **Content-Length guard** -- skip if the `Content-Length` header is present and outside the 256 B to 3 MB range
5. **Body size hint guard** -- skip if the body size hint (when no `Content-Length` is present) is outside the 256 B to 3 MB range
6. **Collect body** -- materialize the response body into memory
7. **Runtime size check** -- verify the collected body is within range (for responses without an upfront size hint)
8. **Compress** -- apply Brotli and discard the result if the compressed output is not smaller than the original

### Brotli parameters

| Parameter | Value | Notes |
|-----------|-------|-------|
| Quality | 4 | Balances speed and ratio for web serving |
| Window size | 20 | 1 MB window, suitable for typical web responses |

Quality level 4 is chosen as a compromise: it compresses well enough for text-based web content without the CPU cost of higher quality levels (9-11) that are better suited for offline compression.

## Compressible types

Compression is applied to the following 19 MIME types (matched exactly, not by prefix):

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

**Other:**
- `image/svg+xml`
- `font/ttf`
- `font/otf`
- `application/x-font-ttf`
- `application/x-font-opentype`
- `application/vnd.ms-fontobject`

Types like `image/png`, `image/jpeg`, `font/woff2`, and `application/zip` are not compressed because they already use internal compression.

## Size limits

| Limit | Value | Reason |
|-------|-------|--------|
| Minimum | 256 bytes | Small responses are unlikely to benefit from compression |
| Maximum | 3 MB | Larger responses should stream from disk without being collected into memory |

Responses outside this range are sent uncompressed.

## Response headers

When compression is applied, the following headers are set:

| Header | Value |
|--------|-------|
| `Content-Encoding` | `br` |
| `Content-Length` | Updated to the compressed size |
| `Vary` | `Accept-Encoding` (appended) |

The `Vary` header ensures that HTTP caches store separate versions for clients that support Brotli and those that do not.

## See Also

- [Static Files](static-files.md) -- file serving and content caching
- [Timeouts](timeouts.md) -- request timeout applies to the full pipeline including compression
