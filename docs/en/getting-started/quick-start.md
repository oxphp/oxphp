---
title: Quick Start
description: Get OxPHP running in under 5 minutes. Create a project, write a PHP app, start the server, and make your first request.
---

# Quick Start

## One Command

If you already have a PHP project with a `public/` directory:

```bash
docker run -p 80:80 -v .:/var/www/html ghcr.io/oxphp/oxphp:0.2.0
```

Open `http://localhost/` — your application is running.

To enable the internal server (health, metrics, config):

```bash
docker run -p 80:80 -p 9090:9090 -e INTERNAL_ADDR=0.0.0.0:9090 -v .:/var/www/html ghcr.io/oxphp/oxphp:0.2.0
```

---

## Step-by-Step Setup with Docker Compose

A more detailed setup — from an empty directory to a working PHP application with health checks and structured logging.

### 1. Create a Project Directory

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

### 2. Create a Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.2.0

COPY --chown=www-data:www-data . /var/www/html
```

The official image includes the server binary, PHP 8.4 ZTS, the OxPHP PHP extension, and all runtime dependencies.

> **Tip:** If your application needs custom PHP extensions (pdo_pgsql, intl, xdebug, etc.), see [`Dockerfile.best.example`](../../../Dockerfile.best.example) in the repository root — a ready-to-use multi-stage Dockerfile with separate `dev` and `prod` targets.

### 3. Add a compose.yaml

```yaml
services:
  oxphp:
    build: .
    ports:
      - "80:80"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=info
      - ACCESS_LOG=all
```

Port `80` serves your application. Port `9090` exposes the internal server for health checks, Prometheus metrics, and the active configuration snapshot.

### 4. Create a PHP Application

```bash
mkdir -p public
```

Create `public/index.php`:

```php
<?php

$requestId = oxphp_request_id();
$info      = oxphp_server_info();

echo "<h1>OxPHP</h1>\n";
echo "<p>Request ID: {$requestId}</p>\n";
echo "<p>Worker: {$info['worker_id']}</p>\n";
echo "<p>SAPI: {$info['sapi']}</p>\n";
echo "<p>Version: {$info['version']}</p>\n";
echo "<p>Time: " . date('c') . "</p>\n";
```

`oxphp_request_id()` returns the unique ID assigned to each request. `oxphp_server_info()` returns details about the running server including `sapi`, `version`, `worker_id`, and `worker_mode`.

### 5. Build and Start

```bash
docker compose up -d --build
```

### 6. Test Your Application

```bash
curl http://localhost/
```

Expected output:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c11a2b00000001</p>
<p>Worker: 0</p>
<p>SAPI: oxphp</p>
<p>Version: 0.2.0</p>
<p>Time: 2026-03-23T12:00:00+00:00</p>
```

Each request gets a unique ID. The worker ID shows which PHP worker thread handled it.

### 7. Check the Internal Endpoints

```bash
# Health check — 200 when healthy, 503 when degraded
curl http://localhost:9090/health

# Prometheus-compatible metrics
curl http://localhost:9090/metrics

# Active configuration (TLS paths redacted)
curl http://localhost:9090/config
```

### 8. View Logs

```bash
docker compose logs -f oxphp
```

Because `ACCESS_LOG=all` is set, every request appears as a structured JSON log line with method, path, status, response time, and request ID.

## What's Next

- [Docker Guide](docker.md) — development and production Dockerfiles, Compose configuration, PHP ini mounts, and health check setup
- [Configuration](../operations/configuration.md) — full environment variable reference
- [Routing](../features/routing.md) — Traditional, Framework, SPA, and Worker routing modes
- [Worker Mode](../features/worker-mode.md) — persistent PHP processes that bootstrap once and handle multiple requests
- [PHP Functions](../php/functions.md) — all OxPHP built-in PHP functions
