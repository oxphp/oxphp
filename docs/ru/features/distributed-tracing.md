---
title: Распределённая трассировка и APM
description: Распространение W3C Trace Context, интеграция с OpenTelemetry, автоматическая инструментация, PHP SDK трассировки и сквозная наблюдаемость в OxPHP.
---

# Распределённая трассировка и APM

OxPHP поддерживает распространение W3C Trace Context, экспорт данных в OpenTelemetry (OTel) и встроенный мониторинг производительности приложений (APM). Входящие заголовки `traceparent` анализируются и продолжают трассировку, идентификаторы трасс доступны в PHP через `$_SERVER`, журналы доступа содержат поля трассировки, а спаны можно экспортировать в Jaeger, Grafana Tempo, Zipkin или любой бэкенд, совместимый с OTLP.

Плагин APM добавляет три уровня трассировки поверх основы OTel:

- **Автоматическая инструментация** — внутренние функции PHP (PDO, mysqli, cURL, Redis, Memcached, файловый ввод-вывод) перехватываются на уровне движка; каждый вызов становится спаном без изменения кода
- **Трассировка через атрибуты** — аннотируйте любую PHP-функцию или метод атрибутом `#[OxPHP\Apm\Trace]` для автоматического создания спанов
- **PHP SDK** — 10 функций `oxphp_apm_*()` для ручного создания спанов, атрибутов, событий и записи ошибок

## Как это работает

1. **Входящий запрос** — OxPHP читает заголовки `traceparent` и `tracestate` согласно спецификации W3C Trace Context
2. **Новый спан** — для данного перехода генерируется новый идентификатор спана. Входящий идентификатор спана становится родительским
3. **Передача в PHP** — идентификаторы трассировки внедряются в `$_SERVER['OXPHP_TRACE_ID']`, `$_SERVER['OXPHP_SPAN_ID']` и `$_SERVER['OXPHP_PARENT_SPAN_ID']`
4. **Журнал доступа** — структурированные JSON-логи содержат поля `trace_id` и `span_id` для корреляции логов
5. **Заголовки ответа** — обновлённый заголовок `traceparent` (с идентификатором спана OxPHP) добавляется в ответ, чтобы нижестоящие сервисы могли продолжить трассировку
6. **Экспорт OTel** (опционально) — когда плагин OTel включён, каждый запрос становится спаном, экспортируемым через OTLP с атрибутами HTTP-семантических соглашений

Если заголовок `traceparent` отсутствует, OxPHP генерирует новые идентификаторы трассы и спана, начиная новую трассировку.

## Конфигурация

### W3C Trace Context (встроенный)

| Переменная | По умолчанию | Описание |
|------------|--------------|----------|
| `TRACE_CONTEXT` | `false` | Включить распространение W3C Trace Context. Установите `true` или `1` |

### Плагин OpenTelemetry

Плагин OTel является функцией времени компиляции (`plugin-otel`). При включении он автоматически устанавливает `TRACE_CONTEXT=true`.

| Переменная | По умолчанию | Описание |
|------------|--------------|----------|
| `OTEL_ENABLED` | `false` | Включить плагин OpenTelemetry. Установите `true` или `1` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | Протокол экспорта: `grpc` или `http/protobuf` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` (gRPC) или `http://localhost:4318` (HTTP) | Конечная точка OTLP-коллектора |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | Тайм-аут экспорта в миллисекундах |
| `OTEL_EXPORTER_OTLP_HEADERS` | *(не задано)* | Заголовки аутентификации: `key=value,key2=value2` |
| `OTEL_SERVICE_NAME` | `oxphp` | Имя сервиса в экспортируемых спанах |
| `OTEL_SERVICE_VERSION` | *(не задано)* | Атрибут версии сервиса |
| `OTEL_RESOURCE_ATTRIBUTES` | *(не задано)* | Дополнительные атрибуты ресурса: `env=prod,region=us-east-1` |
| `OTEL_TRACES_SAMPLER` | `parentbased_traceidratio` | Стратегия выборки: `always_on`, `always_off`, `traceidratio`, `parentbased_always_on`, `parentbased_always_off`, `parentbased_traceidratio` |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Коэффициент выборки (0.0–1.0) для семплеров на основе соотношения |

### Плагин APM

Плагин APM является функцией времени компиляции (`plugin-apm`), зависящей от плагина OTel. Он добавляет автоматическую инструментацию, декоратор `#[OxPHP\Apm\Trace]` и PHP SDK трассировки.

| Переменная | По умолчанию | Описание |
|------------|--------------|----------|
| `OTEL_APM_ENABLED` | `false` | Включить APM: автоинструментацию, захват ошибок, PHP SDK. Требует `OTEL_ENABLED=true` |
| `OTEL_APM_SLOW_QUERY_MS` | `100` | Порог медленного запроса в миллисекундах. Запросы свыше этого значения получают атрибут `oxphp.db.slow=true` на своих спанах |
| `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` | `false` | Записывать параметры привязки в атрибут спана `db.params` |

## Контекст трассировки в PHP

Когда `TRACE_CONTEXT=true`, в PHP-скриптах доступны три переменные `$_SERVER`:

| Переменная | Описание | Пример |
|------------|----------|--------|
| `OXPHP_TRACE_ID` | Идентификатор трассы W3C (32 шестнадцатеричных символа) | `4bf92f3577b34da6a3ce929d0e0e4736` |
| `OXPHP_SPAN_ID` | Идентификатор спана OxPHP для данного запроса (16 шестнадцатеричных символов) | `00f067aa0ba902b7` |
| `OXPHP_PARENT_SPAN_ID` | Входящий идентификатор родительского спана (16 шестнадцатеричных символов, пустой для новой трассы) | `a3ce929d0e0e4736` |

Используйте их для передачи контекста трассировки нижестоящим сервисам:

```php
<?php
$traceId  = $_SERVER['OXPHP_TRACE_ID'] ?? '';
$spanId   = $_SERVER['OXPHP_SPAN_ID'] ?? '';

if ($traceId) {
    // Сформировать заголовок traceparent для нижестоящих вызовов
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

### С Guzzle

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

## Корреляция журналов доступа

Когда контекст трассировки включён, структурированные JSON-журналы доступа содержат поля `trace_id` и `span_id`:

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
    "remote_addr": "10.0.0.1:54321",
    "message": "request completed"
  }
}
```

Это позволяет выполнять поиск логов по идентификатору трассы в системах агрегации журналов (Loki, Elasticsearch, Splunk, CloudWatch) для поиска всех записей, относящихся к распределённой трассировке.

## Заголовки ответа

OxPHP добавляет заголовок `traceparent` в каждый ответ с собственным идентификатором спана:

```http
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
```

Если входящий запрос содержал заголовок `tracestate`, он также передаётся в ответе.

## Интеграция с OpenTelemetry

Когда плагин OTel включён, каждый HTTP-запрос становится спаном, экспортируемым в ваш бэкенд трассировки через OTLP.

### Атрибуты спана

Экспортируемые спаны содержат стандартные атрибуты HTTP-семантических соглашений:

| Атрибут | Описание |
|---------|----------|
| `http.request.method` | HTTP-метод (GET, POST и т.д.) |
| `url.path` | Путь запроса |
| `http.response.status_code` | Код статуса ответа |
| `client.address` | IP-адрес клиента |
| `server.address` | Адрес прослушивания сервера |
| `oxphp.request_id` | Идентификатор запроса OxPHP |
| `http.request.body.size` | Размер тела запроса в байтах (если не равен нулю) |
| `http.response.body.size` | Размер тела ответа в байтах (если не равен нулю) |

Ответы 5xx помечаются как спаны с ошибкой.

### Идентификатор запроса с OTel

Когда плагин OTel активен, идентификаторы запросов формируются из контекста трассировки: первые 16 символов идентификатора трассы и первые 8 символов идентификатора спана, разделённые дефисом. Это значение появляется в логах, заголовке ответа `X-Request-ID` и в `oxphp_request_id()` в PHP.

## APM: автоматическая инструментация

Когда плагин APM включён, OxPHP автоматически перехватывает 33 внутренние функции PHP на уровне движка. Каждый вызов перехваченной функции создаёт дочерний спан под корневым спаном текущего запроса — без каких-либо изменений кода.

### Перехватываемые функции

| Категория | Функции |
|-----------|---------|
| **PDO** | `PDO::__construct`, `PDO::query`, `PDO::exec`, `PDO::prepare`, `PDOStatement::execute` |
| **mysqli** | `mysqli::__construct`, `mysqli::query`, `mysqli::prepare`, `mysqli_stmt::execute` |
| **cURL** | `curl_init`, `curl_setopt`, `curl_exec`, `curl_multi_exec` |
| **Redis** | `Redis::connect`, `Redis::get`, `Redis::set`, `Redis::del`, `Redis::mget`, `Redis::mset`, `Redis::hget`, `Redis::hset`, `Redis::lpush`, `Redis::rpush` |
| **Memcached** | `Memcached::get`, `Memcached::set`, `Memcached::delete`, `Memcached::getMulti`, `Memcached::setMulti` |
| **Файловый ввод-вывод** | `fopen`, `fread`, `fwrite`, `file_get_contents`, `file_put_contents` |

Хуки устанавливаются только для расширений, которые фактически загружены. Если в вашей сборке отсутствует расширение Redis, хуки Redis будут молча пропущены.

### Как это работает

Установка хуков использует двухфазный дизайн для потокобезопасности в режиме PHP ZTS:

1. **Фаза 1 (MINIT)** — во время инициализации модуля OxPHP проверяет каждую целевую функцию по загруженным расширениям и сохраняет указатели на оригинальные обработчики в список одобренных (только для чтения)
2. **Фаза 2 (RINIT)** — при первом запросе на каждом рабочем потоке одобренные хуки устанавливаются в таблицы функций данного потока

Это гарантирует, что каждый рабочий поток ZTS имеет согласованные модификации таблиц функций и потоколокальное состояние.

## APM: трассировка через атрибуты

Атрибут `#[OxPHP\Apm\Trace]` автоматически создаёт спаны вокруг декорированных функций и методов. В отличие от хуков автоинструментации (которые нацелены на внутренние C-функции), это работает с пользовательским PHP-кодом.

```php
<?php
use OxPHP\Apm\Trace;

#[Trace]
function processOrder(int $orderId): void
{
    // A span named "processOrder" is created on entry and closed on exit.
    // If an exception is thrown, the span is marked as error with the
    // exception class recorded as a span event.
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

Атрибут `#[Trace]` применяется как к функциям, так и к методам. Вызов регистрации не требуется — плагин APM автоматически регистрирует декоратор во время инициализации.

Если декорированная функция выбрасывает исключение, статус спана устанавливается как ошибочный, и записывается событие `exception` с именем класса исключения.

## APM: PHP SDK трассировки

Плагин APM регистрирует 10 функций `oxphp_apm_*()` для ручного управления спанами. Все функции являются безопасными no-op, когда APM отключён, поэтому ваш код работает без изменений в любом окружении.

### Создание спанов

```php
<?php
// Start a span and get its local ID
$spanId = oxphp_apm_start('cache.warm', ['cache.size' => '1024']);

// ... do work ...

// Close the span
oxphp_apm_end($spanId);
```

### Добавление атрибутов и событий

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

### Запись ошибок

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

### Передача контекста трассировки

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

### Справочник функций

| Функция | Возвращает | Описание |
|---------|------------|----------|
| `oxphp_apm_trace(name, callback, ?attributes)` | `void` | Выполнить callback внутри спана (зарезервировано для будущего использования) |
| `oxphp_apm_start(name, ?attributes)` | `int` | Открыть спан и вернуть его локальный ID. `0`, когда APM отключён |
| `oxphp_apm_end(span_id)` | `void` | Закрыть спан с указанным локальным ID |
| `oxphp_apm_attribute(key, value, ?span_id)` | `void` | Установить атрибут на текущем или указанном спане |
| `oxphp_apm_event(name, ?attributes, ?span_id)` | `void` | Записать событие с меткой времени на текущем или указанном спане |
| `oxphp_apm_error(exception, ?span_id)` | `void` | Отметить текущий или указанный спан как ошибочный |
| `oxphp_apm_status(code, ?description, ?span_id)` | `void` | Установить статус спана: `0` = Не задан, `1` = Ok, `2` = Ошибка |
| `oxphp_apm_trace_id()` | `string` | Текущий идентификатор трассы (32 шестнадцатеричных символа). Пустой, когда APM отключён |
| `oxphp_apm_span_id()` | `string` | Текущий идентификатор спана (16 шестнадцатеричных символов). Пустой, когда нет активного спана |
| `oxphp_apm_header()` | `string` | Значение W3C-заголовка `traceparent` для текущего контекста спана |

Полный справочник сигнатур функций см. в разделе [PHP Functions](../php/functions.md#oxphp_apm_start).

## Пример Docker

### Только Trace Context

Включение распространения W3C-трассировки без внешнего бэкенда:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
    ports:
      - "80:80"
    environment:
      - TRACE_CONTEXT=true
      - INTERNAL_ADDR=0.0.0.0:9090
```

### С Jaeger

Полный стек наблюдаемости с Jaeger в качестве бэкенда трассировки:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
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

### С Grafana Tempo

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
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

### С Jaeger и APM

Полная наблюдаемость с автоматической инструментацией запросов к базам данных, HTTP-вызовов, операций с кэшем и файлового ввода-вывода:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.3.0
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

> **Примечание:** Cargo-функция `plugin-apm` должна быть включена во время сборки. Официальный образ OxPHP включает её по умолчанию.

## Стек наблюдаемости

OxPHP обеспечивает три компонента наблюдаемости, которые работают совместно:

| Компонент | Функция | Корреляция |
|-----------|---------|------------|
| **Метрики** | Счётчики и гистограммы Prometheus по адресу `/metrics` | Агрегированные данные производительности |
| **Логирование** | Структурированные JSON-журналы доступа с `ACCESS_LOG` | Детальная информация по каждому запросу, поиск по `trace_id` |
| **Трассировка** | W3C Trace Context + экспорт OTLP | Сквозной поток распределённых запросов |

Все три компонента используют общие `trace_id` и `request_id`, обеспечивая бесшовный переход от алерта в дашборде Grafana → трассировке в Tempo → строкам лога в Loki для отдельного запроса.

## Устранение неполадок

### Заголовки трассировки не появляются в ответах

`TRACE_CONTEXT` не включён.

**Решение:** установите `TRACE_CONTEXT=true` или включите плагин OTel с `OTEL_ENABLED=true` (что автоматически включает контекст трассировки).

### Переменные трассировки в $_SERVER пусты

Контекст трассировки отключён, или переменные проверяются вне OxPHP.

**Проверьте:** переменные `OXPHP_TRACE_ID`, `OXPHP_SPAN_ID` и `OXPHP_PARENT_SPAN_ID` существуют только при `TRACE_CONTEXT=true` и когда запрос обслуживается OxPHP. Проверьте так:

```php
<?php
echo $_SERVER['OXPHP_TRACE_ID'] ?? 'trace context not enabled';
```

### Спаны не появляются в Jaeger/Tempo

**Проверьте:** убедитесь, что конечная точка OTLP доступна из контейнера OxPHP:

```bash
docker compose exec app curl -v http://jaeger:4317
```

**Проверьте:** убедитесь, что плагин включён:

```bash
curl -s http://localhost:9090/config | jq '.plugins'
```

**Решение:** убедитесь, что `OTEL_ENABLED=true` и `OTEL_EXPORTER_OTLP_ENDPOINT` указывает на правильный адрес коллектора.

### Большой объём выборки в производственной среде

Экспортировать каждый спан дорого при высоких нагрузках.

**Решение:** уменьшите коэффициент выборки:

```bash
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1   # Выбирать 10% трасс
```

Выборка на основе родительского трассировщика означает, что если входящий запрос содержит трассировку с выборкой, она всегда будет включена в выборку независимо от заданного коэффициента. Новые трассировки, начатые в OxPHP, попадают в выборку с настроенной частотой.

## См. также

- [PHP Functions](../php/functions.md#oxphp_apm_start) — справочник по функциям `oxphp_apm_*()`
- [Decorators](decorators.md) — перехват функций через атрибуты, включая `#[Trace]`
- [Access Logging](access-logging.md) — структурированные JSON-логи с полями трассировки
- [Request IDs](request-ids.md) — взаимодействие идентификаторов запросов с контекстом трассировки
- [Metrics](../operations/metrics.md) — справочник по метрикам Prometheus
- [Health Checks](../operations/health-checks.md) — эндпоинт `/config` с отображением статуса контекста трассировки
- [Configuration Reference](../operations/configuration.md) — все переменные окружения
