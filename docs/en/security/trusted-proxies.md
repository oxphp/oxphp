---
title: Trusted Proxies
description: Configure OxPHP to extract the real client IP, protocol, and host from reverse proxy headers (Forwarded, X-Forwarded-For/Proto/Host).
---

# Trusted Proxies

When OxPHP runs behind a reverse proxy (Kubernetes Ingress, Cloudflare, AWS ALB, nginx), all requests arrive from the proxy's IP address. Without trusted proxy configuration, rate limiting, access logging, and `$_SERVER['REMOTE_ADDR']` all see the proxy IP instead of the real client.

## Configuration

```bash
# Comma-separated CIDR list
TRUSTED_PROXIES="10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"

# Shorthand: all RFC-1918 + loopback + link-local (IPv4 and IPv6)
TRUSTED_PROXIES="private"
```

When unset, OxPHP ignores all forwarding headers — this is the safe default.

## How It Works

When a request arrives from a trusted IP, OxPHP inspects forwarding headers in priority order:

1. **`Forwarded`** ([RFC 7239](https://www.rfc-editor.org/rfc/rfc7239)) — the standardized header
2. **`X-Forwarded-For` / `X-Forwarded-Proto` / `X-Forwarded-Host`** — de-facto fallback

If the `Forwarded` header is present, `X-Forwarded-*` headers are ignored.

### Client IP Extraction

OxPHP uses the **rightmost-non-trusted** algorithm — the same approach used by nginx (`real_ip_recursive on`), Caddy, Traefik, and Apache:

```
X-Forwarded-For: 203.0.113.50, 172.16.1.1, 10.0.0.5
TCP peer: 10.0.0.1 (trusted)

Walk right-to-left:
  10.0.0.5    → trusted → skip
  172.16.1.1  → trusted → skip
  203.0.113.50 → NOT trusted → client IP
```

This prevents spoofing via prepended values — an attacker can add fake IPs to the left, but the rightmost untrusted IP was set by the last trusted proxy in the chain.

## What Changes

When `TRUSTED_PROXIES` is configured and the connecting IP is trusted:

| Component | Without trusted proxies | With trusted proxies |
|-----------|------------------------|---------------------|
| `$_SERVER['REMOTE_ADDR']` | Proxy IP | Real client IP |
| `$_SERVER['HTTPS']` | Based on OxPHP's TLS config | From `Forwarded: proto=` or `X-Forwarded-Proto` |
| `$_SERVER['REQUEST_SCHEME']` | `http` or `https` from TLS | From forwarded protocol |
| `$_SERVER['SERVER_NAME']` | From `Host` header | From `Forwarded: host=` or `X-Forwarded-Host` |
| `$_SERVER['SERVER_PORT']` | From `Host` header | From forwarded host |
| Rate limiting | Per-proxy IP | Per-client IP |
| Access log | Proxy IP | Real client IP |

## `private` Networks

The `private` shorthand includes:

| Network | Description |
|---------|-------------|
| `10.0.0.0/8` | Class A private |
| `172.16.0.0/12` | Class B private |
| `192.168.0.0/16` | Class C private |
| `127.0.0.0/8` | Loopback |
| `169.254.0.0/16` | Link-local |
| `::1/128` | IPv6 loopback |
| `fc00::/7` | IPv6 unique local |
| `fe80::/10` | IPv6 link-local |

## Security

- **Safe default** — without `TRUSTED_PROXIES`, no forwarding headers are processed
- **CIDR validation** — invalid values in `TRUSTED_PROXIES` cause a startup error
- **Spoofing resistance** — the rightmost-non-trusted algorithm ignores attacker-prepended values
- Requests from untrusted IPs have their forwarding headers ignored entirely

## See Also

- [Rate Limiting](../features/rate-limiting.md) — per-IP rate limiting uses the resolved client IP
- [Access Logging](../features/access-logging.md) — `remote_addr` field shows the resolved client IP
- [Configuration Reference](../operations/configuration.md) — all environment variables
