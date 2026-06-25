---
title: Быстрый старт
description: Запустите OxPHP менее чем за 5 минут. Создайте проект, напишите PHP-приложение, запустите сервер и выполните первый запрос.
---

# Быстрый старт

## Одна команда

Если у вас уже есть PHP-проект с директорией `public/`:

```bash
docker run -p 80:80 -v .:/var/www/html ghcr.io/oxphp/oxphp:0.9.0
```

Откройте `http://localhost/` — ваше приложение работает.

Для включения внутреннего сервера (health, metrics, config):

```bash
docker run -p 80:80 -p 9090:9090 -e INTERNAL_ADDR=0.0.0.0:9090 -v .:/var/www/html ghcr.io/oxphp/oxphp:0.9.0
```

---

## Пошаговый запуск с Docker Compose

Более детальная настройка — от пустой директории до работающего PHP-приложения с проверками состояния и структурированным логированием.

### 1. Создайте директорию проекта

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

### 2. Создайте Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.9.0

COPY --chown=www-data:www-data . /var/www/html
```

Официальный образ включает серверный бинарный файл, PHP 8.4 или 8.5 ZTS (по умолчанию 8.5; для 8.4 загрузите тег `:0.9.0-php8.4` или любой вариант `*-php8.4*`), PHP-расширение OxPHP и все зависимости времени выполнения.

> **Совет:** Если вашему приложению нужны дополнительные PHP-расширения (pdo_pgsql, intl, xdebug и т.д.), см. [`examples/dockerfile/Dockerfile`](../../../examples/dockerfile/Dockerfile) в репозитории — готовый многоэтапный Dockerfile с отдельными целями `dev` и `prod`.

### 3. Добавьте compose.yaml

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

### 4. Создайте PHP-приложение

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
echo "<p>SAPI: " . php_sapi_name() . "</p>\n";
echo "<p>Version: {$info['version']}</p>\n";
echo "<p>Time: " . date('c') . "</p>\n";
```

`oxphp_request_id()` возвращает уникальный ID, присвоенный каждому запросу. `oxphp_server_info()` возвращает сведения о работающем сервере, включая `version`, `worker_id`, `request_time` и `worker_mode`.

### 5. Соберите и запустите

```bash
docker compose up -d --build
```

### 6. Протестируйте приложение

```bash
curl http://localhost/
```

Ожидаемый вывод:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c11a2b00000001</p>
<p>Worker: 0</p>
<p>SAPI: cli-server</p>
<p>Version: 0.9.0</p>
<p>Time: 2026-03-23T12:00:00+00:00</p>
```

Каждый запрос получает уникальный ID. ID воркера показывает, какой PHP-поток его обработал.

> **Почему `php_sapi_name()` сообщает `cli-server` вместо `oxphp`?** OxPHP намеренно регистрируется под одним из имён SAPI, которые распознаёт OPcache. Для неизвестных SAPI OPcache отключает сам себя; без этого переименования выполнение PHP проходило бы мимо OPcache и работало бы в несколько раз медленнее. Компромисс в том, что использовать `php_sapi_name()` для определения OxPHP нельзя — используйте `function_exists('oxphp_request_id')` или `OxPHP\Http\Request::current()`.

### 7. Проверьте внутренние эндпоинты

```bash
# Проверка состояния — 200 при нормальной работе, 503 при деградации
curl http://localhost:9090/health

# Метрики, совместимые с Prometheus
curl http://localhost:9090/metrics

# Активная конфигурация (пути TLS скрыты)
curl http://localhost:9090/config
```

### 8. Просмотр логов

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
