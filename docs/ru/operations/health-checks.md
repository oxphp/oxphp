---
title: Проверки состояния
description: Эндпоинты внутреннего сервера для мониторинга состояния и оркестрации контейнеров
---

OxPHP запускает внутренний HTTP-сервер на отдельном порту для проверок состояния, метрик и инспекции конфигурации. Этот сервер изолирован от основного порта трафика, чтобы мониторинговый трафик не конкурировал с запросами приложения.

## Включение внутреннего сервера

Установите переменную окружения `INTERNAL_ADDR` для запуска внутреннего сервера:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Когда эта переменная не установлена, внутренний сервер не запускается.

## Эндпоинты

### `GET /health`

Возвращает статус состояния сервера в формате JSON.

```bash
curl http://localhost:9090/health
```

**Ответ (исправен):**

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

**Ответ (деградация):**

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": false,
  "plugins": {
    "example_plugin": "failed"
  }
}
```

| Поле | Тип | Описание |
|------|-----|----------|
| `status` | `string` | `"ok"`, когда все подсистемы исправны, `"degraded"` в противном случае |
| `uptime_secs` | `integer` | Секунды с момента запуска сервера |
| `total_requests` | `integer` | Общее количество HTTP-запросов, обработанных на основном порту |
| `active_connections` | `integer` | Текущее количество открытых соединений на основном порту |
| `executor_healthy` | `boolean` | Принимает ли пул PHP-воркеров запросы |
| `plugins` | `object` | Статус состояния каждого загруженного плагина. Значения: `"healthy"` или `"failed"` |

**HTTP-коды статуса:**

| Код | Значение |
|-----|----------|
| `200 OK` | Исполнитель и все плагины исправны |
| `503 Service Unavailable` | Исполнитель или один из плагинов сообщает о сбое |

Проверка `executor_healthy` вызывает метод `is_healthy()` PHP-исполнителя. Если пул воркеров завершил работу или по иной причине не может обрабатывать запросы, возвращается `false`. Кроме того, если какой-либо плагин сообщает статус `Failed`, общий статус становится `"degraded"` и эндпоинт возвращает 503.

### `GET /metrics`

Возвращает метрики в текстовом формате Prometheus. Полный справочник метрик см. на странице [Метрики](metrics.md). Плагины могут добавлять дополнительные метрики в этот вывод.

```bash
curl http://localhost:9090/metrics
```

### `GET /config`

Возвращает активную конфигурацию сервера в формате JSON. Чувствительные значения (пути к TLS-ключам) скрыты. Конфигурация плагинов включена в ключ `plugins`.

```bash
curl http://localhost:9090/config
```

```json
{
  "listen_addr": "0.0.0.0:8080",
  "document_root": "/var/www/html/public",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_seconds": 30,
  "header_timeout_seconds": 5,
  "idle_timeout_seconds": 60,
  "request_timeout_seconds": 120,
  "rate_limit": 100,
  "rate_window_seconds": 60,
  "tls_enabled": true,
  "error_pages_dir": "/etc/oxphp/error-pages",
  "compression": true,
  "access_log": true,
  "plugins": {}
}
```

### Внутренние маршруты плагинов

Пути, начинающиеся с `/__`, зарезервированы для внутренних эндпоинтов, определяемых плагинами. Если ни один плагин не обрабатывает путь, возвращается ответ `404 Not Found`.

Любой другой путь возвращает `404 Not Found`.

## Проверки состояния контейнеров

### Docker

```yaml
# docker-compose.yml
services:
  oxphp:
    image: oxphp:latest
    environment:
      INTERNAL_ADDR: "127.0.0.1:9090"
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

### Dockerfile HEALTHCHECK

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=5s \
  CMD wget -qO- http://127.0.0.1:9090/health || exit 1
```

### Kubernetes

```yaml
# Liveness probe --- перезапускает pod, если сервер не отвечает
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3

# Readiness probe --- удаляет pod из сервиса при деградации
readinessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 2
  periodSeconds: 5
  failureThreshold: 2
```

Для Kubernetes используйте поле `executor_healthy` и HTTP-код статуса для управления готовностью. Ответ `503` означает, что пул PHP-воркеров или плагин находится в состоянии деградации, и pod должен быть удалён из списка эндпоинтов сервиса до восстановления.

## Интеграция с балансировщиком нагрузки

Большинство балансировщиков нагрузки поддерживают HTTP-проверки состояния. Направьте их на внутренний порт:

| Балансировщик нагрузки | Цель проверки состояния |
|------------------------|------------------------|
| AWS ALB/NLB | `http://instance:9090/health` |
| HAProxy | `option httpchk GET /health` на порту 9090 |
| nginx upstream | `proxy_pass http://backend:9090/health` |
| Traefik | `traefik.http.services.oxphp.loadbalancer.healthcheck.path=/health` |

Эндпоинт `/health` легковесен --- он читает атомарные счётчики и вызывает `is_healthy()` исполнителя. Дисковый ввод-вывод, обращения к базе данных или выполнение PHP не задействованы.

## Вопросы безопасности

Внутренний сервер по умолчанию привязывается к `127.0.0.1`, что делает его доступным только с локальной машины. Если необходимо предоставить доступ из сети мониторинга, привяжите его к конкретному интерфейсу:

```bash
# Доступен из сети мониторинга
INTERNAL_ADDR=10.0.1.5:9090
```

**Не** привязывайте внутренний сервер к `0.0.0.0` в продакшене, если он не находится за файрволом или сетевой политикой, ограничивающей доступ. Эндпоинт `/config` раскрывает операционные детали, которые не должны быть публичными.

## Смотрите также

- [Метрики](metrics.md) --- полный справочник метрик, совместимых с Prometheus
- [Конфигурация](configuration.md) --- все переменные окружения и их значения по умолчанию
- [Плавная остановка](graceful-shutdown.md) --- как проверки состояния взаимодействуют с дренированием при остановке
