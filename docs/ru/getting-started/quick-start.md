---
title: Быстрый старт
description: Запуск OxPHP менее чем за 5 минут
---

Это руководство проведёт вас через запуск OxPHP с помощью Docker и обработку вашего первого PHP-файла.

## 1. Создайте каталог проекта

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

## 2. Добавьте docker-compose.yml

Создайте минимальный `docker-compose.yml`:

```yaml
services:
  oxphp:
    image: oxphp:latest
    build: .
    ports:
      - "8080:8080"
      - "9090:9090"
    volumes:
      - ./www:/var/www/html:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html
      - INTERNAL_ADDR=0.0.0.0:9090
```

Если у вас нет локального `Dockerfile`, склонируйте репозиторий OxPHP и соберите из него:

```bash
git clone https://github.com/oxphp/oxphp.git
cd oxphp
docker compose build
```

## 3. Создайте тестовый PHP-файл

```bash
mkdir -p www
```

Создайте `www/index.php`:

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

## 4. Запустите сервер

```bash
docker compose up -d
```

## 5. Проверьте ваше приложение

Откройте в браузере `http://localhost:8080/` или используйте curl:

```bash
curl http://localhost:8080/
```

Вы должны увидеть вывод, похожий на следующий:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c100000001</p>
<p>SAPI: oxphp</p>
<p>Version: 0.1.0</p>
<p>Worker: 0</p>
<p>Time: 2026-02-11T12:00:00+00:00</p>
```

## 6. Проверьте состояние сервера

Внутренний сервер предоставляет эндпоинты состояния и метрик на порту 9090:

```bash
# Проверка состояния — возвращает 200 с {"status":"ok"}
curl http://localhost:9090/health

# Метрики, совместимые с Prometheus
curl http://localhost:9090/metrics

# Текущая конфигурация сервера (конфиденциальные значения скрыты)
curl http://localhost:9090/config
```

## 7. Просмотр логов

```bash
docker compose logs -f oxphp
```

OxPHP выводит структурированные JSON-логи. Каждый запрос создаёт запись в журнале доступа с методом, путём, кодом статуса, временем ответа и идентификатором запроса.

## Следующие шаги

- [Руководство по Docker](/getting-started/docker/) -- этапы Dockerfile, описание docker-compose.yml и монтирование томов
- [Конфигурация](/operations/configuration/) -- полный список переменных окружения
- [Маршрутизация](/features/routing/) -- режимы Traditional, Framework и SPA
- [Интеграция с PHP](/php/functions/) -- доступные функции PHP-расширения

## Смотрите также

- [Установка](/getting-started/installation/) -- предварительные требования и инструкции по сборке из исходников
- [Обзор архитектуры](/architecture/overview/) -- модель выполнения и карта компонентов
- [Пул воркеров](/architecture/worker-pool/) -- масштабирование потоков PHP-воркеров и поведение очереди
