<p align="center">
  <img src="logo.svg" alt="OxPHP" width="300">
</p>

<h3 align="center">Многопоточный сервер PHP-приложений, созданный для облачной инфраструктуры.</h3>

<p align="center">
  OxPHP — асинхронный сервер PHP-приложений, написанный на Rust.<br>
  Создан для продакшн-нагрузок, требующих низкой задержки, высокой конкурентности и наблюдаемости без дополнительной настройки.
</p>

<p align="center">
  <a href="docs/ru/">Документация</a> · <a href="#быстрый-старт">Быстрый старт</a> · <a href="#почему-oxphp">Почему OxPHP</a> · <a href="#конфигурация">Конфигурация</a>
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

## Быстрый старт

Две строчки. Это всё.

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

> **Примечание:** По умолчанию `DOCUMENT_ROOT` равен `/var/www/html/public`. Размещайте точки входа (например, `index.php`) в подкаталоге `public/` — OxPHP раздаёт файлы именно оттуда, а не из корня `/var/www/html`. Это соответствует стандартной структуре фреймворков Laravel, Symfony и Slim.

```bash
docker build -t my-app . && docker run -p 8080:8080 my-app
curl http://localhost:8080/
```

Без конфигурации nginx. Без настройки пулов PHP-FPM. Без менеджера процессов. Просто ваше приложение.

---

## Почему OxPHP?

Традиционный PHP-стек — это три компонента, склеенных вместе: веб-сервер, менеджер процессов и среда выполнения PHP. Каждый добавляет поверхность конфигурации, режимы отказа и операционные издержки.

OxPHP объединяет все три в один бинарный файл на Rust со встроенным PHP.

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

## Бенчмарки

> Формальные бенчмарки скоро появятся. Мы работаем над воспроизводимым набором тестов, охватывающим req/s, задержки (p50/p99), использование памяти и пропускную способность воркеров под конкурентной нагрузкой.

---

## Возможности

### PHP-среда выполнения
- **Нативное выполнение PHP** через собственный SAPI (`oxphp`) с пулом ZTS-воркеров
- **Полная поддержка суперглобальных переменных**: `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `php://input`
- **Нативный мост Rust↔PHP** — без сериализации, через прямой доступ к `zval` посредством C-аксессоров
- **Система плагинов** с типизированной диспетчеризацией событий, приоритетной очерёдностью и регистрацией PHP-функций
- **Изоляция паник** через `catch_unwind` — сбой PHP не роняет весь сервер

### Модель воркеров
- **Режим воркера** — постоянные PHP-процессы с мягким сбросом, сохраняющие автозагрузчики и подключения к БД между запросами
- **Автоматическая рециклизация** по числу запросов или порогу памяти
- **Мониторинг здоровья воркеров** — упавшие воркеры автоматически обнаруживаются и перезапускаются
- **Ранний ответ** через `oxphp_finish_request()` — отправка ответа с продолжением фоновой обработки

### Асинхронные промисы
- **`oxphp_async()` / `oxphp_async_await()`** --- отправка замыканий в выделенный пул потоков для настоящего параллельного выполнения
- **Портативная сериализация** `use`-переменных, аргументов и возвращаемых значений --- безопасная бинарная передача между потоками
- Поддерживаемые типы: скаляры, строки, массивы (вложенные). Ресурсы и объекты отклоняются с `E_WARNING`
- **Безопасность исключений и die()** --- исключения, `die()` и `exit()` перехватываются и повторно выбрасываются как `OxPHP\AsyncException`
- **Поддержка таймаутов** --- таймауты для каждой задачи с `OxPHP\AsyncTimeoutException`
- **`oxphp_async_await_all()` / `oxphp_async_await_any()`** --- пакетные и гоночные примитивы

### HTTP и сетевое взаимодействие
- **HTTP/1.1 + HTTP/2** с автоопределением (h2c) через hyper
- **TLS 1.3** с ALPN (h2 + http/1.1) через rustls
- **3 режима маршрутизации** — Traditional, Framework (`index.php`), SPA (`index.html`)
- **Потоковая передача SSE** через автоопределение `Content-Type: text/event-stream` или `oxphp_stream_flush()`
- **Настраиваемые таймауты** — чтение заголовков, обработка запроса, keep-alive

### Производительность
- **LRU-кэш статических файлов** (в памяти для файлов ≤1 МБ, потоковая отдача для больших)
- **HTTP-кеширование** с ETag, Last-Modified и поддержкой 304 Not Modified
- **Сжатие Brotli** для текстовых ответов (диапазон 256 Б – 3 МБ)
- **Аллокатор mimalloc** для снижения задержки выделения памяти под нагрузкой
- **Настраиваемый Tokio runtime** — многопоточный по умолчанию (CPU/2), настраивается через `TOKIO_WORKERS`

### Надёжность и эксплуатация
- **Ограниченная очередь запросов** с противодавлением (503) при переполнении
- **Ограничение частоты запросов по IP** с заголовками `X-RateLimit-*` и ответами 429
- **Метрики Prometheus** на `/metrics` — по каждому воркеру, без зависимостей
- **Проверка работоспособности** на `/health` — готова для проб готовности K8s
- **Структурированное логирование ошибок** — ошибки PHP направляются через `tracing` с полями `php_error_type`, `php_file`, `php_line`
- **JSON-журнал доступа** (уровни: `all`, `error`, выключен через `ACCESS_LOG`)
- **Пользовательские страницы ошибок** — загружаются при старте, без I/O на горячем пути
- **Защита от path traversal** с обнаружением выхода за пределы через символические ссылки
- **Запуск в контейнере без прав root** от имени www-data (UID 82)
- **Генерация и проброс Request ID** (заголовок `X-Request-ID`)

---

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
                           │
                    ┌──────▼───────┐
                    │ Async pool   │  oxphp_async() / oxphp_async_await()
                    │(crossbeam ch)│  dedicated OS threads (ZTS)
                    └──────┬───────┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Async Worker  Async Worker  Async Worker
```

- **Асинхронный Tokio runtime** — многопоточный по умолчанию, настраивается через `TOKIO_WORKERS`
- **Пул ZTS-воркеров** — каждый воркер — отдельный поток ОС с изоляцией через `catch_unwind`
- Воркеры получают запросы через `crossbeam::bounded` и отвечают через `ExecuteResult` (немедленно или отложенно через `oneshot`)
- **Асинхронный пул** — отдельные потоки ОС для задач `oxphp_async()`, предотвращающие дедлоки с HTTP-пулом
- **Режим воркера** — постоянные PHP-процессы с мягким сбросом; сохраняют состояние начальной загрузки (автозагрузчики, подключения к БД) между запросами

### Внутренний сервер

Если задана переменная `INTERNAL_ADDR`, на отдельном порту запускается легковесный HTTP-сервер:

| Эндпоинт | Описание |
|----------|-------------|
| `GET /health` | Статус работоспособности в формате JSON (аптайм, запросы, соединения) |
| `GET /metrics` | Метрики в текстовом формате Prometheus |
| `GET /config` | Конфигурация runtime в формате JSON (пути к TLS-файлам скрыты) |

---

## Конфигурация

Все настройки задаются через переменные окружения — файлы конфигурации не требуются.

| Переменная | По умолчанию | Описание |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Адрес и порт для прослушивания |
| `DOCUMENT_ROOT` | `/var/www/html/public` | Путь в файловой системе для раздачи файлов |
| `INDEX_FILE` | *(не задано)* | Режим маршрутизации: пусто = Traditional, `index.php` = Framework, `index.html` = SPA |
| `TOKIO_WORKERS` | `0` (CPU / 2, мин. 1) | Потоки асинхронного I/O; `0` = авто |
| `EXECUTOR` | `sapi` | Исполнитель PHP: `sapi` (настоящий PHP) или `stub` (режим тестирования) |
| `PHP_WORKERS` | `0` (CPU / 2, мин. 1) | Пул воркеров: `N` = фиксированный, `MIN:MAX` = динамический, `0` = авто |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Таймаут простоя перед завершением динамического воркера |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Размер ограниченного канала; 503 при переполнении |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Таймаут ожидания завершения запросов при плавной остановке |
| `LOG_LEVEL` | `info` | Детализация логов: `error`, `warn`, `info`, `debug`, `trace` |
| `INTERNAL_ADDR` | *(не задано)* | Внутренний сервер для health/metrics/config (например `0.0.0.0:9090`) |
| `RATE_LIMIT` | `0` (выкл.) | Максимум запросов с одного IP за окно |
| `RATE_WINDOW_SECONDS` | `60` | Размер окна ограничения частоты запросов (секунды) |
| `HEADER_TIMEOUT_SECONDS` | `5` | Таймаут чтения заголовков (защита от Slowloris) |
| `REQUEST_TIMEOUT_SECONDS` | `120` | Общий таймаут запроса; 0 = отключён |
| `TLS_CERT` | *(не задано)* | Путь к PEM-файлу TLS-сертификата |
| `TLS_KEY` | *(не задано)* | Путь к PEM-файлу закрытого ключа TLS |
| `ERROR_PAGES_DIR` | *(не задано)* | Каталог с пользовательскими страницами ошибок (`{status}.html`) |
| `STATIC_CACHE_TTL` | `30d` | TTL кеша статических файлов (`30s`, `5m`, `2h`, `30d`, `1y`, `off`) |
| `COMPRESSION_LEVEL` | `4` | Уровень качества Brotli (0 = выкл., 1–11) |
| `ACCESS_LOG` | *(выкл.)* | JSON-журнал доступа: `all`, `error` или не задано |
| `MAX_CONNECTIONS` | `10000` | Максимальное количество одновременных соединений |
| `WORKER_FILE` | *(не задано)* | Путь к PHP-скрипту воркера; включает режим постоянных воркеров |
| `WORKER_MAX_REQUESTS` | `0` (без ограничений) | Макс. запросов на воркер до рециклизации |
| `WORKER_MAX_MEMORY_MIB` | `0` (без ограничений) | Макс. память (МиБ) на воркер до рециклизации |
| `ASYNC_WORKERS` | `0` (отключено) | Выделенные потоки асинхронных воркеров для `oxphp_async()` |
| `ASYNC_QUEUE_CAPACITY` | `ASYNC_WORKERS * 64` | Ограниченная очередь для асинхронных задач; отклоняются при заполнении |

---

## Сборка

```bash
# На хосте (без PHP — все тесты проходят, без выполнения PHP)
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
# Полная проверка (на хосте, 167 тестов)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Дымовой тест через Docker
docker compose build && docker compose up -d
curl http://localhost:8080/
curl "http://localhost:8080/test_superglobals.php?foo=bar"
curl -X POST -d "key=value" http://localhost:8080/test_superglobals.php
curl -H "Cookie: session=abc" http://localhost:8080/test_superglobals.php

# Асинхронные промисы
curl http://localhost:8080/test_async.php
curl http://localhost:8080/test_async_parallel.php
curl http://localhost:8080/test_async_die.php

# Внутренний сервер
INTERNAL_ADDR=127.0.0.1:9090 ./target/release/oxphp &
curl http://localhost:9090/health
curl http://localhost:9090/metrics
```

---

## Дорожная карта

> Элементы не упорядочены по приоритету. Наличие в этом списке не гарантирует реализацию.

| Feature | Описание |
|---|---|
| **PHP 8.5** | Поддержка PHP 8.5 сразу после выхода |
| **Trace Context (W3C)** | Автоматическая передача заголовков `traceparent` / `tracestate` между запросами |
| **OpenTelemetry** | Экспорт трейсов и метрик через OTLP в любой совместимый бэкенд |
| **Custom Metrics** | PHP API для регистрации пользовательских метрик Prometheus из кода приложения |
| **Built-in PHP Profiler** | Низконакладное профилирование без xdebug и внешних агентов, интегрированное прямо в сервер |
| **Dockerfile.bookworm** | Официальный образ на базе Debian Bookworm как альтернатива Alpine |
| **Non-Docker Install** | Нативная установка через системные пакетные менеджеры (apt, brew и т.д.) |
| **HTTP/3** | Поддержка HTTP/3 на базе QUIC |
| **HTTP 103 Early Hints** | Отправка ответов `103 Early Hints`, позволяющих клиентам предварительно загружать ресурсы до получения финального ответа |
| **Ecosystem Plugins** | Расширенная система плагинов: больше хуков жизненного цикла, более богатый PHP API и документация для сторонних авторов плагинов |
| **Shared Async Runtime** | Предоставление Tokio runtime PHP-воркерам для выполнения асинхронных операций из пользовательского кода |
| **Database Connection Pool** | Встроенный пул соединений через `sqlx`, снижающий накладные расходы на подключение при каждом запросе |
| **gRPC Server** | *(предварительно)* Альтернативный серверный режим — gRPC вместо HTTP; реализация не гарантирована |
| ~~**Promise API**~~ | Реализовано — `oxphp_async()` / `oxphp_async_await()` с выделенным пулом потоков, портативной сериализацией и безопасностью исключений |
| **Diagnostics** | Диагностика для продакшна: проверка лимитов ОС (ulimit, TCP backlog, epoll/kqueue, параметры контейнера), выявление узких мест производительности (глубина очереди воркеров, конкуренция за блокировки, нагрузка GC/аллокатора, статистика ZTS) и конкретные рекомендации по устранению |

## Документация

- [English](docs/en/)
- [中文](docs/zh/)
- [Русский](docs/ru/)
- [Беларуская](docs/be/)

## Лицензия

[AGPL-3.0](LICENSE)
