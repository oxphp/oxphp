---
title: OxPHP Documentation
description: Documentation for OxPHP, a high-performance async PHP application server with built-in TLS, compression, rate limiting, metrics, SSE streaming, and worker mode.
---

**English** · [Русский](../ru/index.md) · [中文](../zh/index.md)

[Getting Started](#getting-started) · [Features](#features) · [Security](#security) · [PHP](#php) · [Operations](#operations) · [Architecture](#architecture)

# OxPHP Documentation

OxPHP is a high-performance PHP application server that replaces nginx + PHP-FPM with a single binary — built-in TLS, Brotli compression, rate limiting, health checks, Prometheus metrics, SSE streaming, and persistent worker mode included.

## Why OxPHP

A typical PHP application in production is several containers: nginx, PHP-FPM, sometimes a separate TLS proxy and a metrics exporter. Configuration is scattered across them, and to make everything work together you need to synchronize socket settings, timeouts, and paths. OxPHP replaces the entire stack with [a single container](getting-started/docker.md). Inside is one process that accepts HTTP connections, executes PHP, and serves static files.

The server works out of the box with sensible defaults. Fine-tuning is done through [environment variables](operations/configuration.md): [TLS](features/tls.md) is enabled with two variables (`TLS_CERT`, `TLS_KEY`), [rate limiting](features/rate-limiting.md) with one (`RATE_LIMIT`), and [Brotli compression](features/compression.md) is on by default. No need to edit nginx configs or build separate modules.

On a dedicated [internal port](features/internal-server.md), [health checks](operations/health-checks.md) (`/health`), [Prometheus metrics](operations/metrics.md) (`/metrics`), and a configuration snapshot (`/config`) are available. This is enough for Kubernetes liveness/readiness probes and connecting Grafana without additional sidecar containers.

[Logs](features/access-logging.md) are structured JSON: method, path, status, response time, and [request ID](features/request-ids.md) in every line. They are easy to parse in Loki, Elasticsearch, or any other tool without additional grok patterns.

If you want to try [worker mode](features/worker-mode.md), where the PHP process is not recreated on every request, setting `WORKER_MODE_ENABLED=true` and `ENTRY_FILE=worker.php` is all it takes. The framework initializes once and then handles thousands of requests without reloading. To switch back to classic mode, just remove the variable.

OxPHP also includes capabilities that typically require separate tools or third-party libraries:

- **[Static file serving](features/static-files.md)** — in-memory caching, ETag/Last-Modified, automatic MIME types
- **[Four routing modes](features/routing.md)** — file-based, framework, SPA, and worker
- **[Early response](features/early-response.md)** — send the response immediately and continue background processing
- **[Worker mode](features/worker-mode.md)** — persistent PHP processes with [fiber multiplexing](features/fiber-multiplexing.md)
- **[SSE streaming](features/sse.md)** — real-time Server-Sent Events from PHP
- **[Async promises](features/async-promises.md)** — background execution of PHP closures without blocking the worker
- **[Shared state](features/shared-state.md)** — process-wide concurrent primitives (Counter, Flag, Once, Mutex, Channel, Map, Pool) so workers can coordinate without Redis or APCu
- **[Decorators](features/decorators.md)** — intercept calls via PHP 8 attributes
- **[Distributed tracing & APM](features/distributed-tracing.md)** — W3C Trace Context, OpenTelemetry, automatic instrumentation of database/HTTP/cache/file calls, and a PHP tracing SDK

---

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
- [Shared State](features/shared-state.md) — process-wide primitives: [Counter](features/shared-counter.md), [Atomic](features/shared-atomic.md), [Flag](features/shared-flag.md), [Once](features/shared-once.md), [Mutex](features/shared-mutex.md), [Channel](features/shared-channel.md), [Map](features/shared-map.md), [Pool](features/shared-pool.md); [observability](operations/shared-observability.md); [migrating to an external store](features/migrating-to-external-store.md)
- [Decorators](features/decorators.md) — intercept function and method calls with PHP 8 attributes
- [Distributed Tracing & APM](features/distributed-tracing.md) — W3C Trace Context, OpenTelemetry, auto-instrumentation, and PHP tracing SDK
- [Internal Server](features/internal-server.md) — dedicated port for health checks, Prometheus metrics, and live configuration

## Security

- [Dot-Path Blocking](security/dot-path-blocking.md) — automatic blocking of hidden files and directories (`.env`, `.git/`, `.htaccess`)
- [Trusted Proxies](security/trusted-proxies.md) — real client IP extraction from `Forwarded` (RFC 7239) and `X-Forwarded-*` headers with CIDR-based trust
- [PHP Execution Deny-List](security/php-deny.md) — block `.php` execution at writable public paths (e.g. `/uploads/**`, or specific legacy scripts) to defeat uploaded-shell attacks on legacy apps

## PHP

- [HTTP Request API](php/request-api.md) — object-oriented request access via `oxphp_http_request()`: query params, parsed body, headers, cookies, file uploads, and more
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
