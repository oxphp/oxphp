---
title: Размеркаваная трасіроўка
description: Распаўсюджванне W3C Trace Context і экспарт спанаў OpenTelemetry
---

OxPHP падтрымлівае размеркаваную трасіроўку праз два ўзроўні: распаўсюджванне W3C Trace Context (убудаванае, без залежнасцяў) і экспарт спанаў OpenTelemetry (апцыянальна, праз фічу `plugin-otel`).

## Архітэктура

### Узровень 1: W3C Trace Context (убудаваны)

Калі `TRACE_CONTEXT=true`, OxPHP:

1. Разбірае ўваходныя загалоўкі `traceparent` і `tracestate` згодна са спецыфікацыяй [W3C Trace Context](https://www.w3.org/TR/trace-context/)
2. Генеруе новы trace ID і span ID, калі `traceparent` адсутнічае
3. Устаўляе `traceparent` і `tracestate` у HTTP-адказ
4. Робіць trace ID даступным для PHP праз суперглабалы `$_SERVER`
5. Уключае `trace_id` і `span_id` у запісы журнала доступу

Гэты ўзровень не мае знешніх залежнасцяў і не дадае старонніх крэйтаў у зборку.

### Узровень 2: Экспарт OpenTelemetry (фіча `plugin-otel`)

Калі сабрана з `--features plugin-otel` і `OTEL_ENABLED=true`, OxPHP дадаткова:

1. Стварае спаны OpenTelemetry для кожнага запыту з семантычнымі HTTP-канвенцыямі
2. Экспартуе спаны ў калектар OTLP праз gRPC або HTTP/protobuf
3. Падтрымлівае наладжвальныя семплінг, атрыбуты рэсурсаў і загалоўкі аўтэнтыфікацыі

Уключэнне `OTEL_ENABLED` аўтаматычна ўсталёўвае `TRACE_CONTEXT=true`.

## Канфігурацыя

| Зменная | Па змаўчанні | Апісанне |
|----------|---------|-------------|
| `TRACE_CONTEXT` | `false` | Уключыць распаўсюджванне W3C Trace Context |
| `OTEL_ENABLED` | `false` | Уключыць экспарт спанаў OpenTelemetry (мяркуе `TRACE_CONTEXT=true`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | Канчатковая кропка калектара OTLP |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Пратакол экспарту: `grpc` або `http/protobuf` |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Тайм-аўт экспарту ў мілісекундах |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(няма)* | Загалоўкі аўтэнтыфікацыі (`key=value,key=value`) |
| `OTEL_SERVICE_NAME` | `oxphp` | Назва сэрвісу ў экспартаваных трэйсах |
| `OTEL_SERVICE_VERSION` | *(няма)* | Версія сэрвісу ў экспартаваных трэйсах |
| `OTEL_RESOURCE_ATTRIBUTES` | *(няма)* | Атрыбуты рэсурсаў (`key=value,key=value`) |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Стратэгія семплінгу |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Каэфіцыент семплінгу (0.0-1.0) |

Глядзіце [Канфігурацыя](/be/operations/configuration.md) для поўнай даведкі па зменных асяроддзя.

## Інтэграцыя з PHP

Калі кантэкст трасіроўкі актыўны, чатыры зменныя `$_SERVER` запаўняюцца для кожнага запыту:

| Зменная | Апісанне |
|----------|-------------|
| `$_SERVER['OXPHP_TRACE_ID']` | W3C trace ID (32 шаснаццатковыя сімвалы) |
| `$_SERVER['OXPHP_SPAN_ID']` | Бягучы span ID (16 шаснаццатковых сімвалаў) |
| `$_SERVER['OXPHP_PARENT_SPAN_ID']` | Бацькоўскі span ID з уваходнага `traceparent` (пуста, калі бацькоўскі адсутнічае) |
| `$_SERVER['HTTP_TRACEPARENT']` | Неапрацаванае значэнне загалоўка `traceparent` |

### Карэляцыя логаў

Выкарыстоўвайце trace ID у журналах вашага прыкладання для карэляцыі PHP-логаў з размеркаванымі трэйсамі:

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

### Распаўсюджванне далей па ланцугу

Пры выкананні выходных HTTP-выклікаў перадавайце загаловак `traceparent` для падтрымання ланцуга трасіроўкі:

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

## Карэляцыя ў журнале доступу

Калі `TRACE_CONTEXT` уключаны, кожны запіс журнала доступу ўключае `trace_id` і `span_id`:

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

Калі `TRACE_CONTEXT` адключаны, гэтыя палі апускаюцца.

## Хуткі старт

### Jaeger (лакальная распрацоўка)

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

Адкрыйце `http://localhost:16686` для прагляду трэйсаў.

### Datadog

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://datadog-agent:4317
OTEL_SERVICE_NAME=my-app
OTEL_RESOURCE_ATTRIBUTES=deployment.environment=production
```

Datadog Agent прымае OTLP на порце 4317, калі сканфігуравана `DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT`.

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

Для Grafana Cloud выкарыстоўвайце HTTPS-эндпоінт з загалоўкамі аўтэнтыфікацыі:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=https://tempo-us-central1.grafana.net:443
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_HEADERS=Authorization=Basic YOUR_BASE64_CREDENTIALS
```

## Семплінг

Зменная `OTEL_TRACES_SAMPLER` кантралюе, якія запыты генеруюць спаны:

| Семплер | Паводзіны |
|---------|----------|
| `always_on` | Экспартаваць кожны запыт |
| `always_off` | Нічога не экспартаваць (кантэкст трасіроўкі ўсё роўна распаўсюджваецца) |
| `traceidratio` | Экспартаваць працэнт запытаў на аснове хэша trace ID |
| `parentbased_traceidratio` | Паважаць рашэнне бацькоўскага семплінгу; семпліраваць каранёвыя спаны па каэфіцыенту |

Выкарыстоўвайце `OTEL_TRACES_SAMPLER_ARG` для задання каэфіцыента для семплераў на аснове каэфіцыента. Напрыклад, каб семпліраваць 10% каранёвых трэйсаў:

```bash
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1
```

Семплер `parentbased_traceidratio` (па змаўчанні) рэкамендуецца для вытворчасці. Ён паважае рашэнні аб семплінгу вышэйстаячых сэрвісаў, прымяняючы каэфіцыент да лакальна ініцыяваных трэйсаў.

## Глядзіце таксама

- [Ідэнтыфікатары запытаў](request-ids.md) -- ідэнтыфікатары запытаў на аснове трэйса, калі OTel актыўны
- [Журнал доступу](access-logging.md) -- палі `trace_id` і `span_id` у запісах журнала
- [Жыццёвы цыкл запыту](/be/architecture/request-lifecycle.md) -- TraceContextHandler у канвееры падзей
- [Канфігурацыя](/be/operations/configuration.md) -- поўная даведка па зменных асяроддзя
