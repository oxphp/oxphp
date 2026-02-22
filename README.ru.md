# OxPHP

Асинхронный сервер PHP-приложений, написанный на Rust. Заменяет связку nginx + PHP-FPM единым бинарным файлом, который обрабатывает HTTP, выполняет PHP нативно через собственный SAPI и предоставляет встроенные средства наблюдаемости.

## Возможности

- **Нативное выполнение PHP** через собственный SAPI (`oxphp`) с пулом ZTS-воркеров
- **Полная поддержка суперглобальных переменных**: `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `php://input`
- **Нативный мост Rust↔PHP** — без сериализации, через прямой доступ к `zval` посредством C-аксессоров
- **Система плагинов** с типизированной диспетчеризацией событий, приоритетной очерёдностью и регистрацией PHP-функций
- **Структурированное логирование ошибок** — ошибки PHP направляются через `tracing` с полями `php_error_type`, `php_file`, `php_line`
- **HTTP/1.1 + HTTP/2** с автоопределением (h2c) через hyper
- **TLS 1.3** с ALPN (h2 + http/1.1) через rustls
- **3 режима маршрутизации** — Traditional, Framework (`index.php`), SPA (`index.html`)
- **LRU-кэш статических файлов** (в памяти для файлов ≤1 МБ, потоковая отдача для больших)
- **Сжатие Brotli** для текстовых ответов (диапазон 256 Б – 3 МБ)
- **Ограниченная очередь запросов** с противодавлением (503) при переполнении
- **Ограничение частоты запросов по IP** с заголовками `X-RateLimit-*` и ответами 429
- **Настраиваемые таймауты** — чтение заголовков, обработка запроса, keep-alive
- **Метрики Prometheus** на внутреннем сервере по адресу `/metrics`
- **Эндпоинт проверки работоспособности** `/health` для проб готовности Kubernetes
- **Генерация и проброс Request ID** (заголовок `X-Request-ID`)
- **Журнал доступа** в виде структурированного JSON через tracing (включается/выключается через `ACCESS_LOG`)
- **Пользовательские страницы ошибок** — загружаются при старте, без I/O на горячем пути
- **Структурированное JSON-логирование** через tracing
- **Защита от path traversal** с обнаружением выхода за пределы через символические ссылки
- **Запуск в контейнере без прав root** от имени www-data (UID 82)
- **Аллокатор mimalloc** для снижения задержки выделения памяти под нагрузкой
- **Настраиваемый Tokio runtime** — однопоточный (по умолчанию) или многопоточный через `TOKIO_WORKERS`
- **Мониторинг здоровья воркеров** с автоматическим перезапуском упавших воркеров
- **Потоковая передача SSE** — Server-Sent Events в реальном времени через автоопределение `Content-Type: text/event-stream` или `oxphp_stream_flush()`
- **Ранний ответ** через `oxphp_finish_request()` — немедленная отправка ответа с продолжением фоновой обработки
- **Изоляция паник** через `catch_unwind` — сбой PHP не роняет весь сервер

## Быстрый старт

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data . /var/www/html
```

```bash
docker build -t my-app . && docker run -p 8080:8080 my-app
curl http://localhost:8080/
```

## Конфигурация

Все настройки задаются через переменные окружения:

| Переменная | По умолчанию | Описание |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Адрес и порт для прослушивания |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Путь в файловой системе для раздачи файлов |
| `INDEX_FILE` | *(не задано)* | Режим маршрутизации: пусто = Traditional, `index.php` = Framework, `index.html` = SPA |
| `TOKIO_WORKERS` | `0` (однопоточный) | Потоки асинхронного I/O Tokio; `0` = однопоточный, `N` = многопоточный |
| `EXECUTOR` | `sapi` | Исполнитель PHP: `sapi` (настоящий PHP) или `stub` (режим тестирования) |
| `PHP_WORKERS` | `0` (CPU * 2) | Режим пула воркеров: `N` = фиксированный пул, `MIN:MAX` = динамическое масштабирование, `0` = авто |
| `PHP_WORKERS_IDLE_SEC` | `30` | Таймаут простоя перед завершением динамического воркера (только в динамическом режиме) |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Размер ограниченного канала; 503 при переполнении |
| `DRAIN_TIMEOUT_SECS` | `30` | Таймаут ожидания завершения запросов при плавной остановке (секунды) |
| `LOG_LEVEL` | `info` | Детализация логов: `error`, `warn`, `info`, `debug`, `trace` |
| `INTERNAL_ADDR` | *(не задано)* | Адрес внутреннего сервера для health/metrics/config (например `0.0.0.0:9090`) |
| `RATE_LIMIT` | `0` (выкл) | Максимум запросов с одного IP за окно |
| `RATE_WINDOW` | `60` | Размер окна ограничения частоты запросов (секунды) |
| `HEADER_TIMEOUT_SECS` | `5` | Таймаут чтения заголовков (защита от Slowloris) |
| `IDLE_TIMEOUT_SECS` | `60` | Таймаут простоя keep-alive соединения |
| `REQUEST_TIMEOUT_SECS` | `120` | Общий таймаут запроса; 0 = отключён |
| `TLS_CERT` | *(не задано)* | Путь к PEM-файлу TLS-сертификата |
| `TLS_KEY` | *(не задано)* | Путь к PEM-файлу закрытого ключа TLS |
| `ERROR_PAGES_DIR` | *(не задано)* | Каталог с пользовательскими страницами ошибок (`{status}.html`) |
| `COMPRESSION` | `true` | Включить сжатие Brotli; отключить значениями `false`, `0` или `off` |
| `ACCESS_LOG` | `true` | Включить JSON-журнал доступа для каждого запроса; отключить значениями `false`, `0` или `off` |
| `MAX_CONNECTIONS` | `10000` | Максимальное количество одновременных соединений |

## Архитектура

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

- **Настраиваемый Tokio runtime** — однопоточный по умолчанию (`TOKIO_WORKERS=0`), многопоточный для высоконагруженных сценариев
- **Многопоточный пул PHP-воркеров** на основе PHP ZTS: каждый воркер — отдельный поток ОС с изоляцией через `catch_unwind`
- Воркеры получают запросы через `crossbeam::bounded` и отвечают через `ExecuteResult` (немедленно или отложенно через `oneshot`)
- **Мониторинг здоровья воркеров** — упавшие воркеры автоматически обнаруживаются и перезапускаются

### Внутренний сервер

Если задана переменная `INTERNAL_ADDR`, на отдельном порту запускается легковесный HTTP-сервер:

| Эндпоинт | Описание |
|----------|-------------|
| `GET /health` | Статус работоспособности в формате JSON (аптайм, запросы, соединения) |
| `GET /metrics` | Метрики в текстовом формате Prometheus |
| `GET /config` | Конфигурация runtime в формате JSON (пути к TLS-файлам скрыты) |

## Сборка

```bash
# На хосте (без PHP — выполняются все тесты, без запуска PHP)
cargo build --release

# Через Docker (с PHP — полная функциональность)
docker compose build
```

### Локальный запуск (только статические файлы)

```bash
DOCUMENT_ROOT=./www/public ./target/release/oxphp
```

## Разработка

```bash
# Полная проверка (на хосте, 157 тестов)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Дымовой тест через Docker
docker compose build && docker compose up -d
curl http://localhost:8080/
curl "http://localhost:8080/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost:8080/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost:8080/test_superglobals.php

# Внутренний сервер
INTERNAL_ADDR=127.0.0.1:9090 ./target/release/oxphp &
curl http://localhost:9090/health
curl http://localhost:9090/metrics
```

## Документация

- [English](docs/en/)
- [中文](docs/zh/)
- [Русский](docs/ru/)
- [Беларуская](docs/be/)

## Лицензия

[AGPL-3.0](LICENSE)
