---
title: Docker
description: Этапы Dockerfile, описание docker-compose.yml и советы по развёртыванию
---

OxPHP поставляется с многоэтапным Dockerfile, который создаёт минимальный runtime-образ на базе Alpine. На этой странице описаны этапы сборки, конфигурация `docker-compose.yml` и типичные вопросы развёртывания.

## Этапы Dockerfile

Dockerfile состоит из четырёх этапов. Каждый этап собирает один компонент и передаёт артефакты далее.

### Этап 1: bridge-builder

```dockerfile
FROM alpine:3.21 AS bridge-builder
RUN apk add --no-cache gcc musl-dev make
COPY ext/bridge/ ./
RUN make && make install
```

Компилирует `liboxphp_bridge.so` -- небольшую C-библиотеку, предоставляющую `__thread` TLS-переменные, общие для Rust и PHP-расширения. Сборка происходит на чистом Alpine с gcc -- зависимость от PHP отсутствует.

**Артефакты:** `/usr/local/lib/liboxphp_bridge.so`, `/usr/local/include/oxphp_bridge.h`

### Этап 2: ext-builder

```dockerfile
FROM php:8.4-zts-alpine AS ext-builder
RUN apk add --no-cache gcc musl-dev make autoconf
COPY --from=bridge-builder /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=bridge-builder /usr/local/include/oxphp_bridge.h /usr/local/include/
COPY ext/config.m4 ext/php_oxphp_sapi.h ext/oxphp_sapi.c ./
COPY ext/bridge/oxphp_bridge.h ./bridge/
RUN phpize && ./configure --enable-oxphp-sapi && make && make install
```

Собирает PHP-расширение (`oxphp_sapi.so`) с помощью `phpize` из образа PHP 8.4 ZTS. Расширение линкуется с библиотекой-мостом и предоставляет PHP-коду функции, такие как `oxphp_request_id()` и `oxphp_server_info()`.

**Артефакты:** `.so`-файл PHP-расширения в `/usr/local/lib/php/extensions/`

### Этап 3: builder

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

Собирает бинарный файл Rust внутри того же образа `php:8.4-zts-alpine`. Это необходимо, потому что бинарный файл линкуется с `libphp.so` и `liboxphp_bridge.so` -- сборка в отдельном образе с другой версией musl вызывает повреждение TLS при выполнении.

На этом этапе используется приём кеширования зависимостей: сначала выполняется сборка с фиктивным `main.rs` для кеширования всех crate-зависимостей, затем удаляются только артефакты, специфичные для OxPHP (`target/release/oxphp`, `deps/oxphp-*`, `.fingerprint/oxphp-*`), после чего копируется реальный исходный код. Таким образом, при изменении исходного кода пересобирается только финальный бинарный файл.

Аргумент сборки `CARGO_FEATURES` позволяет включать опциональные фичи Cargo (например, `plugin-debug`) на этапе сборки без изменения Dockerfile.

**Артефакты:** `/build/target/release/oxphp`

### Этап 4: runtime

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

Финальный runtime-образ базируется на `alpine:3.21`. В него копируется только необходимое:

- `libphp.so` -- библиотека среды выполнения PHP
- `liboxphp_bridge.so` -- C-библиотека-мост
- Файлы PHP-расширения
- Бинарный файл `oxphp`
- Конфигурация PHP (`oxphp.ini`, загрузка расширения)
- Содержимое корня веб-документов по умолчанию (`/var/www/html/`)

Пользователь `www-data` (UID 82, GID 82) запускает серверный процесс. В Alpine 3.21 группа `www-data` уже создана, поэтому Dockerfile добавляет только пользователя.

`LD_LIBRARY_PATH=/usr/local/lib` устанавливается для того, чтобы динамический компоновщик мог найти `libphp.so` и `liboxphp_bridge.so` при выполнении.

## Описание docker-compose.yml

```yaml
services:
  oxphp:
    build:
      context: .
      args:
        # Дополнительные фичи Cargo (через пробел), например "plugin-debug"
        CARGO_FEATURES: ""
    ports:
      - "8080:8080"   # Основной HTTP-сервер
      - "9090:9090"   # Внутренний сервер (health/metrics/config)
    volumes:
      - ./www:/var/www/html:ro
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
      - ./certs:/etc/ssl/oxphp:ro
    environment:
      # Сервер
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html
      # - INDEX_FILE=index.php       # Включает режим маршрутизации Framework
      - EXECUTOR=sapi                # "sapi" или "stub"
      # - PHP_WORKERS=0              # Статический: 0 = CPU*2, или фиксированное N
      # - PHP_WORKERS=2:16           # Динамический: масштабирование от 2 до 16
      # - PHP_WORKERS_IDLE_SEC=30    # Тайм-аут простоя для динамического масштабирования
      # - QUEUE_CAPACITY=512         # По умолчанию: PHP_WORKERS * 128

      # Логирование
      - LOG_LEVEL=info

      # Внутренний сервер
      - INTERNAL_ADDR=0.0.0.0:9090

      # Тайм-ауты (в секундах)
      - HEADER_TIMEOUT_SECS=5
      - IDLE_TIMEOUT_SECS=60
      - REQUEST_TIMEOUT_SECS=120
      - DRAIN_TIMEOUT_SECS=30

      # Ограничение частоты запросов (0 = отключено)
      # - RATE_LIMIT=100
      # - RATE_WINDOW=60

      # TLS
      # - TLS_CERT=/etc/ssl/oxphp/server.pem
      # - TLS_KEY=/etc/ssl/oxphp/server.key

      # Страницы ошибок
      # - ERROR_PAGES_DIR=/var/www/errors

      # Сжатие (по умолчанию: true)
      # - COMPRESSION=true
    restart: unless-stopped
```

### Аргументы сборки

| Аргумент | По умолчанию | Описание |
|----------|--------------|----------|
| `CARGO_FEATURES` | `""` | Список дополнительных фич Cargo через пробел (например, `plugin-debug`) |

### Переменные окружения

| Переменная | По умолчанию | Описание |
|------------|--------------|----------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Адрес и порт основного HTTP-сервера |
| `DOCUMENT_ROOT` | `/var/www/html` | Корневой каталог для раздачи файлов |
| `INDEX_FILE` | _(не задано)_ | Установите `index.php` для режима Framework или `index.html` для режима SPA |
| `EXECUTOR` | `sapi` | Тип PHP-исполнителя: `sapi` (реальный PHP) или `stub` (заглушка) |
| `PHP_WORKERS` | `0` (CPU * 2, статический) | Режим пула воркеров. `N` = фиксированный пул, `MIN:MAX` = динамическое масштабирование |
| `PHP_WORKERS_IDLE_SEC` | `30` | Тайм-аут простоя перед завершением динамического воркера |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Размер ограниченной очереди запросов. При переполнении возвращается 503 |
| `LOG_LEVEL` | `info` | Уровень логирования: `trace`, `debug`, `info`, `warn`, `error` |
| `MAX_CONNECTIONS` | `10000` | Максимальное количество одновременных соединений |
| `INTERNAL_ADDR` | _(не задано)_ | Адрес внутреннего сервера. Если не задано, сервер отключён |
| `HEADER_TIMEOUT_SECS` | `5` | Тайм-аут чтения заголовков запроса |
| `IDLE_TIMEOUT_SECS` | `60` | Тайм-аут простоя Keep-Alive |
| `REQUEST_TIMEOUT_SECS` | `120` | Максимальное время обработки запроса. 0 отключает тайм-аут |
| `DRAIN_TIMEOUT_SECS` | `30` | Период ожидания для активных соединений при остановке |
| `RATE_LIMIT` | `0` | Максимум запросов с одного IP за окно. 0 отключает ограничение |
| `RATE_WINDOW` | `60` | Длительность окна ограничения частоты в секундах |
| `TLS_CERT` | _(не задано)_ | Путь к PEM-файлу сертификата TLS |
| `TLS_KEY` | _(не задано)_ | Путь к PEM-файлу приватного ключа TLS |
| `ERROR_PAGES_DIR` | _(не задано)_ | Каталог с HTML-файлами страниц ошибок вида `{status}.html` |
| `COMPRESSION` | `true` | Включить сжатие Brotli. Установите `false`, `0` или `off` для отключения |

### Порты

| Порт | Назначение |
|------|------------|
| `8080` | Основной HTTP-сервер (или HTTPS при настроенном TLS) |
| `9090` | Внутренний сервер: `/health`, `/metrics`, `/config` |

### Монтирование томов

| Путь на хосте | Путь в контейнере | Назначение |
|---------------|-------------------|------------|
| `./www` | `/var/www/html` | Файлы приложения (PHP-скрипты, статические ресурсы). Монтируйте как `:ro` |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | Конфигурация PHP (OPcache, сессии). Монтируйте как `:ro` |
| `./certs` | `/etc/ssl/oxphp` | Файлы сертификата и ключа TLS. Монтируйте как `:ro` |

## Пользователь www-data в Alpine

Runtime-образ работает от имени `www-data` (UID 82, GID 82) для совместимости с соглашениями nginx и Apache. В Alpine 3.21 группа `www-data` уже создана с GID 82, но пользователь отсутствует, поэтому Dockerfile создаёт его:

```dockerfile
RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data 2>/dev/null || true
```

Если вашему приложению необходимо записывать данные в определённые каталоги (сессии, кеш, загрузки), убедитесь, что эти каталоги доступны для записи пользователю с UID 82.

## Смотрите также

- [Установка](/getting-started/installation/) -- предварительные требования и инструкции по сборке из исходников
- [Быстрый старт](/getting-started/quick-start/) -- запуск OxPHP менее чем за 5 минут
- [Конфигурация](/operations/configuration/) -- полный справочник по переменным окружения
- [Плавная остановка](/operations/graceful-shutdown/) -- поведение при завершении и настройки тайм-аутов
