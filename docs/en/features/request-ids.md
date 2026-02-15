---
title: Request IDs
description: Unique request identifiers for tracing and correlation
---

Every request processed by OxPHP receives a unique request ID. This ID appears in access logs, error logs, and response headers, providing a single value to correlate a request across all layers of the stack.

## Configuration

Request ID generation is always enabled. No configuration is required.

## How it works

The `RequestIdGenerator` event handler runs at priority **-100** (the lowest priority value, meaning it runs first). It either preserves an incoming request ID or generates a new one.

### Passthrough

If the incoming request contains an `X-Request-ID` header, its value is used as-is. This allows upstream load balancers or API gateways to assign request IDs that propagate through OxPHP.

### Generation

When no `X-Request-ID` header is present, OxPHP generates an ID in the format:

```
{timestamp:08x}{counter:08x}
```

- **timestamp** (8 hex chars): the current Unix timestamp in seconds, truncated to 32 bits
- **counter** (8 hex chars): a process-wide atomic counter using the full `u32` range

This produces a 16-character lowercase hexadecimal string. For example: `67890abc00000042`.

The counter uses `Relaxed` memory ordering because uniqueness is guaranteed by the atomic increment -- no happens-before relationship with other data is needed.

### Response header

The request ID is included in every HTTP response as the `X-Request-ID` header. This is set by the server header handler during the `ResponseBuilding` event.

## Accessing the request ID in PHP

The request ID is available in PHP through the `oxphp_request_id()` function provided by the OxPHP PHP extension:

```php
<?php
$requestId = oxphp_request_id();
header("X-Correlation-ID: $requestId");
error_log("Processing request $requestId");
```

The function returns the same 16-character hex string (or the passthrough value) that appears in the response header and access logs.

## Access log correlation

Every access log entry includes the `request_id` field:

```json
{
  "request_id": "67890abc00000042",
  "method": "GET",
  "path": "/api/users",
  "status": 200,
  "duration_us": 1234,
  "remote_addr": "10.0.0.1:54321"
}
```

You can filter logs by request ID to trace the full lifecycle of a single request, including any PHP errors that reference the same ID.

## See Also

- [Access Logging](access-logging.md) -- every log entry includes the `request_id` field
- [Rate Limiting](rate-limiting.md) -- rate-limited responses include the `X-Request-ID` header
