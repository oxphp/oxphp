---
title: Внутренний сервер
description: Выделенный HTTP-сервер для проверок работоспособности, метрик Prometheus, просмотра конфигурации и эндпоинтов плагинов.
---

# Внутренний сервер

OxPHP запускает отдельный HTTP-сервер на выделенном порту для проверок работоспособности, метрик Prometheus и просмотра активной конфигурации. Этот сервер полностью изолирован от основного слушателя приложения — у него нет TLS, ограничения частоты запросов, идентификаторов запросов и обработки событий.

## Как это работает

1. **Установите `INTERNAL_ADDR`** в адрес прослушивания (например, `127.0.0.1:9090`). OxPHP запустит второй HTTP-слушатель по этому адресу
2. Внутренний сервер предоставляет три встроенных эндпоинта: `/health`, `/metrics` и `/config`
3. Плагины могут регистрировать дополнительные эндпоинты с префиксом `/__<plugin>/`
4. При плановом завершении работы внутренний сервер остаётся доступным до тех пор, пока основной сервер не завершит обработку соединений

> **Примечание:** внутренний сервер запускается только при явной установке `INTERNAL_ADDR`. Без неё эндпоинты проверки работоспособности, метрик и конфигурации недоступны по HTTP.

## Конфигурация

| Переменная | По умолчанию | Описание |
|------------|--------------|----------|
| `INTERNAL_ADDR` | *(не задано)* | Адрес внутреннего сервера. Не запускается, если не задан. Пример: `127.0.0.1:9090` |

## Эндпоинты

### GET /health

Возвращает JSON-статус работоспособности. Используйте его для проб готовности и жизнеспособности Kubernetes.

**200 OK** — все системы работают нормально:

```json
{
  "status": "ok",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {
    "otel": "ok"
  }
}
```

**503 Service Unavailable** — плагин сообщил об ошибке:

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {
    "otel": "failed"
  }
}
```

Объект `plugins` содержит список всех загруженных плагинов с их статусом работоспособности: `"ok"`, `"degraded"` или `"failed"`. Только статус `"failed"` вызывает ответ 503 — плагины со статусом `"degraded"` отображаются в JSON, но HTTP-статус остаётся 200.

### GET /metrics

Возвращает метрики в формате, совместимом с Prometheus (`text/plain; version=0.0.4`). Всегда возвращает 200.

```bash
curl http://localhost:9090/metrics
```

```text
# HELP oxphp_requests_total Total HTTP requests
# TYPE oxphp_requests_total counter
oxphp_requests_total 48203
# HELP oxphp_active_connections Current open connections
# TYPE oxphp_active_connections gauge
oxphp_active_connections 7
...
```

Полный список доступных метрик см. в [Metrics](../operations/metrics.md).

### GET /config

Возвращает активную конфигурацию сервера в формате JSON. Всегда возвращает 200.

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:80",
  "document_root": "/var/www/html/public",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode": false,
  "worker_file": null,
  "worker_max_requests": 0,
  "worker_max_memory_mib": 0,
  "static_cache_ttl": 2592000,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": false,
  "plugins": {}
}
```

Пути к файлам TLS-сертификата и ключа намеренно исключены из вывода из соображений безопасности. Отображается только булево поле `tls_enabled`.

### Эндпоинты плагинов

Плагины могут регистрировать собственные эндпоинты с префиксом `/__<plugin_name>/`. Например, плагин с именем `otel` может предоставить эндпоинт `/__otel/status`. Эти эндпоинты доступны только если соответствующий плагин загружен и зарегистрировал обработчик.

Любой путь, не соответствующий встроенному или зарегистрированному плагином эндпоинту, возвращает `404 Not Found`.

## Интеграция с Kubernetes

### Проба готовности

Используйте `/health` для управления тем, направляет ли Kubernetes трафик на под:

```yaml
readinessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 2
  periodSeconds: 5
```

Когда `/health` возвращает 503, Kubernetes исключает под из списка эндпоинтов Service. Трафик возобновляется, когда эндпоинт снова возвращает 200.

### Проба жизнеспособности

Используйте тот же эндпоинт для перезапуска подов, переставших отвечать:

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3
```

### Проба запуска

Для приложений с медленной инициализацией (крупные фреймворки, ресурсоёмкие автозагрузчики):

```yaml
startupProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 1
  periodSeconds: 2
  failureThreshold: 15
```

## Безопасность

Внутренний сервер не имеет встроенной аутентификации или контроля доступа. Для его защиты:

- **Привяжите к localhost** — установите `INTERNAL_ADDR=127.0.0.1:9090`, чтобы порт был доступен только изнутри контейнера или хоста
- **Не открывайте как Kubernetes Service** — объявите порт как `containerPort`, но не создавайте для него Service. Пробы Kubernetes обращаются к портам контейнера напрямую
- **Используйте сетевые политики** — ограничьте доступ на сетевом уровне, если порт необходимо открыть

Эндпоинт `/config` раскрывает операционные детали (корень документов, лимиты запросов, количество воркеров, значения тайм-аутов). Хотя пути TLS скрыты, подумайте, должна ли эта информация быть доступна извне пода.

## Пример Docker

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "80:80"
      - "9090:9090"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
```

Для продакшена привяжите внутренний сервер к localhost и используйте пробы Kubernetes:

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "80:80"
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=127.0.0.1:9090
```

## Устранение неполадок

### Эндпоинты проверки работоспособности, метрик и конфигурации недоступны

`INTERNAL_ADDR` не задан.

**Решение:** добавьте переменную окружения:

```bash
INTERNAL_ADDR=0.0.0.0:9090
```

### /health возвращает 503, но приложение работает

Загруженный плагин сообщает о `PluginHealth::Failed`. Проверьте объект `plugins` в ответе на запрос проверки работоспособности, чтобы определить, какой плагин отказал:

```bash
curl -s http://localhost:9090/health | jq '.plugins'
```

### Не удаётся обратиться к внутреннему серверу извне контейнера

Сервер привязан к `127.0.0.1`, что доступно только изнутри контейнера.

**Решение:** измените на `0.0.0.0` для внешнего доступа или используйте пробы Kubernetes, обращающиеся к контейнеру напрямую.

### Метрики не показывают данные режима воркера или асинхронного режима

Метрики режима воркера отображаются только при установленном `WORKER_FILE`. Асинхронные метрики появляются только при `ASYNC_WORKERS > 0` и после того, как хотя бы одна задача была отправлена или отклонена.

## См. также

- [Metrics](../operations/metrics.md) — полный справочник по метрикам Prometheus
- [Health Checks](../operations/health-checks.md) — подробное описание поведения проверок работоспособности
- [Configuration Reference](../operations/configuration.md) — все переменные окружения
- [Graceful Shutdown](../operations/graceful-shutdown.md) — последовательность завершения работы и жизненный цикл внутреннего сервера
