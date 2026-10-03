---
title: Symlink Allow Paths
description: Explicit allow-list for symlink targets outside DOCUMENT_ROOT — Laravel-style storage links, shared asset volumes, multi-tenant uploads.
---

# Symlink Allow Paths

By default OxPHP refuses any request that resolves to a path outside the canonical `DOCUMENT_ROOT`. A symlink inside `DOCUMENT_ROOT` pointing at an external directory returns 404 and the path resolution logs `Blocked request: resolved path escapes document root`.

This is the right default — it stops directory traversal and accidentally exposing config files or secrets sitting one level up. But it also blocks legitimate patterns that frameworks have been using for years: Laravel's `php artisan storage:link`, Symfony asset bundles, shared upload volumes mounted into multiple containers.

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

For static files the check runs a second time at serve time, on the file actually opened and before any byte of it is read. The location is taken from the open descriptor (`/proc/self/fd/N` on Linux, `F_GETPATH` on macOS), so a symlink swapped after the routing check — whose result is cached — is still refused. Where the descriptor's path is unavailable, the path is resolved again and must name the same file that was opened. A file answered from the in-memory cache is not reopened, so a swapped link keeps returning the bytes cached from its earlier, validated target until the entry is evicted (or, with `STATIC_REVALIDATE=on`, until revalidation notices the change).

PHP scripts get the same treatment in every mode except Worker mode (whose entry file is an administrator-configured path). The worker opens the script itself, checks the location of the open descriptor in the same way, and hands the engine that open file rather than the path, so the file that is opened is the file that was checked, except for one case: a script whose resolved name contains `.phar` and which is a zip, tar or compressed phar archive is opened a second time by name by the phar extension, and that second open is not covered by the check. A path that is not a regular file — a directory, a FIFO — answers `500` and logs `Cannot open script`, as does one that exists but cannot be read; PHP's own failed-to-open warning and fatal error are not produced for it. A script deleted since routing answers `404`, including one that OPcache holds and, with `opcache.validate_timestamps` off, used to run from the cache without the file being opened. A link repointed after routing — to a script, or to a text file the engine would print as is — answers `404` and logs `Blocked request: opened script escapes document root`; a link repointed to another script inside `DOCUMENT_ROOT` (or to an allow-listed target) keeps working. `__FILE__`, `__DIR__` and `$_SERVER['SCRIPT_FILENAME']` are the same as without the check. Three things follow from the engine being handed the resolved location of the file: OPcache keeps a script reached through a link under the file the link resolves to, the script's working directory is the directory of that file — for a link to a file in another directory, not the directory the link sits in — and the phar extension decides by that resolved name whether the script is a phar: a link named `app.phar.php` that points at a zip-based phar whose own name has no `.phar` is compiled as a plain file, and a link without `.phar` in its name that points at a phar whose name has it runs as one.

The check covers the script a request was routed to. Files that script pulls in with `include` or `require`, or opens by name, and `auto_prepend_file`, are resolved by PHP as usual; where scripts or their directories are writable by code you do not trust, add `open_basedir` as well. It applies to the script itself too: one outside it is refused the way PHP refuses any script it cannot open — status `500`, with the usual `open_basedir restriction in effect` warnings. The check is made when PHP compiles the script, where PHP makes it for a script it opens itself: after `auto_prepend_file` has run, with the script's directory as the working directory. A relative entry such as `open_basedir=.` therefore means the script's directory, and an `open_basedir` that the prepended file narrows with `ini_set()` applies to the script it prepends to, including one OPcache already holds, which PHP would have run without checking.

## Blacklist

A small set of paths can never appear in `SYMLINK_ALLOW_PATHS` — typos and misunderstandings would otherwise widen the attack surface dramatically. The server refuses to start if any entry resolves to a blacklisted path.

**Forbidden as exact match:**

```
/   /etc   /proc   /sys   /dev   /var   /home   /tmp   /root   /usr   /srv
```

**Forbidden as prefix** (entry lies under one of these directories):

```
/etc   /proc   /sys   /dev   /tmp   /root   /usr
```

Note that `/var`, `/home`, and `/srv` are exact-only — bare `/srv` is rejected, but `/srv/myapp/storage` is allowed, just as `/var/www/storage` and `/home/<any>/...` are allowed. Entries are checked twice: once against the raw admin-supplied path (so that macOS-style `/etc -> /private/etc` cannot launder a blacklisted path through `realpath`), once against the canonical form (defense-in-depth for symlink-target escapes).

The blacklist itself is hardcoded — there is no env var to extend it. The default is the conservative minimum that catches typo-level mistakes; admins who need stricter policies should layer them outside (filesystem permissions, container mount restrictions, AppArmor/SELinux profiles).

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
- Static files and PHP scripts are checked on the opened file, so — on Linux with `/proc` mounted, and on macOS — swapping a symlink after it has been served once cannot make a later response come from, or run a script from, outside the allowed locations; the fallback used elsewhere is narrower, as described above
- File targets match exactly; directory targets match by directory prefix. Listing a file at `/etc/passwd` would still be rejected by the blacklist, but more generally: listing a single file at `/opt/shared/license.key` does not implicitly grant access to siblings
- Routing-time validation is cached per requested URL (one `realpath` per unique URL until eviction); the serve-time check adds a lookup of the open descriptor's location for each static file read from disk, and nothing for a file served from the in-memory cache. For a PHP script outside Worker mode it adds about eight system calls to every request (counted with `strace` on PHP 8.4): an `open` and a `close` of the script, that lookup, and `fstat`, `fcntl` and `lseek` calls on the descriptor — including a request OPcache answers, which would otherwise not touch the file when `opcache.validate_timestamps` is off. Measured on a script that echoes one word, that is about 3% of throughput; a real application spends proportionally less of its request on it

## See Also

- [PHP Deny Paths](php-deny.md) — block PHP execution under specific URI globs; orthogonal to symlink policy
- [Dot Path Blocking](dot-path-blocking.md) — refuses `.well-known`-style traversal and dotfile leaks
- [Trusted Proxies](trusted-proxies.md) — separate trust boundary for `X-Forwarded-*` headers
- [Configuration Reference](../operations/configuration.md) — all environment variables
