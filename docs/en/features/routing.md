---
title: Routing
description: Configure OxPHP routing with three modes — traditional file mapping, framework front-controller, and SPA fallback. Each mode mirrors the equivalent nginx try_files directive.
---

# Routing

OxPHP routes incoming HTTP requests using one of three modes, controlled by a single environment variable. Each mode mirrors a familiar nginx `try_files` configuration so you can predict exactly what happens for any URL.

## How It Works

Every request runs through a shared pipeline before the mode-specific logic kicks in:

1. **Dot-path filter** — paths containing hidden segments (`.git`, `.env`) are blocked, with an exception for `/.well-known/*` ([RFC 8615](https://www.rfc-editor.org/rfc/rfc8615))
2. **Route cache lookup** — recently resolved URIs are returned from an LRU cache (10 000 entries)
3. **Percent-decoding + sanitization** — encoded sequences like `%2e%2e` are decoded and traversal segments (`..`, `.`, empty) are stripped
4. **Well-known PHP block** — defense-in-depth: `.php` scripts inside `/.well-known/` never execute
5. **URI classification** — the sanitized path is classified once into `NoExtension`, `Php`, or `OtherExtension`
6. **Mode dispatch** — each mode handles the three URI kinds with its own rules
7. **Symlink validation** — every resolved filesystem path must canonicalize inside the document root

The classification step is the key efficiency: the disk check for static assets (`/style.css`, `/logo.png`) is performed **once** in the shared layer for `OtherExtension` URIs, so all three modes pay the same cost.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DOCUMENT_ROOT` | `/var/www/html/public` | Root directory for serving files and PHP scripts |
| `ENTRY_FILE` | *(unset)* | Single canonical entry script. Unset = Traditional. `*.php` = Framework. Non-`.php` = SPA. With `WORKER_MODE_ENABLED=true` = Worker. Accepts an absolute path or one relative to `DOCUMENT_ROOT` (`..` allowed); the resolved path must exist |
| `WORKER_MODE_ENABLED` | `false` | Enable persistent worker mode. Requires `ENTRY_FILE` to point at a `.php` script |

The legacy `INDEX_FILE` and `WORKER_FILE` variables are still parsed (with a startup `WARN`) and map onto the new model. See [Configuration → Deprecated](../operations/configuration.md#deprecated-index_file-and-worker_file).

## Traditional Mode

Active when `ENTRY_FILE` is **not set** (or empty) and `WORKER_MODE_ENABLED=false`. Equivalent nginx config:

```nginx
location / {
    try_files $uri $uri/ /index.php /index.html =404;
}
location ~ \.php$ {
    try_files $uri =404;          # PATH_INFO splitting enabled
}
```

**Resolution order:**

1. **`$uri`** — exact file on disk → serve (or execute if `.php`)
2. **`$uri/`** — directory → look for `index.php`, then `index.html` inside it
3. **PATH_INFO split** — when the URI contains `.php/`, the script prefix is matched on disk and the remainder becomes `PATH_INFO` (e.g. `/api.php/users/42` → script `api.php`, `PATH_INFO=/users/42`)
4. **`/index.php`** — root front-controller fallback
5. **`/index.html`** — root static index fallback
6. **Worker fallback** — if `WORKER_MODE_ENABLED=true`, dispatch to the worker `ENTRY_FILE`
7. **404**

**Examples:**

| Request | Result |
|---|---|
| `/about.php` | Execute `about.php` |
| `/style.css` | Serve `style.css` |
| `/blog/` (with `blog/index.php`) | Execute `blog/index.php` |
| `/api.php/users/42` | Execute `api.php` with `PATH_INFO=/users/42` |
| `/missing.txt` | Falls back to `/index.php` |
| `/some/route` | Falls back to `/index.php` |

PATH_INFO splitting is **always on** in Traditional mode. There is no env switch — the previous `SPLIT_PATH_INFO_ENABLED` flag has been removed.

## Framework Mode

Active when `ENTRY_FILE=index.php` (or any value ending in `.php`) and `WORKER_MODE_ENABLED=false`. Equivalent nginx config:

```nginx
location ~ \.(?!php$)[a-zA-Z0-9]+$ {
    try_files $uri /index.php;    # static assets: fall back to front controller
}
location / {
    rewrite ^ /index.php last;    # everything else → front controller
}
location = /index.php {
    fastcgi_param PATH_INFO $request_uri;
    fastcgi_pass ...;
}
```

**Resolution rules:**

| URI kind | Behavior |
|---|---|
| `.css`, `.png`, `.js`, … (any non-php extension) | Serve the file if it exists, otherwise rewrite to `/index.php` with `PATH_INFO` set to the original URI |
| `.php` (any path) | Rewrite to `/index.php` with `PATH_INFO` set to the original URI |
| no extension (`/api/users`, `/`) | Rewrite to `/index.php` with `PATH_INFO` set to the original URI |

**Examples:**

| Request | Result | `$_SERVER['PATH_INFO']` |
|---|---|---|
| `/style.css` (exists) | Serve `style.css` | — |
| `/style.css` (missing) | Execute `index.php` | `/style.css` |
| `/api/users` | Execute `index.php` | `/api/users` |
| `/about.php` | Execute `index.php` | `/about.php` |
| `/api.php/v1/users` | Execute `index.php` | `/api.php/v1/users` |
| `/index.php` (direct) | Execute `index.php` | `/index.php` |
| `/` | Execute `index.php` | `/` |

The front controller always receives the **original URI** in `PATH_INFO`, letting your router decide what to do without inspecting `REQUEST_URI` separately. Direct access to `/index.php` is no longer blocked — the rewrite is idempotent, so hitting it directly produces the same result as hitting `/`.

A missing static asset falls back to the front controller rather than returning a fast 404 — the same `try_files $uri /index.php` behavior Laravel and Symfony ship by default, so your application renders its own 404 for missing assets. The trade-off is that every request to a non-existent asset now runs PHP; if the front controller still has nothing to serve (`/index.php` itself absent), the request returns a hard 404.

## SPA Mode

Active when `ENTRY_FILE=index.html` (or any non-`.php` value) and `WORKER_MODE_ENABLED=false`. Equivalent nginx config:

```nginx
location ~ \.php$ {
    try_files $uri =404;          # PHP: file must exist, no fallback
}
location ~ \. {
    try_files $uri =404;          # other extensions: hard 404 if missing
}
location / {
    try_files /index.html =404;   # no-extension paths: straight to index.html
}
```

**Resolution rules:**

| URI kind | Behavior |
|---|---|
| `.php` | Execute the file if it exists, otherwise **hard 404** |
| `.css`, `.png`, … (any other extension) | Serve the file if it exists, otherwise **hard 404** |
| no extension (`/dashboard`, `/api/users`, `/`) | Serve `/index.html` directly — **no disk probe of `$uri`** |

**Examples:**

| Request | Result |
|---|---|
| `/style.css` (exists) | Serve `style.css` |
| `/style.css` (missing) | **404** |
| `/dashboard` | Serve `/index.html` |
| `/users/42/edit` | Serve `/index.html` |
| `/api.php` (exists) | Execute `api.php` |
| `/api.php` (missing) | **404** |
| `/index.html` (direct) | Serve `index.html` |

Two semantics worth highlighting:

- **No-extension paths skip the disk** — the SPA mode never asks "does `/dashboard` exist on disk?" It always returns the index. This is correct for client-side routers and avoids unnecessary `stat()` calls.
- **Missing static files are hard 404, not fallthrough** — a missing `/style.css` does not silently serve `index.html`. This catches broken asset references early instead of returning HTML where JS expected CSS.

## Worker Mode

Worker mode activates when `WORKER_MODE_ENABLED=true` and `ENTRY_FILE` points at a `.php` script. The router runs Traditional-style direct-mapping for static assets, then dispatches every unmatched request to the worker `ENTRY_FILE` instead of returning 404.

| Step | Behavior |
|---|---|
| Static assets (`.css`, `.png`, …) | Served directly from disk if present, else hard 404 |
| Anything else | Dispatched to the worker `ENTRY_FILE` |

Startup-time validation rejects two combinations:

- `WORKER_MODE_ENABLED=true` with no `ENTRY_FILE` → `WORKER_MODE_ENABLED=true requires ENTRY_FILE to be set`.
- `WORKER_MODE_ENABLED=true` with a non-`.php` `ENTRY_FILE` → `WORKER_MODE_ENABLED=true requires a .php ENTRY_FILE`.

See [Worker Mode](worker-mode.md) for full configuration details.

## PATH_INFO Behavior

`$_SERVER['PATH_INFO']` is populated differently depending on mode:

| Mode | When set | Value |
|---|---|---|
| Traditional | Only when the URI contains `.php/` (PATH_INFO split) | Tail after the script segment, e.g. `/users/42` |
| Framework | **Always** | Full original URI, e.g. `/api/users` |
| SPA | Never | (PHP is only invoked for exact `.php` files; no PATH_INFO) |

In Traditional mode, splitting is always on — the previous `SPLIT_PATH_INFO_ENABLED` env variable has been removed. If you want PATH_INFO routing, use Traditional mode and place a `.php` script as the prefix.

## Path Security

OxPHP applies multiple layers of protection to prevent directory traversal, hidden file disclosure, and symlink escape attacks:

- **Percent-decoding** runs before sanitization, so encoded traversal attempts like `/%2e%2e/etc/passwd` are caught
- **Segment filtering** removes `..`, `.`, and empty segments from the resolved path
- **Symlink validation** canonicalizes every resolved path and verifies it remains within the document root. Symlinks that point outside the served directory are blocked
- **Dot-path blocking** blocks any path segment starting with `.` (e.g. `/.git/config`, `/.env`), with an exception for `/.well-known/*` per RFC 8615
- **Well-known PHP block** — even with the dot-path exception, `.php` scripts under `/.well-known/` are never executed (defense-in-depth)
- **PHP execution deny-list** — in Traditional mode, `PHP_DENY_PATHS` blocks `.php` execution at configured glob patterns (e.g. `/uploads/**`, or a single file like `/admin/legacy.php`) before any disk I/O. See [PHP Execution Deny-List](../security/php-deny.md)

> **Note:** If the document root directory does not exist at startup, the server exits with a fatal error. Symlink escape protection requires a valid, resolvable document root path.

## Troubleshooting

### All requests return 404 in Traditional mode

Check that `index.php` or `index.html` exists at the document root. The Traditional mode's `try_files` chain only falls back to these — if both are missing and no file matches the URL, you get 404.

```bash
docker exec <container> ls /var/www/html/public
```

### Missing static asset returns 404 instead of the SPA shell

This is intentional in SPA mode: a missing `/style.css` is a hard 404, not a silent fallback to `index.html`, which catches broken asset references early. In Framework and Traditional modes a missing static file falls back to the front controller (`/index.php`), so your application router renders the 404. Use SPA mode if you want hard 404s on missing assets.

### Direct `/index.php` no longer returns 404

In Framework mode, direct access to the front controller is now allowed (the rewrite to `/index.php` is idempotent). If you previously relied on the 404 to detect direct hits, switch to checking `PATH_INFO` from inside the controller.

### `PATH_INFO` is empty in Framework mode

Make sure `ENTRY_FILE` ends in `.php`. If it doesn't, OxPHP picks SPA mode, which doesn't populate `PATH_INFO`. The variable is now set unconditionally in Framework mode — you no longer need any feature flag.

### Symlink inside document root returns 404

Symlinks that point outside the document root are blocked by design. Move the target content inside the document root, or mount it as a directory at the correct path.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.7.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - ENTRY_FILE=index.php
```

## See Also

- [Static Files](static-files.md) — MIME detection, caching, and streaming for served files
- [Worker Mode](worker-mode.md) — persistent PHP processes and worker mode routing
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
