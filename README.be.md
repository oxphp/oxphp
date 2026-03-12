<p align="center">
  <img src="logo.svg" alt="OxPHP" width="300">
</p>

<h3 align="center">Шматпатокавы сервер PHP-прыкладанняў, створаны для воблачна-натыўнай інфраструктуры.</h3>

<p align="center">
  OxPHP — гэта асінхронны сервер PHP-прыкладанняў, напісаны на Rust,<br>
  створаны для прадакшн-нагрузак, якія патрабуюць нізкай затрымкі, высокай канкурэнтнасці і назіральнасці без дадатковай канфігурацыі.
</p>

<p align="center">
  <a href="docs/en/">Docs</a> · <a href="#хуткі-старт">Хуткі старт</a> · <a href="#чаму-oxphp">Чаму OxPHP</a> · <a href="#канфігурацыя">Канфігурацыя</a>
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/rust-powered-orange">
  <img alt="PHP" src="https://img.shields.io/badge/php-8.4-blue">
  <img alt="License" src="https://img.shields.io/github/license/oxphp/oxphp">
  <img alt="Release" src="https://img.shields.io/github/v/release/oxphp/oxphp">
  <img alt="Stars" src="https://img.shields.io/github/stars/oxphp/oxphp?style=flat">
  <img alt="Docker" src="https://img.shields.io/badge/docker-ghcr.io-2496ED?logo=docker&logoColor=white">
  <img alt="HTTP/2" src="https://img.shields.io/badge/HTTP%2F2-supported-brightgreen">
  <img alt="TLS" src="https://img.shields.io/badge/TLS-1.3-brightgreen">
</p>

---

## Хуткі старт

Два радкі. І ўсё.

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

> **Заўвага:** Па змаўчанні `DOCUMENT_ROOT` — гэта `/var/www/html/public`. Размяшчайце ўваходныя скрыпты (напрыклад, `index.php`) у паддырэкторыі `public/` — OxPHP будзе раздаваць файлы менавіта адтуль, а не з кораня `/var/www/html`. Гэта адпавядае стандартнай структуры фрэймворкаў, такіх як Laravel, Symfony і Slim.

```bash
docker build -t my-app . && docker run -p 8080:8080 my-app
curl http://localhost:8080/
```

Без канфігурацыі nginx. Без наладкі пулаў PHP-FPM. Без менеджара працэсаў. Проста ваша прыкладанне.

---

## Чаму OxPHP?

Традыцыйны стэк PHP — гэта тры кампаненты, склееныя разам: вэб-сервер, менеджар працэсаў і асяроддзе выканання PHP. Кожны дадае паверхню канфігурацыі, рэжымы збояў і аперацыйныя выдаткі.

OxPHP аб'ядноўвае ўсе тры ў адзін бінарны файл на Rust з убудаваным PHP.

| | nginx + PHP-FPM | FrankenPHP | RoadRunner | **OxPHP** |
|---|---|---|---|---|
| Language | C / C | Go + C | Go | **Rust** |
| HTTP/2 | ✅ | ✅ | ✅ | ✅ |
| TLS built-in | ✅ | ✅ | ✅ | ✅ (rustls, TLS 1.3) |
| Worker mode | ❌ | ✅ | ✅ | ✅ |
| Backpressure / 503 | manual | ❌ | ❌ | ✅ built-in |
| Prometheus metrics | plugin | plugin | plugin | ✅ built-in |
| Per-IP rate limiting | nginx module | ❌ | ❌ | ✅ built-in |
| Custom error pages | ✅ (nginx config) | ✅ (Caddyfile) | ❌ | ✅ preloaded at startup |
| HTTP/3 | ✅ | ✅ | ✅ experimental | 🔜 roadmap |
| HTTP 103 Early Hints | ✅ (v1.29+) | ✅ | ✅ | 🔜 roadmap |
| Memory safety | ❌ | partial | partial | ✅ Rust |

---

## Бенчмаркі

> Фармальныя бенчмаркі хутка з'явяцца. Мы працуем над узнаўляльным наборам тэстаў, які ахоплівае req/s, затрымкі (p50/p99), выкарыстанне памяці і прапускную здольнасць воркераў пад канкурэнтнай нагрузкай.

---

## Магчымасці

### PHP Runtime
- **Натыўнае выкананне PHP** праз уласны SAPI (`oxphp`) з пулам патокаў ZTS
- **Поўная падтрымка супергеталальных зменных**: `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `php://input`
- **Натыўны масток Rust↔PHP** — без серыялізацыі, праз прамы доступ да `zval` з дапамогай функцый-аксесараў на C
- **Сістэма плагінаў** з тыпізаванай дыспетчарызацыяй падзей, упарадкаваннем па прыярытэце і рэгістрацыяй PHP-функцый
- **Ізаляцыя панік** праз `catch_unwind` — збой PHP не прыводзіць да падзення сервера

### Worker Model
- **Рэжым воркера** — пастаянныя PHP-працэсы з мяккім скідам, якія захоўваюць аўтазагрузчыкі і злучэнні з БД паміж запытамі
- **Аўтаматычная рэцыклізацыя** па ліку запытаў або парогу памяці
- **Маніторынг стану воркераў** — аварыйна завершаныя воркеры аўтаматычна вызначаюцца і перазапускаюцца
- **Ранні адказ** праз `oxphp_finish_request()` — адпраўка адказу з працягам фонавай апрацоўкі

### HTTP & Networking
- **HTTP/1.1 + HTTP/2** з аўтавызначэннем (h2c) праз hyper
- **TLS 1.3** з ALPN (h2 + http/1.1) праз rustls
- **3 рэжымы маршрутызацыі** — Traditional, Framework (`index.php`), SPA (`index.html`)
- **Стрымінг SSE** праз аўтавызначэнне `Content-Type: text/event-stream` або `oxphp_stream_flush()`
- **Наладжвальныя тайм-аўты** — чытанне загалоўкаў, апрацоўка запыту і keep-alive

### Performance
- **LRU-кэш статычных файлаў** (у памяці для файлаў ≤1 МБ, патокавая перадача для большых)
- **HTTP-кэшаванне** з ETag, Last-Modified і падтрымкай 304 Not Modified
- **Сціск Brotli** для тэкставых адказаў (дыяпазон 256 Б – 3 МБ)
- **Алакатар mimalloc** для меншай затрымкі выдзялення памяці пад нагрузкай
- **Наладжвальны runtime Tokio** — шматпатокавы па змаўчанні (CPU/2), наладжваецца праз `TOKIO_WORKERS`

### Надзейнасць і эксплуатацыя
- **Абмежаваная чарга запытаў** з адмовай 503 пры перапаўненні
- **Абмежаванне частоты запытаў па IP** з загалоўкамі `X-RateLimit-*` і адказамі 429
- **Метрыкі Prometheus** на `/metrics` — па кожным воркеры, без залежнасцей
- **Праверка стану** на `/health` — гатова для probe-аў гатоўнасці K8s
- **Структураванае лагіраванне памылак** — памылкі PHP перадаюцца праз `tracing` з палямі `php_error_type`, `php_file`, `php_line`
- **JSON-лагіраванне доступу** (узроўні: `all`, `error`, выключана праз `ACCESS_LOG`)
- **Карыстальніцкія старонкі памылак** — загружаюцца пры старце, без I/O на гарачым шляху
- **Абарона ад абходу шляху** з вызначэннем выхаду за межы праз сімвалічныя спасылкі
- **Запуск кантэйнера без правоў root** ад імя www-data (UID 82)
- **Генерацыя ідэнтыфікатара запыту** і яго перадача (`X-Request-ID`)

---

## Архітэктура

```
                    ┌──────────────┐
                    │  Tokio async │  configurable: single- or multi-threaded
                    │  HTTP server │  (hyper + hyper-util + mimalloc)
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │Route dispatch│  static file / PHP / 404
                    └──────┬───────┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Static file   PHP request   Not found
         (LRU cache)   (channel)      (404)
                           │
                    ┌──────▼───────┐
                    │Bounded queue │  crossbeam bounded channel
                    │(backpressure)│  503 when full
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         PHP Worker   PHP Worker   PHP Worker    OS threads (ZTS)
         (SAPI exec)  (SAPI exec)  (SAPI exec)   with thread-local state
```

- **Асінхронны runtime Tokio** — шматпатокавы па змаўчанні, наладжваецца праз `TOKIO_WORKERS`
- **Пул воркераў ZTS** — кожны воркер — гэта выдзелены паток АС з ізаляцыяй праз `catch_unwind`
- Воркеры атрымліваюць запыты праз `crossbeam::bounded` і вяртаюць вынік праз `ExecuteResult` (неадкладна або адкладзена праз `oneshot`)
- **Рэжым воркера** — пастаянныя PHP-працэсы з мяккім скідам; захоўваюць стан загрузкі (аўтазагрузчыкі, злучэнні з БД) паміж запытамі

### Унутраны сервер

Калі зададзена `INTERNAL_ADDR`, на асобным порце запускаецца лёгкі HTTP-сервер:

| Endpoint | Апісанне |
|----------|-------------|
| `GET /health` | JSON-статус стану (uptime, запыты, злучэнні) |
| `GET /metrics` | Метрыкі ў тэкставым фармаце Prometheus |
| `GET /config` | JSON-канфігурацыя runtime (шляхі TLS рэдагуюцца) |

---

## Канфігурацыя

Усе параметры задаюцца праз зменныя асяроддзя — канфігурацыйныя файлы не патрэбныя.

| Variable | Default | Апісанне |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Адрас і порт для прывязкі |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Шлях у файлавай сістэме, з якога раздаюцца файлы |
| `INDEX_FILE` | *(не зададзена)* | Рэжым маршрутызацыі: пуста = Traditional, `index.php` = Framework, `index.html` = SPA |
| `TOKIO_WORKERS` | `0` (CPU / 2, мін. 1) | Патокі асінхроннага I/O; `0` = аўта |
| `EXECUTOR` | `sapi` | Выканаўца PHP: `sapi` (сапраўдны PHP) або `stub` (рэжым тэсціравання) |
| `PHP_WORKERS` | `0` (CPU / 2, мін. 1) | Пул воркераў: `N` = фіксаваны, `MIN:MAX` = дынамічны, `0` = аўта |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Час прастою да завяршэння дынамічнага воркера |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Памер абмежаванага канала; 503 пры перапаўненні |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Тайм-аўт плаўнага завяршэння |
| `LOG_LEVEL` | `info` | Узровень дэталізацыі трэйсінгу: `error`, `warn`, `info`, `debug`, `trace` |
| `INTERNAL_ADDR` | *(не зададзена)* | Унутраны сервер для health/metrics/config (напрыклад, `0.0.0.0:9090`) |
| `RATE_LIMIT` | `0` (выключана) | Максімальная колькасць запытаў з аднаго IP за вакно |
| `RATE_WINDOW_SECONDS` | `60` | Акно абмежавання частоты ў секундах |
| `HEADER_TIMEOUT_SECONDS` | `5` | Тайм-аўт чытання загалоўкаў (абарона ад Slowloris) |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Агульны тайм-аўт запыту; 0 = выключана |
| `TLS_CERT` | *(не зададзена)* | Шлях да PEM-файла TLS-сертыфіката |
| `TLS_KEY` | *(не зададзена)* | Шлях да PEM-файла прыватнага ключа TLS |
| `ERROR_PAGES_DIR` | *(не зададзена)* | Дырэкторыя з карыстальніцкімі старонкамі памылак (`{status}.html`) |
| `STATIC_CACHE_TTL` | `30d` | TTL кэша статычных файлаў (`30s`, `5m`, `2h`, `30d`, `1y`, `off`) |
| `COMPRESSION_LEVEL` | `4` | Якасць Brotli (0 = выключана, 1–11) |
| `ACCESS_LOG` | *(выкл.)* | JSON-лог доступу: `all`, `error` або не зададзена |
| `MAX_CONNECTIONS` | `10000` | Максімальная колькасць адначасовых злучэнняў |
| `WORKER_FILE` | *(не зададзена)* | Шлях да PHP-скрыпту воркера; уключае рэжым пастаянных воркераў |
| `WORKER_MAX_REQUESTS` | `0` (без абмежаванняў) | Макс. запытаў на воркер да рэцыклізацыі |
| `WORKER_MAX_MEMORY_MIB` | `0` (без абмежаванняў) | Макс. памяць (МіБ) на воркер да рэцыклізацыі |

---

## Зборка

```bash
# На хасце (без PHP — усе тэсты праходзяць, без выканання PHP)
cargo build --release

# Docker (з PHP — поўная функцыянальнасць)
docker compose build
```

### Лакальны запуск (толькі статычныя файлы)

```bash
DOCUMENT_ROOT=./www/public ./target/release/oxphp
```

## Распрацоўка

```bash
# Поўная праверка (хост, 167 тэстаў)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Дымавы тэст у Docker
docker compose build && docker compose up -d
curl http://localhost:8080/
curl "http://localhost:8080/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost:8080/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost:8080/test_superglobals.php

# Унутраны сервер
INTERNAL_ADDR=127.0.0.1:9090 ./target/release/oxphp &
curl http://localhost:9090/health
curl http://localhost:9090/metrics
```

---

## Дарожная карта

> Элементы не ўпарадкаваны па прыярытэце. Наяўнасць у гэтым спісе не гарантуе рэалізацыю.

| Feature | Апісанне |
|---|---|
| **PHP 8.5** | Падтрымка PHP 8.5 адразу пасля яго выхаду |
| **Trace Context (W3C)** | Аўтаматычнае распаўсюджванне загалоўкаў `traceparent` / `tracestate` паміж запытамі |
| **OpenTelemetry** | Экспарт трэйсаў і метрык праз OTLP у любы сумяшчальны бэкенд |
| **Custom Metrics** | PHP API для рэгістрацыі карыстальніцкіх метрык Prometheus з кода прыкладання |
| **Built-in PHP Profiler** | Нізканакладнае прафіляванне без xdebug або знешніх агентаў, убудаванае непасрэдна ў сервер |
| **Dockerfile.bookworm** | Афіцыйны вобраз на базе Debian Bookworm як альтэрнатыва Alpine |
| **Non-Docker Install** | Натыўная ўстаноўка праз сістэмныя пакетныя менеджары (apt, brew і інш.) |
| **HTTP/3** | Падтрымка HTTP/3 на базе QUIC |
| **HTTP 103 Early Hints** | Адпраўка адказаў `103 Early Hints` для прадзагрузкі рэсурсаў кліентам да фінальнага адказу |
| **Ecosystem Plugins** | Пашыраная сістэма плагінаў: больш хукаў жыццёвага цыкла, багацейшы PHP API і дакументацыя для аўтараў старонніх плагінаў |
| **Shared Async Runtime** | Адкрыццё runtime Tokio для PHP-воркераў, што дазваляе асінхронныя аперацыі з кода прыкладання |
| **Database Connection Pool** | Убудаваны пул злучэнняў праз `sqlx`, які зніжае накладныя выдаткі на злучэнне пры кожным запыце |
| **gRPC Server** | *(спекулятыўна)* Альтэрнатыўны рэжым сервера — gRPC замест HTTP; вельмі нявызначана, магчыма, не будзе рэалізавана |
| **Promise API** | *(спекулятыўна)* `OxPHP\Promise` і `AsyncTask` — PHP API для асінхроннага выканання задач на базе runtime Tokio; разглядаецца |
| **Diagnostics** | Прадакшн-дыягностыка: праверка абмежаванняў АС (ulimit, TCP backlog, epoll/kqueue, налады кантэйнера), выяўленне вузкіх месцаў прадукцыйнасці (глыбіня чаргі воркераў, канкурэнцыя блакіровак, нагрузка на GC/алакатар, статыстыка ZTS) і канкрэтныя рэкамендацыі па дзеянням |

## Дакументацыя

- [English](docs/en/)
- [中文](docs/zh/)
- [Русский](docs/ru/)
- [Беларуская](docs/be/)

## Ліцэнзія

[AGPL-3.0](LICENSE)
