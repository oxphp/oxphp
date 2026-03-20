---
title: Метрики
description: Метрики, совместимые с Prometheus, предоставляемые внутренним сервером
---

OxPHP предоставляет метрики в текстовом формате Prometheus на `GET /metrics` внутреннего сервера. Все счётчики используют неблокирующие атомарные операции с порядком `Relaxed` для минимального влияния на производительность обработки запросов.

## Включение метрик

Метрики доступны, когда работает внутренний сервер. Установите переменную окружения `INTERNAL_ADDR`:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Затем собирайте данные через Prometheus или любой совместимый коллектор:

```bash
curl http://localhost:9090/metrics
```

## Справочник метрик

### Сервер

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_uptime_seconds` | gauge | Время работы серверного процесса в секундах |

### Запросы

| Метрика | Тип | Метки | Описание |
|---------|-----|-------|----------|
| `oxphp_requests_total` | counter | --- | Общее количество HTTP-запросов, полученных на основном порту |
| `oxphp_requests_by_method_total` | counter | `method` | Запросы в разбивке по HTTP-методу |
| `oxphp_responses_by_status_total` | counter | `status` | Ответы в разбивке по классу статуса |
| `oxphp_response_time_us_total` | counter | --- | Суммарное время ответа в микросекундах |
| `oxphp_request_duration_us` | histogram | --- | Длительность запроса в микросекундах (все запросы) |
| `oxphp_request_bytes_total` | counter | --- | Общий объём полученных байт тела запроса |
| `oxphp_response_bytes_total` | counter | --- | Общий объём отправленных байт тела ответа |

**Метки методов:** `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `OTHER`.

**Метки статусов:** `1xx`, `2xx`, `3xx`, `4xx`, `5xx`.

Выводятся только методы и классы статусов, для которых зафиксировано хотя бы одно событие. Метки с нулевыми счётчиками опускаются для компактности вывода.

#### Гистограмма длительности запросов

Границы бакетов (микросекунды): 100, 500, 1000, 2500, 5000, 10000, 25000, 50000, 100000, 250000, 500000, 1000000, +Inf.

Эта гистограмма охватывает все запросы (статические файлы и PHP), в отличие от `oxphp_worker_request_duration_us`, которая измеряет только время выполнения PHP-обработчика. `_sum` переиспользует `oxphp_response_time_us_total` (то же значение, один атомик), а `_count` вычисляется как сумма всех счётчиков `oxphp_responses_by_status_total`.

### Соединения

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_active_connections` | gauge | Текущее количество открытых TCP-соединений на основном порту |

### Очередь PHP-воркеров

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_pending_requests` | gauge | Запросы, ожидающие в очереди PHP-воркеров |
| `oxphp_dropped_requests_total` | counter | Запросы, отклонённые с кодом 529, поскольку очередь заполнена |
| `oxphp_busy_workers` | gauge | Потоки воркеров, обрабатывающие запрос в данный момент |
| `oxphp_queue_wait_us` | histogram | Время ожидания в очереди до подхвата воркером (микросекунды) |

#### Гистограмма ожидания в очереди

Границы бакетов (микросекунды): 50, 100, 250, 500, 1000, 2500, 5000, 10000, 50000, +Inf.

Измеряет время между отправкой запроса в очередь воркеров и его подхватом воркером. Помогает выявлять давление на очередь и нехватку воркеров.

### Пул PHP-воркеров

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_workers_current` | gauge | Текущее количество потоков воркеров в пуле |
| `oxphp_workers_min` | gauge | Минимальное количество потоков воркеров (равно текущему в статическом режиме) |
| `oxphp_workers_max` | gauge | Максимальное количество потоков воркеров (равно текущему в статическом режиме) |
| `oxphp_workers_idle` | gauge | Потоки воркеров, не обрабатывающие запрос в данный момент (только в динамическом режиме, 0 в статическом) |
| `oxphp_workers_spawned_total` | counter | Общее количество воркеров, запущенных с момента старта (включая начальных) |
| `oxphp_workers_retired_total` | counter | Общее количество воркеров, выведенных ScaleManager (только в динамическом режиме) |

### Ограничение частоты запросов

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_rate_limited_total` | counter | Запросы, отклонённые ограничителем частоты (429) |

Этот счётчик увеличивается каждый раз, когда запрос отклоняется с ответом 429. Выводится только при включённом ограничении частоты (`RATE_LIMIT` > 0).

### Кеш статических файлов

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_static_cache_hits_total` | counter | Запросы статических файлов, обслуженные из кеша содержимого |
| `oxphp_static_cache_misses_total` | counter | Запросы статических файлов, потребовавшие дискового ввода-вывода |

Коэффициент попаданий в кеш вычисляется как `hits / (hits + misses)`. Низкий коэффициент может указывать на то, что бюджет кеша содержимого (64 МБ) слишком мал для рабочего набора.

### Сжатие

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_compressed_responses_total` | counter | Ответы, сжатые с помощью Brotli |
| `oxphp_compression_bytes_saved_total` | counter | Общее количество байт, сэкономленных сжатием (оригинал − сжатый) |

Эти счётчики увеличиваются только при включённом сжатии (`COMPRESSION_LEVEL` > 0) и когда ответ действительно сжат (сжатый вывод меньше оригинала).

### Асинхронные задачи

Эти метрики выводятся только при активном асинхронном пуле (`ASYNC_WORKERS` > 0) и когда хотя бы одна задача была отправлена или отклонена.

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_async_tasks_dispatched_total` | counter | Общее количество задач, отправленных через `oxphp_async()` |
| `oxphp_async_tasks_completed_total` | counter | Задачи, успешно вернувшие значение |
| `oxphp_async_tasks_failed_total` | counter | Задачи, выбросившие исключение или вызвавшие `die()`/`exit()` |
| `oxphp_async_tasks_cancelled_total` | counter | Задачи, отменённые (истёк таймаут или очистка при RSHUTDOWN) |
| `oxphp_async_tasks_rejected_total` | counter | Задачи, отклонённые из-за заполненности асинхронной очереди |

Долю успешных задач можно вычислить как `completed / dispatched`. Высокое значение `rejected` указывает на то, что `ASYNC_QUEUE_CAPACITY` слишком мал или количество `ASYNC_WORKERS` недостаточно.

### Режим воркера

Эти метрики выводятся только при активном режиме воркера (установлен `WORKER_FILE`). Они обеспечивают наблюдаемость жизненного цикла персистентных PHP-воркеров, рециклирования и времени выполнения каждого запроса.

#### Глобальные счётчики

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_worker_mode_enabled` | gauge | Всегда `1`, когда режим воркера активен |
| `oxphp_worker_requests_handled_total` | counter | Общее количество запросов, обработанных персистентными воркерами |
| `oxphp_worker_recycles_total` | counter | Общее количество рециклирований воркеров (воркер завершился и был пересоздан) |
| `oxphp_worker_recycles_by_reason_total` | counter | Рециклирования в разбивке по причине. Метки: `reason="max_requests"`, `reason="max_memory"`, `reason="error"` |
| `oxphp_worker_soft_resets_total` | counter | Общее количество мягких сбросов между запросами (должно равняться `requests_handled_total`) |

#### Метрики отдельных воркеров

| Метрика | Тип | Метки | Описание |
|---------|-----|-------|----------|
| `oxphp_worker_memory_bytes` | gauge | `worker` | Текущее использование PHP-кучи в байтах для каждого воркера |
| `oxphp_worker_uptime_seconds` | gauge | `worker` | Время в секундах с момента создания потока воркера |
| `oxphp_worker_requests_count` | gauge | `worker` | Количество запросов, обработанных данным экземпляром воркера |

Метрики отдельных воркеров индексируются по слоту воркера (например, `worker="0"`, `worker="1"`). Значения выводятся только для активных воркеров.

#### Гистограмма длительности запросов

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_worker_request_duration_us` | histogram | Время выполнения PHP-обработчика на каждый запрос в микросекундах |

Границы бакетов (микросекунды): 100, 250, 500, 1000, 2500, 5000, 10000, 25000, 50000, +Inf.

Эта гистограмма измеряет время, затраченное на выполнение callback-обработчика PHP, исключая время ожидания в очереди. Она помогает выявлять медленные запросы и хвостовые задержки.

## Пример вывода

```
# HELP oxphp_uptime_seconds Server uptime in seconds.
# TYPE oxphp_uptime_seconds gauge
oxphp_uptime_seconds 3612

# HELP oxphp_requests_total Total HTTP requests.
# TYPE oxphp_requests_total counter
oxphp_requests_total 48203

# HELP oxphp_requests_by_method_total Requests by HTTP method.
# TYPE oxphp_requests_by_method_total counter
oxphp_requests_by_method_total{method="GET"} 42100
oxphp_requests_by_method_total{method="POST"} 6103

# HELP oxphp_responses_by_status_total Responses by status class.
# TYPE oxphp_responses_by_status_total counter
oxphp_responses_by_status_total{status="2xx"} 47500
oxphp_responses_by_status_total{status="4xx"} 650
oxphp_responses_by_status_total{status="5xx"} 53

# HELP oxphp_active_connections Current active connections.
# TYPE oxphp_active_connections gauge
oxphp_active_connections 7

# HELP oxphp_pending_requests Requests waiting in queue.
# TYPE oxphp_pending_requests gauge
oxphp_pending_requests 2

# HELP oxphp_dropped_requests_total Requests dropped (529).
# TYPE oxphp_dropped_requests_total counter
oxphp_dropped_requests_total 0

# HELP oxphp_response_time_us_total Total response time in microseconds.
# TYPE oxphp_response_time_us_total counter
oxphp_response_time_us_total 192000000

# HELP oxphp_request_duration_us Request duration in microseconds.
# TYPE oxphp_request_duration_us histogram
oxphp_request_duration_us_bucket{le="100"} 5200
oxphp_request_duration_us_bucket{le="500"} 18400
oxphp_request_duration_us_bucket{le="1000"} 31000
oxphp_request_duration_us_bucket{le="2500"} 40200
oxphp_request_duration_us_bucket{le="5000"} 44800
oxphp_request_duration_us_bucket{le="10000"} 46900
oxphp_request_duration_us_bucket{le="25000"} 47600
oxphp_request_duration_us_bucket{le="50000"} 47900
oxphp_request_duration_us_bucket{le="100000"} 48100
oxphp_request_duration_us_bucket{le="250000"} 48180
oxphp_request_duration_us_bucket{le="500000"} 48200
oxphp_request_duration_us_bucket{le="1000000"} 48203
oxphp_request_duration_us_bucket{le="+Inf"} 48203
oxphp_request_duration_us_sum 192000000
oxphp_request_duration_us_count 48203

# HELP oxphp_request_bytes_total Total request body bytes received.
# TYPE oxphp_request_bytes_total counter
oxphp_request_bytes_total 15360000

# HELP oxphp_response_bytes_total Total response body bytes sent.
# TYPE oxphp_response_bytes_total counter
oxphp_response_bytes_total 482030000

# HELP oxphp_busy_workers Currently busy worker threads.
# TYPE oxphp_busy_workers gauge
oxphp_busy_workers 2

# HELP oxphp_workers_current Current number of worker threads.
# TYPE oxphp_workers_current gauge
oxphp_workers_current 8

# HELP oxphp_workers_min Minimum worker thread count.
# TYPE oxphp_workers_min gauge
oxphp_workers_min 2

# HELP oxphp_workers_max Maximum worker thread count.
# TYPE oxphp_workers_max gauge
oxphp_workers_max 16

# HELP oxphp_workers_idle Currently idle worker threads.
# TYPE oxphp_workers_idle gauge
oxphp_workers_idle 6

# HELP oxphp_workers_spawned_total Total workers spawned.
# TYPE oxphp_workers_spawned_total counter
oxphp_workers_spawned_total 12

# HELP oxphp_workers_retired_total Total workers retired.
# TYPE oxphp_workers_retired_total counter
oxphp_workers_retired_total 4

# HELP oxphp_queue_wait_us Time waiting in queue before worker pickup.
# TYPE oxphp_queue_wait_us histogram
oxphp_queue_wait_us_bucket{le="50"} 20100
oxphp_queue_wait_us_bucket{le="100"} 35400
oxphp_queue_wait_us_bucket{le="250"} 42000
oxphp_queue_wait_us_bucket{le="500"} 45600
oxphp_queue_wait_us_bucket{le="1000"} 47200
oxphp_queue_wait_us_bucket{le="2500"} 47900
oxphp_queue_wait_us_bucket{le="5000"} 48100
oxphp_queue_wait_us_bucket{le="10000"} 48180
oxphp_queue_wait_us_bucket{le="50000"} 48203
oxphp_queue_wait_us_bucket{le="+Inf"} 48203
oxphp_queue_wait_us_sum 4820300
oxphp_queue_wait_us_count 48203

# HELP oxphp_rate_limited_total Requests rejected by rate limiter.
# TYPE oxphp_rate_limited_total counter
oxphp_rate_limited_total 23

# HELP oxphp_static_cache_hits_total Static file cache hits.
# TYPE oxphp_static_cache_hits_total counter
oxphp_static_cache_hits_total 12400

# HELP oxphp_static_cache_misses_total Static file cache misses.
# TYPE oxphp_static_cache_misses_total counter
oxphp_static_cache_misses_total 350

# HELP oxphp_compressed_responses_total Responses compressed with brotli.
# TYPE oxphp_compressed_responses_total counter
oxphp_compressed_responses_total 38500

# HELP oxphp_compression_bytes_saved_total Bytes saved by compression.
# TYPE oxphp_compression_bytes_saved_total counter
oxphp_compression_bytes_saved_total 96250000

# HELP oxphp_async_tasks_dispatched_total Total async tasks dispatched.
# TYPE oxphp_async_tasks_dispatched_total counter
oxphp_async_tasks_dispatched_total 1250

# HELP oxphp_async_tasks_completed_total Async tasks completed successfully.
# TYPE oxphp_async_tasks_completed_total counter
oxphp_async_tasks_completed_total 1200

# HELP oxphp_async_tasks_failed_total Async tasks that threw exceptions.
# TYPE oxphp_async_tasks_failed_total counter
oxphp_async_tasks_failed_total 45

# HELP oxphp_async_tasks_cancelled_total Async tasks cancelled.
# TYPE oxphp_async_tasks_cancelled_total counter
oxphp_async_tasks_cancelled_total 5

# HELP oxphp_async_tasks_rejected_total Async tasks rejected (queue full).
# TYPE oxphp_async_tasks_rejected_total counter
oxphp_async_tasks_rejected_total 0

# HELP oxphp_worker_mode_enabled Whether worker mode is active.
# TYPE oxphp_worker_mode_enabled gauge
oxphp_worker_mode_enabled 1

# HELP oxphp_worker_requests_handled_total Total requests processed by worker mode.
# TYPE oxphp_worker_requests_handled_total counter
oxphp_worker_requests_handled_total 48203

# HELP oxphp_worker_recycles_total Total worker recycles.
# TYPE oxphp_worker_recycles_total counter
oxphp_worker_recycles_total 5

# HELP oxphp_worker_recycles_by_reason_total Worker recycles by reason.
# TYPE oxphp_worker_recycles_by_reason_total counter
oxphp_worker_recycles_by_reason_total{reason="max_requests"} 4
oxphp_worker_recycles_by_reason_total{reason="max_memory"} 1
oxphp_worker_recycles_by_reason_total{reason="error"} 0

# HELP oxphp_worker_soft_resets_total Total soft resets between requests.
# TYPE oxphp_worker_soft_resets_total counter
oxphp_worker_soft_resets_total 48203

# HELP oxphp_worker_memory_bytes Current PHP heap per worker.
# TYPE oxphp_worker_memory_bytes gauge
# HELP oxphp_worker_uptime_seconds Time since worker thread spawned.
# TYPE oxphp_worker_uptime_seconds gauge
# HELP oxphp_worker_requests_count Requests handled by this worker instance.
# TYPE oxphp_worker_requests_count gauge
oxphp_worker_memory_bytes{worker="0"} 524288
oxphp_worker_uptime_seconds{worker="0"} 3600
oxphp_worker_requests_count{worker="0"} 6025
oxphp_worker_memory_bytes{worker="1"} 491520
oxphp_worker_uptime_seconds{worker="1"} 3600
oxphp_worker_requests_count{worker="1"} 6030

# HELP oxphp_worker_request_duration_us PHP execution time per request.
# TYPE oxphp_worker_request_duration_us histogram
oxphp_worker_request_duration_us_bucket{le="100"} 12050
oxphp_worker_request_duration_us_bucket{le="250"} 30100
oxphp_worker_request_duration_us_bucket{le="500"} 42000
oxphp_worker_request_duration_us_bucket{le="1000"} 46500
oxphp_worker_request_duration_us_bucket{le="2500"} 47800
oxphp_worker_request_duration_us_bucket{le="5000"} 48100
oxphp_worker_request_duration_us_bucket{le="10000"} 48180
oxphp_worker_request_duration_us_bucket{le="25000"} 48200
oxphp_worker_request_duration_us_bucket{le="50000"} 48203
oxphp_worker_request_duration_us_bucket{le="+Inf"} 48203
oxphp_worker_request_duration_us_sum 9640600
oxphp_worker_request_duration_us_count 48203
```

## Конфигурация Prometheus

Добавьте задание сбора метрик в `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: "oxphp"
    scrape_interval: 15s
    static_configs:
      - targets: ["oxphp:9090"]
```

Для обнаружения сервисов в Kubernetes:

```yaml
scrape_configs:
  - job_name: "oxphp"
    kubernetes_sd_configs:
      - role: pod
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_label_app]
        regex: oxphp
        action: keep
      - source_labels: [__meta_kubernetes_pod_ip]
        target_label: __address__
        replacement: "$1:9090"
```

## Полезные PromQL-запросы

**Частота запросов (запросов в секунду):**

```promql
rate(oxphp_requests_total[5m])
```

**Частота ошибок (5xx-ответы в процентах):**

```promql
rate(oxphp_responses_by_status_total{status="5xx"}[5m])
/ rate(oxphp_requests_total[5m]) * 100
```

**Среднее время ответа (миллисекунды):**

```promql
rate(oxphp_response_time_us_total[5m])
/ rate(oxphp_requests_total[5m]) / 1000
```

**Частота отклонений (529-отклонений в секунду):**

```promql
rate(oxphp_dropped_requests_total[5m])
```

**Утилизация пула воркеров (динамический режим):**

```promql
1 - (oxphp_workers_idle / oxphp_workers_current)
```

**Скорость масштабирования воркеров (запусков в минуту):**

```promql
rate(oxphp_workers_spawned_total[5m]) * 60
```

**p99 задержки запросов (микросекунды):**

```promql
histogram_quantile(0.99, rate(oxphp_request_duration_us_bucket[5m]))
```

**Пропускная способность запросов (байт в секунду, вход/выход):**

```promql
rate(oxphp_request_bytes_total[5m])
rate(oxphp_response_bytes_total[5m])
```

**p95 ожидания в очереди (микросекунды):**

```promql
histogram_quantile(0.95, rate(oxphp_queue_wait_us_bucket[5m]))
```

**Отклонённые ограничителем запросы в секунду:**

```promql
rate(oxphp_rate_limited_total[5m])
```

**Коэффициент попаданий в кеш статических файлов:**

```promql
rate(oxphp_static_cache_hits_total[5m])
/ (rate(oxphp_static_cache_hits_total[5m]) + rate(oxphp_static_cache_misses_total[5m]))
```

**Коэффициент экономии от сжатия:**

```promql
rate(oxphp_compression_bytes_saved_total[5m])
/ rate(oxphp_response_bytes_total[5m])
```

**Частота отправки асинхронных задач (задач в секунду):**

```promql
rate(oxphp_async_tasks_dispatched_total[5m])
```

**Доля ошибок асинхронных задач:**

```promql
rate(oxphp_async_tasks_failed_total[5m])
/ rate(oxphp_async_tasks_dispatched_total[5m]) * 100
```

**Частота отклонения асинхронных задач (очередь заполнена):**

```promql
rate(oxphp_async_tasks_rejected_total[5m])
```

**Частота запросов в режиме воркера:**

```promql
rate(oxphp_worker_requests_handled_total[5m])
```

**Частота рециклирования воркеров (рециклирований в минуту):**

```promql
rate(oxphp_worker_recycles_total[5m]) * 60
```

**p99 длительности запроса в режиме воркера (микросекунды):**

```promql
histogram_quantile(0.99, rate(oxphp_worker_request_duration_us_bucket[5m]))
```

**Среднее использование памяти воркерами (байты):**

```promql
avg(oxphp_worker_memory_bytes)
```

**Частота рециклирования воркеров по ошибкам:**

```promql
rate(oxphp_worker_recycles_by_reason_total{reason="error"}[5m])
```

## Примеры правил алертинга

```yaml
groups:
  - name: oxphp
    rules:
      - alert: OxPHPHighErrorRate
        expr: >
          rate(oxphp_responses_by_status_total{status="5xx"}[5m])
          / rate(oxphp_requests_total[5m]) > 0.05
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP error rate above 5%"

      - alert: OxPHPQueueDropping
        expr: rate(oxphp_dropped_requests_total[5m]) > 0
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "OxPHP is dropping requests (529)"

      - alert: OxPHPWorkerErrorRecycles
        expr: rate(oxphp_worker_recycles_by_reason_total{reason="error"}[5m]) > 0
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP workers are recycling due to errors"

      - alert: OxPHPWorkerHighMemory
        expr: oxphp_worker_memory_bytes > 134217728
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP worker memory exceeds 128 MiB"

      - alert: OxPHPWorkerSlowRequests
        expr: >
          histogram_quantile(0.99,
            rate(oxphp_worker_request_duration_us_bucket[5m])
          ) > 50000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP worker p99 latency exceeds 50ms"

      - alert: OxPHPHighRequestLatency
        expr: >
          histogram_quantile(0.99,
            rate(oxphp_request_duration_us_bucket[5m])
          ) > 500000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP request p99 latency exceeds 500ms"

      - alert: OxPHPHighQueueWait
        expr: >
          histogram_quantile(0.95,
            rate(oxphp_queue_wait_us_bucket[5m])
          ) > 10000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP queue wait p95 exceeds 10ms — consider adding workers"

      - alert: OxPHPRateLimiting
        expr: rate(oxphp_rate_limited_total[5m]) > 1
        for: 5m
        labels:
          severity: info
        annotations:
          summary: "OxPHP is actively rate-limiting requests"

      - alert: OxPHPAsyncTaskRejections
        expr: rate(oxphp_async_tasks_rejected_total[5m]) > 0
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP async queue is full — tasks are being rejected"

      - alert: OxPHPAsyncHighFailureRate
        expr: >
          rate(oxphp_async_tasks_failed_total[5m])
          / rate(oxphp_async_tasks_dispatched_total[5m]) > 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP async task failure rate above 10%"

      - alert: OxPHPLowCacheHitRate
        expr: >
          rate(oxphp_static_cache_hits_total[5m])
          / (rate(oxphp_static_cache_hits_total[5m])
             + rate(oxphp_static_cache_misses_total[5m])) < 0.5
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "OxPHP static file cache hit rate below 50%"

```

## Заметки о реализации

Все счётчики метрик используют типы `std::sync::atomic` с `Ordering::Relaxed`. Это означает:

- Чтение счётчиков может немного отставать (на микросекунды) от фактического состояния.
- На пути обработки запросов нет блокировок или барьеров памяти.
- Prometheus собирает данные с интервалом 15 секунд, поэтому субмиллисекундное отставание несущественно.

### Метрики плагинов

Плагины могут добавлять дополнительные метрики в вывод `/metrics`. Метрики плагинов добавляются после основных метрик, перечисленных выше, и следуют тому же текстовому формату Prometheus.

## Смотрите также

- [Проверки состояния](health-checks.md) --- эндпоинты `/health` и `/config` внутреннего сервера
- [Конфигурация](configuration.md) --- `INTERNAL_ADDR` и другие переменные окружения
- [Асинхронные промисы](/features/async-promises.md) --- параллельное выполнение PHP и метрики асинхронных задач
- [Пул воркеров](/architecture/worker-pool.md) --- статическое и динамическое масштабирование, режим воркера и поведение рециклирования
- [Плавная остановка](graceful-shutdown.md) --- как дренирование соединений влияет на `oxphp_active_connections`
