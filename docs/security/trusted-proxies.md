---
title: Trusted Proxies
description: Configure OxPHP to extract the real client IP, protocol, and host from reverse proxy headers (Forwarded, X-Forwarded-For/Proto/Host, and CF-Connecting-IP behind Cloudflare).
---

# Trusted Proxies

When OxPHP runs behind a reverse proxy (Kubernetes Ingress, Cloudflare, AWS ALB, nginx), all requests arrive from the proxy's IP address. Without trusted proxy configuration, rate limiting, access logging, and `$_SERVER['REMOTE_ADDR']` all see the proxy IP instead of the real client.

## Configuration

```bash
# Comma-separated CIDR list
TRUSTED_PROXIES="10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"

# Shorthand: all RFC-1918 + loopback + link-local (IPv4 and IPv6)
TRUSTED_PROXIES="private"

# Keywords and CIDRs combine in one list
TRUSTED_PROXIES="private,cloudflare,100.64.0.0/10"
```

Each comma-separated entry is a CIDR, a bare IP address (a single host), or one of the keywords `private` and `cloudflare`, in any letter case. An entry that is none of these is a startup error.

When unset, OxPHP ignores all forwarding headers — this is the safe default.

## How It Works

When a request arrives from a trusted IP, OxPHP inspects forwarding headers in priority order:

1. **`Forwarded`** ([RFC 7239](https://www.rfc-editor.org/rfc/rfc7239)) — the standardized header
2. **`X-Forwarded-For` / `X-Forwarded-Proto` / `X-Forwarded-Host` / `X-Forwarded-Port`** — de-facto fallback

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

With the `cloudflare` keyword, a request whose forwarding chain shows it arrived from a Cloudflare edge takes its client address from the `CF-Connecting-IP` header instead — see [Behind a CDN and a Platform Load Balancer](#behind-a-cdn-and-a-platform-load-balancer).

## What Changes

When `TRUSTED_PROXIES` is configured and the connecting IP is trusted:

| Component | Without trusted proxies | With trusted proxies |
|-----------|------------------------|---------------------|
| `$_SERVER['REMOTE_ADDR']` | Proxy IP | Real client IP |
| `$_SERVER['REMOTE_PORT']` | Proxy source port | Client port from `Forwarded: for=ip:port`, otherwise `0` |
| `$_SERVER['HTTPS']` | Based on OxPHP's TLS config | From `Forwarded: proto=` or `X-Forwarded-Proto` |
| `$_SERVER['REQUEST_SCHEME']` | `http` or `https` from TLS | From forwarded protocol |
| `$_SERVER['SERVER_NAME']` | From `Host` header | From `Forwarded: host=` or `X-Forwarded-Host` |
| `$_SERVER['SERVER_PORT']` | From `Host` header | From `X-Forwarded-Port`, else port of `X-Forwarded-Host` / `Forwarded: host=`, else 443/80 |
| Rate limiting | Per-proxy IP | Per-client IP |
| Access log | Proxy IP | Real client IP |

`REMOTE_PORT` is `0` behind a proxy unless an RFC 7239 `Forwarded: for=ip:port` node carries the client's source port — `X-Forwarded-For` has no port field, so the rewritten value cannot be reconstructed and is zeroed instead of guessed. An address taken from `CF-Connecting-IP` has no port either, so `REMOTE_PORT` is `0` then too.

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

Shared address space (`100.64.0.0/10`, RFC 6598) is not part of `private`. If your load balancer connects from it, list it next to the keyword: `TRUSTED_PROXIES="private,100.64.0.0/10"`.

## `cloudflare` Networks

The `cloudflare` keyword adds the edge ranges Cloudflare publishes at [cloudflare.com/ips](https://www.cloudflare.com/ips/) — 15 IPv4 and 7 IPv6 ranges, as published on 2026-10-05. The list is compiled into OxPHP and changes only with an OxPHP release. If Cloudflare publishes a range the keyword does not have yet, list it next to the keyword: `TRUSTED_PROXIES="private,cloudflare,<new range>"`. A listed range counts as one of your own proxies rather than as a Cloudflare edge, so requests through it take their address by the rightmost-non-trusted rule, not from `CF-Connecting-IP`, until a release adds the range to the keyword. A range Cloudflare withdraws cannot be subtracted from the keyword; to drop one before an OxPHP release does, list the ranges you still trust as CIDRs instead of using the keyword — which also gives up reading `CF-Connecting-IP`, since that is tied to the keyword.

## Behind a CDN and a Platform Load Balancer

A common deployment puts a CDN in front of a hosting platform's load balancer:

```
client → Cloudflare → load balancer → OxPHP
```

With `TRUSTED_PROXIES=private`, only the load balancer is trusted. The rightmost untrusted address in `X-Forwarded-For` is then the Cloudflare edge that connected to the load balancer, and every request — in `REMOTE_ADDR`, the access log, rate limiting and the `client.address` span attribute — appears to come from Cloudflare. Trust both hops:

```bash
TRUSTED_PROXIES="private,cloudflare"
```

With the `cloudflare` keyword, OxPHP resolves the client address in three steps:

1. Starting at the connecting peer, it walks left through the forwarding chain (`Forwarded` if present, otherwise `X-Forwarded-For`), past your own proxies — the `private` ranges and the CIDRs you listed, except that an address inside Cloudflare's ranges counts as Cloudflare even when you listed it.
2. If the first hop that is not one of your own proxies is a Cloudflare address, the request came through Cloudflare, and the client address is the `CF-Connecting-IP` header. The header must appear exactly once and hold a valid IP address.
3. Otherwise — the request did not come through Cloudflare, or `CF-Connecting-IP` is missing, repeated or malformed — the rightmost-non-trusted rule applies, with the Cloudflare ranges counted as trusted.

The hop where the walk stops is the connecting peer or an address one of your own proxies recorded, so a client that reaches the load balancer directly cannot make OxPHP read a `CF-Connecting-IP` it sent itself: the load balancer records the client's own address there, and a Cloudflare address the client wrote further left is never reached. Like the rightmost-non-trusted rule, this relies on your proxies maintaining the header OxPHP reads — appending the address that connected to them instead of passing a client's value through unchanged.

That includes `Forwarded`. OxPHP reads it in preference to `X-Forwarded-For`, and Cloudflare passes it on as the visitor sent it, since it is not one of the headers Cloudflare sets. A load balancer that maintains only `X-Forwarded-For` therefore has to remove `Forwarded` from the request; if it passes the header through, any client, through Cloudflare or not, chooses the address OxPHP sees, with or without the `cloudflare` keyword.

`CF-Connecting-IP` is not read either when the walk reaches a hop whose address is missing or does not parse. A `Forwarded` or `X-Forwarded-For` line OxPHP cannot read as text (one with a non-ASCII byte, for example) is left out of the chain, for the walk and for the rightmost-non-trusted rule alike. A record one of your proxies puts on a line of its own survives this; a record it writes onto the same line as text the client sent is lost with that line, and what is left of the chain may then be only what the client sent.

Cloudflare appends the address of whoever connected to it to `X-Forwarded-For`, so for an ordinary visitor both headers name the same client. They can differ for requests that Cloudflare Workers send to your origin, which also leave from Cloudflare's addresses, whichever account owns the Worker. Cloudflare's [HTTP headers reference](https://developers.cloudflare.com/fundamentals/reference/http-headers/) does not say what such a request may carry in `X-Forwarded-For`. For `CF-Connecting-IP` it does: a Worker can choose the value only for a request to a hostname in its own zone; for a hostname in another Cloudflare zone the header is the fixed address `2a06:98c0:3600::103`, and for a hostname not on Cloudflare it is the client's address, which the Worker cannot alter. Trusting the Cloudflare ranges still means trusting everything that leaves Cloudflare's network, not only your own zone; if the client address feeds a security decision, make the origin accept traffic from your zone only, by whatever means your platform offers.

If the load balancer connects from an address outside the `private` ranges, list its range too: `TRUSTED_PROXIES="private,100.64.0.0/10,cloudflare"`.

## Security

- **Safe default** — without `TRUSTED_PROXIES`, no forwarding headers are processed
- **CIDR validation** — invalid values in `TRUSTED_PROXIES` cause a startup error
- **Spoofing resistance** — the rightmost-non-trusted algorithm ignores attacker-prepended values
- Requests from untrusted IPs have their forwarding headers ignored entirely

## See Also

- [Rate Limiting](../features/rate-limiting.md) — per-IP rate limiting uses the resolved client IP
- [Access Logging](../features/access-logging.md) — `remote_ip` field shows the resolved client IP
- [Configuration Reference](../operations/configuration.md) — all environment variables
