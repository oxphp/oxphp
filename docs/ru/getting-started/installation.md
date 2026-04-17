---
title: Установка
description: Установка OxPHP через Docker-образ или сборка из исходного кода. Охватывает предварительные требования, проверку установки и особенности платформ.
---

# Установка

OxPHP распространяется в виде Docker-образа — это самый быстрый и рекомендуемый способ начать обслуживать PHP-приложения. Образ содержит серверный бинарный файл, PHP 8.4 ZTS, расширение OxPHP и все зависимости времени выполнения на базе Alpine Linux.

## Docker (рекомендуется)

Загрузите официальный образ из GitHub Container Registry:

```bash
docker pull ghcr.io/oxphp/oxphp:0.2.0
```

В образ входит:

- **Бинарный файл сервера OxPHP** — асинхронный HTTP-сервер
- **PHP 8.4 ZTS** — потокобезопасная среда выполнения PHP для многоворкерного режима
- **PHP-расширение OxPHP** (`oxphp_sapi.so`) — предоставляет `oxphp_request_id()`, `oxphp_server_info()`, `oxphp_worker()` и другие встроенные функции
- **Библиотека-мост** (`liboxphp_bridge.so`) — связывает Rust-сервер со средой выполнения PHP
- **Alpine Linux** — минимальный базовый образ
- Запускается от имени **www-data** (UID 82, GID 82) для выполнения контейнера без root-прав

### Структура образа

Файловая структура runtime-образа:

```
/usr/local/
├── bin/
│   └── oxphp                                        # серверный бинарный файл
├── lib/
│   ├── libphp.so                                    # PHP 8.4 ZTS runtime
│   ├── liboxphp_bridge.so                           # библиотека-мост C
│   └── php/extensions/no-debug-zts-20240924/
│       └── oxphp_sapi.so                            # PHP-расширение OxPHP
├── etc/php/
│   └── conf.d/
│       ├── oxphp.ini                                # настройки PHP для OxPHP
│       └── extension.ini                            # extension=oxphp_sapi.so
```

Три компонента OxPHP и их назначение:

| Компонент | Размер | Назначение |
|-----------|--------|------------|
| `oxphp` | ~8 МБ | HTTP-сервер, маршрутизация, плагины, метрики |
| `liboxphp_bridge.so` | ~50 КБ | Разделяемая библиотека-мост, связывающая сервер со средой выполнения PHP |
| `oxphp_sapi.so` | ~200 КБ | PHP-функции (`oxphp_request_id()`, `OxPHP\Http\Request` и др.) |

Цепочка зависимостей:

```
oxphp ──► libphp.so ──► libxml2, libcurl, libsqlite3, libonig, ...
  │
  └──► liboxphp_bridge.so ◄── oxphp_sapi.so
```

Бинарник `oxphp` линкуется к `libphp.so` и `liboxphp_bridge.so`. PHP-расширение `oxphp_sapi.so` также линкуется к bridge-библиотеке, благодаря чему per-request состояние доступно в вашем PHP-коде.

### Минимальный Dockerfile

Базовый образ `php:8.4-zts-alpine3.23` уже содержит `libphp.so` и все его зависимости. Достаточно скопировать три артефакта OxPHP:

```dockerfile
FROM php:8.4-zts-alpine3.23

COPY --from=ghcr.io/oxphp/oxphp:0.2.0 /usr/local/bin/oxphp /usr/local/bin/oxphp
COPY --from=ghcr.io/oxphp/oxphp:0.2.0 /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ghcr.io/oxphp/oxphp:0.2.0 /usr/local/lib/php/extensions/no-debug-zts-20240924/oxphp_sapi.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp.ini

COPY --chown=www-data:www-data . /var/www/html

EXPOSE 80 443 9090

CMD ["oxphp"]
```

Этот подход удобен для разработки — доступен PHP CLI, `composer`, `docker-php-ext-install`, `xdebug`. Подробнее — в [руководстве по Docker](docker.md).

### Продакшен Dockerfile

Официальный образ OxPHP минимален — в нём нет PHP CLI и инструментов сборки расширений. Если приложению нужны дополнительные расширения (pdo_mysql, intl и т.д.), соберите их в отдельном стейдже и скопируйте в финальный образ:

```dockerfile
# Стейдж сборки расширений
FROM php:8.4-zts-alpine3.23 AS extensions

RUN apk add --no-cache icu-dev postgresql-dev \
    && docker-php-ext-install pdo pdo_mysql pdo_pgsql intl

# Продакшен
FROM ghcr.io/oxphp/oxphp:0.2.0

# Runtime-зависимости расширений
USER root
RUN apk add --no-cache icu-libs libpq

# Скопировать скомпилированные расширения
COPY --from=extensions /usr/local/lib/php/extensions/no-debug-zts-20240924/*.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

# Подключить расширения
RUN { \
        echo "extension=pdo.so"; \
        echo "extension=pdo_mysql.so"; \
        echo "extension=pdo_pgsql.so"; \
        echo "extension=intl.so"; \
    } > /usr/local/etc/php/conf.d/app-extensions.ini

USER www-data

COPY --chown=www-data:www-data . /var/www/html
```

Если приложению не нужны дополнительные расширения, достаточно:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.2.0

COPY --chown=www-data:www-data . /var/www/html
```

Соберите и запустите:

```bash
docker build -t my-app .
docker run -p 80:80 my-app
```

По умолчанию сервер слушает порт `80`. Корень документов — `/var/www/html/public`. Проекты на Laravel и Symfony уже содержат директорию `public/`, поэтому достаточно скопировать проект в `/var/www/html/`. Если структура проекта отличается, переопределите корень документов через переменную окружения `DOCUMENT_ROOT`.

## Сборка из исходного кода (без PHP)

Соберите OxPHP из исходного кода с отключённой функцией PHP для отдачи только статических файлов:

```bash
cargo build --release --no-default-features
```

Бинарный файл находится по пути `target/release/oxphp`. Он использует заглушку-исполнитель, которая возвращает placeholder-ответ на PHP-запросы, при этом нормально отдавая статические файлы. Этот режим полезен для тестирования сервера без среды выполнения PHP.

## Сборка из исходного кода (с PHP)

Сборка OxPHP с полной поддержкой PHP требует предварительной компиляции и установки библиотеки-моста и PHP-расширения.

### Предварительные требования

- Инструментарий Rust (версия 1.91.1 или новее)
- PHP 8.4 с включённым ZTS (Zend Thread Safety)
- Компилятор C (gcc или clang)
- `phpize` и заголовочные файлы для разработки PHP

### Шаги сборки

```bash
# 1. Собрать и установить библиотеку-мост
cd ext/bridge
make && sudo make install

# 2. Собрать и установить PHP-расширение
cd ../
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# 3. Собрать OxPHP (по умолчанию включает php)
cargo build --release
```

Для работы бинарного файла разделяемые библиотеки должны быть доступны в пути поиска библиотек:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

> **Примечание:** При развёртывании на Alpine Linux выполняйте сборку внутри того же образа `php:8.4-zts-alpine`, который используется для среды выполнения PHP. Смешивание сборок glibc и musl вызывает ошибки времени выполнения. Официальный Docker-образ обрабатывает это корректно.

## Проверка установки

После запуска OxPHP структурированный JSON-вывод в логах подтверждает, что сервер работает:

```text
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:80",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:80"}
```

Проверьте, что сервер отвечает:

```bash
curl http://localhost/
```

Если вы включили внутренний сервер с помощью `INTERNAL_ADDR`, проверьте эндпоинт состояния:

```bash
curl http://localhost:9090/health
```

Исправно работающий сервер возвращает `200` с JSON-статусом. Деградировавший сервер возвращает `503`.

## Что дальше

- [Быстрый старт](quick-start.md) — создайте проект, запустите OxPHP с Docker Compose и выполните первый запрос
- [Руководство по Docker](docker.md) — Dockerfiles для разработки и продакшена, конфигурация Compose и монтирование томов
- [Конфигурация](../operations/configuration.md) — полный справочник переменных окружения
