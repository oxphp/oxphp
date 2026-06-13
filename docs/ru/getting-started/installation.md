---
title: Установка
description: Установка OxPHP через Docker-образ или сборка из исходного кода. Охватывает предварительные требования, проверку установки и особенности платформ.
---

# Установка

OxPHP распространяется в виде Docker-образа — это самый быстрый и рекомендуемый способ начать обслуживать PHP-приложения. Образ содержит серверный бинарный файл, PHP 8.4 или 8.5 ZTS, расширение OxPHP и все зависимости времени выполнения на базе Alpine Linux. По умолчанию теги `:0.8.0` и `:latest` поставляются с PHP 8.5; для PHP 8.4 используйте теги `:0.8.0-php8.4`, `:php8.4` или любой вариант `*-php8.4*`.

## Docker (рекомендуется)

Загрузите официальный образ из GitHub Container Registry:

```bash
docker pull ghcr.io/oxphp/oxphp:0.8.0
```

В образ входит:

- **Бинарный файл сервера OxPHP** — асинхронный HTTP-сервер
- **PHP ZTS** — 8.4 или 8.5 в зависимости от выбранного тега; потокобезопасная среда выполнения PHP для многоворкерного режима
- **PHP-расширение OxPHP** (`oxphp_sapi.so`) — предоставляет `oxphp_request_id()`, `oxphp_server_info()`, `oxphp_worker()` и другие встроенные функции
- **Библиотека-мост** (`liboxphp_bridge.so`) — связывает Rust-сервер со средой выполнения PHP
- **Alpine Linux** — минимальный базовый образ
- **Без директивы `USER`** — образ по умолчанию запускается от **root**, что соответствует поведению `nginx:alpine` / `php-fpm:alpine` / `frankenphp:alpine`. Пользователь `www-data` (UID 82, GID 82) предварительно создан, а директория `/var/www/html` принадлежит ему уже на этапе сборки; снижайте привилегии на уровне оркестратора при развёртывании:
  - `docker run --user www-data ghcr.io/oxphp/oxphp:0.8.0`
  - Compose: `services.app.user: www-data`
  - Kubernetes: `securityContext.runAsUser: 82`

### Структура образа

Файловая структура runtime-образа:

```
/usr/local/
├── bin/
│   └── oxphp                                        # серверный бинарный файл
├── lib/
│   ├── libphp.so                                    # PHP ZTS runtime (8.4 или 8.5, соответствует тегу образа)
│   ├── liboxphp_bridge.so                           # библиотека-мост C
│   └── php/extensions/no-debug-zts-<ABI>/
│       └── oxphp_sapi.so                            # PHP-расширение OxPHP
├── etc/php/
│   └── conf.d/
│       ├── custom.ini                                # настройки PHP для OxPHP
│       └── oxphp.ini                                # extension=oxphp_sapi.so
```

> **Значение `<ABI>` зависит от минора PHP.** PHP 8.4 использует `20240924`, у PHP 8.5 другая дата. В примерах ниже жёстко прописан `20240924`, потому что в их `FROM` указан `php:8.4-zts-alpine3.23` — поменяете FROM, придётся менять и дату. Чтобы получить её портабельно прямо в сборке:
>
> ```bash
> php -r 'echo ini_get("extension_dir");'
> # /usr/local/lib/php/extensions/no-debug-zts-20240924
> ```
>
> Используйте `$(php -r 'echo ini_get("extension_dir");')` в shell-командах, чтобы не хардкодить значение.

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

Базовый образ `php:8.4-zts-alpine3.23` (или `php:8.5-zts-alpine3.23`) уже содержит `libphp.so` и все его зависимости. Минор PHP в `FROM` должен совпадать с тегом OxPHP, из которого вы копируете артефакты. Достаточно скопировать три артефакта OxPHP:

```dockerfile
FROM php:8.4-zts-alpine3.23

COPY --from=ghcr.io/oxphp/oxphp:0.8.0 /usr/local/bin/oxphp /usr/local/bin/oxphp
COPY --from=ghcr.io/oxphp/oxphp:0.8.0 /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ghcr.io/oxphp/oxphp:0.8.0 /usr/local/lib/php/extensions/no-debug-zts-20240924/oxphp_sapi.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp.ini

COPY --chown=www-data:www-data . /var/www/html/public

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
FROM ghcr.io/oxphp/oxphp:0.8.0

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

COPY --chown=www-data:www-data . /var/www/html/public
```

Если приложению не нужны дополнительные расширения, достаточно:

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.8.0

COPY --chown=www-data:www-data . /var/www/html/public
```

Соберите и запустите:

```bash
docker build -t my-app .
docker run -p 80:80 my-app
```

По умолчанию сервер слушает порт `80`. Корень документов — `/var/www/html/public` — сниппеты выше копируют проект прямо в него. Для Laravel, Symfony и других фреймворков с собственным подкаталогом `public/` используйте `COPY --chown=www-data:www-data . /var/www/html`, чтобы `public/` фреймворка совпал с дефолтным корнем. Если структура отличается ещё сильнее, переопределите корень документов через переменную окружения `DOCUMENT_ROOT`.

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
- PHP 8.4 или 8.5 с включённым ZTS (Zend Thread Safety)
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

> **Примечание:** При развёртывании на Alpine Linux выполняйте сборку внутри того же образа `php:{8.4,8.5}-zts-alpine`, который используется для среды выполнения PHP — минор должен совпадать с тегом образа OxPHP. Смешивание сборок glibc и musl вызывает ошибки времени выполнения. Официальный Docker-образ обрабатывает это корректно.

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
