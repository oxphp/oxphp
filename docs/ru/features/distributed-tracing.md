---
title: Распределённая трассировка
description: Пропуск контекста трассировки W3C Trace Context и экспорт спанов OpenTelemetry
---

OxPHP поддерживает распределённую трассировку через два уровня: пропуск контекста W3C Trace Context (встроен, без зависимостей) и экспорт спанов OpenTelemetry (опционально, через feature `plugin-otel`).

## Архитектура

### Уровень 1: W3C Trace Context (встроенный)

При `TRACE_CONTEXT=true` OxPHP:

1. Разбирает входящие заголовки `traceparent` и `tracestate` в соответствии со спецификацией [W3C Trace Context](https://www.w3.org/TR/trace-context/)
2. Генерирует новый trace ID и span ID при отсутствии `traceparent`
3. Вставляет `traceparent` и `tracestate` в HTTP-ответ
4. Предоставляет trace ID в PHP через суперглобальные переменные `$_SERVER`
5. Включает `trace_id` и `span_id` в записи журнала доступа

Этот уровень не имеет внешних зависимостей и не добавляет сторонних крейтов в сборку.

### Уровень 2: Экспорт OpenTelemetry (feature `plugin-otel`)

При сборке с `--features plugin-otel` и `OTEL_ENABLED=true` OxPHP дополнительно:

1. Создаёт спаны OpenTelemetry для каждого запроса с семантическими HTTP-конвенциями
2. Экспортирует спаны в OTLP-коллектор через gRPC или HTTP/protobuf
3. Поддерживает настраиваемые семплирование, атрибуты ресурсов и заголовки аутентификации

Включение `OTEL_ENABLED` автоматически устанавливает `TRACE_CONTEXT=true`.

## Настройка

| Переменная | По умолчанию | Описание |
|------------|--------------|----------|
| `TRACE_CONTEXT` | `false` | Включить пропуск контекста W3C Trace Context |
| `OTEL_ENABLED` | `false` | Включить экспорт спанов OpenTelemetry (подразумевает `TRACE_CONTEXT=true`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | Эндпоинт OTLP-коллектора |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Протокол экспорта: `grpc` или `http/protobuf` |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Таймаут экспорта в миллисекундах |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(нет)* | Заголовки аутентификации (`key=value,key=value`) |
| `OTEL_SERVICE_NAME` | `oxphp` | Имя сервиса в экспортируемых трейсах |
| `OTEL_SERVICE_VERSION` | *(нет)* | Версия сервиса в экспортируемых трейсах |
| `OTEL_RESOURCE_ATTRIBUTES` | *(нет)* | Атрибуты ресурса (`key=value,key=value`) |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Стратегия семплирования |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Коэффициент семплирования (0.0-1.0) |

См. [Конфигурация](/operations/configuration.md) для полного справочника переменных окружения.

## Интеграция с PHP

При активном контексте трассировки для каждого запроса заполняются четыре переменные `$_SERVER`:

| Переменная | Описание |
|------------|----------|
| `$_SERVER['OXPHP_TRACE_ID']` | Идентификатор трассировки W3C (32 шестнадцатеричных символа) |
| `$_SERVER['OXPHP_SPAN_ID']` | Идентификатор текущего спана (16 шестнадцатеричных символов) |
| `$_SERVER['OXPHP_PARENT_SPAN_ID']` | Идентификатор родительского спана из входящего `traceparent` (пусто при отсутствии родителя) |
| `$_SERVER['HTTP_TRACEPARENT']` | Исходное значение заголовка `traceparent` |

### Корреляция логов

Используйте trace ID в логах вашего приложения для корреляции PHP-логов с распределёнными трейсами:

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

### Передача контекста downstream-сервисам

При выполнении исходящих HTTP-запросов передавайте заголовок `traceparent` для сохранения цепочки трассировки:

```php
<?php
$traceId  = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId   = $_SERVER['OXPHP_SPAN_ID'] ?? '';

// Генерация нового span ID для downstream-вызова
$childSpanId = bin2hex(random_bytes(8));
$traceparent = "00-{$traceId}-{$childSpanId}-01";

$ch = curl_init('https://api.internal/orders');
curl_setopt($ch, CURLOPT_HTTPHEADER, [
    "traceparent: {$traceparent}",
]);
curl_exec($ch);
```

## Корреляция в журнале доступа

При включённом `TRACE_CONTEXT` каждая запись журнала доступа включает `trace_id` и `span_id`:

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

При отключённом `TRACE_CONTEXT` эти поля не включаются.

## Быстрый старт

### Jaeger (локальная разработка)

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

Откройте `http://localhost:16686` для просмотра трейсов.

### Datadog

```bash
OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=http://datadog-agent:4317
OTEL_SERVICE_NAME=my-app
OTEL_RESOURCE_ATTRIBUTES=deployment.environment=production
```

Datadog Agent принимает OTLP на порту 4317 при настроенном `DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT`.

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

Для Grafana Cloud используйте HTTPS-эндпоинт с заголовками аутентификации:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=https://tempo-us-central1.grafana.net:443
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_HEADERS=Authorization=Basic YOUR_BASE64_CREDENTIALS
```

## Семплирование

Переменная `OTEL_TRACES_SAMPLER` определяет, для каких запросов создаются спаны:

| Семплер | Поведение |
|---------|-----------|
| `always_on` | Экспортировать каждый запрос |
| `always_off` | Ничего не экспортировать (контекст трассировки по-прежнему передаётся) |
| `traceidratio` | Экспортировать процент запросов на основе хеша trace ID |
| `parentbased_traceidratio` | Учитывать решение родителя о семплировании; семплировать корневые спаны по коэффициенту |

Используйте `OTEL_TRACES_SAMPLER_ARG` для задания коэффициента для семплеров на основе соотношения. Например, чтобы семплировать 10% корневых трейсов:

```bash
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1
```

Семплер `parentbased_traceidratio` (по умолчанию) рекомендуется для продакшна. Он учитывает решения о семплировании от вышестоящих сервисов, при этом применяя коэффициент к локально инициированным трейсам.

## Смотрите также

- [Идентификаторы запросов](request-ids.md) — идентификаторы на основе трейсов при активном OTel
- [Журнал доступа](access-logging.md) — поля `trace_id` и `span_id` в записях журнала
- [Жизненный цикл запроса](/architecture/request-lifecycle.md) — TraceContextHandler в конвейере событий
- [Конфигурация](/operations/configuration.md) — полный справочник переменных окружения
