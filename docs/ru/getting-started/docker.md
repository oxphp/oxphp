---
title: Docker
description: Использование Docker-образа, справочник по compose.yml и советы по развёртыванию
---

OxPHP распространяется в виде готового Docker-образа по адресу `ghcr.io/oxphp/oxphp:nightly`. На этой странице описано, как использовать образ, настраивать его с помощью `compose.yml`, а также рассмотрены распространённые аспекты развёртывания.

## Использование образа

Самый простой способ запустить OxPHP — расширить базовый образ файлами вашего приложения:

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data ./src /var/www/html
```

Образ включает:

- Бинарный файл `oxphp`
- Среду выполнения PHP 8.4 ZTS (`libphp.so`)
- Библиотеку-мост (`liboxphp_bridge.so`)
- PHP-расширение (`oxphp_sapi.so`) с функциями `oxphp_request_id()`, `oxphp_server_info()` и другими
- Базовую систему Alpine Linux с минимальными зависимостями времени выполнения
- Пользователя `www-data` (UID 82, GID 82) для запуска без прав root

Корневая директория документов по умолчанию — `/var/www/html`. Сервер слушает на порту 8080. `CMD` — `["oxphp"]`.

## Справочник по compose.yml

```yaml
services:
  oxphp:
    build: .
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
      # - PHP_WORKERS_IDLE_SEC=30    # Таймаут простоя для динамического уменьшения
      # - QUEUE_CAPACITY=512         # По умолчанию: PHP_WORKERS * 128

      # Логирование
      - LOG_LEVEL=info

      # Внутренний сервер
      - INTERNAL_ADDR=0.0.0.0:9090

      # Таймауты (в секундах)
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

Для разработки можно монтировать директорию с исходниками как том вместо копирования файлов в образ:

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:nightly
    ports:
      - "8080:8080"
    volumes:
      - ./www:/var/www/html:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html
```

### Переменные окружения

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Адрес и порт основного HTTP-сервера |
| `DOCUMENT_ROOT` | `/var/www/html` | Корневая директория для раздачи файлов |
| `INDEX_FILE` | _(unset)_ | Укажите `index.php` для режима Framework или `index.html` для SPA |
| `EXECUTOR` | `sapi` | Тип PHP-исполнителя: `sapi` (настоящий PHP) или `stub` (заглушка) |
| `PHP_WORKERS` | `0` (CPU * 2, static) | Режим пула воркеров. `N` = фиксированный пул, `MIN:MAX` = динамическое масштабирование |
| `PHP_WORKERS_IDLE_SEC` | `30` | Таймаут простоя до завершения динамического воркера |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Размер ограниченной очереди запросов. При переполнении возвращается 503 |
| `LOG_LEVEL` | `info` | Уровень логирования: `trace`, `debug`, `info`, `warn`, `error` |
| `MAX_CONNECTIONS` | `10000` | Максимальное число одновременных соединений |
| `INTERNAL_ADDR` | _(unset)_ | Адрес внутреннего сервера. Если не задан — сервер отключён |
| `HEADER_TIMEOUT_SECS` | `5` | Таймаут чтения заголовков запроса |
| `IDLE_TIMEOUT_SECS` | `60` | Таймаут простоя keep-alive |
| `REQUEST_TIMEOUT_SECS` | `120` | Максимальное время обработки запроса. 0 отключает таймаут |
| `DRAIN_TIMEOUT_SECS` | `30` | Период ожидания активных соединений при завершении работы |
| `RATE_LIMIT` | `0` | Максимум запросов с одного IP за окно. 0 отключает ограничение |
| `RATE_WINDOW` | `60` | Окно ограничения частоты запросов в секундах |
| `TLS_CERT` | _(unset)_ | Путь к PEM-файлу TLS-сертификата |
| `TLS_KEY` | _(unset)_ | Путь к PEM-файлу приватного TLS-ключа |
| `ERROR_PAGES_DIR` | _(unset)_ | Директория с файлами страниц ошибок `{status}.html` |
| `COMPRESSION` | `true` | Включить сжатие Brotli. Установите `false`, `0` или `off` для отключения |
| `TOKIO_WORKERS` | `0` | Потоки асинхронного рантайма Tokio (0 = однопоточный) |
| `ACCESS_LOG` | `true` | Включить JSON-журнал доступа для каждого запроса |
| `SLOT_POOL_SIZE` | `QUEUE_CAPACITY + PHP_WORKERS*2` | Размер пула предварительно выделенных слотов ответа |

### Порты

| Port | Purpose |
|------|---------|
| `8080` | Основной HTTP-сервер (или HTTPS при настроенном TLS) |
| `9090` | Внутренний сервер: `/health`, `/metrics`, `/config` |

### Монтирование томов

| Host Path | Container Path | Purpose |
|-----------|---------------|---------|
| `./www` | `/var/www/html` | Файлы приложения (PHP-скрипты, статические ресурсы). Монтируйте как `:ro` |
| `./oxphp.ini` | `/usr/local/etc/php/conf.d/oxphp.ini` | Конфигурация PHP (OPcache, сессии). Монтируйте как `:ro` |
| `./certs` | `/etc/ssl/oxphp` | Файлы TLS-сертификата и ключа. Монтируйте как `:ro` |

## Конфигурация PHP

Чтобы настроить параметры PHP (OPcache, JIT, сессии и т.д.), создайте файл `oxphp.ini` и смонтируйте его в контейнер:

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

Рекомендуемые настройки см. в разделе [OPcache](../php/opcache.md).

## Пользователь www-data в Alpine

Образ запускается от имени `www-data` (UID 82, GID 82) для совместимости с соглашениями nginx и Apache. Если вашему приложению нужно писать в определённые директории (сессии, кэш, загрузки), убедитесь, что эти директории доступны для записи пользователю с UID 82.

## Сборка из исходников

Если вам нужно собрать OxPHP из исходников (например, чтобы включить пользовательские Cargo-фичи или изменить сервер), обратитесь к руководству по [Установке](installation.md) для получения инструкций по сборке из исходников. Репозиторий OxPHP включает многостадийный Dockerfile, который компилирует библиотеку-мост, PHP-расширение и бинарный файл Rust из исходников.

## Смотрите также

- [Установка](installation.md) -- требования и инструкции по сборке из исходников
- [Быстрый старт](quick-start.md) -- запустите OxPHP менее чем за 5 минут
- [Конфигурация](../operations/configuration.md) -- полный справочник переменных окружения
- [Плавное завершение работы](../operations/graceful-shutdown.md) -- поведение drain и настройки таймаутов
