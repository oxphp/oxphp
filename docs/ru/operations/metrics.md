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
- [Пул воркеров](/architecture/worker-pool.md) --- статическое и динамическое масштабирование, формирующее метрики воркеров
- [Плавная остановка](graceful-shutdown.md) --- как дренирование соединений влияет на `oxphp_active_connections`
