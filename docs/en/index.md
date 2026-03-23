---
title: OxPHP Documentation
description: Documentation for OxPHP, a high-performance async PHP application server with built-in TLS, compression, rate limiting, metrics, SSE streaming, and worker mode.
---

# OxPHP Documentation

OxPHP is a high-performance PHP application server that replaces nginx + PHP-FPM with a single binary — built-in TLS, Brotli compression, rate limiting, health checks, Prometheus metrics, SSE streaming, and persistent worker mode included.

## Why OxPHP

Traditional PHP deployments require multiple moving parts: a web server, a process manager, a TLS proxy, and separate tooling for metrics and rate limiting. OxPHP collapses this entire stack into one binary.

- **Single binary** — no nginx, no PHP-FPM, no process manager. One container runs your entire application.
- **Built-in TLS** — terminate TLS directly without a reverse proxy. Configure it with two environment variables.
- **Brotli compression** — text response compression is enabled out of the box with configurable quality levels.
- **Rate limiting** — per-IP rate limiting with configurable limits and time windows, built in.
- **Health checks** — `/health`, `/metrics`, and `/config` endpoints on a dedicated internal port for Kubernetes probes and monitoring systems.
- **Prometheus metrics** — request counts, response times, queue wait, worker pool stats, compression savings, and more at `/metrics`.
- **Static file serving** — in-memory caching, automatic MIME detection, ETag/Last-Modified headers, and configurable cache TTL with zero configuration.
- **Worker mode** — persistent PHP processes that bootstrap once and handle thousands of requests, eliminating per-request startup overhead for frameworks like Laravel and Symfony.
- **SSE streaming** — real-time Server-Sent Events pushed from PHP to the browser without polling.
- **Early response** — send the HTTP response immediately and continue processing in the background.
- **Four routing modes** — traditional file mapping, framework front-controller (`index.php`), SPA fallback (`index.html`), and worker mode.
- **Async promises** — run PHP closures on a dedicated thread pool and await results without blocking the worker.
- **Decorators** — intercept function and method calls with PHP 8 attributes for logging, timing, caching, and access control.
- **W3C Trace Context** — propagate distributed tracing headers from upstream services into PHP via `$_SERVER`.
- **OpenTelemetry** — export request spans to Jaeger, Grafana Tempo, Zipkin, or any OTLP-compatible backend.

## Quick Start

Get OxPHP running in 30 seconds with Docker:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

```bash
docker build -t my-app .
docker run -p 8080:80 my-app
curl http://localhost:8080/
```

The OxPHP image includes the server binary, PHP 8.4, OPcache with JIT, and all required dependencies. Your application code goes in `/var/www/html` and the default document root is `/var/www/html/public`.

For a full walkthrough, see the [Quick Start](getting-started/quick-start.md) guide.

## Getting Started

- [Installation](getting-started/installation.md) — system requirements and installation options
- [Quick Start](getting-started/quick-start.md) — build and run your first OxPHP application in under 5 minutes
- [Docker Guide](getting-started/docker.md) — Dockerfiles, Compose configuration, volumes, and deployment patterns

## Features

- [Routing](features/routing.md) — four routing modes: traditional file mapping, framework front-controller, SPA fallback, and worker mode
- [Static Files](features/static-files.md) — file cache, MIME detection, ETag/Last-Modified headers, and streaming
- [Worker Mode](features/worker-mode.md) — persistent PHP processes with automatic soft reset between requests
- [Fiber Multiplexing](features/fiber-multiplexing.md) — handle hundreds of concurrent requests per worker thread with cooperative multitasking
- [Compression](features/compression.md) — Brotli compression for text-based responses
- [TLS](features/tls.md) — built-in TLS termination with certificate and key configuration
- [Rate Limiting](features/rate-limiting.md) — per-IP rate limiting with configurable windows and limits
- [Timeouts](features/timeouts.md) — header read and request timeouts
- [Access Logging](features/access-logging.md) — structured JSON access logs with request ID, method, path, status, and duration
- [Request IDs](features/request-ids.md) — automatic `X-Request-ID` generation and pass-through
- [Error Pages](features/error-pages.md) — custom HTML error pages for any HTTP status code
- [SSE](features/sse.md) — real-time Server-Sent Events streaming from PHP
- [Early Response](features/early-response.md) — send the response immediately and continue background processing
- [Async Promises](features/async-promises.md) — run PHP closures on background threads and await results
- [Decorators](features/decorators.md) — intercept function and method calls with PHP 8 attributes
- [Distributed Tracing](features/distributed-tracing.md) — W3C Trace Context, OpenTelemetry integration, and log correlation
- [Internal Server](features/internal-server.md) — dedicated port for health checks, Prometheus metrics, and live configuration

## PHP

- [Functions](php/functions.md) — built-in PHP functions provided by OxPHP (`oxphp_worker()`, `oxphp_request_id()`, `oxphp_server_info()`, and more)
- [Superglobals](php/superglobals.md) — how `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, and `php://input` are populated
- [OPcache and JIT](php/opcache.md) — OPcache configuration and JIT compilation settings

## Operations

- [Configuration Reference](operations/configuration.md) — complete list of environment variables with defaults and descriptions
- [Health Checks](operations/health-checks.md) — the `/health`, `/metrics`, and `/config` internal server endpoints
- [Metrics](operations/metrics.md) — Prometheus-compatible metrics reference
- [Graceful Shutdown](operations/graceful-shutdown.md) — drain behavior, timeouts, and shutdown sequence

## Architecture

- [Architecture Overview](architecture/overview.md) — how OxPHP works: async HTTP handling, PHP worker pool, request flow, and safety guarantees
