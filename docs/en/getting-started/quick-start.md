---
title: Quick Start
description: Get OxPHP running in under 5 minutes
---

This guide walks you through running OxPHP with Docker and serving your first PHP file.

## 1. Create a Project Directory

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

## 2. Create a Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

## 3. Add a compose.yml

Create a `compose.yml`:

```yaml
services:
  oxphp:
    build: .
    ports:
      - "8080:8080"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
```

## 4. Create a Test PHP File

```bash
mkdir -p www
```

Create `public/index.php`:

```php
<?php

$info = oxphp_server_info();
$requestId = oxphp_request_id();

echo "<h1>OxPHP</h1>\n";
echo "<p>Request ID: {$requestId}</p>\n";
echo "<p>SAPI: {$info['sapi']}</p>\n";
echo "<p>Version: {$info['version']}</p>\n";
echo "<p>Worker: {$info['worker_id']}</p>\n";
echo "<p>Time: " . date('c') . "</p>\n";
```

## 5. Start the Server

```bash
docker compose up -d
```

## 6. Test Your Application

Open your browser to `http://localhost:8080/` or use curl:

```bash
curl http://localhost:8080/
```

You should see output similar to:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c100000001</p>
<p>SAPI: oxphp</p>
<p>Version: 0.1.0</p>
<p>Worker: 0</p>
<p>Time: 2026-02-11T12:00:00+00:00</p>
```

## 7. Check Server Health

The internal server exposes health and metrics endpoints on port 9090:

```bash
# Health check — returns 200 with {"status":"ok"}
curl http://localhost:9090/health

# Prometheus-compatible metrics
curl http://localhost:9090/metrics

# Current server configuration (sensitive values redacted)
curl http://localhost:9090/config
```

## 8. View Logs

```bash
docker compose logs -f oxphp
```

OxPHP outputs structured JSON logs. Each request produces an access log entry with the method, path, status code, response time, and request ID.

## Next Steps

- [Docker guide](docker.md) -- compose.yml reference, volume mounts, and deployment tips
- [Configuration](../operations/configuration.md) -- full list of environment variables
- [Routing](../features/routing.md) -- Traditional, Framework, and SPA routing modes
- [PHP Integration](../php/functions.md) -- available PHP extension functions

## See Also

- [Installation](installation.md) -- source build instructions and prerequisites
- [Architecture Overview](../architecture/overview.md) -- runtime model and component map
- [Worker Pool](../architecture/worker-pool.md) -- PHP worker thread scaling and queue behavior
