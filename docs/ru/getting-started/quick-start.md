---
title: Быстрый старт
description: Запустите OxPHP менее чем за 5 минут. Создайте проект, напишите PHP-приложение, запустите сервер и выполните первый запрос.
---

# Быстрый старт

Запустите OxPHP менее чем за 5 минут с Docker Compose. Это руководство проведёт вас от пустой директории до работающего PHP-приложения с проверками состояния и структурированным логированием.

## 1. Создайте директорию проекта

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

## 2. Создайте Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

Официальный образ включает серверный бинарный файл, PHP 8.4 ZTS, PHP-расширение OxPHP и все зависимости времени выполнения.

## 3. Добавьте compose.yaml

```yaml
services:
  oxphp:
    build: .
    ports:
      - "80:80"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=info
      - ACCESS_LOG=all
```

Порт `80` обслуживает ваше приложение. Порт `9090` открывает внутренний сервер для проверок состояния, метрик Prometheus и снимка активной конфигурации.

## 4. Создайте PHP-приложение

```bash
mkdir -p public
```

Создайте `public/index.php`:

```php
<?php

$requestId = oxphp_request_id();
$info      = oxphp_server_info();

echo "<h1>OxPHP</h1>\n";
echo "<p>Request ID: {$requestId}</p>\n";
echo "<p>Worker: {$info['worker_id']}</p>\n";
echo "<p>SAPI: {$info['sapi']}</p>\n";
echo "<p>Version: {$info['version']}</p>\n";
echo "<p>Time: " . date('c') . "</p>\n";
```

`oxphp_request_id()` возвращает уникальный ID, присвоенный каждому запросу. `oxphp_server_info()` возвращает сведения о работающем сервере, включая `sapi`, `version`, `worker_id` и `worker_mode`.

## 5. Соберите и запустите

```bash
docker compose up -d --build
```

## 6. Протестируйте приложение

```bash
curl http://localhost/
```

Ожидаемый вывод:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c11a2b00000001</p>
<p>Worker: 0</p>
<p>SAPI: oxphp</p>
<p>Version: 0.1.0</p>
<p>Time: 2026-03-23T12:00:00+00:00</p>
```

Каждый запрос получает уникальный ID. ID воркера показывает, какой PHP-поток его обработал.

## 7. Проверьте внутренние эндпоинты

```bash
# Проверка состояния — 200 при нормальной работе, 503 при деградации
curl http://localhost:9090/health

# Метрики, совместимые с Prometheus
curl http://localhost:9090/metrics

# Активная конфигурация (пути TLS скрыты)
curl http://localhost:9090/config
```

## 8. Просмотр логов

```bash
docker compose logs -f oxphp
```

Поскольку задано `ACCESS_LOG=all`, каждый запрос отображается как структурированная JSON-строка лога с методом, путём, статусом, временем ответа и ID запроса.

## Что дальше

- [Руководство по Docker](docker.md) — Dockerfiles для разработки и продакшена, конфигурация Compose, монтирование PHP ini и настройка проверок состояния
- [Конфигурация](../operations/configuration.md) — полный справочник переменных окружения
- [Маршрутизация](../features/routing.md) — режимы маршрутизации: Traditional, Framework, SPA и Worker
- [Режим воркеров](../features/worker-mode.md) — постоянные PHP-процессы, выполняющие инициализацию один раз и обрабатывающие несколько запросов
- [PHP-функции](../php/functions.md) — все встроенные PHP-функции OxPHP
