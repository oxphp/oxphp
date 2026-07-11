---
title: Distributed Tracing & APM
description: W3C Trace Context propagation, OpenTelemetry integration, automatic instrumentation, PHP tracing SDK, and end-to-end observability with OxPHP.
---

# Distributed Tracing & APM

OxPHP supports W3C Trace Context propagation, OpenTelemetry (OTel) export, and built-in Application Performance Monitoring (APM). Incoming `traceparent` headers are parsed and continued, trace IDs are available in PHP via `$_SERVER`, access logs include trace fields, and spans can be exported to Jaeger, Grafana Tempo, Zipkin, or any OTLP-compatible backend.

The APM plugin adds three layers of tracing on top of the OTel foundation:

- **Automatic instrumentation** — internal PHP functions (PDO, mysqli, cURL, Redis, Memcached, file I/O) are hooked at the engine level; every call becomes a span with zero code changes
- **Attribute-based tracing** — annotate any PHP function or method with `#[OxPHP\Apm\Trace]` to create spans automatically
- **PHP SDK** — 10 `oxphp_apm_*()` functions for manual span creation, attributes, events, and error recording

## How It Works

1. **Incoming request** — OxPHP reads the `traceparent` and `tracestate` headers per the W3C Trace Context specification
2. **New span** — a new span ID is generated for this hop. The incoming span ID becomes the parent
3. **Propagation to PHP** — trace IDs are injected into `$_SERVER['OXPHP_TRACE_ID']`, `$_SERVER['OXPHP_SPAN_ID']`, and `$_SERVER['OXPHP_PARENT_SPAN_ID']`
4. **Access log** — structured JSON logs include `trace_id` and `span_id` fields for log correlation
5. **Response headers** — the updated `traceparent` header (with OxPHP's span ID) is added to the response, so downstream services can continue the trace
6. **OTel export** (optional) — when the OTel plugin is enabled, each request becomes a span exported via OTLP with HTTP semantic convention attributes

If no `traceparent` header is present, OxPHP generates a new trace ID and span ID, starting a fresh trace.

## Configuration

### W3C Trace Context (Built-in)

| Variable | Default | Description |
|----------|---------|-------------|
| `TRACE_CONTEXT` | `false` | Enable W3C Trace Context propagation. Set to `true` or `1` |

### OpenTelemetry Plugin

The OTel plugin is a compile-time feature (`plugin-otel`). When enabled, it automatically turns on trace-context propagation (the same effect as setting `TRACE_CONTEXT=true`).

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | Enable the OpenTelemetry plugin. Boolean — see [Boolean values](../operations/configuration.md#boolean-values) |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Export protocol: `grpc` or `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` (gRPC) or `http://localhost:4318` (HTTP) | OTLP collector endpoint. An `https://` URL is exported over TLS on both transports, verified against the system trust store (the runtime image must ship a CA bundle such as `ca-certificates` — the official image installs it); custom CA bundles and mTLS are not yet supported |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Export timeout in milliseconds |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(unset)* | Authentication headers: `key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | Service name in exported spans |
| `OTEL_SERVICE_VERSION` | *(unset)* | Service version attribute |
| `OTEL_RESOURCE_ATTRIBUTES` | *(unset)* | Additional resource attributes: `env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Sampling strategy: `always_on`, `always_off`, `traceidratio`, `parentbased_always_on`, `parentbased_always_off`, `parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Sampling ratio (0.0–1.0) for ratio-based samplers |

> **Note:** Invalid or out-of-range `OTEL_TRACES_SAMPLER_ARG` values are clamped to `[0.0, 1.0]` and logged at warn level. Unknown `OTEL_TRACES_SAMPLER` values fall back to `parentbased_traceidratio` and are logged.

### APM Plugin

The APM plugin is a compile-time feature (`plugin-apm`) that depends on the OTel plugin. It adds automatic instrumentation, the `#[OxPHP\Apm\Trace]` decorator, and the PHP tracing SDK.

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_APM_ENABLED` | `false` | Enable APM: auto-instrumentation, error capture, PHP SDK. Requires `OTEL_ENABLED=true`. Boolean — see [Boolean values](../operations/configuration.md#boolean-values) |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | Slow query threshold in milliseconds. Queries above this get `oxphp.db.slow=true` on their spans |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | Record bind parameters in the `db.params` span attribute. Boolean — see [Boolean values](../operations/configuration.md#boolean-values) |
| `OTEL_APM_STACKTRACE_MAX_BYTES` | `8192` | Maximum size in bytes of the `exception.stacktrace` attribute. Over the cap the stacktrace is truncated from the tail (the root frame is kept) with a `…(truncated)` marker. `0` disables truncation |
| `OTEL_APM_MESSAGE_MAX_BYTES` | `4096` | Maximum size in bytes of the `exception.message` attribute (default matches New Relic's per-attribute value limit). Over the cap the message is truncated from the tail with a `…(truncated)` marker. `0` disables truncation |

## Trace Context in PHP

When `TRACE_CONTEXT=true`, three `$_SERVER` variables are available in your PHP scripts:

| Variable | Description | Example |
|----------|-------------|---------|
| `OXPHP_TRACE_ID` | W3C trace ID (32 hex chars) | `4bf92f3577b34da6a3ce929d0e0e4736` |
| `OXPHP_SPAN_ID` | OxPHP's span ID for this request (16 hex chars) | `00f067aa0ba902b7` |
| `OXPHP_PARENT_SPAN_ID` | Incoming parent span ID (16 hex chars, empty if new trace) | `a3ce929d0e0e4736` |

Use these to propagate trace context to downstream services:

```php
<?php
$traceId  = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId   = $_SERVER['OXPHP_SPAN_ID'] ?? '';

if ($traceId) {
    // Build a traceparent header for downstream calls
    $traceparent = "00-{$traceId}-{$spanId}-01";

    $response = file_get_contents('https://api.example.com/data', false,
        stream_context_create([
            'http' => [
                'header' => "traceparent: {$traceparent}\r\n",
            ],
        ])
    );
}
```

### With Guzzle

```php
<?php
$traceId = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId  = $_SERVER['OXPHP_SPAN_ID'] ?? '';

$client = new \GuzzleHttp\Client();
$response = $client->get('https://api.example.com/users', [
    'headers' => [
        'traceparent' => "00-{$traceId}-{$spanId}-01",
    ],
]);
```

## Access Log Correlation

When trace context is enabled, structured JSON access logs include `trace_id` and `span_id` fields:

```json
{
  "timestamp": "2026-03-23T10:15:30.123Z",
  "level": "INFO",
  "fields": {
    "request_id": "4bf92f3577b34da6-00f067aa",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "span_id": "00f067aa0ba902b7",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1523,
    "remote_ip": "10.0.0.1",
    "message": "request completed"
  }
}
```

This enables searching logs by trace ID in log aggregation systems (Loki, Elasticsearch, Splunk, CloudWatch) to find all log entries for a distributed trace.

## Response Headers

OxPHP adds the `traceparent` header to every response, with OxPHP's own span ID:

```http
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
```

If the incoming request included a `tracestate` header, it is forwarded in the response as well.

## OpenTelemetry Integration

When the OTel plugin is enabled, each HTTP request becomes a span exported to your tracing backend via OTLP.

### Span Attributes

Exported spans include standard HTTP semantic convention attributes:

| Attribute | Description |
|-----------|-------------|
| `http.request.method` | HTTP method (GET, POST, etc.) |
| `url.path` | Request path |
| `http.response.status_code` | Response status code |
| `client.address` | Client IP address |
| `server.address` | Server listen address |
| `oxphp.request_id` | OxPHP request ID |
| `http.request.body.size` | Request body size in bytes (if non-zero) |
| `http.response.body.size` | Response body size in bytes (if non-zero) |

5xx responses are marked as error spans.

### Span Events

Child spans also carry **span events** — timestamped annotations exported as
OpenTelemetry events and rendered natively by Jaeger, Grafana Tempo, and other
OTLP backends. A `oxphp.event.kind` attribute on each event identifies its type:

| `oxphp.event.kind` | Source | Event attributes |
|--------------------|--------|------------------|
| `exception` | A `#[OxPHP\Apm\Trace]` function that threw, or `oxphp_apm_error()` | `exception.type`, `exception.message`, `exception.stacktrace` |
| `custom` | `oxphp_apm_event()` | user-supplied |
| `mark` | profiler `#[Mark]` annotation | user-supplied |
| `slow` | profiler `#[SlowThreshold]` breach | `threshold_ms`, `elapsed_ms` |
| `memory_spike` | profiler `#[MemoryThreshold]` breach | `threshold_kb`, `delta_bytes` |

The `oxphp.event.kind` attribute may additionally carry `sql`, `http`, or `alloc` on events generated by APM instrumentation.

### Request ID with OTel

When the OTel plugin is active, request IDs are derived from the trace context: the first 16 characters of the trace ID and first 8 characters of the span ID, separated by a dash. This appears in logs, the `X-Request-ID` response header, and `oxphp_request_id()` in PHP.

## APM: Automatic Instrumentation

When the APM plugin is enabled, OxPHP automatically hooks 33 internal PHP functions at the engine level. Every call to a hooked function creates a child span under the current request's root span — with zero code changes required.

### Hooked Functions

| Category | Functions |
|----------|-----------|
| **PDO** | `PDO::__construct`, `PDO::query`, `PDO::exec`, `PDO::prepare`, `PDOStatement::execute` |
| **mysqli** | `mysqli::__construct`, `mysqli::query`, `mysqli::prepare`, `mysqli_stmt::execute` |
| **cURL** | `curl_init`, `curl_setopt`, `curl_exec`, `curl_multi_exec` |
| **Redis** | `Redis::connect`, `Redis::get`, `Redis::set`, `Redis::del`, `Redis::mget`, `Redis::mset`, `Redis::hget`, `Redis::hset`, `Redis::lpush`, `Redis::rpush` |
| **Memcached** | `Memcached::get`, `Memcached::set`, `Memcached::delete`, `Memcached::getMulti`, `Memcached::setMulti` |
| **File I/O** | `fopen`, `fread`, `fwrite`, `file_get_contents`, `file_put_contents` |

Hooks are only installed for extensions that are actually loaded. If your build does not include the Redis extension, the Redis hooks are silently skipped.

### How It Works

Hook installation uses a two-phase design for thread safety under PHP ZTS:

1. **Phase 1 (MINIT)** — during module initialization, OxPHP validates each target function against the loaded extensions and captures original handler pointers into a read-only approved list
2. **Phase 2 (RINIT)** — on the first request per worker thread, the approved hooks are installed into that thread's function tables

This ensures each ZTS worker thread has consistent function table modifications and thread-local state.

## APM: Attribute-Based Tracing

The `#[OxPHP\Apm\Trace]` attribute creates spans automatically around decorated functions and methods. Unlike the auto-instrumentation hooks (which target internal C functions), this works on user-defined PHP code.

```php
<?php
use OxPHP\Apm\Trace;

#[Trace]
function processOrder(int $orderId): void
{
    // A span named "processOrder" is created on entry and closed on exit.
    // If an exception is thrown, the span is marked as error and an
    // "exception" span event records exception.type, exception.message
    // and exception.stacktrace.
}

class PaymentService
{
    #[Trace]
    public function charge(float $amount): bool
    {
        // Span named "PaymentService::charge"
        return true;
    }
}
```

The `#[Trace]` attribute targets both functions and methods. No registration call is needed — the APM plugin registers the decorator automatically during initialization.

If the decorated function throws an exception, the span's status is set to error and an `exception` event is recorded with the full OpenTelemetry semantic-convention data: `exception.type` (the class), `exception.message` (the message), and `exception.stacktrace` (the call stack from `getTraceAsString()`). The message is truncated to `OTEL_APM_MESSAGE_MAX_BYTES` bytes (default 4096) and the stacktrace to `OTEL_APM_STACKTRACE_MAX_BYTES` bytes (default 8192); `0` disables either cap. Argument capture inside frames follows PHP's own `zend.exception_ignore_args` setting.

## APM: PHP Tracing SDK

The APM plugin registers 10 `oxphp_apm_*()` functions for manual span management. All functions are safe no-ops when APM is disabled, so your code works without modification in any environment.

### Creating Spans

```php
<?php
// Start a span and get its local ID
$spanId = oxphp_apm_start('cache.warm', ['cache.size' => '1024']);

// ... do work ...

// Close the span
oxphp_apm_end($spanId);
```

### Adding Attributes and Events

```php
<?php
$spanId = oxphp_apm_start('order.process');

// Add attributes to the current span (or a specific one)
oxphp_apm_attribute('order.id', $orderId);
oxphp_apm_attribute('order.total', $total, $spanId);

// Record an event on the span
oxphp_apm_event('payment.authorized', [
    'provider' => 'stripe',
    'amount' => (string) $amount,
]);

oxphp_apm_end($spanId);
```

### Error Recording

```php
<?php
$spanId = oxphp_apm_start('external.api');

try {
    $result = callExternalApi();
} catch (\Throwable $e) {
    // Mark the span as error
    oxphp_apm_error($e, $spanId);
    throw $e;
} finally {
    oxphp_apm_end($spanId);
}
```

### Propagating Trace Context

```php
<?php
// Get the current trace ID and span ID
$traceId = oxphp_apm_trace_id();
$currentSpanId = oxphp_apm_span_id();

// Or get a ready-to-use traceparent header value
$traceparent = oxphp_apm_header();
// "00-{trace_id}-{span_id}-01"

// Propagate to downstream services
$response = file_get_contents('https://api.example.com/data', false,
    stream_context_create([
        'http' => [
            'header' => "traceparent: {$traceparent}\r\n",
        ],
    ])
);
```

### Function Reference

| Function | Returns | Description |
|----------|---------|-------------|
| `oxphp_apm_trace(name, callback, ?attributes)` | `void` | Execute a callback inside a span (reserved for future use) |
| `oxphp_apm_start(name, ?attributes)` | `int` | Open a span and return its local ID. `0` when APM is disabled |
| `oxphp_apm_end(span_id)` | `void` | Close the span with the given local ID |
| `oxphp_apm_attribute(key, value, ?span_id)` | `void` | Set an attribute on the current or specified span |
| `oxphp_apm_event(name, ?attributes, ?span_id)` | `void` | Record a timestamped event on the current or specified span |
| `oxphp_apm_error(exception, ?span_id)` | `void` | Mark the current or specified span as error and record an `exception` event. A Throwable object contributes `exception.type`, `exception.message`, and `exception.stacktrace`; a bare string argument is recorded as `exception.message` under a generic `exception.type` of `Error` (so the event stays visible in backends that group by type) |
| `oxphp_apm_status(code, ?description, ?span_id)` | `void` | Set span status: `0` = Unset, `1` = Ok, `2` = Error |
| `oxphp_apm_trace_id()` | `string` | Current trace ID (32 hex chars). Empty when APM is disabled |
| `oxphp_apm_span_id()` | `string` | Current span ID (16 hex chars). Empty when no active span |
| `oxphp_apm_header()` | `string` | W3C `traceparent` header value for the current span context |

For the complete function signature reference, see [PHP Functions](../php/functions.md#oxphp_apm_start).

## Docker Example

### Trace Context Only

Enable W3C trace propagation without an external backend:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "80:80"
    environment:
      - TRACE_CONTEXT=true
      - INTERNAL_ADDR=0.0.0.0:9090
```

### With Jaeger

Full observability stack with Jaeger as the tracing backend:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "80:80"
    environment:
      - OTEL_ENABLED=true
      - OTEL_EXPORTER_OTLP_ENDPOINT=http://jaeger:4317
      - OTEL_SERVICE_NAME=my-app
      - OTEL_SERVICE_VERSION=1.0.0
      - OTEL_RESOURCE_ATTRIBUTES=env=production
      - INTERNAL_ADDR=0.0.0.0:9090

  jaeger:
    image: jaegertracing/all-in-one:latest
    ports:
      - "16686:16686"   # Jaeger UI
      - "4317:4317"     # OTLP gRPC
```

### With Grafana Tempo

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "80:80"
    environment:
      - OTEL_ENABLED=true
      - OTEL_EXPORTER_OTLP_ENDPOINT=http://tempo:4317
      - OTEL_SERVICE_NAME=my-app

  tempo:
    image: grafana/tempo:latest
    ports:
      - "4317:4317"

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
```

### With Jaeger and APM

Full observability with automatic instrumentation of database queries, HTTP calls, cache operations, and file I/O:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.10.0
    ports:
      - "80:80"
    environment:
      - OTEL_ENABLED=true
      - OTEL_EXPORTER_OTLP_ENDPOINT=http://jaeger:4317
      - OTEL_SERVICE_NAME=my-app
      - OTEL_APM_ENABLED=true
      - OTEL_APM_SLOW_QUERY_MS=50
      - INTERNAL_ADDR=0.0.0.0:9090

  jaeger:
    image: jaegertracing/all-in-one:latest
    ports:
      - "16686:16686"   # Jaeger UI
      - "4317:4317"     # OTLP gRPC
    environment:
      - COLLECTOR_OTLP_ENABLED=true
```

> **Note:** The `plugin-apm` Cargo feature must be enabled at build time. The official OxPHP image includes it by default.

## The Observability Stack

OxPHP provides three observability pillars that work together:

| Pillar | Feature | Correlation |
|--------|---------|-------------|
| **Metrics** | Prometheus counters and histograms at `/metrics` | Aggregate performance data |
| **Logging** | Structured JSON access logs with `ACCESS_LOG` | Per-request detail, searchable by `trace_id` |
| **Tracing** | W3C Trace Context + OTLP export | End-to-end distributed request flow |

All three share the same `trace_id` and `request_id`, enabling seamless drill-down from a Grafana dashboard alert → Tempo trace → Loki log lines for a single request.

## Troubleshooting

### Trace headers not appearing in responses

`TRACE_CONTEXT` is not enabled.

**Fix:** Set `TRACE_CONTEXT=true` or enable the OTel plugin with `OTEL_ENABLED=true` (which enables trace context automatically).

### $_SERVER trace variables are empty

Trace context is disabled, or the variables are being checked outside of OxPHP.

**Check:** The `OXPHP_TRACE_ID`, `OXPHP_SPAN_ID`, and `OXPHP_PARENT_SPAN_ID` variables only exist when `TRACE_CONTEXT=true` and the request is served by OxPHP. Test with:

```php
<?php
echo $_SERVER['OXPHP_TRACE_ID'] ?? 'trace context not enabled';
```

### Spans not appearing in Jaeger/Tempo

**Check:** Verify the OTLP endpoint is reachable from the OxPHP container:

```bash
docker compose exec app curl -v http://jaeger:4317
```

**Check:** Verify the plugin is enabled:

```bash
curl -s http://localhost:9090/config | jq '.plugins'
```

**Fix:** Ensure `OTEL_ENABLED=true` and the `OTEL_EXPORTER_OTLP_ENDPOINT` points to the correct collector address.

### High sampling volume in production

Exporting every span is expensive at high traffic volumes.

**Fix:** Reduce the sampling ratio:

```bash
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1   # Sample 10% of traces
```

Parent-based sampling means that if an incoming request carries a sampled trace, it will always be sampled regardless of the ratio. New traces started at OxPHP are sampled at the configured rate. If `OTEL_TRACES_SAMPLER` is set to an unknown value, OxPHP logs a warning and falls back to `parentbased_traceidratio`.

## See Also

- [PHP Functions](../php/functions.md#oxphp_apm_start) -- `oxphp_apm_*()` function reference
- [Decorators](decorators.md) -- attribute-based function interception including `#[Trace]`
- [Access Logging](access-logging.md) -- structured JSON logs with trace fields
- [Request IDs](request-ids.md) -- how request IDs interact with trace context
- [Metrics](../operations/metrics.md) -- Prometheus metrics reference
- [Health Checks](../operations/health-checks.md) -- `/config` endpoint showing trace context status
- [Configuration Reference](../operations/configuration.md) -- all environment variables
