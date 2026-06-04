---
title: Custom Error Pages
description: Serve branded HTML error pages for 4xx and 5xx responses in OxPHP, loaded once at startup and served from memory.
---

# Custom Error Pages

OxPHP serves branded HTML error pages for 4xx and 5xx responses. Error pages are loaded from disk once at startup and served from memory, so no disk I/O occurs during request handling.

## How It Works

1. At startup, OxPHP reads the directory specified by `ERROR_PAGES_DIR` and loads every valid `{status}.html` file into memory.
2. Files must be named with a numeric HTTP status code in the 400–599 range (for example, `404.html`, `503.html`). Files with non-numeric names, status codes outside that range (including `200.html`), or non-`.html` extensions are silently ignored.
3. When OxPHP generates a 4xx or 5xx response, it checks for a matching pre-loaded error page. If one exists, the response body is replaced with the custom HTML and the `Content-Type` is set to `text/html; charset=utf-8`.
4. If the directory does not exist or cannot be read at startup, OxPHP logs a warning and continues without custom error pages. Error responses fall back to plain-text bodies until the directory is fixed and the server is restarted.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `ERROR_PAGES_DIR` | *(unset)* | Directory containing custom error page HTML files. Files must be named `{status}.html` for status codes 400–599. When unset, error responses use plain-text bodies |

## Example Pages

A minimal 404 page:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>404 - Page Not Found</title>
  <style>
    body { font-family: system-ui, sans-serif; text-align: center; padding: 4rem 1rem; color: #333; }
    h1 { font-size: 2rem; margin-bottom: 0.5rem; }
    p { color: #666; }
  </style>
</head>
<body>
  <h1>Page Not Found</h1>
  <p>The page you requested does not exist.</p>
</body>
</html>
```

A 503 maintenance page with auto-refresh:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta http-equiv="refresh" content="30">
  <title>503 - Service Unavailable</title>
  <style>
    body { font-family: system-ui, sans-serif; text-align: center; padding: 4rem 1rem; color: #333; }
    h1 { font-size: 2rem; margin-bottom: 0.5rem; }
    p { color: #666; }
  </style>
</head>
<body>
  <h1>Service Unavailable</h1>
  <p>We are performing maintenance. This page will refresh automatically.</p>
</body>
</html>
```

## Troubleshooting

### Custom error pages are not appearing

Verify that `ERROR_PAGES_DIR` is set and that files are named correctly.

**Check:** Confirm the active directory path and that OxPHP logged "Loaded custom error page" lines at startup:

```bash
docker logs my-app 2>&1 | grep "error page"
```

**Fix:** Ensure the directory path is correct, the files are named `{status}.html`, and the container has read access to the directory.

### Startup warning about missing error pages directory

OxPHP logs a warning and continues without custom error pages if the `ERROR_PAGES_DIR` directory does not exist or cannot be read. Error responses then use plain-text bodies. Check that the volume is mounted correctly in Docker:

```bash
docker run --rm -v ./errors:/var/www/errors:ro \
  -e ERROR_PAGES_DIR=/var/www/errors \
  ghcr.io/oxphp/oxphp:0.7.0
```

### A 429 response still shows the default body

Some responses generated before the response pipeline runs — such as rate-limit rejections — are not processed by the error page handler. The `429 Too Many Requests` response from the rate limiter uses its default body regardless of whether a `429.html` file is present.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.7.0
    ports:
      - "8080:8080"
    volumes:
      - ./src:/var/www/html:ro
      - ./errors:/var/www/errors:ro
    environment:
      ERROR_PAGES_DIR: "/var/www/errors"
      ENTRY_FILE: "index.php"
```

Directory structure:

```text
project/
  src/
    public/
      index.php
  errors/
    403.html
    404.html
    500.html
    502.html
    503.html
```

## Best Practices

- Keep error pages self-contained with inline CSS. Do not reference external stylesheets or scripts — those secondary requests may themselves fail.
- Include a `<meta http-equiv="refresh" content="30">` tag on `503.html` so users automatically retry after maintenance completes.
- Keep error pages small. Every loaded page is held in memory for the lifetime of the server process.

## Notes

Custom error pages apply to responses that flow through the normal request pipeline. The `429 Too Many Requests` response from the rate limiter is generated before the error page handler runs and uses its default plain-text body.

## See Also

- [Routing](routing.md) -- how 404 responses are generated for unmatched paths
- [Rate Limiting](rate-limiting.md) -- rate limit behavior and 429 responses
- [Configuration Reference](../operations/configuration.md) -- complete environment variable reference
