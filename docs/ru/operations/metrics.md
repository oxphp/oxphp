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

**Метки методов:** `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `OTHER`.

**Метки статусов:** `1xx`, `2xx`, `3xx`, `4xx`, `5xx`.

Выводятся только методы и классы статусов, для которых зафиксировано хотя бы одно событие. Метки с нулевыми счётчиками опускаются для компактности вывода.

### Соединения

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_active_connections` | gauge | Текущее количество открытых TCP-соединений на основном порту |

### Очередь PHP-воркеров

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_pending_requests` | gauge | Запросы, ожидающие в очереди PHP-воркеров |
| `oxphp_dropped_requests_total` | counter | Запросы, отклонённые с кодом 503, поскольку очередь заполнена |
| `oxphp_busy_workers` | gauge | Потоки воркеров, обрабатывающие запрос в данный момент |

### Пул PHP-воркеров

| Метрика | Тип | Описание |
|---------|-----|----------|
| `oxphp_workers_current` | gauge | Текущее количество потоков воркеров в пуле |
| `oxphp_workers_min` | gauge | Минимальное количество потоков воркеров (равно текущему в статическом режиме) |
| `oxphp_workers_max` | gauge | Максимальное количество потоков воркеров (равно текущему в статическом режиме) |
| `oxphp_workers_idle` | gauge | Потоки воркеров, не обрабатывающие запрос в данный момент (только в динамическом режиме, 0 в статическом) |
| `oxphp_workers_spawned_total` | counter | Общее количество воркеров, запущенных с момента старта (включая начальных) |
| `oxphp_workers_retired_total` | counter | Общее количество воркеров, выведенных ScaleManager (только в динамическом режиме) |

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

# HELP oxphp_dropped_requests_total Requests dropped (503).
# TYPE oxphp_dropped_requests_total counter
oxphp_dropped_requests_total 0

# HELP oxphp_response_time_us_total Total response time in microseconds.
# TYPE oxphp_response_time_us_total counter
oxphp_response_time_us_total 192000000

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

**Частота отклонений (503-отклонений в секунду):**

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
          summary: "OxPHP is dropping requests (503)"

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
- [Пул воркеров](/architecture/worker-pool.md) --- статическое и динамическое масштабирование, режим воркера и поведение рециклирования
- [Плавная остановка](graceful-shutdown.md) --- как дренирование соединений влияет на `oxphp_active_connections`
