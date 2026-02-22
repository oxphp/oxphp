---
title: Установка
description: Как установить и запустить OxPHP
---

## Docker-образ (рекомендуется)

OxPHP распространяется в виде готового Docker-образа. Загрузите последнюю ночную сборку:

```bash
docker pull ghcr.io/oxphp/oxphp:nightly
```

Создайте `Dockerfile` в корне проекта:

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data . /var/www/html
```

Соберите образ и запустите контейнер:

```bash
docker build -t my-app .
docker run -p 8080:8080 my-app
```

Вот и всё. Образ включает бинарный файл Rust, среду выполнения PHP 8.4 ZTS, библиотеку-мост, PHP-расширение и все необходимые зависимости. Инструменты сборки не требуются.

## Требования

**Docker (рекомендуется):**

- Docker Engine 20.10+ или Docker Desktop

**Сборка из исходников (без PHP):**

- Инструментарий Rust 1.75+ (рекомендуется `rustup`)

**Сборка из исходников (с PHP):**

- Инструментарий Rust 1.75+
- PHP 8.4 с включённым ZTS (Zend Thread Safety)
- `libphp.so`, доступный в пути поиска библиотек
- C-компилятор (gcc или clang) для библиотеки-моста и PHP-расширения

## Сборка из исходников (Stub Executor)

Чтобы собрать OxPHP без поддержки PHP (только раздача статических файлов, удобно для разработки), используйте `--no-default-features` для отключения фичи `php`:

```bash
cargo build --release --no-default-features
```

Готовый бинарный файл находится по пути `target/release/oxphp`. Он использует stub executor, который возвращает заглушку в ответ на PHP-запросы.

**Примечание:** Фича `php` включена по умолчанию. Запуск `cargo build --release` без `--no-default-features` требует наличия `libphp.so` и библиотеки-моста на хосте.

## Сборка из исходников (с PHP)

Сборка с поддержкой PHP требует наличия `libphp.so` (ZTS-сборка) и библиотеки-моста, установленных на хосте:

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

Во время выполнения бинарному файлу нужны `libphp.so` и `liboxphp_bridge.so` в пути поиска библиотек:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

### Совместимость с Alpine

Если вы разворачиваете приложение на Alpine Linux, бинарный файл Rust необходимо собирать внутри того же образа `php:8.4-zts-alpine`, который используется для среды выполнения PHP. Сборка в отдельном образе или с другой libc (glibc против musl) приводит к повреждению TLS во время выполнения. Прилагаемый Dockerfile обрабатывает это корректно.

## Запуск тестов

Запустите набор тестов на хосте без PHP, отключив фичи по умолчанию:

```bash
# Все проверки (форматирование, линтинг, тесты)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Только юнит-тесты
cargo test --no-default-features --lib

# Все тесты (юнит + интеграционные)
cargo test --no-default-features

# С примером плагина
cargo clippy --no-default-features --features plugin-example -- -D warnings && cargo test --no-default-features --features plugin-example
```

## Проверка установки

После запуска OxPHP вы должны увидеть структурированный вывод в формате JSON:

```
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:8080",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:8080"}
```

Проверьте, что сервер отвечает:

```bash
curl http://localhost:8080/
```

Если вы настроили внутренний сервер, проверьте эндпоинт здоровья:

```bash
curl http://localhost:9090/health
```

## Смотрите также

- [Быстрый старт](quick-start.md) -- запустите OxPHP менее чем за 5 минут
- [Docker](docker.md) -- справочник по compose.yml, стадии Dockerfile и советы по развёртыванию
- [Конфигурация](../operations/configuration.md) -- полный список переменных окружения
