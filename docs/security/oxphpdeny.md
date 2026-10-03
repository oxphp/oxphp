---
title: Path Deny Rules (.oxphpdeny)
description: Keep files in DOCUMENT_ROOT from being served — or hand their requests to the application — with a gitignore-style file read at startup.
---

# Path Deny Rules (`.oxphpdeny`)

A `.oxphpdeny` file at the top of `DOCUMENT_ROOT` lists request paths the server must not serve directly. It is written in `.gitignore` syntax with one addition: a rule prefixed with `>` hands the request to the application's entry script instead of denying it.

```gitignore
# Never served — answered with PHP_DENY_FALLBACK (404 by default)
/vendor
/composer.*
*.sql
*.bak

# Not served directly — the front controller decides
> /storage/invoices/
```

It is meant for files a document root should not hold but often does: `vendor/` and `composer.json` in a legacy application served from its project root, database dumps and backups, build artefacts and source maps in a framework's `public/`. The rules apply in all four routing modes, to static files and PHP scripts alike, and they are checked before any disk access, so a denied path gets the same answer whether or not the file exists.

The file is read once, at startup; edits take effect on the next start. Without the file nothing changes. The file itself is never served: like every path segment starting with `.` other than a leading `/.well-known/`, it is refused before routing.

## Syntax

What `.gitignore` supports, `.oxphpdeny` supports:

- Blank lines and lines starting with `#` are skipped. A UTF-8 byte-order mark and CRLF line endings are accepted.
- Trailing spaces are dropped unless escaped (`\ `).
- `!` re-includes what an earlier rule excluded. `\#` and `\!` at the start of a line are a literal `#` and `!`.
- A trailing `/` makes the rule match directories only.
- A `/` at the start or in the middle anchors the pattern to the top of `DOCUMENT_ROOT`. Without one, the pattern matches at any depth: `vendor` covers `/vendor` and `/lib/vendor`.
- `*` and `?` stay within one path segment; `[abc]`, `[a-z]`, `[!0-9]` and `[^0-9]` are character classes. As in git, `?` and a class match a single byte, and a class never matches `/`: `[!-]` is any byte of a name but `-`. A letter outside ASCII takes two or more bytes, so only `*` stands for one — `/caf*.txt`, not `/caf?.txt`. POSIX classes, backslash escapes, characters outside ASCII, a `-` right after a range and a range that runs backwards are not supported inside a class, and a negated class in a deny or `>` rule cannot name an ASCII letter — see below.
- `**/` at the start, `/**` at the end and `/**/` in the middle match across directories; `**` anywhere else is a plain `*`. A longer run of `*` counts as `**`, as in git: `***/secret.txt` covers `/secret.txt` at any depth.
- The last matching rule wins, and nothing below an excluded directory can be re-included: with `private/` excluded, `!private/ok.txt` has no effect. To exclude a directory's contents but keep some, exclude `/private/*` and re-include `!/private/ok.txt`.

### `>` rules: hand the request to the application

```gitignore
> /storage/invoices/
> *.map
```

A request matching a `>` rule is not served from disk; it goes to the entry script — the front controller (`ENTRY_FILE=index.php`) or the worker — with `REQUEST_URI` intact, exactly as a request for a missing file would. The application can check a session and stream the file itself. Spaces and tabs between `>` and the pattern are allowed; `\>` is a literal `>`. A `>` rule cannot be negated (`!>x` is an error), and `>` alone is an error.

A deny rule still applies below a `>` directory: with `> /storage/invoices/` and `*.sql`, `/storage/invoices/dump.sql` is denied, whichever line comes first. A `!` rule that lifts the deny leaves the file to the `>` directory, as in git: add `!/storage/invoices/dump.sql` after `*.sql` and the entry script gets that request. `!` re-includes nothing below a `>` directory, as below any excluded one, so the file is still not served from disk.

`>` rules need an entry script: in Traditional and SPA modes they stop startup.

### Differences from `.gitignore`

- **A malformed line stops startup**, with the file path and line number — git skips such lines silently. So does a file that is not UTF-8.
- **`{a,b}` is literal.** Braces match themselves, as in git.
- **A class that names `/` stops startup** — `[-/]`, or a range across it such as `[+-0]`. git accepts it and never matches `/` with it; write the separator outside the class.
- **A POSIX class, a backslash inside a class, or a `-` right after a range stops startup** — `[[:digit:]]`, `[\]]`, `[a-b-c]`. git reads each its own way; write the range instead (`[0-9]`), put `]` first to match it, and put a literal `-` first or last (`[]-]`, `[-a-c]`).
- **A range that runs backwards stops startup** — `[z-a]`, `[a--]`. git reads it as its first character alone, so `x[z-a]` covers only `xz`; write that character, or put the lower end first.
- **A class holding a character outside ASCII stops startup** — `[äÄ]`. git matches a class against one byte, as the server does, and such a character takes two or more: `/[äÄ]dmin` would cover neither `/ädmin` nor `/Ädmin`, and `/[äÄ]*` would cover `/öffnungszeiten`, which starts with the same byte. Put `*` in its place.
- **An escaped `/` stops startup** — `**\/secret.txt`, `\/vendor`. git reads `**\/secret.txt` as `secret.txt` below at least one directory — not `/secret.txt` itself — which the server would not, and `\/vendor` matches no path at all. A separator never needs the escape: write `/`.
- **A line that covers other than it reads stops startup** — one that starts with a space or tab, ends in an unescaped tab, has `//` anywhere, or holds a NUL byte. git accepts each. Whitespace at either end becomes part of the name: ` > /storage/invoices/`, indented, is a deny rule for a path starting with a space rather than a `>` rule. Escape it with `\` when a name really starts or ends with it. `//vendor` and `vendor//` match no path at all. git ends a pattern at a NUL, so `vendor`, a NUL and more text cover `/vendor` in git, where the server would read the whole line and cover only a name holding that NUL. A file saved as UTF-16 has a NUL after every ASCII character: save it as UTF-8.
- **A line of spaces and tabs is blank.** git reads a line holding a tab as a pattern, for a name made of that whitespace.
- **Deny and `>` rules ignore the case of ASCII letters; `!` rules do not.** The server runs `/uploads/shell.PHP` as a PHP script on any filesystem, and a case-insensitive one (macOS, Windows) serves `/VENDOR/autoload.php` from `vendor/`, so a rule that matched one spelling would let the others through: `*.php` covers `shell.PHP`, and `vendor/` covers `/Vendor/`. A `!` rule re-includes only the spelling it names — with `*` and `!/index.php`, `/INDEX.PHP` stays denied — so ignoring case never lets the server serve from disk a path that exact matching would have kept from it. To keep it so, a negated class in a deny or `>` rule cannot name an ASCII letter, and one that does stops startup: ignoring case would make `[!i]` leave out `I` as well, and `/[!i]*.php` would run `/Info.php`. Match any character there and re-include the exception, which a `!` rule then matches exactly: `/*.php` and `!/i*.php`. git matches case-sensitively unless `core.ignorecase` is set.
- **A few letters outside ASCII are read as ASCII.** A case-insensitive filesystem (macOS) reads them so, and serves `license.txt` for `/licenſe.txt`: `ſ` (long s) as `s`, `ß` and `ẞ` as `ss`, the Kelvin sign (U+212A) as `k`, the ligatures `ﬀ` `ﬁ` `ﬂ` `ﬃ` `ﬄ` `ﬅ` `ﬆ` as the letters they join, and the Greek question mark (U+037E) and varia (U+1FEF) as `;` and `` ` ``. Deny and `>` rules read them so too, in the request and in the rule: `/license.txt` covers `/licenſe.txt`, and `/straße/` covers `/strasse/`. A rule still covers what it covered as written, and `!` rules read these letters only as written, so a denial only widens.
- **Other letters outside ASCII match only as written, and so does each Unicode form of a name.** A rule for `/ädmin` does not cover `/Ädmin`, and a rule for `café.txt` written with a composed `é` does not cover `cafe` followed by a combining accent (U+0301). Most Linux filesystems keep each spelling a separate file, so this matters on macOS, which treats both pairs as one file — a bind mount from macOS into Docker Desktop included: there, a request for the other spelling is served from disk. Put `*` in place of such letters (`/caf*.txt`) — `?` and a class match a single byte — or keep those files out of `DOCUMENT_ROOT`.
- **At most 256 rules, each at most 128 bytes.** A rule past either limit stops startup with its line number — see [Limits](#limits).
- **One file, at the top of `DOCUMENT_ROOT`, read at startup.** Files in subdirectories are not read. It must be a regular file of at most 1 MiB — a symlink to one is fine. A FIFO or a device stops startup unread; a larger file stops it once its first MiB is read.

## What Is Matched

Rules match the request path after percent-decoding and normalization: empty segments are dropped, so `//vendor/x`, `/vend%6Fr/x` and `/vendor%2Fx` are all `/vendor/x`. A segment starting with `.` — `.`, `..`, `.git` — never reaches the rules: the request is refused with a plain `404` first, without `PHP_DENY_FALLBACK` and without being counted. The exception is a leading `/.well-known/` ([RFC 8615](https://www.rfc-editor.org/rfc/rfc8615)), which [dot-path blocking](dot-path-blocking.md) lets through and the rules see like any other path. A request is a directory request when its decoded path ends with `/` (`/admin/`, `/admin%2F`, `/admin//`); a trailing-slash rule matches only those, and everything below them:

| Rule | `/admin` | `/admin/` | `/admin/x` | `/admin/x/y` |
|---|---|---|---|---|
| `admin/` | — | denied | denied | denied |
| `admin` or `/admin` | denied | denied | denied | denied |
| `admin/*`, `admin/x` | — | — | denied | denied |

The bare `/` is never matched, so an allowlist-style file still serves the site root:

```gitignore
*
!/index.php
!/assets/
!/assets/**
!/.well-known/
!/.well-known/**
```

Keep the `/.well-known/` lines if the site renews its TLS certificate over ACME HTTP-01: without them, `/.well-known/acme-challenge/<token>` is denied.

Two other readings of a request are judged too, and a denial under either stands. A directory request is also judged as the file it names: the router drops the trailing slash and may serve `/secret.txt/` as the file `secret.txt`. In Traditional mode, a `.php` segment followed by more path is judged as the script it names: `/secret.php/x.js` runs `secret.php` with PATH_INFO `/x.js`, so it is denied wherever `/secret.php` is. Either way, `!*/` — the gitignore way to re-include every directory — cannot bring a denied file back. In an allowlist, re-include a directory without the slash (`!/docs`) to keep `/docs/` itself reachable. No other file the router resolves a request to is judged — see the next section.

### The Request Path, Not the File

Rules see the path the client asked for, not the file the router resolves it to. A request that resolves to a different file is judged by its own path:

- `/admin` with the rule `admin/`: in Traditional mode the router answers `/admin` with `admin/index.php`, and `/admin` is not a directory request. **To close a directory, write it without the trailing slash** (`/admin`).
- `/uploads/` with the rule `uploads/*.php`: the directory index `uploads/index.php` runs, because the request path is `/uploads/`.
- `/x.php/y.png` (PATH_INFO) runs `x.php` although the path ends in `.png`. The script is named in the path, so Traditional mode judges the request as `/x.php` too — see above.

To stop `.php` execution in writable directories, use [`PHP_DENY_PATHS`](php-deny.md) as well: it also checks the script a request resolves to.

## Routing Modes

| Routing mode | Deny rules | `>` rules |
|---|---|---|
| Traditional (no `ENTRY_FILE`) | Yes | Startup error — there is no entry script |
| SPA (`ENTRY_FILE=index.html`) | Yes | Startup error — `index.html` cannot take a request |
| Framework (`ENTRY_FILE=index.php`) | Yes | Front controller |
| Worker (`WORKER_MODE_ENABLED=true`) | Yes, with a status-code fallback | Worker entry |

## Fallback

A deny rule answers with [`PHP_DENY_FALLBACK`](php-deny.md#fallback-modes), the setting `PHP_DENY_PATHS` uses: an HTTP status (`404` by default; pairs with `ERROR_PAGES_DIR`) or a PHP script inside `DOCUMENT_ROOT`. The script receives `OXPHP_DENIED_PATH` (the request path, with a leading `/`) and `OXPHP_DENIED_PATTERN` (the rule as written in the file, e.g. `/composer.*`).

The script may sit under a path the file denies — `/_security/` hides it from direct requests, and a direct request for it is answered by the script itself, as a denial. In worker mode the fallback must be a status code: a worker thread runs nothing but the worker entry. `PHP_DENY_FALLBACK` is read only when the file has at least one deny rule (or `PHP_DENY_PATHS` is in effect, in Traditional and SPA modes).

When both `.oxphpdeny` and `PHP_DENY_PATHS` match a request, `.oxphpdeny` answers it.

## Observability

- **Metric:** `oxphp_path_deny_total` counts requests denied by a rule. `>` rules are routing, not denials, and are not counted. `oxphp_php_deny_total` keeps counting `PHP_DENY_PATHS` only; a request both deny is counted once, here.
- **`/config`:** the `deny_file` entry gives the rule counts by action and the fallback kind, never the rules — see [Internal Server](../features/internal-server.md).
- **Startup log:** `loaded 5 rules (3 deny, 1 entry, 1 allow) from /var/www/html/public/.oxphpdeny`. With `LOG_LEVEL=debug`, one `oxphpdeny rule` line per rule shows how it was read: line number, action, the pattern as written and the glob it compiled to.
- **Per request:** each denial logs `request denied by .oxphpdeny` at `debug`, with the path, the rule and its line. A request refused for its depth and length logs `request refused: path too deep or too long to match against .oxphpdeny` at `debug`, with its segment count and length; it is not counted.

## Startup Errors

| Condition | Result |
|---|---|
| No `.oxphpdeny` | Feature off; nothing changes |
| File unreadable or not UTF-8, or a symlink whose target is missing | Startup error |
| Malformed line | Startup error with `<path>: line N: <message>` |
| More than 256 rules, or a pattern longer than 128 bytes | Startup error with the line number |
| `>` rule in Traditional or SPA mode | Startup error |
| Worker mode, a deny rule, and a script `PHP_DENY_FALLBACK` | Startup error |
| Invalid `PHP_DENY_FALLBACK` and at least one deny rule | Startup error |
| Empty file, comments only, or only `!` rules | Nothing to deny: routing is unchanged, depth limit included; `/config` shows the file as loaded with its rule counts |

`oxphp config --check` reports the file errors before a deploy (`config: INVALID`); the fallback checks run when the server builds its routing, at startup.

## Limits

- **Depth and length.** With at least one deny or `>` rule, a request path deeper than 64 segments, or whose segment count times its length passes 128 KiB, is answered `404` without being matched: matching re-checks every parent directory and scans each one's path, so its cost would otherwise grow with both. A path of up to 2 KiB is refused only for its depth; past that the budget is spent by 16 segments at 8 KiB, or 3 at about 43 KiB.
- **Rules.** At most 256 rules, each pattern at most 128 bytes, not counting a leading `!` or `>`. A request path is matched against the rules one by one, and against each parent directory too, so these limits and the one above bound what a request costs. A typical file takes well under a millisecond a request. A file at both limits built to be slow — 256 long rules starting `/x/**/*`, against paths made to almost match them — takes about 40 ms a request and keeps about 50 MB of match state per Tokio worker thread.
- **Case sensitivity** — see [Differences from `.gitignore`](#differences-from-gitignore).
- **No reload.** Restart to apply an edit.

## Examples

A legacy application served from its project root:

```gitignore
/vendor
/composer.*
/config
*.sql
*.sql.gz
*.bak
*.log
```

WordPress:

```gitignore
/readme.html
/license.txt
/wp-config-sample.php
/wp-content/debug.log
```

Downloads behind a login, in Framework mode — the front controller gets `/storage/invoices/2026-07.pdf`, checks the session and streams the file:

```gitignore
> /storage/invoices/
```

## See Also

- [PHP Execution Deny-List](php-deny.md) — `PHP_DENY_PATHS` and `PHP_DENY_FALLBACK`
- [Routing](../features/routing.md) — routing modes and path security
- [Metrics](../operations/metrics.md) — `oxphp_path_deny_total`
