---
title: Проверки работоспособности
description: Конечные точки внутреннего сервера для мониторинга работоспособности, проб Kubernetes, метрик Prometheus и инспекции конфигурации.
---

# Проверки работоспособности

OxPHP предоставляет внутренний HTTP-сервер на отдельном порту для мониторинга работоспособности, сбора метрик и инспекции конфигурации. Этот сервер изолирован от трафика приложения, чтобы мониторинг не конкурировал с пользовательскими запросами.

## Настройка

Задайте `INTERNAL_ADDR` для включения внутреннего сервера:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Если `INTERNAL_ADDR` не задан, внутренний сервер не запускается и конечные точки работоспособности недоступны.

> **Примечание:** В продакшене привязывайтесь к `127.0.0.1`, если только внутренний сервер не защищён брандмауэром. Конечная точка `/config` раскрывает операционные детали, которые не должны быть публичными.

## Пробы Kubernetes

OxPHP предоставляет отдельные конечные точки для каждого типа проб Kubernetes. Каждая точка также доступна по короткому алиасу (`/healthz`, `/readyz`, `/startupz`).

| Конечная точка | Алиас | Проверки | 200 | 503 |
|----------------|-------|----------|-----|-----|
| `/health/liveness` | `/healthz` | Нет (жив, если отвечает) | Всегда | Никогда |
| `/health/readiness` | `/readyz` | Не в shutdown, executor healthy, нет failed-плагинов | Готов | Не готов |
| `/health/startup` | `/startupz` | Executor healthy | Готов | Не готов |

**Liveness** всегда возвращает `200 OK`. Если процесс отвечает на HTTP-запрос — он жив. Проверки executor и плагинов не выполняются — это предотвращает перезапуск подов из-за временных проблем пула воркеров.

**Readiness** возвращает `503 Service Unavailable` когда:
- Сервер завершает работу (graceful shutdown)
- Пул PHP-воркеров неисправен
- Любой плагин сообщает об ошибке

При graceful shutdown readiness сразу возвращает `503`, заставляя Kubernetes убрать под из эндпоинтов Service до завершения дренирования.

**Startup** возвращает `503 Service Unavailable` когда executor ещё не готов. Используйте эту пробу для предотвращения преждевременного убийства пода при медленной инициализации.

Все конечные точки проб возвращают `Content-Type: text/plain` с названием пробы в теле (например, `readiness`). Kubernetes проверяет только HTTP-код статуса.

```bash
# Быстрая проверка
curl -s -o /dev/null -w '%{http_code}' http://localhost:9090/health/readiness
```

## GET /health

Возвращает полный статус работоспособности сервера в формате JSON. Используйте для дашбордов и систем мониторинга, а не для проб Kubernetes.

```bash
curl http://localhost:9090/health
```

**Ответ при нормальной работе (200 OK):**

```json
{
  "status": "ok",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {}
}
```

**Ответ при деградации (503 Service Unavailable):**

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": false,
  "plugins": {}
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `status` | string | `"ok"` если все подсистемы работают нормально, `"degraded"` в противном случае |
| `uptime_secs` | integer | Секунды с момента запуска сервера |
| `total_requests` | integer | Общее количество HTTP-запросов, обработанных на основном порту |
| `active_connections` | integer | Текущее число открытых соединений на основном порту |
| `executor_healthy` | boolean | Принимает ли пул PHP-воркеров запросы |

## GET /metrics

Возвращает метрики в формате текстовой экспозиции Prometheus. Полный справочник по метрикам см. в разделе [Метрики Prometheus](metrics.md).

```bash
curl http://localhost:9090/metrics
```

## GET /config

Возвращает активную конфигурацию сервера в формате JSON. Пути к TLS-сертификату и ключу опускаются из соображений безопасности.

```bash
curl -s http://localhost:9090/config | jq .
```

```json
{
  "listen_addr": "0.0.0.0:80",
  "document_root": "/var/www/html/public",
  "entry_file": "/var/www/html/public/index.php",
  "log_level": "info",
  "executor_type": "sapi",
  "php_workers": "8",
  "tokio_workers": 4,
  "queue_capacity": 1024,
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "internal_addr": "127.0.0.1:9090",
  "header_timeout_seconds": 5,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": null,
  "compression_level": 4,
  "access_log": "all",
  "max_query_body": 524288,
  "worker_mode_enabled": false,
  "worker_max_memory_mib": 0,
  "static_cache_ttl": 2592000,
  "static_cache_enabled": true,
  "async_workers": 0,
  "async_queue_capacity": 0,
  "trace_context": false,
  "superglobals_enabled": true,
  "trusted_proxies": false,
  "plugins": {}
}
```

> **Примечание:** Пути к TLS-сертификату и ключу опускаются. Булево поле `tls_enabled` указывает, активен ли TLS.

## Интеграция с Kubernetes

Используйте отдельные конечные точки для каждого типа проб:

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:latest
          env:
            - name: INTERNAL_ADDR
              value: "0.0.0.0:9090"
          ports:
            - containerPort: 8080
            - containerPort: 9090
          startupProbe:
            httpGet:
              path: /health/startup
              port: 9090
            initialDelaySeconds: 1
            periodSeconds: 2
            failureThreshold: 15
          livenessProbe:
            httpGet:
              path: /health/liveness
              port: 9090
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health/readiness
              port: 9090
            periodSeconds: 5
            failureThreshold: 2
```

| Проба | Реакция на ошибку |
|-------|-------------------|
| Startup | Kubernetes ждёт — не убивает под во время инициализации |
| Liveness | Kubernetes перезапускает под |
| Readiness | Kubernetes убирает под из эндпоинтов Service (без перезапуска) |

Короткие алиасы (`/healthz`, `/readyz`, `/startupz`) полностью эквивалентны и могут использоваться вместо полных путей.

## Проверка работоспособности в Docker Compose

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:latest
    ports:
      - "8080:80"
    environment:
      INTERNAL_ADDR: "127.0.0.1:9090"
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

Docker помечает контейнер как `unhealthy` после настроенного числа неудачных попыток, что может активировать политику перезапуска или исключение из балансировщика нагрузки.

## См. также

- [Метрики Prometheus](metrics.md) — полный справочник по всем публикуемым метрикам
- [Штатное завершение работы](graceful-shutdown.md) — взаимодействие проверок работоспособности с дренированием при завершении
- [Справочник по конфигурации](configuration.md) — все переменные окружения, включая `INTERNAL_ADDR`
