---
title: Compression
description: OxPHP compresses responses with Brotli or gzip, reducing transfer sizes for text, JSON, SVG, and other compressible content types.
---

# Compression

OxPHP compresses HTTP responses with Brotli or gzip, whichever the client accepts. Compression applies automatically to text-based content types, reducing transfer sizes without any application code changes.

## How It Works

1. **Content coding negotiation** — the client's `Accept-Encoding` header decides which coding the response is encoded with, or whether it is encoded at all. See [Choosing a coding](#choosing-a-coding).
2. **Content type check** — the response MIME type is verified against the list of compressible types.
3. **Already encoded check** — responses with an existing `Content-Encoding` header are skipped to avoid double-compression.
4. **Size range check** — only responses between 256 bytes and 3 MB are compressed. Smaller responses see little benefit; larger responses are streamed without buffering.
5. **Compression** — the negotiated coding is applied. If the compressed output is not smaller than the original, the uncompressed response is sent instead.

Compression happens after PHP execution and after static file serving. The entire compressed body is held in memory briefly, which is why responses above 3 MB are excluded.

## Choosing a Coding

`Accept-Encoding` is a ranking, not a list (RFC 9110 §12.5.3), and OxPHP reads it as one:

- Coding names are matched case-insensitively, and unknown codings are ignored.
- `br;q=0` is a refusal, not support — the same as omitting the coding entirely.
- A `*` covers any coding the header does not name explicitly, at the weight given to it.
- Among the codings a client accepts, the one it weighted highest wins.
- When weights tie — the usual case, since browsers send every coding they support without weights — Brotli wins, because it is the coding whose cached static artifacts are smallest.

A client that accepts neither coding receives the response unencoded. In practice that means gzip is the fallback: every HTTP client of the last twenty years accepts it, while Chromium-based browsers advertise `br` only over HTTPS.

Because the answer depends on the request header, every compressible response carries `Vary: Accept-Encoding` so shared caches keep the variants apart.

## Cached Static Files

A static file small enough to sit in the content cache (1 MiB or less) is compressed once rather than on every request. Once such a file has been served twice to a client accepting a given coding, OxPHP compresses it at that coding's maximum level on a background thread and keeps the result next to the cached bytes; every later request that negotiates the same coding is answered from that stored copy.

This is invisible from the outside apart from the response getting smaller — maximum quality typically produces 12–19% less than the per-request level. No request waits for the compression: the one that triggers it, and any that arrive while it runs, are served at the configured per-request level as before. Response headers do not change.

Each coding gets its own stored copy, built on demand: a file only ever served to Brotli clients never costs a gzip compression. All of them share the cached file's validator, so they are discarded together with the cached bytes when [`STATIC_REVALIDATE`](static-files.md) notices the file changed on disk, and they count against the same content-cache budget. Bytes that do not compress are marked and not retried. Setting `COMPRESSION_LEVEL=0` disables this along with all other compression.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `COMPRESSION_LEVEL` | `4` | Brotli quality level (0–11). Higher values produce smaller output at the cost of more CPU time. Set to `0` to disable compression entirely — gzip included |
| `GZIP_LEVEL` | `6` | Gzip level (0–9). Set to `0` to offer only Brotli, leaving clients that do not accept it with unencoded responses |

The two knobs are not symmetric, and deliberately so: `COMPRESSION_LEVEL=0` has meant "no compression" since Brotli was the only coding, so it stays the switch for all of it. `GZIP_LEVEL=0` turns off gzip alone.

Levels 9–11 of Brotli are better suited for offline or build-time compression than for per-request work; cached static files use them anyway, because that cost is paid once. Gzip level 6 is zlib's own default and is close to the point of diminishing returns — level 9 costs roughly twice as much for a percent or two.

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

- The client accepts neither `br` nor `gzip` in the `Accept-Encoding` header, or accepts them only with a zero weight (`br;q=0, gzip;q=0`)
- The response already has a `Content-Encoding` header (e.g. pre-compressed content)
- The response body is smaller than 256 bytes or larger than 3 MB
- The content type is not in the compressible list (e.g. `image/png`, `image/jpeg`, `font/woff2`, `application/zip` — these formats already use internal compression)
- The response is streamed — its length is unknown when headers are sent (PHP scripts using `oxphp_stream_flush()`, Server-Sent Events). Compressing a stream would require buffering it entirely in memory, destroying time-to-first-byte, so streamed responses always pass through uncompressed

## Response Headers

When compression is applied, OxPHP sets the following headers:

| Header | Value |
|--------|-------|
| `Content-Encoding` | `br` or `gzip`, whichever was negotiated |
| `Content-Length` | Updated to the compressed body size |
| `Vary` | `Accept-Encoding` is appended, ensuring HTTP caches store separate versions per coding |

## Troubleshooting

### Responses are not compressed

Verify that the client sends an `Accept-Encoding` header at all — browsers do, but some HTTP testing tools send none by default, and a request without one gets an unencoded response.

A weight of zero is a refusal, not a preference: `Accept-Encoding: br;q=0, gzip;q=0` disables compression for that request as surely as sending no header.

**Check** with curl:

```bash
curl -H "Accept-Encoding: br, gzip" -I http://localhost/
```

Look for `Content-Encoding` in the response headers. If it is absent, check that:

1. `COMPRESSION_LEVEL` is not set to `0`
2. The response body is at least 256 bytes
3. The response `Content-Type` is in the compressible list above

### A browser gets gzip where curl gets Brotli

Chromium-based browsers advertise `br` only over HTTPS. Over plain HTTP they send `Accept-Encoding: gzip, deflate` and are answered with gzip, which is the intended fallback — nothing is misconfigured.

### Compression is making responses larger

For very small responses (under a few hundred bytes), Brotli overhead occasionally produces a larger output than the original. OxPHP detects this and sends the uncompressed response automatically — no configuration change is needed.

### High CPU usage from compression

Higher quality levels (8–11) compress significantly better but use much more CPU. If you observe high CPU consumption from compression:

**Fix:** Lower `COMPRESSION_LEVEL` to `4` or `5`, and `GZIP_LEVEL` to `4`–`6`. These levels provide 80–90% of the size reduction of maximum quality at a fraction of the CPU cost. Static files are unaffected either way: their compressed copies are built once in the background, not per request.

### Pre-compressed assets are being compressed again

If your build pipeline generates `.br` or `.gz` files and sets the `Content-Encoding` header on those files, OxPHP skips re-compression automatically. If your pre-compressed content is being compressed again, verify that the `Content-Encoding` header is present in the original response before compression runs.

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
      - COMPRESSION_LEVEL=6
      - GZIP_LEVEL=6
```

## See Also

- [Static Files](static-files.md) — file serving, MIME detection, and HTTP caching
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
