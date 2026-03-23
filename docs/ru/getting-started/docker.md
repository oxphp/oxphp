---
title: Руководство по Docker
description: Запуск OxPHP с Docker. Охватывает минимальные и многоэтапные Dockerfiles, конфигурацию Compose, монтирование PHP ini, проверки состояния и справочник портов.
---

# Руководство по Docker

OxPHP разработан для запуска в контейнере. Это руководство охватывает всё необходимое для сборки, конфигурации и эксплуатации OxPHP с Docker — от минимального одноэтапного образа до полноценного многоэтапного с отдельными целями для разработки и продакшена.

## Минимальный Dockerfile

Простейший способ контейнеризировать ваше приложение:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

Эта команда копирует ваше приложение в контейнер и обслуживает его из `/var/www/html/public`. По умолчанию сервер слушает порт `80`.

## Многоэтапный Dockerfile

Для реальных приложений используйте многоэтапный Dockerfile с отдельными целями `dev` и `prod`. Цель `dev` включает PHP CLI, Composer и Xdebug. Цель `prod` строится на минимальном образе OxPHP только с тем, что необходимо в продакшене.

```dockerfile
# ── Stage: php-base — shared PHP extensions ──────────────────
FROM php:8.4-zts-alpine3.23 AS php-base

RUN apk add --no-cache \
        icu-dev \
        icu-libs \
        postgresql-dev \
        libpq \
    && docker-php-ext-install \
        pdo \
        pdo_mysql \
        pdo_pgsql \
        intl \
    && apk del icu-dev postgresql-dev

# ── Stage: php-dev — add Xdebug on top of base ───────────────
FROM php-base AS php-dev

RUN apk add --no-cache $PHPIZE_DEPS linux-headers \
    && pecl install xdebug \
    && docker-php-ext-enable xdebug \
    && apk del $PHPIZE_DEPS linux-headers

# ── Stage: composer ───────────────────────────────────────────
FROM composer:2 AS composer

# ── Stage: oxphp — pull OxPHP artifacts ──────────────────────
FROM ghcr.io/oxphp/oxphp:0.1.0 AS oxphp

# ── Target: dev ──────────────────────────────────────────────
# Includes: PHP CLI, Composer, Xdebug, OxPHP binary + extension
FROM php-dev AS dev

RUN apk add --no-cache libgcc

# Composer
COPY --from=composer /usr/bin/composer /usr/local/bin/composer

# OxPHP binary
COPY --from=oxphp /usr/local/bin/oxphp /usr/local/bin/oxphp

# Bridge library
COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/

# OxPHP PHP extension
RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    echo "$EXT_DIR" > /tmp/ext_dir
COPY --from=oxphp /usr/local/lib/php/extensions/ /tmp/oxphp-ext/
RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(cat /tmp/ext_dir)/" && \
    rm -rf /tmp/oxphp-ext /tmp/ext_dir

# PHP config
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini

# Dev-friendly OPcache (validates timestamps)
RUN { \
        echo "[opcache]"; \
        echo "opcache.enable=1"; \
        echo "opcache.enable_cli=1"; \
        echo "opcache.validate_timestamps=1"; \
        echo "opcache.revalidate_freq=0"; \
    } > /usr/local/etc/php/conf.d/opcache-dev.ini

# Xdebug — connect back to host
RUN { \
        echo "[xdebug]"; \
        echo "xdebug.mode=debug"; \
        echo "xdebug.start_with_request=trigger"; \
        echo "xdebug.client_host=host.docker.internal"; \
        echo "xdebug.client_port=9003"; \
    } > /usr/local/etc/php/conf.d/xdebug-config.ini

RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data 2>/dev/null || true
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

ENV LD_LIBRARY_PATH=/usr/local/lib

COPY --chown=www-data:www-data . /var/www/html

EXPOSE 80 443

CMD ["oxphp"]

# ── Stage: prod-extensions — compile extensions for prod ─────
FROM php-base AS prod-extensions

RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    mkdir -p /ext-out && \
    cp "$EXT_DIR"/pdo.so \
       "$EXT_DIR"/pdo_mysql.so \
       "$EXT_DIR"/pdo_pgsql.so \
       "$EXT_DIR"/intl.so \
       /ext-out/

# ── Target: prod — minimal, based on OxPHP image ─────────────
FROM oxphp AS prod

USER root
RUN apk add --no-cache icu-libs libpq

COPY --from=prod-extensions /ext-out/*.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN { \
        echo "extension=pdo_mysql.so"; \
        echo "extension=pdo_pgsql.so"; \
        echo "extension=intl.so"; \
    } > /usr/local/etc/php/conf.d/app-extensions.ini

COPY --chown=www-data:www-data . /var/www/html

USER www-data

EXPOSE 80 443

CMD ["oxphp"]
```

Сборка каждой цели:

```bash
# Образ для разработки (включает PHP CLI, Composer, Xdebug)
docker build --target dev -t myapp:dev .

# Продакшен-образ (минимальный)
docker build --target prod -t myapp:prod .
```

> **Примечание:** Цель `dev` основана на `php:8.4-zts-alpine` с добавленным OxPHP, что даёт полный доступ к PHP CLI и Composer. Цель `prod` основана непосредственно на образе OxPHP, что делает продакшен-образ компактным.

## Docker Compose

### Продакшен

```yaml
services:
  oxphp:
    build:
      context: .
      target: prod
    ports:
      - "80:80"
      - "443:443"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - INDEX_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=info
      - ACCESS_LOG=error
      - PHP_WORKERS=4
      - REQUEST_TIMEOUT_SECONDS=120
      - DRAIN_TIMEOUT_SECONDS=30
      - COMPRESSION_LEVEL=4
    restart: unless-stopped
```

### Разработка

Монтируйте исходную директорию как том, чтобы изменения файлов отражались без пересборки образа. В цели `dev` включена проверка временных меток OPcache, поэтому PHP автоматически подхватывает изменения.

```yaml
services:
  oxphp:
    build:
      context: .
      target: dev
    ports:
      - "80:80"
      - "9090:9090"
    volumes:
      - ./src:/var/www/html:ro
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - INDEX_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=debug
      - ACCESS_LOG=all
```

## Монтирование томов

| Путь на хосте | Путь в контейнере | Назначение |
|---------------|-------------------|------------|
| `./src` | `/var/www/html` | Файлы приложения (PHP-скрипты, статические ресурсы). Используйте `:ro` в продакшене |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | Конфигурация времени выполнения PHP (OPcache, сессии, JIT). Используйте `:ro` |
| `./certs` | `/etc/ssl/oxphp` | TLS-сертификат и приватный ключ. Используйте `:ro` |

## Справочник портов

| Порт | Переменная окружения | Назначение |
|------|---------------------|------------|
| `80` | `LISTEN_ADDR` | Основной HTTP-сервер |
| `443` | `LISTEN_ADDR` | Основной HTTPS-сервер (при настроенном TLS) |
| `9090` | `INTERNAL_ADDR` | Внутренний сервер: `/health`, `/metrics`, `/config` |

> **Примечание:** Внутренний сервер отключён по умолчанию. Установите `INTERNAL_ADDR`, чтобы включить его. В продакшене держите внутренний порт доступным только для вашего оркестратора или системы мониторинга — не открывайте его публично.

## Конфигурация PHP

Настраивайте параметры PHP, создав файл `oxphp.ini` и монтируя его в контейнер. Это рекомендуемый способ конфигурации OPcache, JIT, сессий и других параметров времени выполнения PHP.

```ini
zend_extension=opcache

[opcache]
opcache.enable = 1
opcache.enable_cli = 1
opcache.memory_consumption = 128
opcache.interned_strings_buffer = 16
opcache.max_accelerated_files = 10000
opcache.validate_timestamps = 0
opcache.jit_buffer_size = 64M
opcache.jit = tracing

[Session]
session.save_path = /tmp
session.use_cookies = 1
session.use_only_cookies = 1
```

В процессе разработки установите `opcache.validate_timestamps = 1` и `opcache.revalidate_freq = 0`, чтобы PHP подхватывал изменения файлов без перезапуска контейнера.

См. раздел [OPcache](../php/opcache.md) для получения рекомендуемых настроек и конфигурации JIT.

## Проверки состояния

Добавьте проверку состояния Docker, чтобы Docker или ваш оркестратор могли контролировать работоспособность контейнера. Для этого необходимо задать `INTERNAL_ADDR`.

В `compose.yaml`:

```yaml
services:
  oxphp:
    environment:
      - INTERNAL_ADDR=0.0.0.0:9090
    healthcheck:
      test: ["CMD", "wget", "--quiet", "--tries=1", "--spider", "http://localhost:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

В `Dockerfile`:

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=5s \
    CMD wget --quiet --tries=1 --spider http://localhost:9090/health || exit 1
```

Эндпоинт `/health` возвращает `200`, когда сервер работает нормально, и `503` при деградации. JSON-ответ включает время работы, общее количество запросов и количество активных соединений. Для Kubernetes используйте тот же эндпоинт в качестве пробы жизнеспособности и готовности.

## Что дальше

- [Конфигурация](../operations/configuration.md) — полный справочник переменных окружения
- [Маршрутизация](../features/routing.md) — режимы маршрутизации: Traditional, Framework, SPA и Worker
- [Режим воркеров](../features/worker-mode.md) — постоянные PHP-процессы для приложений на фреймворках
- [TLS](../features/tls.md) — HTTPS со встроенной терминацией TLS
- [Проверки состояния](../operations/health-checks.md) — подробности об эндпоинте состояния и интеграция с Kubernetes
- [Плавное завершение](../operations/graceful-shutdown.md) — поведение при дрейне и последовательность завершения
