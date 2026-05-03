---
title: Метрики Prometheus
description: Справочник по всем метрикам в формате Prometheus, публикуемым OxPHP на конечной точке /metrics, включая метрики запросов, соединений, воркеров и сжатия.
---

# Метрики Prometheus

OxPHP публикует метрики в формате текстовой экспозиции Prometheus на конечной точке `GET /metrics` внутреннего сервера. Метрики охватывают пропускную способность запросов, время ответа, состояние соединений, работоспособность пула воркеров, кэширование статических файлов, эффективность сжатия и производительность в режиме worker.

## Включение метрик

Задайте `INTERNAL_ADDR` для запуска внутреннего сервера:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Затем собирайте метрики через Prometheus или совместимый коллектор:

```bash
curl http://localhost:9090/metrics
```

## Метрики сервера

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_uptime_seconds` | gauge | Секунды с момента запуска серверного процесса |
| `oxphp_requests_total` | counter | Общее количество HTTP-запросов, полученных на основном порту |

## Метрики запросов

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_requests_by_method_total` | counter | Запросы по HTTP-методу. Метка: `method` (`GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `QUERY`, `OTHER`) |
| `oxphp_responses_by_status_total` | counter | Ответы по классу статуса. Метка: `status` (`1xx`, `2xx`, `3xx`, `4xx`, `5xx`) |
| `oxphp_request_bytes_total` | counter | Общее количество полученных байт тела запроса |
| `oxphp_response_bytes_total` | counter | Общее количество отправленных байт тела ответа |

> **Примечание:** Публикуются только методы и классы статусов с хотя бы одним зафиксированным событием. Метки с нулевым числом записей опускаются.

## Гистограмма продолжительности запросов

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_request_duration_us` | histogram | Сквозная продолжительность запроса в микросекундах для всех запросов (статические файлы и PHP) |

Границы бакетов (микросекунды): `100`, `500`, `1000`, `2500`, `5000`, `10000`, `25000`, `50000`, `100000`, `250000`, `500000`, `1000000`, `+Inf`.

Используйте эту гистограмму для отслеживания общей задержки, выявления медленных эндпоинтов и измерения перцентилей хвостовой задержки.

## Метрики соединений

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_active_connections` | gauge | Текущее число открытых TCP-соединений на основном порту |
| `oxphp_pending_requests` | gauge | Запросы, переданные в обработку PHP-воркерам (в очереди и в обработке) |
| `oxphp_dropped_requests_total` | counter | Запросы, для которых PHP-воркер завершился с ошибкой после принятия запроса |

## Метрики пула воркеров

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_workers_current` | gauge | Текущее количество PHP-воркер-потоков |
| `oxphp_workers_min` | gauge | Минимальное количество воркеров (равно текущему в статическом режиме) |
| `oxphp_workers_max` | gauge | Максимальное количество воркеров (равно текущему в статическом режиме) |
| `oxphp_workers_idle` | gauge | Воркеры, не обрабатывающие запрос в данный момент |
| `oxphp_busy_workers` | gauge | Воркеры, обрабатывающие запрос в данный момент |
| `oxphp_workers_spawned_total` | counter | Общее количество воркеров, запущенных с момента старта (включая начальные) |
| `oxphp_workers_retired_total` | counter | Общее количество воркеров, завершённых по тайм-ауту простоя (только в динамическом режиме) |

## Гистограмма времени ожидания в очереди

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_queue_wait_us` | histogram | Время ожидания запроса в очереди до подхвата воркером, в микросекундах |

Границы бакетов (микросекунды): `50`, `100`, `250`, `500`, `1000`, `2500`, `5000`, `10000`, `50000`, `+Inf`.

Высокое время ожидания в очереди означает, что все воркеры заняты, и следует увеличить `PHP_WORKERS` или `QUEUE_CAPACITY`.

## Метрики ограничения частоты запросов

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_rate_limited_total` | counter | Запросы, отклонённые ограничителем частоты (вернул 429) |
| `oxphp_php_deny_total` | counter | Запросы, заблокированные `PHP_DENY_DIRS` (выполнение `.php` запрещено). См. [Deny-лист выполнения PHP](../security/php-deny-dirs.md) |

## Метрики кэша статических файлов

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_static_cache_hits_total` | counter | Запросы статических файлов, обслуженные из кэша в памяти |
| `oxphp_static_cache_misses_total` | counter | Запросы статических файлов, потребовавшие чтения с диска |

## Метрики сжатия

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_compressed_responses_total` | counter | Ответы, сжатые с помощью Brotli |
| `oxphp_compression_bytes_saved_total` | counter | Общее количество байт, сэкономленных за счёт сжатия (исходный размер минус сжатый) |

## Метрики режима Worker

Эти метрики публикуются только когда включён режим воркера (`WORKER_MODE_ENABLED=true`).

### Глобальные счётчики

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_worker_mode_enabled` | gauge | Всегда `1` при активном режиме worker |
| `oxphp_worker_requests_handled_total` | counter | Общее количество запросов, обработанных постоянными воркерами |
| `oxphp_worker_recycles_total` | counter | Общее количество перезапусков воркеров (воркер завершился и был перезапущен) |
| `oxphp_worker_recycles_by_reason_total` | counter | Перезапуски по причине. Метка: `reason` (`max_requests`, `max_memory`, `error`) |
| `oxphp_worker_soft_resets_total` | counter | Общее количество мягких сбросов между запросами |

### Метрики по воркерам

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_worker_memory_bytes` | gauge | Текущее использование PHP-кучи на воркер. Метка: `worker` (индекс слота, например `"0"`, `"1"`) |
| `oxphp_worker_uptime_seconds` | gauge | Секунды с момента запуска каждого воркера. Метка: `worker` |
| `oxphp_worker_requests_count` | gauge | Запросы, обработанные каждым экземпляром воркера. Метка: `worker` |

### Гистограмма продолжительности запросов воркера

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_worker_request_duration_us` | histogram | Время выполнения PHP-обработчика на запрос в микросекундах (только в режиме worker) |

Границы бакетов (микросекунды): `100`, `250`, `500`, `1000`, `2500`, `5000`, `10000`, `25000`, `50000`, `+Inf`.

Эта гистограмма измеряет время, проведённое внутри PHP-обработчика, без учёта времени ожидания в очереди. Используйте её для выявления медленных обработчиков и отслеживания хвостовой задержки в режиме worker.

## Метрики асинхронного пула

Эти метрики публикуются только когда `ASYNC_WORKERS` установлен в ненулевое значение.

| Метрика | Тип | Описание |
|--------|-----|---------|
| `oxphp_async_tasks_dispatched_total` | counter | Общее количество асинхронных задач, отправленных в фоновый пул |
| `oxphp_async_tasks_completed_total` | counter | Асинхронные задачи, завершившиеся успешно |
| `oxphp_async_tasks_failed_total` | counter | Асинхронные задачи, завершившиеся с исключением |
| `oxphp_async_tasks_cancelled_total` | counter | Отменённые асинхронные задачи |
| `oxphp_async_tasks_rejected_total` | counter | Асинхронные задачи, отклонённые из-за переполнения очереди пула |

## Советы по дашборду Grafana

Следующие запросы PromQL полезны для построения дашбордов:

**Скорость запросов (запросов в секунду):**

```text
rate(oxphp_requests_total[5m])
```

**Среднее время ответа (миллисекунды):**

```text
rate(oxphp_request_duration_us_sum[5m])
/ rate(oxphp_requests_total[5m]) / 1000
```

**p99 продолжительности запроса (миллисекунды):**

```text
histogram_quantile(0.99, rate(oxphp_request_duration_us_bucket[5m])) / 1000
```

**Доля ошибок (ответы 5xx в процентах):**

```text
rate(oxphp_responses_by_status_total{status="5xx"}[5m])
/ rate(oxphp_requests_total[5m]) * 100
```

**Утилизация пула воркеров:**

```text
oxphp_busy_workers / oxphp_workers_current
```

**Насыщение очереди (число дропов в секунду):**

```text
rate(oxphp_dropped_requests_total[5m])
```

**p99 времени ожидания в очереди (микросекунды):**

```text
histogram_quantile(0.99, rate(oxphp_queue_wait_us_bucket[5m]))
```

**Доля попаданий в кэш статических файлов:**

```text
rate(oxphp_static_cache_hits_total[5m])
/ (rate(oxphp_static_cache_hits_total[5m]) + rate(oxphp_static_cache_misses_total[5m]))
```

**Байт сэкономлено сжатием в секунду:**

```text
rate(oxphp_compression_bytes_saved_total[5m])
```

**p99 задержки в режиме worker (микросекунды):**

```text
histogram_quantile(0.99, rate(oxphp_worker_request_duration_us_bucket[5m]))
```

**Скорость перезапусков воркеров (в минуту):**

```text
rate(oxphp_worker_recycles_total[5m]) * 60
```

**Среднее использование памяти воркерами:**

```text
avg(oxphp_worker_memory_bytes)
```

## Конфигурация сбора метрик Prometheus

Добавьте задание сбора в ваш `prometheus.yml`:

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

## См. также

- [Проверки работоспособности](health-checks.md) — конечные точки `/health` и `/config` на внутреннем сервере
- [Справочник по конфигурации](configuration.md) — все переменные окружения, включая `INTERNAL_ADDR`
- [Штатное завершение работы](graceful-shutdown.md) — влияние дренирования соединений на `oxphp_active_connections`
- [Режим Worker](../features/worker-mode.md) — постоянные воркеры и публикуемые ими метрики
