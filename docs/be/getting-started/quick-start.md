---
title: Хуткі старт
description: Запусціце OxPHP менш чым за 5 хвілін
---

Гэты даведнік правядзе вас праз запуск OxPHP з Docker і абслугоўванне вашага першага PHP-файла.

## 1. Стварыце каталог праекта

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

## 2. Дадайце compose.yml

Стварыце мінімальны `compose.yml`:

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

Калі ў вас няма лакальнага `Dockerfile`, кланіруйце рэпазіторый OxPHP і збярыце з яго:

```bash
git clone https://github.com/oxphp/oxphp.git
cd oxphp
docker compose build
```

## 3. Стварыце тэставы PHP-файл

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

## 4. Запусціце сервер

```bash
docker compose up -d
```

## 5. Пратэстуйце ваш дадатак

Адкрыйце браўзер на `http://localhost:8080/` або выкарыстайце curl:

```bash
curl http://localhost:8080/
```

Вы павінны ўбачыць вывад, падобны да гэтага:

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c100000001</p>
<p>SAPI: oxphp</p>
<p>Version: 0.1.0</p>
<p>Worker: 0</p>
<p>Time: 2026-02-11T12:00:00+00:00</p>
```

## 6. Праверце стан сервера

Унутраны сервер прадастаўляе эндпойнты стану і метрык на порце 9090:

```bash
# Праверка стану -- вяртае 200 з {"status":"ok"}
curl http://localhost:9090/health

# Метрыкі, сумяшчальныя з Prometheus
curl http://localhost:9090/metrics

# Бягучая канфігурацыя сервера (адчувальныя значэнні схаваны)
curl http://localhost:9090/config
```

## 7. Прагляд логаў

```bash
docker compose logs -f oxphp
```

OxPHP выводзіць структураваныя JSON-логі. Кожны запыт стварае запіс у логу доступу з метадам, шляхом, кодам стану, часам адказу і ідэнтыфікатарам запыту.

## Наступныя крокі

- [Даведнік па Docker](/getting-started/docker/) -- стадыі Dockerfile, даведнік compose.yml і мантаванне тамоў
- [Канфігурацыя](/operations/configuration/) -- поўны спіс зменных асяроддзя
- [Маршрутызацыя](/features/routing/) -- традыцыйны, фрэймворкавы і SPA-рэжымы маршрутызацыі
- [Інтэграцыя з PHP](/php/functions/) -- даступныя функцыі PHP-пашырэння

## Глядзіце таксама

- [Усталяванне](/getting-started/installation/) -- перадумовы зборкі і інструкцыі па зборцы з зыходнікаў
- [Агляд архітэктуры](/architecture/overview/) -- мадэль выканання і карта кампанентаў
- [Пул воркераў](/architecture/worker-pool/) -- маштабаванне патокаў PHP-воркераў і паводзіны чаргі
