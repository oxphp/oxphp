---
title: Distributed Tracing
description: W3C Trace Context propagation and OpenTelemetry span export
---

OxPHP supports distributed tracing through two layers: W3C Trace Context propagation (built-in, zero dependencies) and OpenTelemetry span export (opt-in via the `plugin-otel` feature).

## Architecture

### Layer 1: W3C Trace Context (built-in)

When `TRACE_CONTEXT=true`, OxPHP:

1. Parses incoming `traceparent` and `tracestate` headers per the [W3C Trace Context](https://www.w3.org/TR/trace-context/) specification
2. Generates a new trace ID and span ID when no `traceparent` is present
3. Injects `traceparent` and `tracestate` into the HTTP response
4. Exposes trace IDs to PHP via `$_SERVER` superglobals
5. Includes `trace_id` and `span_id` in access log entries

This layer has no external dependencies and adds no third-party crates to the build.

### Layer 2: OpenTelemetry Export (`plugin-otel` feature)

When built with `--features plugin-otel` and `OTEL_ENABLED=true`, OxPHP additionally:

1. Creates OpenTelemetry spans for each request with semantic HTTP conventions
2. Exports spans to an OTLP collector via gRPC or HTTP/protobuf
3. Supports configurable sampling, resource attributes, and authentication headers

Enabling `OTEL_ENABLED` automatically sets `TRACE_CONTEXT=true`.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `TRACE_CONTEXT` | `false` | Enable W3C Trace Context propagation |
| `OTEL_ENABLED` | `false` | Enable OpenTelemetry span export (implies `TRACE_CONTEXT=true`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP collector endpoint |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Export protocol: `grpc` or `http/protobuf` |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Export timeout in milliseconds |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(none)* | Authentication headers (`key=value,key=value`) |
| `OTEL_SERVICE_NAME` | `oxphp` | Service name in exported traces |
| `OTEL_SERVICE_VERSION` | *(none)* | Service version in exported traces |
| `OTEL_RESOURCE_ATTRIBUTES` | *(none)* | Resource attributes (`key=value,key=value`) |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Sampling strategy |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Sampling ratio (0.0-1.0) |

See [Configuration](/operations/configuration.md) for the full environment variable reference.

## PHP Integration

When trace context is active, four `$_SERVER` variables are populated for every request:

| Variable | Description |
|----------|-------------|
| `$_SERVER['OXPHP_TRACE_ID']` | W3C trace ID (32 hex chars) |
| `$_SERVER['OXPHP_SPAN_ID']` | Current span ID (16 hex chars) |
| `$_SERVER['OXPHP_PARENT_SPAN_ID']` | Parent span ID from incoming `traceparent` (empty if no parent) |
| `$_SERVER['HTTP_TRACEPARENT']` | Raw `traceparent` header value |

### Log correlation

Use the trace ID in your application logs to correlate PHP-level logs with distributed traces:

```php
<?php
$traceId = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId  = $_SERVER['OXPHP_SPAN_ID'] ?? '';

error_log(json_encode([
    'trace_id' => $traceId,
    'span_id'  => $spanId,
    'message'  => 'Processing payment',
    'order_id' => $orderId,
]));
```

### Downstream propagation

When making outbound HTTP calls, forward the `traceparent` header to maintain the trace chain:

```php
<?php
$traceId  = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId   = $_SERVER['OXPHP_SPAN_ID'] ?? '';

// Generate a new span ID for the downstream call
$childSpanId = bin2hex(random_bytes(8));
$traceparent = "00-{$traceId}-{$childSpanId}-01";

$ch = curl_init('https://api.internal/orders');
curl_setopt($ch, CURLOPT_HTTPHEADER, [
    "traceparent: {$traceparent}",
]);
curl_exec($ch);
```

## Access Log Correlation

When `TRACE_CONTEXT` is enabled, every access log entry includes `trace_id` and `span_id`:

```json
{
  "timestamp": "2026-02-11T12:34:56.789Z",
  "level": "INFO",
  "fields": {
    "request_id": "67890abc00000042",
    "trace_id": "4bf92f3577b16e8264cabd64a999f321",
    "span_id": "a1b2c3d4e5f6a7b8",
    "method": "GET",
    "path": "/api/users",
    "status": 200,
    "duration_us": 1234,
    "remote_addr": "10.0.0.1:54321",
    "message": "request completed"
  }
}
```

When `TRACE_CONTEXT` is disabled, these fields are omitted.

## Quick Start

### Jaeger (local development)

```yaml
services:
  jaeger:
    image: jaegertracing/all-in-one:latest
    ports:
      - "16686:16686"   # Jaeger UI
      - "4317:4317"     # OTLP gRPC

  oxphp:
    image: oxphp:latest
    environment:
      OTEL_ENABLED: "true"
      OTEL_EXPORTER_OTLP_ENDPOINT: "http://jaeger:4317"
      OTEL_SERVICE_NAME: "my-app"
    ports:
      - "8080:8080"
```

Open `http://localhost:16686` to view traces.

### Datadog

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://datadog-agent:4317
OTEL_SERVICE_NAME=my-app
OTEL_RESOURCE_ATTRIBUTES=deployment.environment=production
```

The Datadog Agent accepts OTLP on port 4317 when `DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT` is configured.

### New Relic

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=https://otlp.nr-data.net:4317
OTEL_EXPORTER_OTLP_HEADERS=api-key=YOUR_INGEST_LICENSE_KEY
OTEL_SERVICE_NAME=my-app
```

### Grafana Tempo

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://tempo:4317
OTEL_SERVICE_NAME=my-app
```

For Grafana Cloud, use the HTTPS endpoint with authentication headers:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=https://tempo-us-central1.grafana.net:443
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_HEADERS=Authorization=Basic YOUR_BASE64_CREDENTIALS
```

## Sampling

The `OTEL_TRACES_SAMPLER` variable controls which requests generate spans:

| Sampler | Behavior |
|---------|----------|
| `always_on` | Export every request |
| `always_off` | Export nothing (trace context still propagated) |
| `traceidratio` | Export a percentage of requests based on trace ID hash |
| `parentbased_traceidratio` | Respect the parent's sampling decision; sample root spans by ratio |

Use `OTEL_TRACES_SAMPLER_ARG` to set the ratio for ratio-based samplers. For example, to sample 10% of root traces:

```bash
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1
```

The `parentbased_traceidratio` sampler (default) is recommended for production. It respects upstream sampling decisions while applying a ratio to locally-originated traces.

## See Also

- [Request IDs](request-ids.md) -- trace-derived request IDs when OTel is active
- [Access Logging](access-logging.md) -- `trace_id` and `span_id` fields in log entries
- [Request Lifecycle](/architecture/request-lifecycle.md) -- TraceContextHandler in the event pipeline
- [Configuration](/operations/configuration.md) -- full environment variable reference
