---
title: Routing
description: Configure OxPHP routing with four modes — traditional file mapping, framework front-controller, SPA fallback, and worker mode.
---

# Routing

OxPHP routes incoming HTTP requests using one of four modes, controlled by a single environment variable. The mode you choose determines how URL paths map to files on disk.

## How It Works

When a request arrives, OxPHP processes the URL path through a security pipeline before resolving it to a file:

1. **Percent-decoding** — encoded characters like `%2e%2e` are decoded to their literal values
2. **Segment filtering** — path traversal segments (`..`), current-directory segments (`.`), and empty segments are stripped
3. **Mode-based routing** — the sanitized path is matched against the filesystem according to the active routing mode
4. **Symlink validation** — the resolved filesystem path is checked against the document root boundary to prevent symlink escapes

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DOCUMENT_ROOT` | `/var/www/html/public` | Root directory for serving files and PHP scripts |
| `INDEX_FILE` | *(unset)* | Determines routing mode. Unset = traditional, `index.php` = framework, `index.html` = SPA |
| `SPLIT_PATH_INFO_ENABLED` | `false` | Split URIs like `/script.php/path` into script + `PATH_INFO` |

## Traditional Mode

Traditional mode is active when `INDEX_FILE` is not set. URLs map directly to files on disk, similar to classic PHP hosting with Apache or nginx.

- `/about.php` executes `DOCUMENT_ROOT/about.php`
- `/style.css` serves `DOCUMENT_ROOT/style.css`
- `/` resolves to `index.php` if it exists, otherwise `index.html`
- `/blog/` tries `blog/index.php`, then `blog/index.html`
- Any path that does not match a file returns 404

This mode works well for WordPress, legacy PHP applications, or any project where each URL corresponds to a specific file.

## Framework Mode

Framework mode is active when `INDEX_FILE=index.php`. All requests that do not match an existing static file are routed to the front controller, exactly as Laravel, Symfony, and other PHP frameworks expect.

- `/style.css` serves the static file directly (if it exists on disk)
- `/api/users` executes `index.php` (the path does not exist as a file)
- `/about.php` returns 404 (direct `.php` access is blocked)
- `/index.php` returns 404 (direct access to the front controller is blocked)

Blocking direct `.php` access prevents URL leaks and enforces that all PHP requests pass through the framework's router.

## SPA Mode

SPA mode is active when `INDEX_FILE=index.html`. Requests that do not match an existing file fall back to the HTML entry point, allowing client-side routers (React Router, Vue Router, etc.) to handle the path.

- `/style.css` serves the static file
- `/app/dashboard` serves `index.html` (the client-side router handles the path)
- `/api.php` executes the PHP script if it exists on disk
- `/index.html` returns 404 (direct access to the index file is blocked)

## Worker Mode

Worker mode routing activates automatically when `WORKER_FILE` is set. All incoming requests that do not match a static file on disk are dispatched to the persistent PHP worker process rather than returning 404.

- `/style.css` serves the static file directly
- `/api/users` dispatches to the worker (no file exists at that path)
- `/` dispatches to the worker if no `index.php` or `index.html` exists

Worker mode is compatible with `INDEX_FILE`. Setting both `WORKER_FILE` and `INDEX_FILE=index.php` combines worker mode routing with framework mode static file handling — static files are served directly, and everything else goes to the worker.

See [Worker Mode](worker-mode.md) for full configuration details.

## PATH_INFO Splitting

Some legacy PHP applications use `PATH_INFO` for routing — placing extra path segments after a `.php` script name (e.g., `/api.php/users/42`). By default, OxPHP treats the entire URI as a single filesystem path, so these requests return 404.

Enable `SPLIT_PATH_INFO_ENABLED=true` to activate path splitting:

```bash
SPLIT_PATH_INFO_ENABLED=true
```

When enabled, OxPHP scans the URI left-to-right for the first `.php` segment that maps to an existing file on disk. Everything after it becomes `PATH_INFO`:

```
/app.php/user/42
├── Script: DOCUMENT_ROOT/app.php
└── PATH_INFO: /user/42
```

This populates `$_SERVER` correctly for CGI-style routing:

| Variable | Value |
|---|---|
| `SCRIPT_NAME` | `/app.php` |
| `SCRIPT_FILENAME` | `/var/www/html/public/app.php` |
| `PATH_INFO` | `/user/42` |
| `PHP_SELF` | `/app.php/user/42` |

If no `.php` file is found along the path, routing falls through to the normal resolution chain (worker fallback, `INDEX_FILE` fallback, or 404).

> **When to use:** Drupal, MediaWiki, legacy REST APIs built with `$_SERVER['PATH_INFO']`, or any application that was previously configured with nginx `fastcgi_split_path_info`.

## Path Security

OxPHP applies multiple layers of protection to prevent directory traversal and symlink escape attacks:

- **Percent-decoding** runs before sanitization, so encoded traversal attempts like `/%2e%2e/etc/passwd` are caught
- **Segment filtering** removes `..`, `.`, and empty segments from the resolved path
- **Symlink validation** canonicalizes every resolved path and verifies it remains within the document root. Symlinks that point outside the served directory are blocked
- **Dot-path blocking** blocks any path segment starting with `.` (e.g. `/.git/config`, `/.env`), with an exception for `/.well-known/*`. See [Dot-Path Blocking](../security/dot-path-blocking.md)

> **Note:** If the document root directory does not exist at startup, the server exits with a fatal error. Symlink escape protection requires a valid, resolvable document root path.

## Troubleshooting

### All requests return 404

Verify that `DOCUMENT_ROOT` points to the correct directory and that the directory exists on disk. OxPHP exits at startup if the document root cannot be resolved, so a running server means the directory existed at startup — but a volume mount or wrong path still causes every request to miss.

**Check:** Confirm the document root path inside the container:

```bash
docker exec <container> ls /var/www/html/public
```

**Fix:** Correct `DOCUMENT_ROOT` or ensure your volume mounts the right path.

### Framework mode returns 404 for PHP routes

Direct `.php` access is intentionally blocked in framework mode. If your application links directly to `.php` files, switch to traditional mode (`INDEX_FILE` unset) or update the links to use clean URLs.

### URLs with special characters return 404

OxPHP percent-decodes URLs before routing. Requests for paths like `/café/menu` work correctly. If a path still returns 404, confirm the file exists on disk with the decoded name.

### Symlink inside document root returns 404

Symlinks that point outside the document root are blocked by design. Move the target content inside the document root, or mount it as a directory at the correct path.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.2.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INDEX_FILE=index.php
```

## See Also

- [Static Files](static-files.md) — MIME detection, caching, and streaming for served files
- [Worker Mode](worker-mode.md) — persistent PHP processes and worker mode routing
- [Configuration Reference](../operations/configuration.md) — full list of environment variables
