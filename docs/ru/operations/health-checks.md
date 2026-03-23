---
title: Проверки работоспособности
description: Конечные точки внутреннего сервера для мониторинга работоспособности, сбора метрик Prometheus и инспекции конфигурации во время выполнения.
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

## GET /health

Возвращает статус работоспособности сервера в формате JSON. Используйте эту конечную точку для проверок готовности и жизнеспособности.

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

**HTTP-коды статуса:**

| Код | Значение |
|-----|---------|
| `200 OK` | Все подсистемы работают нормально |
| `503 Service Unavailable` | Пул PHP-воркеров деградировал или недоступен, либо плагин сообщает об ошибке |

Конечная точка `/health` работает с минимальными затратами — она читает счётчики в памяти без дискового ввода-вывода, обращения к базе данных или выполнения PHP.

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

> **Примечание:** Пути к TLS-сертификату и ключу опускаются. Булево поле `tls_enabled` указывает, активен ли TLS.

## Интеграция с Kubernetes

Используйте конечную точку `/health` как для проверки жизнеспособности, так и для проверки готовности:

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: oxphp
          image: ghcr.io/oxphp/oxphp:0.1.0
          env:
            - name: INTERNAL_ADDR
              value: "0.0.0.0:9090"
          ports:
            - containerPort: 8080
            - containerPort: 9090
          livenessProbe:
            httpGet:
              path: /health
              port: 9090
            initialDelaySeconds: 5
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health
              port: 9090
            initialDelaySeconds: 2
            periodSeconds: 5
            failureThreshold: 2
```

Ответ `503` от `/health` заставляет Kubernetes исключить под из списка эндпоинтов Service (проверка готовности) или перезапустить его (проверка жизнеспособности) в зависимости от типа проверки.

## Проверка работоспособности в Docker Compose

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.1.0
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
