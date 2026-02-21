---
title: Хуткі старт
description: Запусціце OxPHP менш чым за 5 хвілін
---

Гэты даведнік правядзе вас праз запуск OxPHP з Docker і абслугоўванне вашага першага PHP-файла.

## 1. Стварыце каталог праекта

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

## 2. Стварыце Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data ./www /var/www/html
```

## 3. Дадайце compose.yml

Стварыце `compose.yml`:

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

## 4. Стварыце тэставы PHP-файл

```bash
mkdir -p www
```

Стварыце `www/index.php`:

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

## 5. Запусціце сервер

```bash
docker compose up -d
```

## 6. Пратэстуйце сваю праграму

Адкрыйце браўзер па адрасе `http://localhost:8080/` або выкарыстоўвайце curl:

```bash
curl http://localhost:8080/
```

Вы павінны ўбачыць вывад, падобны да наступнага:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c100000001</p>
<p>SAPI: oxphp</p>
<p>Version: 0.1.0</p>
<p>Worker: 0</p>
<p>Time: 2026-02-11T12:00:00+00:00</p>
```

## 7. Праверце здароўе сервера

Унутраны сервер прадастаўляе эндпойнты здароўя і метрык на порце 9090:

```bash
# Праверка здароўя — вяртае 200 з {"status":"ok"}
curl http://localhost:9090/health

# Метрыкі, сумяшчальныя з Prometheus
curl http://localhost:9090/metrics

# Бягучая канфігурацыя сервера (адчувальныя значэнні схаваны)
curl http://localhost:9090/config
```

## 8. Прагляд журналаў

```bash
docker compose logs -f oxphp
```

OxPHP выводзіць структураваныя JSON-журналы. Кожны запыт стварае запіс у журнале доступу з метадам, шляхом, кодам стану, часам адказу і ідэнтыфікатарам запыту.

## Наступныя крокі

- [Даведнік па Docker](docker.md) -- даведнік па compose.yml, мантаванне тамоў і парады па разгортванні
- [Канфігурацыя](../operations/configuration.md) -- поўны спіс зменных асяроддзя
- [Маршрутызацыя](../features/routing.md) -- традыцыйны, фрэймворкавы і SPA-рэжымы маршрутызацыі
- [Інтэграцыя з PHP](../php/functions.md) -- даступныя функцыі PHP-пашырэння

## Гл. таксама

- [Усталяванне](installation.md) -- інструкцыі па зборцы з зыходных кодаў і папярэднія патрабаванні
- [Агляд архітэктуры](../architecture/overview.md) -- мадэль выканання і карта кампанентаў
- [Пул воркераў](../architecture/worker-pool.md) -- маштабаванне патокаў PHP-воркераў і паводзіны чаргі
