---
title: PHP Execution Deny-List
description: Block PHP execution at specific URI paths to harden legacy applications against uploaded PHP shells and unintended script exposure.
---

# PHP Execution Deny-List

`PHP_DENY_PATHS` blocks direct execution of `.php` files matching configured glob patterns. It targets a recurring class of vulnerabilities in legacy PHP applications: an attacker uploads a PHP file to a writable public directory (`/uploads`, `/cache`, image-resize temp dirs) and reaches it via direct URI to gain code execution.

The check runs **before any disk I/O**, so denied paths return the same response whether the file exists on disk or not — there is no existence oracle for attackers to probe upload directories with.

## When It Applies

Direct file mapping mode only. With `ENTRY_FILE` set (front-controller, SPA, or worker mode), every request already routes through one trusted entry script — arbitrary `.php` files in the document root cannot be invoked directly, so the deny-list is redundant. Setting `PHP_DENY_PATHS` together with `ENTRY_FILE` emits a startup warning and disables the check.

| Routing mode | `PHP_DENY_PATHS` honored? |
|---|---|
| Traditional (no `ENTRY_FILE`) | Yes |
| Framework (`ENTRY_FILE=index.php`) | No — warned and ignored |
| SPA (`ENTRY_FILE=index.html`) | No — warned and ignored |
| Worker (`WORKER_MODE_ENABLED=true`) | No — warned and ignored |

## Configuration

```bash
# Comma-separated glob patterns
PHP_DENY_PATHS="/uploads/**,/cache/**,/tmp/**"

# What to return on a match (default: 404)
PHP_DENY_FALLBACK="403"
```

A request to `/uploads/shell.php` now returns 403 without touching the disk. A request to `/uploads/image.png` is served normally — the deny-list only affects `.php` execution, never static-file serving.

## Pattern Syntax

Patterns are matched against the sanitized URI (the request path with `..` segments and percent-encoded bypasses already resolved) using the `globset` syntax. The leading `/` on each pattern is optional — `/uploads/**` and `uploads/**` are equivalent.

| Pattern | Matches | Doesn't match |
|---|---|---|
| `/uploads/**` | `/uploads/x.php`, `/uploads/a/b/c.php`, `/uploads/shell.php/extra` | `/uploads.php`, `/public/uploads/x.php` |
| `/files/*.php` | `/files/x.php` | `/files/sub/x.php` (single `*` does not cross `/`) |
| `/admin/legacy.php` | `/admin/legacy.php` | `/admin/legacy.php/x` (PATH_INFO not covered — see below) |
| `/admin/legacy.php{,/**}` | `/admin/legacy.php`, `/admin/legacy.php/x` | `/admin/other.php` |
| `/**/wp-config.php` | `/wp-config.php`, `/site/wp-config.php` | `/wp-config.txt` |

Multiple patterns are combined with OR — a request matches the deny-list if it matches any pattern.

### Single Files vs Directories

Both work. `/uploads/**` blocks an entire subtree; `/admin/legacy.php` blocks one specific script. To block a single legacy entry point and any `PATH_INFO` invocations of it (`/admin/legacy.php/foo`), use the brace form: `/admin/legacy.php{,/**}`.

### Case Sensitivity

Matching is **case-sensitive**. On case-insensitive filesystems (default macOS HFS+/APFS, default Windows NTFS, ext4 with `casefold`), a request to `/uploads/Shell.PHP` would bypass a pattern of `/uploads/**/*.php`. Use a broad directory pattern like `/uploads/**` (which matches all extensions) when serving from such a filesystem, or normalize uploads to lowercase at write time.

## Fallback Modes

`PHP_DENY_FALLBACK` controls what is returned on a match.

### HTTP Status

Any value in `400`–`599` (default `404`). Pairs with `ERROR_PAGES_DIR` for a custom HTML body:

```bash
PHP_DENY_PATHS="/uploads/**"
PHP_DENY_FALLBACK="403"
ERROR_PAGES_DIR="/var/www/errors"  # serves errors/403.html
```

### PHP Script

A `/`-prefixed URI path to a fallback script inside `DOCUMENT_ROOT`:

```bash
PHP_DENY_PATHS="/uploads/**"
PHP_DENY_FALLBACK="/_security/denied.php"
```

The script is validated at startup — it must exist, canonicalize inside `DOCUMENT_ROOT`, and must not itself match `PHP_DENY_PATHS` (loop prevention; startup aborts otherwise). The script runs with two extra `$_SERVER` keys identifying the original request:

| `$_SERVER` key | Value |
|---|---|
| `OXPHP_DENIED_PATH` | Original sanitized URI (without leading `/`) |
| `OXPHP_DENIED_PATTERN` | The glob pattern that matched |

Example honeypot:

```php
<?php
// /_security/denied.php — runs in place of any matched .php request.
error_log(sprintf(
    "PHP execution denied: path=%s pattern=%s ip=%s ua=%s",
    $_SERVER['OXPHP_DENIED_PATH'] ?? '',
    $_SERVER['OXPHP_DENIED_PATTERN'] ?? '',
    $_SERVER['REMOTE_ADDR'] ?? '',
    $_SERVER['HTTP_USER_AGENT'] ?? '-',
));

http_response_code(404);
echo "Not Found";
```

This lets you decide the response per-request (return 404 to attackers, 403 to authenticated admins, redirect probe scanners to a sinkhole) instead of being limited to one static status.

## No Existence Oracle

Both `Status` and `Script` fallbacks are returned without touching the filesystem. A request to `/uploads/never-uploaded.php` and a request to `/uploads/actually-on-disk.php` produce identical responses — no timing difference, no body difference. An attacker scanning for uploaded shells cannot use the deny-list to enumerate which filenames exist.

## Observability

| Metric | Description |
|---|---|
| `oxphp_php_deny_total` | Counter incremented on every denied request |

Each denial also produces a `tracing::info` log:

```
PHP execution denied by PHP_DENY_PATHS path=uploads/shell.php pattern=uploads/**
```

Access logs record the resulting status (the `PHP_DENY_FALLBACK` value or the fallback script's `http_response_code()`) — denied requests are not distinguished from normal requests at the access-log level. Cross-reference with the metric or the structured log to attribute spikes.

## Performance

Matching is a `globset::GlobSet` lookup — typically a single SIMD pass over the URI bytes. A hit also bypasses the route cache (denied URIs come from attacker-controlled spraying with effectively unbounded cardinality; caching them would let an attacker evict legitimate entries from the LRU). Both the hit and miss paths are alloc-free after warm-up.

## Limitations

Honest list of what this feature does *not* do:

- **PATH_INFO bypass for literal file patterns.** A pattern of `/admin/legacy.php` does not match `/admin/legacy.php/extra`. Use `/admin/legacy.php{,/**}` to cover both, or use a directory pattern.
- **Case-sensitive matching** (see [Case Sensitivity](#case-sensitivity) above).
- **No regex.** Patterns are globs only — anchored, with `*`/`**`/`?`/`[abc]`/`{a,b}` operators. Use multiple comma-separated patterns instead of `(a|b)`.
- **No effect on `include` / `require` / `eval`.** The deny-list governs *direct URI* execution only. A vulnerable script that does `include $_GET['page']` can still load PHP from anywhere readable by the server.

## Deprecated Alias

The legacy `PHP_DENY_DIRS` variable is accepted as a deprecated alias and emits a startup warning:

```
WARN PHP_DENY_DIRS is deprecated, use PHP_DENY_PATHS instead — the alias will be removed in a future release
```

When both are set, `PHP_DENY_PATHS` wins and `PHP_DENY_DIRS` is reported as ignored. Values are not merged.

## See Also

- [Routing](../features/routing.md) — routing modes and path security
- [Error Pages](../features/error-pages.md) — custom HTML bodies for status-fallback responses
- [Configuration Reference](../operations/configuration.md) — full env-var list
- [Metrics](../operations/metrics.md) — `oxphp_php_deny_total` and friends
