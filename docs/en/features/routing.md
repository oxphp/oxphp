---
title: Routing
description: Three routing modes for traditional PHP, framework, and SPA applications
---

OxPHP supports three routing modes controlled by a single environment variable. Each mode determines how incoming URL paths map to files on disk.

## Routing modes

The `INDEX_FILE` environment variable selects the routing mode:

| Mode | `INDEX_FILE` value | Use case |
|------|-------------------|----------|
| Traditional | *(unset)* | Classic PHP hosting, WordPress with per-file routing |
| Framework | `index.php` | Laravel, Symfony, or any front-controller framework |
| SPA | `index.html` | React, Vue, Angular with client-side routing |

### Traditional mode

When `INDEX_FILE` is not set, OxPHP maps URLs directly to files on disk.

- `/style.css` serves `DOCUMENT_ROOT/style.css`
- `/about.php` executes `DOCUMENT_ROOT/about.php`
- `/` resolves to `index.php` if it exists, otherwise `index.html`
- `/subdir/` tries `subdir/index.php`, then `subdir/index.html`
- Missing files return 404

```bash
# No INDEX_FILE set -- traditional mode is the default
DOCUMENT_ROOT=/var/www/html/public
```

### Framework mode

When `INDEX_FILE=index.php`, all requests that do not match an existing static file are routed to the front controller.

- `/style.css` serves the static file directly
- `/api/users` executes `index.php` (file does not exist on disk)
- `/about.php` returns 404 (direct `.php` access is blocked)
- `/index.php` returns 404 (direct access to the index file is blocked)

```bash
INDEX_FILE=index.php
DOCUMENT_ROOT=/var/www/html/public
```

Blocking direct `.php` access prevents URL leaks and enforces that all PHP requests go through the framework's router.

### SPA mode

When `INDEX_FILE=index.html`, missing paths fall back to the HTML entry point. PHP files still execute normally.

- `/style.css` serves the static file
- `/app/dashboard` serves `index.html` (client-side router handles it)
- `/api.php` executes the PHP script
- `/index.html` returns 404 (direct access to the index file is blocked)

```bash
INDEX_FILE=index.html
DOCUMENT_ROOT=/var/www/html/public
```

## Root path resolution

Requests to `/` use pre-computed paths to avoid allocations on every request. The server checks for `index.php` first, then `index.html`. If neither exists, it returns 404.

Subdirectory paths with a trailing slash (like `/blog/`) follow the same index resolution: `index.php` first, then `index.html`.

## Path sanitization

Every incoming URI path goes through a sanitization pipeline before reaching the filesystem:

1. **Percent-decoding** -- `%2e%2e` is decoded to `..` before sanitization catches it
2. **Segment filtering** -- `..`, `.`, and empty segments are stripped
3. **Symlink validation** -- resolved paths are checked against the canonical document root

A request like `/%2e%2e/etc/passwd` is decoded to `/../etc/passwd`, sanitized to `etc/passwd`, and then validated against the document root boundary.

## Symlink escape protection

At startup, OxPHP canonicalizes the document root path. Every resolved file path is canonicalized and checked to ensure it remains within the document root. This blocks symlinks that point outside the served directory.

Canonical path results are cached to avoid repeated `realpath(3)` syscalls. The canonical path cache has the same 200-entry capacity limit as the metadata cache, but is stored and evicted independently in its own HashMap.

If the document root cannot be canonicalized at startup (for example, the directory does not exist yet), symlink protection is disabled and a warning is logged.

### TOCTOU mitigation

The route cache caches validated `RouteResult` entries. TOCTOU re-canonicalization on every request is performed in `static_file::serve()`, just before reading from disk, not in the routing layer. This mitigates time-of-check-to-time-of-use attacks where a symlink is swapped between route resolution and file read.

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `DOCUMENT_ROOT` | Filesystem path to serve files from | `/var/www/html/public` |
| `INDEX_FILE` | Index file name, controls routing mode | *(unset)* |

## See Also

- [Static Files](static-files.md) -- caching, MIME detection, and streaming for served files
- [Compression](compression.md) -- Brotli compression applied to static file responses
- [Error Pages](error-pages.md) -- custom HTML pages for 404 and other error responses
