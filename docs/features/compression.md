---
title: Compression
description: OxPHP compresses responses with Brotli, Zstandard, or gzip, reducing transfer sizes for text, JSON, SVG, and other compressible content types.
---

# Compression

OxPHP compresses HTTP responses with Brotli, Zstandard, or gzip, whichever the client accepts. Compression applies automatically to text-based content types, reducing transfer sizes without any application code changes.

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

Weights tie in the usual case, since browsers send every coding they support without weights. OxPHP breaks that tie by what becomes of the compressed bytes:

| The response is | Preferred coding | Why |
|---|---|---|
| A cached static file | Brotli, then Zstandard, then gzip | The bytes are compressed once at maximum quality and served from memory from then on, so nothing but size counts — and at the top of its range Brotli is the smallest of the three |
| Everything else | Zstandard, then Brotli, then gzip | The bytes are compressed while the client waits and discarded afterwards, so the cost is paid on every request — and at levels a request can afford, Zstandard measured within a few percent of Brotli on size for well under half the CPU |

A client that accepts none of the three receives the response unencoded. In practice gzip is the fallback: every HTTP client of the last twenty years accepts it, while Zstandard needs a browser released in 2024 or later, and Chromium-based browsers advertise `br` only over HTTPS.

Because the answer depends on the request header, every compressible response carries `Vary: Accept-Encoding` so shared caches keep the variants apart.

## Cached Static Files

A static file small enough to sit in the content cache (1 MiB or less) is compressed once rather than on every request. Once such a file has been served twice to a client accepting a given coding, OxPHP compresses it at that coding's maximum level on a background thread and keeps the result next to the cached bytes; every later request that negotiates the same coding is answered from that stored copy.

This is invisible from the outside apart from the response getting smaller — maximum quality typically produces 8–12% less than the per-request level. No request waits for the compression: the one that triggers it, and any that arrive while it runs, are served at the configured per-request level as before. Response headers do not change.

Each coding gets its own stored copy, built on demand: a file only ever served to gzip clients never costs a Brotli compression. All of them share the cached file's validator, so they are discarded together with the cached bytes when [`STATIC_REVALIDATE`](static-files.md) notices the file changed on disk, and they count against the same content-cache budget. Bytes that do not compress are marked and not retried.

Because the two preferences above differ, a client that accepts both Brotli and Zstandard sees the coding change once per file: the first hits are answered with Zstandard, compressed to serve that request, and every hit after the stored copy lands is answered with Brotli. Both are valid representations of the same resource and both carry `Vary`, so caches and clients need no help with the switch.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `COMPRESSION_ENCODINGS` | `br,zstd,gzip` | Which codings the server offers, comma-separated. Accepts `br` (or `brotli`), `zstd`, `gzip`, and `off` to switch compression off entirely. The order written here is ignored — the server picks per response, see [Choosing a coding](#choosing-a-coding) |
| `BROTLI_LEVEL` | `5` | Brotli quality (0–11) |
| `ZSTD_LEVEL` | `6` | Zstandard level (0–19) |
| `GZIP_LEVEL` | `6` | Gzip level (0–9) |
| `COMPRESSION_LEVEL` | *(unset)* | Deprecated name for `BROTLI_LEVEL`, kept for existing deployments together with the second meaning it carried when Brotli was the only coding: `COMPRESSION_LEVEL=0` switches off all compression. Setting it logs a warning at startup, and an explicit `BROTLI_LEVEL` overrides it |

A coding is offered when it is listed in `COMPRESSION_ENCODINGS` **and** its level is not `0`; either one alone withdraws it. An unknown name in the list is a startup error rather than a silently dropped coding.

Brotli defaults to 5 rather than 4 because its quality knee — a change of hasher — sits between the two: at 4 it produced *more* bytes than gzip does at its own default on JSON above 4 KB and on real minified assets, and spent more CPU doing it, which leaves no reason to prefer it over gzip at all. At 5 it is the smaller of the two on every body measured, for roughly twice gzip's CPU. Levels 9–11 are better suited for offline or build-time compression than for per-request work; cached static files use them anyway, because that cost is paid once. Gzip level 6 is zlib's own default and close to the point of diminishing returns — level 9 costs roughly twice as much for a percent or two. Zstandard defaults to 6 rather than to its own default of 3: on bodies over a few kilobytes level 6 costs less time than the Brotli quality earlier releases compressed everything with, while producing fewer bytes, so nothing regresses against what those releases sent.

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

- The client accepts none of `br`, `zstd`, and `gzip` in the `Accept-Encoding` header, or accepts them only with a zero weight (`br;q=0, zstd;q=0, gzip;q=0`)
- The response already has a `Content-Encoding` header (e.g. pre-compressed content)
- The response body is smaller than 256 bytes or larger than 3 MB
- The content type is not in the compressible list (e.g. `image/png`, `image/jpeg`, `font/woff2`, `application/zip` — these formats already use internal compression)
- The response is streamed — its length is unknown when headers are sent (PHP scripts using `oxphp_stream_flush()`, Server-Sent Events). Compressing a stream would require buffering it entirely in memory, destroying time-to-first-byte, so streamed responses always pass through uncompressed

## Response Headers

When compression is applied, OxPHP sets the following headers:

| Header | Value |
|--------|-------|
| `Content-Encoding` | `br`, `zstd`, or `gzip`, whichever was negotiated |
| `Content-Length` | Updated to the compressed body size |
| `Vary` | `Accept-Encoding` is appended, ensuring HTTP caches store separate versions per coding |

## Troubleshooting

### Responses are not compressed

Verify that the client sends an `Accept-Encoding` header at all — browsers do, but some HTTP testing tools send none by default, and a request without one gets an unencoded response.

A weight of zero is a refusal, not a preference: `Accept-Encoding: br;q=0, zstd;q=0, gzip;q=0` disables compression for that request as surely as sending no header.

**Check** with curl:

```bash
curl -H "Accept-Encoding: br, zstd, gzip" -I http://localhost/
```

Look for `Content-Encoding` in the response headers. If it is absent, check that:

1. `COMPRESSION_ENCODINGS` still lists the coding you asked for, and its level is not `0`
2. The response body is at least 256 bytes
3. The response `Content-Type` is in the compressible list above

### Different clients get different codings

This is the point of negotiation, and three cases account for nearly all of it. Chromium-based browsers advertise `br` only over HTTPS; over plain HTTP they send `Accept-Encoding: gzip, deflate` and are answered with gzip. Command-line tools often send no `Accept-Encoding` at all and are answered unencoded. And a browser new enough to send `zstd` gets Zstandard on dynamic responses where an older one gets Brotli. Nothing is misconfigured in any of these — `Vary: Accept-Encoding` is on every one of those responses so caches keep them apart.

### The coding changes for the same static file

Expected: the first hits on a cacheable static file are compressed with Zstandard to answer that request, and once the stored Brotli copy is built in the background, later hits are answered from it. See [Cached static files](#cached-static-files).

### Compression is making responses larger

For very small responses (under a few hundred bytes), framing overhead occasionally produces a larger output than the original whichever coding is used. OxPHP detects this and sends the uncompressed response automatically — no configuration change is needed.

### High CPU usage from compression

Higher quality levels (8–11) compress significantly better but use much more CPU. If you observe high CPU consumption from compression:

**Fix:** Lower `ZSTD_LEVEL` to `3` and `GZIP_LEVEL` to `4`. These levels provide 80–90% of the size reduction of maximum quality at a fraction of the CPU cost. Brotli is the expensive coding of the three, but lowering `BROTLI_LEVEL` to `4` is not the way to save that CPU — at 4 it produces more bytes than gzip does at its own default while still costing more than gzip. Drop `br` from `COMPRESSION_ENCODINGS` instead: clients that accept Brotli then get gzip, which on real bodies is 2–6% larger for roughly half the CPU. Static files cost nothing per request whichever codings remain, since their compressed copies are built once in the background.

### Pre-compressed assets are being compressed again

If your build pipeline generates `.br`, `.zst`, or `.gz` files and sets the `Content-Encoding` header on those files, OxPHP skips re-compression automatically. If your pre-compressed content is being compressed again, verify that the `Content-Encoding` header is present in the original response before compression runs.

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
      - COMPRESSION_ENCODINGS=br,zstd,gzip
      - BROTLI_LEVEL=5
      - ZSTD_LEVEL=6
      - GZIP_LEVEL=6
```

## See Also

- [Static Files](static-files.md) — file serving, MIME detection, and HTTP caching
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
