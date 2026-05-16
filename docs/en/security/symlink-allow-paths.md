---
title: Symlink Allow Paths
description: Explicit allow-list for symlink targets outside DOCUMENT_ROOT — Laravel-style storage links, shared asset volumes, multi-tenant uploads.
---

# Symlink Allow Paths

By default OxPHP refuses any request that resolves to a path outside the canonical `DOCUMENT_ROOT`. A symlink inside `DOCUMENT_ROOT` pointing at an external directory returns 404 and the path resolution logs `Blocked request: resolved path escapes document root`.

This is the right default — it stops directory traversal, symlink-swap TOCTOU attacks, and accidentally exposing config files or secrets sitting one level up. But it also blocks legitimate patterns that frameworks have been using for years: Laravel's `php artisan storage:link`, Symfony asset bundles, shared upload volumes mounted into multiple containers.

`SYMLINK_ALLOW_PATHS` is the explicit opt-in: you list the filesystem paths that symlinks under `DOCUMENT_ROOT` are permitted to resolve to. Anything not on the list keeps the strict 404 behaviour.

## Configuration

```bash
# Absolute paths, comma-separated
SYMLINK_ALLOW_PATHS=/var/www/storage,/opt/shared/assets

# Relative paths resolve against DOCUMENT_ROOT
SYMLINK_ALLOW_PATHS=../storage,../shared/uploads

# Mixed
SYMLINK_ALLOW_PATHS=/opt/shared/cdn,../storage/app/public
```

When unset (the default), no symlink can leave `DOCUMENT_ROOT`.

## Laravel Example

```bash
DOCUMENT_ROOT=/app/public
SYMLINK_ALLOW_PATHS=../storage/app/public
```

Then `php artisan storage:link` creates `public/storage -> ../storage/app/public` inside the project. URLs to `/storage/<file>` resolve through the symlink to `/app/storage/app/public/<file>`, which canonicalises to a path the allow-list authorises. No code change in the application.

## How It Works

At startup, each entry is resolved:

- **Absolute entries** — checked against the blacklist (see below), then passed through `realpath(3)` (i.e. `std::fs::canonicalize`). If `realpath` fails (target doesn't exist) the server refuses to start.
- **Relative entries** — joined with the canonical `DOCUMENT_ROOT`, then `realpath`'d.

The resulting canonical paths are stored as the allow-list. Duplicates are silently deduped.

At request time, the routing layer canonicalises the resolved file path and verifies it satisfies one of:

1. lives inside `DOCUMENT_ROOT`, or
2. equals exactly one of the allow-list entries (file targets), or
3. starts with one of the allow-list entries followed by `/` (directory targets).

The same check runs a second time as a TOCTOU guard inside the static-file serve path, after the route cache, before any read syscall.

## Blacklist

A small set of paths can never appear in `SYMLINK_ALLOW_PATHS` — typos and misunderstandings would otherwise widen the attack surface dramatically. The server refuses to start if any entry resolves to a blacklisted path.

**Forbidden as exact match:**

```
/   /etc   /proc   /sys   /dev   /var   /home   /tmp   /root   /usr
```

**Forbidden as prefix** (entry lies under one of these directories):

```
/etc   /proc   /sys   /dev   /tmp   /root   /usr
```

Note that `/var` and `/home` are exact-only — bare `/home` is rejected, but `/home/<any>/...` is allowed, just as `/var/www/storage` or `/var/lib/myapp/uploads` are allowed. Entries are checked twice: once against the raw admin-supplied path (so that macOS-style `/etc -> /private/etc` cannot launder a blacklisted path through `realpath`), once against the canonical form (defense-in-depth for symlink-target escapes).

## Failure Modes

| Configuration error | Result |
|---|---|
| Entry target does not exist on disk | Server refuses to start, error names the entry and `canonicalize` |
| Entry matches the blacklist (raw or canonical) | Server refuses to start, error names the entry and the blacklist rule |
| Empty/whitespace-only env var | Treated as unset — strict default behaviour |
| Duplicate entries | Deduped silently after canonicalisation |
| No symlink exists yet at startup | Allow-list is registered but inert until a symlink appears; no startup check requires the symlink |

## Security Notes

- The allow-list is opt-in — the safe default of "no escapes" is preserved when the variable is unset
- Entries are canonicalised at startup, so `..` and intermediate symlinks in the entry path are collapsed before storage
- Runtime canonicalisation closes the symlink-swap TOCTOU window — the validated path is the path that gets read
- File targets match exactly; directory targets match by directory prefix. Listing a file at `/etc/passwd` would still be rejected by the blacklist, but more generally: listing a single file at `/opt/shared/license.key` does not implicitly grant access to siblings
- Path validation results are cached per requested URL (one `realpath` per unique URL until eviction), so the runtime cost is amortised away

## See Also

- [PHP Deny Paths](php-deny.md) — block PHP execution under specific URI globs; orthogonal to symlink policy
- [Dot Path Blocking](dot-path-blocking.md) — refuses `.well-known`-style traversal and dotfile leaks
- [Trusted Proxies](trusted-proxies.md) — separate trust boundary for `X-Forwarded-*` headers
- [Configuration Reference](../operations/configuration.md) — all environment variables
