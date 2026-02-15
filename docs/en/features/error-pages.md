---
title: Custom Error Pages
description: Pre-loaded HTML error pages for 4xx and 5xx responses
---

OxPHP can serve custom HTML error pages for 4xx and 5xx responses. Error pages are loaded from disk once at startup and served from memory on the hot path.

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `ERROR_PAGES_DIR` | Directory containing error page HTML files | *(unset)* |

```bash
ERROR_PAGES_DIR=/var/www/errors
```

When this variable is not set, error responses use their default plain-text bodies (for example, `404 Not Found`).

## File naming

Error page files must follow the naming convention `{status}.html`, where `{status}` is the HTTP status code:

```
errors/
  403.html
  404.html
  500.html
  502.html
  503.html
```

Only status codes in the 400-599 range are loaded. Files with non-numeric names, names outside this range, or non-`.html` extensions are ignored.

## How it works

### Loading

At startup, OxPHP reads the configured directory and loads every valid `{status}.html` file into a `HashMap<u16, Bytes>`. The file contents are stored as reference-counted byte buffers for zero-copy serving. Each loaded page is logged at the `info` level.

If the directory does not exist or cannot be read, a warning is logged and the server starts without custom error pages.

### Serving

The `ErrorPagesHandler` runs as an event handler on `ResponseBuilding` events at priority **60**. This places it after most processing but before the server header and access log handlers (priority 100).

For every response with a status code of 400 or higher, the handler checks the pre-loaded pages. If a matching page exists, the response body is replaced with the custom HTML content and the `Content-Type` header is set to `text/html; charset=utf-8`.

Responses with a 2xx or 3xx status are not affected.

### Example page

A minimal 404 error page:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>404 - Page Not Found</title>
</head>
<body>
  <h1>Page Not Found</h1>
  <p>The requested page does not exist.</p>
</body>
</html>
```

Save this as `404.html` in the directory specified by `ERROR_PAGES_DIR`.

## Performance

Error pages are loaded into memory once at startup. Serving a custom error page is a `HashMap::get()` followed by a `Bytes::clone()` (an atomic reference count increment). No disk I/O occurs during request handling.

## Limitations

Custom error pages only apply to responses that flow through the normal request pipeline. Responses set as early returns (such as 429 from rate limiting) bypass the `ResponseBuilding` event and are not affected by custom error pages.

## See Also

- [Routing](routing.md) -- how 404 responses are generated
- [Rate Limiting](rate-limiting.md) -- rate-limited responses bypass custom error pages
- [Request IDs](request-ids.md) -- error responses include the `X-Request-ID` header
