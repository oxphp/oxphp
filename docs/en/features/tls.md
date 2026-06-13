---
title: TLS
description: Configure native TLS termination in OxPHP with support for TLS 1.2, TLS 1.3, HTTP/2 ALPN negotiation, and PEM certificates.
---

# TLS

OxPHP handles TLS termination natively — no reverse proxy or external SSL library required. When configured, the server accepts HTTPS connections and automatically negotiates the best available protocol.

## How It Works

To enable TLS, set `TLS_CERT` and `TLS_KEY` to point at your PEM-encoded certificate and private key files. Once both are set, the server listens for HTTPS connections on the address specified by `LISTEN_ADDR`.

The TLS handshake happens before any HTTP processing:

1. A TCP connection arrives on `LISTEN_ADDR`.
2. The server performs a TLS handshake using the configured certificate and key.
3. Protocol negotiation (ALPN) selects HTTP/2 (`h2`) or HTTP/1.1 based on client support.
4. The encrypted connection is passed to the HTTP layer for normal request handling.

> **Note:** When TLS is enabled, header and request timeouts apply per-request after the TLS handshake completes.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `TLS_CERT` | *(unset)* | Path to PEM-encoded certificate file. Both `TLS_CERT` and `TLS_KEY` must be set to enable TLS |
| `TLS_KEY` | *(unset)* | Path to PEM-encoded private key file |
| `LISTEN_ADDR` | `0.0.0.0:80` | Address and port to listen on. Change to `0.0.0.0:443` when using TLS |

If only one of `TLS_CERT` or `TLS_KEY` is provided, TLS is not enabled and the server starts in plain HTTP mode.

## Supported Protocols

| Capability | Detail |
|------------|--------|
| TLS versions | TLS 1.2 and TLS 1.3 |
| ALPN protocols | `h2` (HTTP/2) and `http/1.1`, negotiated in that order |
| Client certificates | Not supported (no mutual TLS) |

## HTTP/2

OxPHP serves HTTP/2 and HTTP/1.1 on the same port. The protocol is chosen per connection — there is no setting to turn HTTP/2 on or off:

- **Over TLS**, the protocol is negotiated during the handshake via ALPN. OxPHP advertises `h2` then `http/1.1`, so HTTP/2-capable clients get HTTP/2 and everyone else falls back to HTTP/1.1 transparently.
- **Without TLS (h2c)**, OxPHP detects the HTTP/2 connection preface and serves cleartext HTTP/2 to clients that connect with prior knowledge (e.g. `curl --http2-prior-knowledge`). Clients that don't speak HTTP/2 keep using HTTP/1.1 on the same port. (The `Upgrade: h2c` handshake is not used — HTTP/2 over cleartext requires prior knowledge.)

HTTP/2 flow-control windows are raised above the protocol defaults — 8 MB per connection and 4 MB per stream, versus the 64 KB default — to avoid stalls on typical PHP responses, which are usually larger than one default window.

PHP sees the negotiated protocol in `$_SERVER['SERVER_PROTOCOL']` (`"HTTP/2"` or `"HTTP/1.1"`).

### Verify

```bash
# HTTP/2 over TLS (negotiated via ALPN)
curl -k --http2 -I https://localhost/

# Cleartext HTTP/2 (h2c, prior knowledge)
curl --http2-prior-knowledge -I http://localhost/
```

Look for `HTTP/2 200` in the response line.

### Connection Limits

OxPHP applies per-connection HTTP/2 limits to bound the amplification a single
TCP connection can exert on the PHP worker pool. Each accepted stream becomes a
queued PHP request, so an unbounded stream count from one connection would
saturate the pool on its own.

| Variable | Default | Description |
|----------|---------|-------------|
| `H2_MAX_CONCURRENT_STREAMS` | `PHP_WORKERS_MAX × 4` (min 32) | Maximum simultaneous open streams per connection. Excess streams receive `REFUSED_STREAM` |
| `H2_MAX_PENDING_RESET` | `20` | Maximum `RST_STREAM` frames queued before the connection is closed (CVE-2023-44487 Rapid Reset protection) |
| `H2_MAX_HEADER_LIST_BYTES` | `65536` | Maximum total decoded header bytes per request (HPACK bomb guard) |
| `H2_KEEPALIVE_INTERVAL_SECS` | `20` | Seconds between HTTP/2 PING frames; `0` disables keepalive |
| `H2_KEEPALIVE_TIMEOUT_SECS` | `10` | Seconds to wait for a PING reply before closing the connection |

`PHP_WORKERS_MAX` is the maximum worker count set by `PHP_WORKERS`. For a
dynamic range like `4:16`, the maximum (16) is used. The default scales with
workers so that legitimate concurrent page loads on one connection do not create
more queue pressure than the pool can absorb.

> **Trade-off:** `H2_MAX_CONCURRENT_STREAMS` is intentionally tuned to pool
> capacity, not to browser multiplexing behaviour. Browsers send dozens of
> asset requests over a single HTTP/2 connection; those that exceed the cap
> receive `REFUSED_STREAM` and are automatically retried by the browser in
> the next batch. This adds a small latency penalty on heavy fan-out pages
> (many assets per connection) but prevents a single connection from filling
> the PHP request queue. If your site has many large-asset pages and the
> default causes measurable latency, raise `H2_MAX_CONCURRENT_STREAMS`
> explicitly rather than increasing `PHP_WORKERS`.

## Supported Key Types

The private key file must contain a single PEM-encoded key in one of the following formats:

- **RSA**
- **ECDSA** (e.g., prime256v1, secp384r1)
- **Ed25519**

The certificate file may contain one or more PEM-encoded certificates. For production use, include the full chain: your server certificate followed by any intermediate certificates.

## Self-Signed Certificate for Development

Generate a self-signed ECDSA certificate for local development:

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout key.pem -out cert.pem -days 365 -nodes \
  -subj "/CN=localhost"
```

Then configure OxPHP to use the generated files:

```bash
TLS_CERT=./cert.pem
TLS_KEY=./key.pem
LISTEN_ADDR=0.0.0.0:443
```

## Troubleshooting

### Server starts but TLS is not active

OxPHP requires **both** `TLS_CERT` and `TLS_KEY` to be set. If either is missing, the server starts in plain HTTP mode without any warning. Confirm both variables are set:

```bash
docker exec <container> env | grep TLS
```

### `no private key found in PEM file` error at startup

The key file is empty, corrupt, or contains only a certificate. Verify that the key file contains a `-----BEGIN ... PRIVATE KEY-----` block:

```bash
grep "PRIVATE KEY" key.pem
```

If the key is missing, regenerate the certificate and key pair.

### `no certificates found in PEM file` error at startup

The certificate file is empty or corrupt. Verify that the cert file contains at least one `-----BEGIN CERTIFICATE-----` block:

```bash
grep "BEGIN CERTIFICATE" cert.pem
```

### Clients see a certificate chain error

The server is sending only the leaf certificate without intermediate certificates. Concatenate the full chain into a single PEM file:

```bash
cat cert.pem intermediate.pem > fullchain.pem
```

Then set `TLS_CERT=./fullchain.pem`.

### Certificate expired

OxPHP reads certificate files at startup and holds them in memory. Renewing the certificate on disk has no effect until the server restarts.

**Fix:** Restart OxPHP after certificate renewal. Automate this with your certificate renewal tool (e.g. certbot's `--deploy-hook` option).

### Cannot serve HTTP and HTTPS on the same port

OxPHP listens on a single port. To support both protocols simultaneously, use a reverse proxy (Caddy, Traefik, nginx) that handles the HTTP-to-HTTPS redirect, or run a second OxPHP instance on port 80 dedicated to redirecting traffic.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.8.0
    ports:
      - "443:443"
    environment:
      LISTEN_ADDR: "0.0.0.0:443"
      TLS_CERT: "/etc/ssl/oxphp/cert.pem"
      TLS_KEY: "/etc/ssl/oxphp/key.pem"
    volumes:
      - ./app:/var/www/html:ro
      - ./certs:/etc/ssl/oxphp:ro
```

## Best Practices

- **Include intermediate certificates** in the PEM chain. Place the server certificate first, followed by intermediates in order, so clients can verify the full trust path.
- **Automate certificate renewal.** Use certbot or acme.sh to renew certificates before expiry, then restart OxPHP to load the new files.
- **Use a reverse proxy for HTTP-to-HTTPS redirection.** OxPHP does not serve both HTTP and HTTPS on the same port simultaneously.

## Notes

- OxPHP does not depend on OpenSSL. TLS is handled by a built-in implementation, eliminating a common source of external library CVEs.
- Certificate and key files are read at startup only. Updating certificates on disk requires restarting the server.

## See Also

- [Configuration Reference](../operations/configuration.md) — full list of environment variables
- [Docker Guide](../getting-started/docker.md) — volume mounts and certificate management in Docker
