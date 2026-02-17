---
title: SAPI и мост
description: Пользовательский PHP SAPI в OxPHP, библиотека C-моста с __thread TLS и API PHP-расширения
---

OxPHP использует пользовательский SAPI (Server API) для интеграции с PHP вместо стандартного `php-embed` SAPI. Разделяемая библиотека C-моста обеспечивает механизм обмена состоянием запроса между бинарным файлом Rust и PHP-расширением. На этой странице объясняется, зачем нужна такая архитектура и как компоненты взаимодействуют.

## Зачем нужен пользовательский SAPI?

Слой SAPI в PHP — это интерфейс между веб-сервером и движком PHP. Стандартные SAPI (cli, fpm, embed) делают допущения о жизненном цикле процесса, которые не соответствуют модели OxPHP:

- **php-embed** рассчитан на один запрос на процесс. Он не поддерживает параллельную обработку запросов в нескольких потоках.
- **php-fpm** — это отдельный менеджер процессов. OxPHP устраняет необходимость в межпроцессном взаимодействии.
- **php-cli** не имеет HTTP-интеграции.

OxPHP регистрирует собственную `sapi_module_struct` с именем `"oxphp"`. Это даёт полный контроль над:

- Захватом вывода (перехват буфера вывода PHP)
- Обработкой заголовков (сбор вызовов `header()`)
- `php://input` (предоставление тела запроса)
- Заполнением `$_SERVER` (установка суперглобальных переменных из данных запроса на стороне Rust)
- Временем запроса (через `sapi_get_request_time`)

## Проблема моста

Когда бинарный файл Rust OxPHP компилируется, он линкуется с `libphp.so`. Расширения PHP загружаются `libphp.so` во время выполнения через `dlopen()`. Это создаёт проблему видимости:

```
┌────────────────────┐         ┌───────────────────┐
│  Rust Binary       │         │  libphp.so        │
│                    │ links   │                   │
│  thread_local! {   │────────▶│  dlopen() ───────▶│ oxphp_sapi.so
│    // Rust TLS     │         │                   │  (PHP extension)
│  }                 │         └───────────────────┘
└────────────────────┘                             │
                                                   │
  Rust thread_local! vars are INVISIBLE            │
  to dlopen'd shared libraries ──────────────────▶ │
```

Макрос `thread_local!` в Rust использует ELF TLS или платформозависимый механизм, который разрешается во время линковки. Разделяемые библиотеки, загруженные через `dlopen()` во время выполнения, не могут видеть эти символы. Это означает, что PHP-расширение не может напрямую читать данные запроса, которые Rust хранит в потоколокальном хранилище.

## Библиотека моста

Решение — `liboxphp_bridge.so` — небольшая разделяемая библиотека на C, с которой линкуются и бинарный файл Rust, и PHP-расширение. Она использует C `__thread` TLS, который виден всем библиотекам, загруженным через `dlopen`, в одном адресном пространстве.

```
┌────────────────────┐
│  Rust Binary       │──links──┐
└────────────────────┘         │
                               ▼
                    ┌──────────────────────┐
                    │  liboxphp_bridge.so  │
                    │                      │
                    │  static __thread     │
                    │    oxphp_ctx_t ctx;  │
                    │                      │
                    │  static (global)     │
                    │    plugin_functions  │
                    │    dispatch_fn       │
                    │    call_php_fn       │
                    └──────────────────────┘
                               ▲
┌────────────────────┐         │
│  oxphp_sapi.so     │──links──┘
│  (PHP extension)   │
└────────────────────┘
```

И бинарный файл Rust, и PHP-расширение вызывают функции в `liboxphp_bridge.so` для чтения и записи одной и той же переменной `__thread`. Поскольку они находятся в одном процессе и в одном потоке ОС, они разделяют один и тот же слот TLS.

### Контекст моста

Контекст запроса определён в `ext/bridge/oxphp_bridge.h`:

```c
typedef struct {
    char request_id[65];    // Hex request ID (64 chars + null)
    int32_t worker_id;      // Worker thread index
    double request_time;    // Unix epoch, microseconds
    bool stream_mode;       // Streaming mode active
    bool headers_sent;      // Headers sent (streaming)
    bool finished;          // oxphp_finish_request() called
} oxphp_ctx_t;
```

### API моста

Мост предоставляет функции getter/setter, работающие с потоколокальной переменной `__thread` `ctx`:

| Функция | Назначение |
|---|---|
| `oxphp_bridge_init_ctx()` | Обнуление контекста (вызывать перед `php_request_startup`) |
| `oxphp_bridge_clear_ctx()` | Обнуление контекста после завершения запроса |
| `oxphp_bridge_get_ctx()` | Получение указателя на структуру контекста |
| `oxphp_bridge_set_request_id(id)` | Копирование идентификатора запроса (до 64 символов) |
| `oxphp_bridge_get_request_id()` | Получение указателя на идентификатор запроса |
| `oxphp_bridge_set_worker_id(id)` | Установка индекса потока воркера |
| `oxphp_bridge_set_request_time(time)` | Установка времени начала запроса |
| `oxphp_bridge_get_request_time()` | Получение времени начала запроса |
| `oxphp_bridge_set_stream_mode(mode)` | Включение/выключение потокового режима |
| `oxphp_bridge_is_streaming()` | Проверка, активен ли потоковый режим |
| `oxphp_bridge_set_finished(bool)` | Отметка запроса как завершённого |
| `oxphp_bridge_is_finished()` | Проверка, завершён ли запрос |
| `oxphp_bridge_set_headers_sent(bool)` | Отметка заголовков как отправленных |
| `oxphp_bridge_get_headers_sent()` | Проверка, были ли отправлены заголовки |

Реализация в `ext/bridge/oxphp_bridge.c` проста — каждая функция читает или записывает поле переменной `static __thread oxphp_ctx_t ctx`.

### Критический инвариант

**`init_ctx()` и `set_request_time()` должны быть вызваны ДО `php_request_startup()`.**

Обработчик RINIT в OPcache читает `sapi_get_request_time()` во время `php_request_startup()`. Callback `sapi_get_request_time` пользовательского SAPI читает из контекста моста. Если мост возвращает 0 (неинициализированное значение), проверка `file_update_protection` в OPcache завершается неудачей, что приводит к 0% попаданий в кэш.

Правильный порядок вызовов в каждом потоке воркера:

```
1. oxphp_bridge_init_ctx()
2. oxphp_bridge_set_request_id(...)
3. oxphp_bridge_set_request_time(...)
4. sapi::set_request_data(request)    // server vars, cookies, body
5. php_request_startup()              // triggers RINIT for all extensions
6. php_execute_script(...)
7. php_request_shutdown()
8. oxphp_bridge_clear_ctx()
```

## Реестр функций плагинов

Мост также предоставляет **глобальный** (не `__thread`) реестр функций плагинов. Это позволяет плагинам Rust регистрировать функции, которые могут вызывать PHP-скрипты, а также PHP-функции, которые может вызывать Rust.

### API реестра

| Функция | Назначение |
|---|---|
| `oxphp_bridge_register_plugin_fn(name, required, total)` | Регистрация функции плагина (вызывается Rust при запуске) |
| `oxphp_bridge_get_plugin_fn_count()` | Получение количества зарегистрированных функций плагинов |
| `oxphp_bridge_get_plugin_fn_name(index)` | Получение имени функции плагина по индексу |
| `oxphp_bridge_get_plugin_fn_required(index)` | Получение количества обязательных параметров по индексу |
| `oxphp_bridge_get_plugin_fn_total(index)` | Получение общего количества параметров по индексу |
| `oxphp_bridge_set_dispatch_fn(fn)` | Установка callback диспетчеризации Rust |
| `oxphp_bridge_get_dispatch_fn()` | Получение callback диспетчеризации Rust |
| `oxphp_bridge_set_call_php_fn(fn)` | Установка callback вызова PHP |
| `oxphp_bridge_get_call_php_fn()` | Получение callback вызова PHP |
| `oxphp_bridge_dispatch(name, json_args)` | Диспетчеризация к обработчику Rust |
| `oxphp_bridge_call_php(name, json_args)` | Вызов PHP-функции из Rust |
| `oxphp_bridge_strdup(s)` | Дублирование строки через C `malloc` |
| `oxphp_bridge_free_string(ptr)` | Освобождение строки, аллоцированной через `strdup` |

Реестр является глобальным (не потоковым), поскольку он записывается один раз из главного потока при запуске и читается во время MINIT — без конкурентного доступа. Он никогда не освобождается; он существует в течение всего времени жизни процесса.

### Формат данных при пересечении границы

Все межграничные вызовы функций используют JSON-конверт:

- **Аргументы**: JSON-кодированный массив параметров
- **Успешный результат**: `{"ok": value}`
- **Результат с ошибкой**: `{"err": "message"}`

Пара `oxphp_bridge_strdup`/`oxphp_bridge_free_string` использует `malloc`/`free` из C, чтобы избежать несоответствия аллокаторов между Rust и библиотекой C.

## PHP-расширение

PHP-расширение (`ext/oxphp_sapi.c`) предоставляет серверные функции PHP-скриптам. Оно линкуется с `liboxphp_bridge.so` для чтения контекста моста.

### Доступные функции

| Функция | Тип возврата | Описание |
|---|---|---|
| `oxphp_request_id()` | `string` | Возвращает hex-идентификатор текущего запроса |
| `oxphp_worker_id()` | `int` | Возвращает индекс потока воркера (начиная с 0) |
| `oxphp_server_info()` | `array` | Возвращает `sapi`, `version`, `worker_id`, `request_time` |
| `oxphp_request_heartbeat(int $time = 10)` | `bool` | Заглушка для продления тайм-аута (в настоящее время возвращает `true`) |
| `oxphp_finish_request()` | `bool` | Отмечает запрос как завершённый для фоновой обработки |
| `oxphp_is_streaming()` | `bool` | Проверяет, использует ли текущий запрос потоковый режим |

### Нативная диспетчеризация плагинов

Расширение регистрирует `oxphp_native_dispatch` — обработчик с нулевой сериализацией для всех зарегистрированных функций плагинов. Когда PHP-скрипт вызывает функцию плагина (например, `oxphp_example_info()`), движок Zend перенаправляет вызов в `oxphp_native_dispatch`, который:

1. Читает имя функции из `execute_data->func->common.function_name`
2. Передаёт указатели `zval*` на аргументы и возвращаемое значение напрямую в Rust через callback моста
3. Rust читает/пишет zval'ы через C-функции доступа (`oxphp_arg_long`, `oxphp_ret_str` и т.д.) — без сериализации
4. При ошибке выдаёт PHP-предупреждение `E_WARNING` и возвращает `NULL`

### Вызов PHP из Rust

Мост предоставляет `oxphp_call_php_native()` — функцию, которую Rust может вызывать для вызова PHP-функций:

1. Rust вызывает `oxphp_call_php_native(func_name, args, argc, result)` с подготовленными zval-аргументами
2. C-сторона разрешает функцию через `zend_hash_str_find_ptr` и вызывает `zend_call_known_function` напрямую
3. Результат-zval принадлежит Rust и освобождается через `zval_ptr_dtor` при удалении

### Пример использования

```php
<?php
// Get the request ID assigned by the server
$requestId = oxphp_request_id();
header("X-Debug-Worker: " . oxphp_worker_id());

// Examine SAPI details
$info = oxphp_server_info();
// $info = [
//     'sapi' => 'oxphp',
//     'version' => '0.1.0',
//     'worker_id' => 3,
//     'request_time' => 1707609600.123456,
// ]

// Finish the response but continue processing
oxphp_finish_request();
// ... background work here (logging, cleanup, etc.)
```

### Регистрация расширения

Расширение регистрируется как стандартный PHP-модуль с хуком MINIT, который настраивает мост функций плагинов:

```c
zend_module_entry oxphp_sapi_module_entry = {
    STANDARD_MODULE_HEADER,
    "oxphp_sapi",
    oxphp_sapi_functions,
    PHP_MINIT(oxphp_sapi),  // sets call_php callback, registers plugin fns
    NULL,                    // MSHUTDOWN
    NULL,                    // RINIT
    NULL,                    // RSHUTDOWN
    PHP_MINFO(oxphp_sapi),
    "0.1.0",
    STANDARD_MODULE_PROPERTIES
};
```

**MINIT** выполняет две задачи:

1. Устанавливает `oxphp_bridge_set_call_php_fn(oxphp_sapi_call_php)`, чтобы Rust мог вызывать PHP-функции
2. Читает реестр функций плагинов из моста и регистрирует каждую функцию в Zend через `zend_register_functions()` — это должно происходить при запуске модуля (не при запуске запроса), чтобы оптимизация `function_exists()` на этапе компиляции в OPcache могла видеть эти функции

## Сводка потока данных

```
Rust (Tokio task)                     PHP Worker Thread
─────────────────                     ──────────────────
ScriptRequest ──sync_channel──▶ recv()
                                      │
                                      ├── bridge::init_ctx()
                                      ├── bridge::set_request_id()
                                      ├── bridge::set_request_time()
                                      ├── sapi::set_request_data()
                                      │     ├── server vars → TLS
                                      │     ├── cookies → TLS
                                      │     └── body → TLS
                                      │
                                      ├── php_request_startup()
                                      │     ├── RINIT for all extensions
                                      │     └── OPcache reads request_time
                                      │
                                      ├── php_execute_script()
                                      │     ├── PHP reads $_SERVER, $_GET, etc.
                                      │     ├── PHP calls oxphp_request_id()
                                      │     │     └── bridge::get_request_id()
                                      │     ├── PHP calls plugin function
                                      │     │     └── bridge::dispatch() → Rust
                                      │     └── Output captured by SAPI
                                      │
                                      ├── php_request_shutdown()
                                      │
                                      ├── sapi::take_response()
                                      │     ├── output buffer
                                      │     ├── response headers
                                      │     └── status code
                                      │
                                      └── bridge::clear_ctx()
                                      │
ScriptResponse ◀──oneshot──────────── tx.send()
```

## Сборка моста и расширения

Библиотека моста и PHP-расширение собираются как часть Docker-образа. Для локальной разработки:

```bash
# Build the bridge library
cd ext/bridge
make
sudo make install  # installs liboxphp_bridge.so

# Build the PHP extension
cd ext
phpize
./configure --enable-oxphp-sapi
make
sudo make install  # installs oxphp_sapi.so
```

Оба артефакта должны быть доступны во время выполнения:
- `liboxphp_bridge.so` в пути поиска библиотек (`LD_LIBRARY_PATH=/usr/local/lib`)
- `oxphp_sapi.so` в директории расширений PHP (или загружается через `extension=oxphp_sapi.so` в `php.ini`)

## См. также

- [Обзор архитектуры](./overview.md) — Карта компонентов и последовательность запуска
- [Пул воркеров](./worker-pool.md) — Жизненный цикл потока воркера, который вызывает мост
- [Жизненный цикл запроса](./request-lifecycle.md) — Полный конвейер запроса от TCP до ответа
- [PHP-функции](../php/functions.md) — Справочник по функциям, вызываемым из PHP
- [Суперглобальные переменные](../php/superglobals.md) — Как заполняются `$_SERVER`, `$_GET` и т.д.
- [OPcache](../php/opcache.md) — Интеграция OPcache и инвариант `request_time`
