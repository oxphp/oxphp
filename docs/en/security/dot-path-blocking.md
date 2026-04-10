---
title: Dot-Path Blocking
description: OxPHP blocks access to hidden files and directories (starting with ".") to prevent exposure of sensitive configuration, version control data, and other dotfiles.
---

# Dot-Path Blocking

OxPHP blocks access to any URL path containing a segment that starts with `.` — returning 404 Not Found. This prevents exposure of sensitive files like `.env`, `.git/`, `.htaccess`, `.svn/`, and `.DS_Store` that are commonly targeted by automated scanners.

This protection is always on and cannot be disabled. It applies to all routing modes (traditional, framework, SPA, and worker).

## What Is Blocked

Any request where a path segment starts with `.` returns 404:

| Request | Result |
|---------|--------|
| `/.env` | 404 |
| `/.git/config` | 404 |
| `/.htaccess` | 404 |
| `/.DS_Store` | 404 |
| `/.docker/config.json` | 404 |
| `/path/.hidden/file.txt` | 404 |
| `/path/to/.env` | 404 |

Percent-encoded bypasses are caught — `/%2egit/HEAD` decodes to `/.git/HEAD` and is blocked.

## `.well-known` Exception

[RFC 8615](https://www.rfc-editor.org/rfc/rfc8615) defines `/.well-known/` as a standard location for site-wide metadata. OxPHP allows access to sub-paths within `.well-known`, with restrictions:

| Request | Result |
|---------|--------|
| `/.well-known/security.txt` | Served as a static file |
| `/.well-known/acme-challenge/token` | Served as a static file (Let's Encrypt) |
| `/.well-known/openid-configuration` | Served if file exists, otherwise INDEX_FILE fallback or 404 |
| `/.well-known` | 404 (bare path) |
| `/.well-known/` | 404 (directory listing) |
| `/.well-known/test.php` | 404 (PHP execution blocked) |

PHP files inside `.well-known` are never executed — they always return 404. This directory serves static content only.

MIME types are determined by file extension. Files without an extension (like `openid-configuration`) are served as `application/octet-stream`.

## See Also

- [Routing](../features/routing.md) — routing modes and path security
