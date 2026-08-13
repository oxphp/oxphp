---
title: Request IDs
description: Automatic unique request identifiers in OxPHP for distributed tracing and log correlation across your stack.
---

# Request IDs

Every request processed by OxPHP receives a unique identifier for tracing and log correlation. The ID appears in response headers and access logs, giving you a single value to trace a request across every layer of your stack.

## How It Works

1. OxPHP assigns a unique ID to each incoming request before any other processing occurs.
2. If the client sends an `X-Request-ID` header — for example, from a load balancer or API gateway — OxPHP validates and preserves the value. To pass validation, the header must be 1–64 characters long and contain only alphanumerics, hyphens (`-`), underscores (`_`), or dots (`.`). If validation fails, OxPHP generates a new ID instead.
3. When no valid `X-Request-ID` header is present, OxPHP generates a 20-character lowercase hexadecimal ID (for example, `67890abc12341a2b0042`). The ID encodes a timestamp, a process-unique value, and a monotonic counter, making collisions across containers and restarts extremely unlikely.
4. The ID appears in the `X-Request-ID` response header on every response.
5. When access logging is enabled, the ID appears in the `request_id` field of every access log entry.
6. PHP scripts can read the ID via `oxphp_request_id()`.

## Response Header

Every HTTP response from OxPHP includes the `X-Request-ID` header:

```http
HTTP/1.1 200 OK
X-Request-ID: 67890abc12341a2b0042
Content-Type: text/html; charset=utf-8
```

When an upstream load balancer or gateway provides an `X-Request-ID` on the incoming request, the same value is echoed back in the response, preserving end-to-end traceability across your infrastructure.

## PHP Examples

Read the current request ID from PHP using `oxphp_request_id()`:

```php
<?php
$requestId = oxphp_request_id();

// Include in application logs for correlation
$logger->info('Processing order', [
    'request_id' => $requestId,
    'order_id'   => $orderId,
]);
```

Forward the request ID to downstream services to maintain traceability across API calls:

```php
<?php
$requestId = oxphp_request_id();

$ch = curl_init('https://api.example.com/users');
curl_setopt($ch, CURLOPT_HTTPHEADER, [
    "X-Request-ID: $requestId",
]);
$response = curl_exec($ch);
```

## Examples

When access logging is enabled, every log entry includes the `request_id` field:

```json
{
  "timestamp": "2026-02-11T12:34:56.789Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc12341a2b0042",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1234,
    "remote_ip": "10.0.0.1",
    "message": "request completed"
  }
}
```

Filter your log aggregator by `request_id` to trace the full lifecycle of a single request, including any PHP errors or application log entries that reference the same ID.

## Troubleshooting

### The `X-Request-ID` header is missing from responses

This is unexpected — OxPHP adds the header to every response. If the header is absent, an intermediate proxy may be stripping it.

**Check:** Test directly against OxPHP without any proxy in the path:

```bash
curl -v http://localhost:8080/ 2>&1 | grep -i x-request-id
```

### An upstream ID is not being preserved

The incoming `X-Request-ID` value may be failing validation. OxPHP rejects IDs that are empty, longer than 64 characters, or contain characters other than alphanumerics, hyphens, underscores, or dots.

**Check:** Inspect the value your upstream sends and verify it meets the character and length requirements. Common failures include values with slashes, spaces, or brace characters.

### `oxphp_request_id()` returns an empty string

This function is only available within OxPHP. If you run the same PHP code under PHP-FPM or CLI, the function is not defined. Guard calls with a compatibility check:

```php
<?php
$requestId = function_exists('oxphp_request_id')
    ? oxphp_request_id()
    : ($_SERVER['HTTP_X_REQUEST_ID'] ?? uniqid('', true));
```

## See Also

- [Access Logging](access-logging.md) -- every log entry includes the `request_id` field
- [PHP Functions](../php/functions.md) -- full reference for `oxphp_request_id()` and other built-in functions
