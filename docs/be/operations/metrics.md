---
title: Метрыкі
description: Метрыкі, сумяшчальныя з Prometheus, якія прадастаўляюцца ўнутраным серверам
---

OxPHP прадастаўляе метрыкі ў фармаце тэкставай экспазіцыі Prometheus праз `GET /metrics` на ўнутраным серверы. Усе лічыльнікі выкарыстоўваюць бязблакіровачныя атамарныя аперацыі з парадкам `Relaxed` для мінімальнага ўплыву на прадукцыйнасць шляху запыту.

## Уключэнне метрык

Метрыкі даступныя, калі ўнутраны сервер працуе. Усталюйце зменную асяроддзя `INTERNAL_ADDR`:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Потым збірайце метрыкі з Prometheus або любога сумяшчальнага калектара:

```bash
curl http://localhost:9090/metrics
```

## Даведка па метрыках

### Сервер

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_uptime_seconds` | gauge | Секунды з моманту запуску серверанага працэсу |

### Запыты

| Метрыка | Тып | Пазнакі | Апісанне |
|--------|------|--------|-------------|
| `oxphp_requests_total` | counter | --- | Агульная колькасць HTTP-запытаў, атрыманых на асноўным порце |
| `oxphp_requests_by_method_total` | counter | `method` | Запыты ў разбіўцы па HTTP-метадах |
| `oxphp_responses_by_status_total` | counter | `status` | Адказы ў разбіўцы па класах статусу |
| `oxphp_response_time_us_total` | counter | --- | Сукупны час адказу ў мікрасекундах |
| `oxphp_request_duration_us` | histogram | --- | Працягласць запыту ў мікрасекундах (усе запыты) |
| `oxphp_request_bytes_total` | counter | --- | Агульны аб'ём атрыманых байтаў цела запыту |
| `oxphp_response_bytes_total` | counter | --- | Агульны аб'ём адпраўленых байтаў цела адказу |

**Пазнакі метадаў:** `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `CONNECT`, `OTHER`.

**Пазнакі статусаў:** `1xx`, `2xx`, `3xx`, `4xx`, `5xx`.

Выводзяцца толькі метады і класы статусаў з хаця б адной зафіксаванай падзеяй. Пазнакі з нулявым лікам апускаюцца для кампактнасці вываду.

#### Гістаграма працягласці запытаў

Межы бакетаў (мікрасекунды): 100, 500, 1000, 2500, 5000, 10000, 25000, 50000, 100000, 250000, 500000, 1000000, +Inf.

Гэта гістаграма ахоплівае ўсе запыты (статычныя файлы і PHP), у адрозненне ад `oxphp_worker_request_duration_us`, якая вымярае толькі час выканання PHP-апрацоўшчыка. `_sum` пераўжывае `oxphp_response_time_us_total` (тое ж значэнне, адзін атамік), а `_count` вылічаецца як сума ўсіх лічыльнікаў `oxphp_responses_by_status_total`.

### Злучэнні

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_active_connections` | gauge | Бягучыя адкрытыя TCP-злучэнні на асноўным порце |

### Чарга PHP-воркераў

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_pending_requests` | gauge | Запыты, якія зараз чакаюць у чарзе PHP-воркераў |
| `oxphp_dropped_requests_total` | counter | Запыты, адхіленыя з кодам 503, бо чарга была поўная |
| `oxphp_busy_workers` | gauge | Патокі воркераў, якія зараз апрацоўваюць запыт |
| `oxphp_queue_wait_us` | histogram | Час чакання ў чарзе да падхопу воркерам (мікрасекунды) |

#### Гістаграма чакання ў чарзе

Межы бакетаў (мікрасекунды): 50, 100, 250, 500, 1000, 2500, 5000, 10000, 50000, +Inf.

Вымярае час паміж адпраўкай запыту ў чаргу воркераў і яго падхопам воркерам. Дапамагае выяўляць ціск на чаргу і нястачу воркераў.

### Пул PHP-воркераў

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_workers_current` | gauge | Бягучая колькасць патокаў воркераў у пуле |
| `oxphp_workers_min` | gauge | Мінімальная колькасць патокаў воркераў (роўная бягучай для статычнага рэжыму) |
| `oxphp_workers_max` | gauge | Максімальная колькасць патокаў воркераў (роўная бягучай для статычнага рэжыму) |
| `oxphp_workers_idle` | gauge | Патокі воркераў, якія зараз не апрацоўваюць запыт (толькі ў дынамічным рэжыме, 0 у статычным) |
| `oxphp_workers_spawned_total` | counter | Агульная колькасць воркераў, створаных з моманту запуску (уключаючы пачатковых) |
| `oxphp_workers_retired_total` | counter | Агульная колькасць воркераў, спісаных ScaleManager (толькі ў дынамічным рэжыме) |

### Абмежаванне частаты запытаў

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_rate_limited_total` | counter | Запыты, адхіленыя абмежавальнікам частаты (429) |

Гэты лічыльнік павялічваецца кожны раз, калі запыт адхіляецца з адказам 429. Выводзіцца толькі пры ўключаным абмежаванні частаты (`RATE_LIMIT` > 0).

### Кэш статычных файлаў

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_static_cache_hits_total` | counter | Запыты статычных файлаў, абслужаныя з кэша змесціва |
| `oxphp_static_cache_misses_total` | counter | Запыты статычных файлаў, якія патрабавалі дыскавага ўводу-вываду |

Каэфіцыент трапленняў у кэш вылічаецца як `hits / (hits + misses)`. Нізкі каэфіцыент можа паказваць, што бюджэт кэша змесціва (64 МБ) занадта малы для рабочага набору.

### Сціск

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_compressed_responses_total` | counter | Адказы, сціснутыя з дапамогай Brotli |
| `oxphp_compression_bytes_saved_total` | counter | Агульная колькасць байтаў, зэканомленых сціскам (арыгінал − сціснуты) |

Гэтыя лічыльнікі павялічваюцца толькі пры ўключаным сціску (`COMPRESSION_LEVEL` > 0) і калі адказ сапраўды сціснуты (сціснуты вывад меншы за арыгінал).

### Асінхронныя задачы

Гэтыя метрыкі выводзяцца толькі калі асінхронны пул актыўны (`ASYNC_WORKERS` > 0) і хаця б адна задача была адпраўлена або адхілена.

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_async_tasks_dispatched_total` | counter | Агульная колькасць задач, адпраўленых праз `oxphp_async()` |
| `oxphp_async_tasks_completed_total` | counter | Задачы, якія паспяхова вярнулі значэнне |
| `oxphp_async_tasks_failed_total` | counter | Задачы, якія выклікалі выключэнне або `die()`/`exit()` |
| `oxphp_async_tasks_cancelled_total` | counter | Задачы, скасаваныя (тайм-аўт скончыўся або ачыстка RSHUTDOWN) |
| `oxphp_async_tasks_rejected_total` | counter | Задачы, адхіленыя з-за поўнай асінхроннай чаргі |

Каэфіцыент поспеху задач вылічаецца як `completed / dispatched`. Высокая колькасць `rejected` паказвае, што `ASYNC_QUEUE_CAPACITY` занадта малая або `ASYNC_WORKERS` недастаткова.

### Рэжым воркера

Гэтыя метрыкі выводзяцца толькі калі рэжым воркера актыўны (усталяваны `WORKER_FILE`). Яны забяспечваюць бачнасць жыццёвага цыклу персістэнтных PHP-воркераў, рэцыклінгу і часу выканання кожнага запыту.

#### Глабальныя лічыльнікі

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_worker_mode_enabled` | gauge | Заўсёды `1`, калі рэжым воркера актыўны |
| `oxphp_worker_requests_handled_total` | counter | Агульная колькасць запытаў, апрацаваных персістэнтнымі воркерамі |
| `oxphp_worker_recycles_total` | counter | Агульная колькасць рэцыклінгаў воркераў (воркер завяршыўся і быў перазапушчаны) |
| `oxphp_worker_recycles_by_reason_total` | counter | Рэцыклінгі ў разбіўцы па прычыне. Пазнакі: `reason="max_requests"`, `reason="max_memory"`, `reason="error"` |
| `oxphp_worker_soft_resets_total` | counter | Агульная колькасць мяккіх скідаў паміж запытамі (павінна роўняцца `requests_handled_total`) |

#### Gauge-метрыкі кожнага воркера

| Метрыка | Тып | Пазнакі | Апісанне |
|--------|------|--------|-------------|
| `oxphp_worker_memory_bytes` | gauge | `worker` | Бягучае выкарыстанне PHP-кучы ў байтах для кожнага воркера |
| `oxphp_worker_uptime_seconds` | gauge | `worker` | Секунды з моманту запуску патоку воркера |
| `oxphp_worker_requests_count` | gauge | `worker` | Запыты, апрацаваныя гэтым канкрэтным экзэмплярам воркера |

Метрыкі кожнага воркера індэксаваны па слоце воркера (напрыклад, `worker="0"`, `worker="1"`). Толькі актыўныя воркеры выводзяць значэнні.

#### Гістаграма працягласці запытаў

| Метрыка | Тып | Апісанне |
|--------|------|-------------|
| `oxphp_worker_request_duration_us` | histogram | Час выканання PHP-апрацоўшчыка на запыт у мікрасекундах |

Межы бакетаў (мікрасекунды): 100, 250, 500, 1000, 2500, 5000, 10000, 25000, 50000, +Inf.

Гэта гістаграма вымярае час, затрачаны на выкананне callback PHP-апрацоўшчыка, без уліку часу чакання ў чарзе. Яна дапамагае выяўляць павольныя запыты і хваставую затрымку.

## Прыклад вываду

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

## Канфігурацыя Prometheus

Дадайце задачу збору ў `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: "oxphp"
    scrape_interval: 15s
    static_configs:
      - targets: ["oxphp:9090"]
```

Для выяўлення сэрвісаў Kubernetes:

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

## Карысныя запыты PromQL

**Хуткасць запытаў (запыты ў секунду):**

```promql
rate(oxphp_requests_total[5m])
```

**Частка памылак (адказы 5xx у працэнтах):**

```promql
rate(oxphp_responses_by_status_total{status="5xx"}[5m])
/ rate(oxphp_requests_total[5m]) * 100
```

**Сярэдні час адказу (мілісекунды):**

```promql
rate(oxphp_response_time_us_total[5m])
/ rate(oxphp_requests_total[5m]) / 1000
```

**Хуткасць адхіленняў (адхіленні 503 у секунду):**

```promql
rate(oxphp_dropped_requests_total[5m])
```

**Выкарыстанне пула воркераў (дынамічны рэжым):**

```promql
1 - (oxphp_workers_idle / oxphp_workers_current)
```

**Хуткасць маштабавання воркераў (стварэнні ў хвіліну):**

```promql
rate(oxphp_workers_spawned_total[5m]) * 60
```

**p99 затрымкі запытаў (мікрасекунды):**

```promql
histogram_quantile(0.99, rate(oxphp_request_duration_us_bucket[5m]))
```

**Прапускная здольнасць запытаў (байтаў у секунду, уваход/выхад):**

```promql
rate(oxphp_request_bytes_total[5m])
rate(oxphp_response_bytes_total[5m])
```

**p95 чакання ў чарзе (мікрасекунды):**

```promql
histogram_quantile(0.95, rate(oxphp_queue_wait_us_bucket[5m]))
```

**Адхіленыя абмежавальнікам запыты ў секунду:**

```promql
rate(oxphp_rate_limited_total[5m])
```

**Каэфіцыент трапленняў у кэш статычных файлаў:**

```promql
rate(oxphp_static_cache_hits_total[5m])
/ (rate(oxphp_static_cache_hits_total[5m]) + rate(oxphp_static_cache_misses_total[5m]))
```

**Каэфіцыент эканоміі ад сціску:**

```promql
rate(oxphp_compression_bytes_saved_total[5m])
/ rate(oxphp_response_bytes_total[5m])
```

**Хуткасць адпраўкі асінхронных задач (задач у секунду):**

```promql
rate(oxphp_async_tasks_dispatched_total[5m])
```

**Частка адмоваў асінхронных задач:**

```promql
rate(oxphp_async_tasks_failed_total[5m])
/ rate(oxphp_async_tasks_dispatched_total[5m]) * 100
```

**Хуткасць адхіленняў асінхронных задач (чарга поўная):**

```promql
rate(oxphp_async_tasks_rejected_total[5m])
```

**Хуткасць запытаў у рэжыме воркера:**

```promql
rate(oxphp_worker_requests_handled_total[5m])
```

**Хуткасць рэцыклінгу воркераў (рэцыклінгі ў хвіліну):**

```promql
rate(oxphp_worker_recycles_total[5m]) * 60
```

**p99 працягласці запыту ў рэжыме воркера (мікрасекунды):**

```promql
histogram_quantile(0.99, rate(oxphp_worker_request_duration_us_bucket[5m]))
```

**Сярэдняе выкарыстанне памяці воркерам (байты):**

```promql
avg(oxphp_worker_memory_bytes)
```

**Хуткасць рэцыклінгу воркераў з-за памылак:**

```promql
rate(oxphp_worker_recycles_by_reason_total{reason="error"}[5m])
```

## Прыклады алертаў

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

## Заўвагі па рэалізацыі

Усе лічыльнікі метрык выкарыстоўваюць тыпы `std::sync::atomic` з `Ordering::Relaxed`. Гэта азначае:

- Чытанне лічыльнікаў можа быць злёгку неактуальным (на мікрасекунды) адносна рэальнага стану.
- Няма накладных выдаткаў на блакіроўкі або бар'еры памяці на шляху запыту.
- Prometheus збірае дадзеныя з інтэрвалам у 15 секунд, таму неактуальнасць на долі мілісекунды не мае значэння.

### Метрыкі плагінаў

Плагіны могуць дадаваць дадатковыя метрыкі да вываду `/metrics`. Метрыкі плагінаў дадаюцца пасля асноўных метрык, пералічаных вышэй, і адпавядаюць таму ж фармату тэкставай экспазіцыі Prometheus.

## Глядзіце таксама

- [Праверкі стану](health-checks.md) --- канчатковыя кропкі `/health` і `/config` на ўнутраным серверы
- [Канфігурацыя](configuration.md) --- `INTERNAL_ADDR` і іншыя зменныя асяроддзя
- [Асінхронныя промісы](/be/features/async-promises.md) --- паралельнае выкананне PHP і метрыкі асінхронных задач
- [Пул воркераў](/be/architecture/worker-pool.md) --- статычнае і дынамічнае маштабаванне, рэжым воркера і паводзіны рэцыклінгу
- [Плаўная спынка](graceful-shutdown.md) --- як адвод злучэнняў уплывае на `oxphp_active_connections`
