---
title: TLS
description: HTTPS support via rustls with automatic ALPN negotiation
---

OxPHP terminates TLS directly using rustls, with no dependency on OpenSSL. When TLS is configured, the server accepts HTTPS connections and negotiates HTTP/2 or HTTP/1.1 via ALPN.

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `TLS_CERT` | Path to PEM-encoded certificate file | *(unset)* |
| `TLS_KEY` | Path to PEM-encoded private key file | *(unset)* |

Both variables must be set to enable TLS. If only one is provided, TLS is not enabled.

```bash
TLS_CERT=/etc/oxphp/cert.pem
TLS_KEY=/etc/oxphp/key.pem
LISTEN_ADDR=0.0.0.0:443
```

## How it works

At startup, OxPHP reads the certificate and key files, parses them as PEM, and creates a `TlsAcceptor` from the rustls configuration.

The TLS configuration includes:

- **Crypto provider**: ring (via `rustls::crypto::ring::default_provider()`)
- **Protocol versions**: safe defaults selected by rustls (TLS 1.2 and 1.3)
- **Client auth**: disabled (no client certificate verification)
- **ALPN protocols**: `h2` and `http/1.1`, in that order

When a TCP connection arrives, the server calls `acceptor.accept(stream)` to perform the TLS handshake before passing the encrypted stream to hyper for HTTP processing.

## Certificate formats

The certificate file must contain one or more PEM-encoded certificates (the server certificate followed by intermediate certificates if applicable). The key file must contain a single PEM-encoded private key (RSA, ECDSA, or Ed25519).

### Self-signed certificate for development

Generate a self-signed certificate for local development:

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout key.pem -out cert.pem -days 365 -nodes \
  -subj "/CN=localhost"
```

Then configure OxPHP:

```bash
TLS_CERT=./cert.pem
TLS_KEY=./key.pem
```

### Docker example

Mount certificates into the container:

```yaml
services:
  oxphp:
    image: oxphp:latest
    ports:
      - "443:443"
    environment:
      LISTEN_ADDR: "0.0.0.0:443"
      TLS_CERT: /certs/cert.pem
      TLS_KEY: /certs/key.pem
    volumes:
      - ./certs:/certs:ro
```

## Mixed-mode operation

OxPHP does not serve both HTTP and HTTPS on the same listener. To support both protocols, run two instances or use a reverse proxy for HTTP-to-HTTPS redirection.

## No OpenSSL dependency

Using rustls means the server binary does not link against OpenSSL at all. This eliminates a common source of CVEs in production deployments and simplifies the container image (no need for `libssl` packages).

## See Also

- [Timeouts](timeouts.md) -- header read timeout applies after the TLS handshake completes
- [Rate Limiting](rate-limiting.md) -- per-IP rate limiting works with both HTTP and HTTPS connections
