---
title: Docker
description: Выкарыстанне Docker-выявы, даведнік па compose.yml і парады па разгортванні
---

OxPHP распаўсюджваецца як гатовая Docker-выява па адрасе `ghcr.io/oxphp/oxphp:0.1.0`. На гэтай старонцы апісваецца, як выкарыстоўваць выяву, наладжваць яе з дапамогай `compose.yml` і распаўсюджаныя пытанні разгортвання.

## Выкарыстанне выявы

Самы просты спосаб запусціць OxPHP — пашырыць базавую выяву файламі вашай праграмы:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

Выява ўключае:

- Бінарны файл `oxphp`
- Асяроддзе выканання PHP 8.4 ZTS (`libphp.so`)
- Бібліятэку моста (`liboxphp_bridge.so`)
- PHP-пашырэнне (`oxphp_sapi.so`) з функцыямі `oxphp_request_id()`, `oxphp_server_info()` і іншымі
- Базавую сістэму Alpine Linux з мінімальнымі залежнасцямі выканання
- Карыстальніка `www-data` (UID 82, GID 82) для выканання без прывілеяў root

Каранёвы каталог дакументаў па змаўчанні — `/var/www/html/public`. Сервер слухае на порце 8080. `CMD` — `["oxphp"]`.

## Даведнік па compose.yml

```yaml
services:
  oxphp:
    build: .
    ports:
      - "8080:8080"   # Асноўны HTTP-сервер
      - "9090:9090"   # Унутраны сервер (health/metrics/config)
    volumes:
      - ./www:/var/www/html:ro
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
      - ./certs:/etc/ssl/oxphp:ro
    environment:
      # Сервер
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html/public
      # - INDEX_FILE=index.php       # Уключае фрэймворкавы рэжым маршрутызацыі
      - EXECUTOR=sapi                # "sapi" або "stub"
      # - PHP_WORKERS=0              # Статычны: 0 = CPU/2 (мін. 1), або фіксаванае N
      # - PHP_WORKERS=2:16           # Дынамічны: маштабаванне паміж 2 і 16
      # - PHP_WORKERS_IDLE_SECONDS=30    # Тайм-аўт прастою для дынамічнага памяншэння
      # - QUEUE_CAPACITY=512         # Па змаўчанні: PHP_WORKERS * 128

      # Журналаванне
      - LOG_LEVEL=info

      # Унутраны сервер
      - INTERNAL_ADDR=0.0.0.0:9090

      # Тайм-аўты (секунды)
      - HEADER_TIMEOUT_SECONDS=5
      - REQUEST_TIMEOUT_SECONDS=120
      - DRAIN_TIMEOUT_SECONDS=30

      # Абмежаванне частаты запытаў (0 = выключана)
      # - RATE_LIMIT=100
      # - RATE_WINDOW_SECONDS=60

      # TLS
      # - TLS_CERT=/etc/ssl/oxphp/server.pem
      # - TLS_KEY=/etc/ssl/oxphp/server.key

      # Старонкі памылак
      # - ERROR_PAGES_DIR=/var/www/errors

      # Узровень сціску (0-11, 0=адключана, па змаўчанні: 4)
      # - COMPRESSION_LEVEL=4
    restart: unless-stopped
```

Для распрацоўкі можна замантаваць зыходны каталог як том замест капіравання файлаў у выяву:

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "8080:8080"
    volumes:
      - ./www:/var/www/html:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html/public
```

### Зменныя асяроддзя

| Зменная | Па змаўчанні | Апісанне |
|---------|-------------|----------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Адрас і порт асноўнага HTTP-сервера |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Каранёвы каталог для абслугоўвання файлаў |
| `INDEX_FILE` | _(не зададзена)_ | Усталюйце `index.php` для фрэймворкавага рэжыму або `index.html` для SPA-рэжыму |
| `EXECUTOR` | `sapi` | Тып PHP executor: `sapi` (рэальны PHP) або `stub` (загальнік) |
| `PHP_WORKERS` | `0` (CPU / 2, мін. 1, статычны) | Рэжым пула воркераў. `N` = фіксаваны пул, `MIN:MAX` = дынамічнае маштабаванне |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Тайм-аўт прастою перад выдаленнем дынамічнага воркера |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Памер абмежаванай чаргі запытаў. 503 вяртаецца, калі поўная |
| `LOG_LEVEL` | `info` | Узровень журналавання: `trace`, `debug`, `info`, `warn`, `error` |
| `MAX_CONNECTIONS` | `10000` | Максімальная колькасць адначасовых злучэнняў |
| `INTERNAL_ADDR` | _(не зададзена)_ | Адрас унутранага сервера. Калі не зададзена, ён адключаны |
| `HEADER_TIMEOUT_SECONDS` | `5` | Тайм-аўт чытання загалоўкаў запыту |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Максімальны час апрацоўкі запыту. 0 адключае тайм-аўт |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Перыяд чакання для незавершаных злучэнняў падчас спынкі |
| `RATE_LIMIT` | `0` | Максімальная колькасць запытаў на IP за акно. 0 адключае абмежаванне |
| `RATE_WINDOW_SECONDS` | `60` | Акно абмежавання частаты запытаў у секундах |
| `TLS_CERT` | _(не зададзена)_ | Шлях да файла сертыфіката TLS у фармаце PEM |
| `TLS_KEY` | _(не зададзена)_ | Шлях да файла прыватнага ключа TLS у фармаце PEM |
| `ERROR_PAGES_DIR` | _(не зададзена)_ | Каталог з файламі старонак памылак `{status}.html` |
| `COMPRESSION_LEVEL` | `4` | Узровень якасці сціску Brotli (0-11). `0` адключае сціск |
| `TOKIO_WORKERS` | `0` (CPU / 2, мін. 1) | Патокі асінхроннага асяроддзя выканання Tokio (0 = аўта, 1 = аднапаточны) |
| `ACCESS_LOG` | *(выкл.)* | JSON-журнал доступу: `all`, `error` (толькі 4xx/5xx), пустое = выкл. |


### Парты

| Порт | Прызначэнне |
|------|------------|
| `8080` | Асноўны HTTP-сервер (або HTTPS, калі наладжаны TLS) |
| `9090` | Унутраны сервер: `/health`, `/metrics`, `/config` |

### Мантаванне тамоў

| Шлях на хасце | Шлях у кантэйнеры | Прызначэнне |
|---------------|-------------------|-------------|
| `./www` | `/var/www/html` | Файлы праграмы (PHP-скрыпты, статычныя рэсурсы). Мантуйце як `:ro` |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | Канфігурацыя PHP (OPcache, сесіі). Мантуйце як `:ro` |
| `./certs` | `/etc/ssl/oxphp` | Файлы сертыфіката і ключа TLS. Мантуйце як `:ro` |

## Канфігурацыя PHP

Каб наладзіць параметры PHP (OPcache, JIT, сесіі і г. д.), стварыце файл `oxphp.ini` і замантуйце яго ў кантэйнер:

```ini
[opcache]
opcache.enable=1
opcache.jit=1255
opcache.jit_buffer_size=64M
```

```yaml
volumes:
  - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
```

Гл. [OPcache](../php/opcache.md) для рэкамендаваных параметраў.

## Карыстальнік www-data у Alpine

Выява выконваецца ад імя `www-data` (UID 82, GID 82) для сумяшчальнасці з канвенцыямі nginx і Apache. Калі вашай праграме патрэбны запіс у пэўныя каталогі (сесіі, кэш, загрузкі), пераканайцеся, што гэтыя каталогі даступныя для запісу карыстальніку з UID 82.

## Зборка з зыходных кодаў

Калі вам патрэбна сабраць OxPHP з зыходных кодаў (напрыклад, каб уключыць карыстальніцкія магчымасці Cargo або змяніць сервер), звярніцеся да даведніка [Усталяванне](installation.md) для атрымання інструкцый па зборцы. Рэпазіторый OxPHP уключае шматэтапны Dockerfile, які кампілюе бібліятэку моста, PHP-пашырэнне і бінарны файл Rust з зыходных кодаў.

## Гл. таксама

- [Усталяванне](installation.md) -- папярэднія патрабаванні і інструкцыі па зборцы з зыходных кодаў
- [Хуткі старт](quick-start.md) -- запусціце OxPHP менш чым за 5 хвілін
- [Канфігурацыя](../operations/configuration.md) -- поўны даведнік па зменных асяроддзя
- [Плаўная спынка](../operations/graceful-shutdown.md) -- паводзіны завяршэння і налады тайм-аўтаў
