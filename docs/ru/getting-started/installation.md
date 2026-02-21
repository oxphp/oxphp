---
title: Установка
description: Как установить и собрать OxPHP
---

## Предварительные требования

**Docker (рекомендуется):**

- Docker Engine 20.10+ или Docker Desktop
- Docker Compose v2

**Сборка из исходников (без PHP):**

- Набор инструментов Rust 1.75+ (рекомендуется `rustup`)

**Сборка из исходников (с PHP):**

- Набор инструментов Rust 1.75+
- PHP 8.4 с включённым ZTS (Zend Thread Safety)
- `libphp.so`, доступная в путях поиска библиотек
- Компилятор C (gcc или clang) для сборки библиотеки-моста и PHP-расширения

## Сборка через Docker

Docker -- основной метод сборки. Он создаёт минимальный образ на базе Alpine с бинарным файлом Rust, средой выполнения PHP, библиотекой-мостом и предварительно настроенным PHP-расширением.

```bash
docker compose build
docker compose up -d
```

Многоэтапный Dockerfile обеспечивает полный конвейер сборки:

1. Компилирует C-библиотеку-мост (`liboxphp_bridge.so`)
2. Собирает PHP-расширение (`oxphp_sapi.so`) для PHP 8.4 ZTS
3. Собирает бинарный файл Rust внутри того же образа `php:8.4-zts-alpine`
4. Копирует только необходимые артефакты в минимальный образ Alpine

Чтобы включить опциональные возможности, например плагин примера, передайте `CARGO_FEATURES` как аргумент сборки:

```bash
docker compose build --build-arg CARGO_FEATURES="plugin-example"
```

См. [руководство по Docker](/getting-started/docker/) для подробного описания этапов Dockerfile и конфигурации `compose.yml`.

## Сборка из исходников (Stub Executor)

Чтобы собрать OxPHP без поддержки PHP (только раздача статических файлов, полезно для разработки), используйте `--no-default-features` для отключения фичи `php`:

```bash
cargo build --release --no-default-features
```

Результирующий бинарный файл находится по пути `target/release/oxphp`. Он использует stub executor, который возвращает заглушку для PHP-запросов.

**Примечание:** Фича `php` включена по умолчанию. Запуск `cargo build --release` без `--no-default-features` требует наличия `libphp.so` и библиотеки-моста на хосте.

## Сборка из исходников (с PHP)

Сборка с PHP требует установки `libphp.so` (сборка ZTS) и библиотеки-моста на хосте:

```bash
# Сборка и установка библиотеки-моста
cd ext/bridge
make && sudo make install

# Сборка и установка PHP-расширения
cd ext
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# Сборка OxPHP с поддержкой PHP (фичи по умолчанию включают php)
cargo build --release
```

При запуске бинарному файлу необходимы `libphp.so` и `liboxphp_bridge.so` в путях поиска библиотек:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

### Совместимость с Alpine

При развёртывании на Alpine Linux необходимо собирать бинарный файл Rust внутри того же образа `php:8.4-zts-alpine`, который используется для среды выполнения PHP. Сборка в отдельном образе или с другой libc (glibc vs musl) вызывает повреждение TLS при выполнении. Предоставленный Dockerfile обрабатывает это корректно.

## Запуск тестов

Запустите набор тестов на хосте без PHP, отключив фичи по умолчанию:

```bash
# Все проверки (форматирование, линтинг, тесты)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Только юнит-тесты
cargo test --no-default-features --lib

# Все тесты (юнит + интеграционные)
cargo test --no-default-features

# С плагином примера
cargo clippy --no-default-features --features plugin-example -- -D warnings && cargo test --no-default-features --features plugin-example
```

## Проверка установки

После запуска OxPHP вы должны увидеть структурированный JSON-вывод в логах:

```
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:8080",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:8080"}
```

Проверьте, что сервер отвечает:

```bash
curl http://localhost:8080/
```

Если вы настроили внутренний сервер, проверьте эндпоинт состояния:

```bash
curl http://localhost:9090/health
```

## Смотрите также

- [Быстрый старт](/getting-started/quick-start/) -- запуск OxPHP менее чем за 5 минут
- [Docker](/getting-started/docker/) -- этапы Dockerfile, описание compose.yml и советы по развёртыванию
- [Конфигурация](/operations/configuration/) -- полный список переменных окружения
