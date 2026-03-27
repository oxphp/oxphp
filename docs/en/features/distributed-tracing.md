---
title: Distributed Tracing
description: W3C Trace Context propagation, OpenTelemetry integration, and end-to-end observability with OxPHP.
---

# Distributed Tracing

OxPHP supports W3C Trace Context propagation and OpenTelemetry (OTel) export. Incoming `traceparent` headers are parsed and continued, trace IDs are available in PHP via `$_SERVER`, access logs include trace fields, and spans can be exported to Jaeger, Grafana Tempo, Zipkin, or any OTLP-compatible backend.

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

The OTel plugin is a compile-time feature (`plugin-otel`). When enabled, it automatically sets `TRACE_CONTEXT=true`.

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_ENABLED` | `false` | Enable the OpenTelemetry plugin. Set to `true` or `1` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Export protocol: `grpc` or `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` (gRPC) or `http://localhost:4318` (HTTP) | OTLP collector endpoint |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Export timeout in milliseconds |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(unset)* | Authentication headers: `key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | Service name in exported spans |
| `OTEL_SERVICE_VERSION` | *(unset)* | Service version attribute |
| `OTEL_RESOURCE_ATTRIBUTES` | *(unset)* | Additional resource attributes: `env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Sampling strategy: `always_on`, `always_off`, `traceidratio`, `parentbased_always_on`, `parentbased_always_off`, `parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Sampling ratio (0.0–1.0) for ratio-based samplers |

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
  "target": "access_log",
  "request_id": "4bf92f35-00f067aa",
  "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
  "span_id": "00f067aa0ba902b7",
  "method": "GET",
  "path": "/api/users",
  "status": 200,
  "duration_us": 1523
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

### Request ID with OTel

When the OTel plugin is active, request IDs are derived from the trace context: the first 16 characters of the trace ID and first 8 characters of the span ID, separated by a dash. This appears in logs, the `X-Request-ID` response header, and `oxphp_request_id()` in PHP.

## Docker Example

### Trace Context Only

Enable W3C trace propagation without an external backend:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.2.0
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
    image: ghcr.io/oxphp/oxphp:0.2.0
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
    image: ghcr.io/oxphp/oxphp:0.2.0
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

Parent-based sampling means that if an incoming request carries a sampled trace, it will always be sampled regardless of the ratio. New traces started at OxPHP are sampled at the configured rate.

## See Also

- [Access Logging](access-logging.md) -- structured JSON logs with trace fields
- [Request IDs](request-ids.md) -- how request IDs interact with trace context
- [Metrics](../operations/metrics.md) -- Prometheus metrics reference
- [Health Checks](../operations/health-checks.md) -- `/config` endpoint showing trace context status
- [Configuration Reference](../operations/configuration.md) -- all environment variables
