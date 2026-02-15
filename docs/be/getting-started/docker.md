---
title: Docker
description: Стадыі Dockerfile, даведнік docker-compose.yml і парады па разгортванні
---

OxPHP пастаўляецца з шматстадыйным Dockerfile, які стварае мінімальны рантайм-вобраз Alpine. На гэтай старонцы тлумачыцца кожная стадыя зборкі, канфігурацыя `docker-compose.yml` і тыповыя пытанні разгортвання.

## Стадыі Dockerfile

Dockerfile мае чатыры стадыі. Кожная стадыя збірае адзін кампанент і перадае артэфакты далей.

### Стадыя 1: bridge-builder

```dockerfile
FROM alpine:3.21 AS bridge-builder
RUN apk add --no-cache gcc musl-dev make
COPY ext/bridge/ ./
RUN make && make install
```

Кампілюе `liboxphp_bridge.so` -- невялікую агульную бібліятэку на C, якая забяспечвае зменныя `__thread` TLS, агульныя паміж Rust і PHP-пашырэннем. Збіраецца на чыстым Alpine толькі з gcc -- без залежнасці ад PHP.

**Артэфакты:** `/usr/local/lib/liboxphp_bridge.so`, `/usr/local/include/oxphp_bridge.h`

### Стадыя 2: ext-builder

```dockerfile
FROM php:8.4-zts-alpine AS ext-builder
RUN apk add --no-cache gcc musl-dev make autoconf
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/
COPY ext/config.m4 ext/php_oxphp_sapi.h ext/oxphp_sapi.c ./
COPY ext/bridge/oxphp_bridge.h ./bridge/
RUN phpize && ./configure --enable-oxphp-sapi && make && make install
```

Збірае PHP-пашырэнне (`oxphp_sapi.so`) з дапамогай `phpize` з вобраза PHP 8.4 ZTS. Пашырэнне звязваецца з бібліятэкай моста і робіць даступнымі для PHP такія функцыі, як `oxphp_request_id()` і `oxphp_server_info()`.

**Артэфакты:** файл `.so` PHP-пашырэння ў `/usr/local/lib/php/extensions/`

### Стадыя 3: builder

```dockerfile
FROM php:8.4-zts-alpine AS builder
RUN apk add --no-cache rust cargo musl-dev pkgconfig ...
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY Cargo.toml Cargo.lock ./

ARG CARGO_FEATURES=""

RUN mkdir src && echo "fn main() {}" > src/main.rs && touch src/lib.rs && \
    cargo build --release && \
    rm -rf src target/release/oxphp target/release/deps/oxphp-* target/release/.fingerprint/oxphp-*
COPY src ./src
COPY build.rs ./
RUN if [ -n "${CARGO_FEATURES}" ]; then \
        cargo build --release --features "${CARGO_FEATURES}"; \
    else \
        cargo build --release; \
    fi
```

Збірае бінарны файл Rust у тым самым вобразе `php:8.4-zts-alpine`. Гэта неабходна, таму што бінарны файл звязваецца з `libphp.so` і `liboxphp_bridge.so` -- зборка ў іншым вобразе з іншай версіяй musl выклікае пашкоджанне TLS падчас выканання.

На гэтай стадыі выкарыстоўваецца прыём кэшавання залежнасцяў: спачатку адбываецца зборка з фіктыўным `main.rs` для кэшавання ўсіх крэйтаў залежнасцяў, потым выдаляюцца толькі артэфакты, спецыфічныя для OxPHP (`target/release/oxphp`, `deps/oxphp-*`, `.fingerprint/oxphp-*`), перш чым капіраваць рэальны зыходны код. Такім чынам, пры зменах зыходнага кода перазбіраецца толькі фінальны бінарны файл.

Аргумент зборкі `CARGO_FEATURES` дазваляе ўключаць дадатковыя магчымасці Cargo (напрыклад, `plugin-debug`) падчас зборкі без змены Dockerfile.

**Артэфакты:** `/build/target/release/oxphp`

### Стадыя 4: runtime

```dockerfile
FROM alpine:3.21
RUN apk add --no-cache libgcc libxml2 sqlite-libs libcurl oniguruma argon2-libs zlib ...
COPY --from=builder /usr/local/lib/libphp.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ext-builder /usr/local/lib/php/extensions/ /usr/local/lib/php/extensions/
COPY --from=builder /build/target/release/oxphp /usr/local/bin/oxphp
ENV LD_LIBRARY_PATH=/usr/local/lib
USER www-data
EXPOSE 8080
CMD ["oxphp"]
```

Фінальны рантайм-вобраз заснаваны на `alpine:3.21`. Ён капіруе толькі неабходнае:

- `libphp.so` -- бібліятэка рантайму PHP
- `liboxphp_bridge.so` -- бібліятэка моста на C
- Файлы PHP-пашырэння
- Бінарны файл `oxphp`
- Канфігурацыя PHP (`oxphp.ini`, загрузка пашырэнняў)
- Змесціва вэб-кораня па змаўчанні (`/var/www/html/`)

Карыстальнік `www-data` (UID 82, GID 82) запускае серверны працэс. Alpine 3.21 ужо мае папярэдне створаную групу `www-data`, таму Dockerfile дадае толькі карыстальніка.

`LD_LIBRARY_PATH=/usr/local/lib` зададзена, каб дынамічны кампаноўшчык мог знайсці `libphp.so` і `liboxphp_bridge.so` падчас выканання.

## Даведнік docker-compose.yml

```yaml
services:
  oxphp:
    build:
      context: .
      args:
        # Дадатковыя магчымасці Cargo (праз прабел), напр. "plugin-debug"
        CARGO_FEATURES: ""
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
      - DOCUMENT_ROOT=/var/www/html
      # - INDEX_FILE=index.php       # Уключае фрэймворкавы рэжым маршрутызацыі
      - EXECUTOR=sapi                # "sapi" або "stub"
      # - PHP_WORKERS=0              # Статычны: 0 = CPU*2, або фіксаванае N
      # - PHP_WORKERS=2:16           # Дынамічны: маштабаванне паміж 2 і 16
      # - PHP_WORKERS_IDLE_SEC=30    # Тайм-аўт прастою для дынамічнага памяншэння
      # - QUEUE_CAPACITY=512         # Па змаўчанні: PHP_WORKERS * 128
      # Лагіраванне
      - LOG_LEVEL=info

      # Унутраны сервер
      - INTERNAL_ADDR=0.0.0.0:9090

      # Тайм-аўты (секунды)
      - HEADER_TIMEOUT_SECS=5
      - IDLE_TIMEOUT_SECS=60
      - REQUEST_TIMEOUT_SECS=120
      - DRAIN_TIMEOUT_SECS=30

      # Абмежаванне частаты запытаў (0 = выключана)
      # - RATE_LIMIT=100
      # - RATE_WINDOW=60

      # TLS
      # - TLS_CERT=/etc/ssl/oxphp/server.pem
      # - TLS_KEY=/etc/ssl/oxphp/server.key

      # Старонкі памылак
      # - ERROR_PAGES_DIR=/var/www/errors

      # Сціск (па змаўчанні: true)
      # - COMPRESSION=true
    restart: unless-stopped
```

### Аргументы зборкі

| Аргумент | Па змаўчанні | Апісанне |
|----------|--------------|----------|
| `CARGO_FEATURES` | `""` | Спіс дадатковых магчымасцяў Cargo праз прабел (напр. `plugin-debug`) |

### Зменныя асяроддзя

| Зменная | Па змаўчанні | Апісанне |
|---------|--------------|----------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Адрас і порт асноўнага HTTP-сервера |
| `DOCUMENT_ROOT` | `/var/www/html` | Каранёвы каталог для абслугоўвання файлаў |
| `INDEX_FILE` | _(не зададзена)_ | Усталюйце `index.php` для фрэймворкавага рэжыму або `index.html` для SPA-рэжыму |
| `EXECUTOR` | `sapi` | Тып PHP executor: `sapi` (рэальны PHP) або `stub` (заглушка) |
| `PHP_WORKERS` | `0` (CPU * 2, статычны) | Рэжым пула воркераў. `N` = фіксаваны пул, `MIN:MAX` = дынамічнае маштабаванне |
| `PHP_WORKERS_IDLE_SEC` | `30` | Тайм-аўт прастою перад выдаленнем дынамічнага воркера |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Памер абмежаванай чаргі запытаў. 503 вяртаецца, калі поўная |
| `LOG_LEVEL` | `info` | Узровень лагіравання: `trace`, `debug`, `info`, `warn`, `error` |
| `MAX_CONNECTIONS` | `10000` | Максімальная колькасць адначасовых злучэнняў |
| `INTERNAL_ADDR` | _(не зададзена)_ | Адрас унутранага сервера. Калі не зададзена, ён адключаны |
| `HEADER_TIMEOUT_SECS` | `5` | Тайм-аўт чытання загалоўкаў запыту |
| `IDLE_TIMEOUT_SECS` | `60` | Тайм-аўт прастою keep-alive |
| `REQUEST_TIMEOUT_SECS` | `120` | Максімальны час апрацоўкі запыту. 0 адключае тайм-аўт |
| `DRAIN_TIMEOUT_SECS` | `30` | Перыяд чакання для незавершаных злучэнняў падчас спынкі |
| `RATE_LIMIT` | `0` | Максімальная колькасць запытаў на IP за акно. 0 адключае абмежаванне |
| `RATE_WINDOW` | `60` | Акно абмежавання частаты запытаў у секундах |
| `TLS_CERT` | _(не зададзена)_ | Шлях да файла сертыфіката TLS у фармаце PEM |
| `TLS_KEY` | _(не зададзена)_ | Шлях да файла прыватнага ключа TLS у фармаце PEM |
| `ERROR_PAGES_DIR` | _(не зададзена)_ | Каталог з файламі старонак памылак `{status}.html` |
| `COMPRESSION` | `true` | Уключыць сціск Brotli. Усталюйце `false`, `0` або `off` для адключэння |

### Парты

| Порт | Прызначэнне |
|------|-------------|
| `8080` | Асноўны HTTP-сервер (або HTTPS, калі наладжаны TLS) |
| `9090` | Унутраны сервер: `/health`, `/metrics`, `/config` |

### Мантаванне тамоў

| Шлях на хасце | Шлях у кантэйнеры | Прызначэнне |
|---------------|-------------------|-------------|
| `./www` | `/var/www/html` | Файлы дадатку (PHP-скрыпты, статычныя рэсурсы). Мантуйце як `:ro` |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | Канфігурацыя PHP (OPcache, сесіі). Мантуйце як `:ro` |
| `./certs` | `/etc/ssl/oxphp` | Файлы сертыфіката і ключа TLS. Мантуйце як `:ro` |

## Карыстальнік www-data у Alpine

Рантайм-вобраз працуе ад імя `www-data` (UID 82, GID 82) для сумяшчальнасці з канвенцыямі nginx і Apache. Alpine 3.21 мае папярэдне створаную групу `www-data` з GID 82, але не ўключае карыстальніка, таму Dockerfile стварае яго:

```dockerfile
RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data 2>/dev/null || true
```

Калі ваш дадатак павінен пісаць у пэўныя каталогі (сесіі, кэш, загрузкі), пераканайцеся, што гэтыя каталогі даступныя для запісу карыстальніку з UID 82.

## Глядзіце таксама

- [Усталяванне](/getting-started/installation/) -- перадумовы зборкі і інструкцыі па зборцы з зыходнікаў
- [Хуткі старт](/getting-started/quick-start/) -- запусціце OxPHP менш чым за 5 хвілін
- [Канфігурацыя](/operations/configuration/) -- поўны даведнік па зменных асяроддзя
- [Плаўная спынка](/operations/graceful-shutdown/) -- паводзіны завяршэння і налады тайм-аўтаў
