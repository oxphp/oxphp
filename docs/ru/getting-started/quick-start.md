---
title: Быстрый старт
description: Запустите OxPHP менее чем за 5 минут
---

Это руководство проведёт вас через запуск OxPHP с Docker и раздачу вашего первого PHP-файла.

## 1. Создайте директорию проекта

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

## 2. Создайте Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data ./www /var/www/html
```

## 3. Добавьте compose.yml

Создайте файл `compose.yml`:

```yaml
services:
  oxphp:
    build: .
    ports:
      - "8080:8080"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html
      - INTERNAL_ADDR=0.0.0.0:9090
```

## 4. Создайте тестовый PHP-файл

```bash
mkdir -p www
```

Создайте файл `www/index.php`:

```php
<?php

$info = oxphp_server_info();
$requestId = oxphp_request_id();

echo "<h1>OxPHP</h1>\n";
echo "<p>Request ID: {$requestId}</p>\n";
echo "<p>SAPI: {$info['sapi']}</p>\n";
echo "<p>Version: {$info['version']}</p>\n";
echo "<p>Worker: {$info['worker_id']}</p>\n";
echo "<p>Time: " . date('c') . "</p>\n";
```

## 5. Запустите сервер

```bash
docker compose up -d
```

## 6. Протестируйте приложение

Откройте браузер по адресу `http://localhost:8080/` или воспользуйтесь curl:

```bash
curl http://localhost:8080/
```

Вы должны увидеть вывод, похожий на этот:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c100000001</p>
<p>SAPI: oxphp</p>
<p>Version: 0.1.0</p>
<p>Worker: 0</p>
<p>Time: 2026-02-11T12:00:00+00:00</p>
```

## 7. Проверьте работоспособность сервера

Внутренний сервер предоставляет эндпоинты здоровья и метрик на порту 9090:

```bash
# Проверка здоровья — возвращает 200 с {"status":"ok"}
curl http://localhost:9090/health

# Метрики в формате Prometheus
curl http://localhost:9090/metrics

# Текущая конфигурация сервера (чувствительные значения скрыты)
curl http://localhost:9090/config
```

## 8. Просмотрите логи

```bash
docker compose logs -f oxphp
```

OxPHP выводит структурированные логи в формате JSON. Каждый запрос порождает запись в журнале доступа с методом, путём, кодом статуса, временем ответа и идентификатором запроса.

## Следующие шаги

- [Руководство по Docker](docker.md) -- справочник по compose.yml, монтирование томов и советы по развёртыванию
- [Конфигурация](../operations/configuration.md) -- полный список переменных окружения
- [Маршрутизация](../features/routing.md) -- режимы маршрутизации Traditional, Framework и SPA
- [PHP-интеграция](../php/functions.md) -- доступные функции PHP-расширения

## Смотрите также

- [Установка](installation.md) -- инструкции по сборке из исходников и требования
- [Обзор архитектуры](../architecture/overview.md) -- модель выполнения и карта компонентов
- [Пул воркеров](../architecture/worker-pool.md) -- масштабирование PHP-воркеров и поведение очереди
