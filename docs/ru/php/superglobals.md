---
title: Суперглобальные переменные
description: Как OxPHP заполняет суперглобальные массивы PHP
---

OxPHP заполняет все стандартные суперглобальные переменные PHP, чтобы существующий PHP-код работал без модификации. Каждое значение устанавливается **до** вызова `php_request_startup()`, поэтому суперглобальные переменные доступны с первой строки вашего скрипта --- в том числе внутри расширений вроде OPcache, которые читают их во время инициализации запроса.

## `$_SERVER`

Пользовательский SAPI регистрирует полный набор CGI/1.1-переменных через callback `register_server_variables`. Эти переменные формируются из HTTP-запроса и внедряются в `$_SERVER` во время инициализации PHP-запроса.

### Стандартные CGI-переменные

| Переменная | Источник | Пример |
|------------|----------|--------|
| `REQUEST_METHOD` | HTTP-метод | `GET` |
| `REQUEST_URI` | Полный URI с query-строкой | `/app?page=2` |
| `QUERY_STRING` | Query-часть URI | `page=2` |
| `SERVER_PROTOCOL` | Всегда `HTTP/1.1` | `HTTP/1.1` |
| `SCRIPT_NAME` | Путь URI без query-строки | `/app` |
| `PHP_SELF` | То же, что `SCRIPT_NAME` | `/app` |
| `SCRIPT_FILENAME` | Абсолютный путь к скрипту в файловой системе | `/var/www/html/public/index.php` |
| `DOCUMENT_ROOT` | Корневая директория веб-сервера | `/var/www/html/public` |
| `SERVER_SOFTWARE` | Идентификатор сервера | `OxPHP/0.1.0` |
| `GATEWAY_INTERFACE` | Версия CGI | `CGI/1.1` |
| `REMOTE_ADDR` | IP-адрес клиента | `172.17.0.1` |
| `REMOTE_PORT` | Порт клиента | `54321` |
| `SERVER_NAME` | Из заголовка `Host` (часть хоста) | `example.com` |
| `SERVER_PORT` | Из заголовка `Host` (часть порта) | `8080` |
| `CONTENT_TYPE` | Из заголовка `Content-Type` | `application/json` |
| `CONTENT_LENGTH` | Из заголовка `Content-Length` | `128` |

Когда заголовок `Host` отсутствует, `SERVER_NAME` по умолчанию равен `localhost`, а `SERVER_PORT` --- `80`.

### Заголовки HTTP-запроса

Все заголовки HTTP-запроса добавляются в `$_SERVER` с префиксом `HTTP_` и заменой дефисов на подчёркивания, в соответствии со стандартными соглашениями CGI:

```
Accept: text/html         → HTTP_ACCEPT = "text/html"
X-Forwarded-For: 1.2.3.4 → HTTP_X_FORWARDED_FOR = "1.2.3.4"
Authorization: Bearer ... → HTTP_AUTHORIZATION = "Bearer ..."
```

`Content-Type` и `Content-Length` **не** получают префикс `HTTP_` --- они добавляются как `CONTENT_TYPE` и `CONTENT_LENGTH` напрямую, как того требует спецификация CGI.

### Пример использования

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$docroot = $_SERVER['DOCUMENT_ROOT'];

// Доступ к пользовательским заголовкам
$token   = $_SERVER['HTTP_AUTHORIZATION'] ?? '';
$xff     = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'];
```

## `$_GET`

Параметры query-строки разбираются стандартным движком PHP из серверной переменной `QUERY_STRING`. OxPHP устанавливает `QUERY_STRING` из URI запроса, а PHP автоматически заполняет `$_GET` во время инициализации запроса.

```php
<?php
// Запрос: GET /search?q=oxphp&page=2
$query = $_GET['q'];     // "oxphp"
$page  = $_GET['page'];  // "2"
```

## `$_POST`

SAPI предоставляет callback `read_post`, который передаёт тело запроса стандартному парсеру POST в PHP. PHP обрабатывает оба типа содержимого: `application/x-www-form-urlencoded` и `multipart/form-data`. Тело читается инкрементально --- PHP вызывает callback повторно, пока тот не вернёт 0 байт.

```php
<?php
// Запрос: POST /login с телом application/x-www-form-urlencoded
$username = $_POST['username'];
$password = $_POST['password'];
```

Сырое тело запроса также доступно через `php://input`:

```php
<?php
$json = json_decode(file_get_contents('php://input'), true);
```

## `$_COOKIE`

SAPI предоставляет callback `read_cookies`, который возвращает сырую строку заголовка `Cookie`. Движок PHP разбирает её в массив `$_COOKIE` во время инициализации запроса.

```php
<?php
// Запрос с Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

## `$_REQUEST`

`$_REQUEST` заполняется PHP на основе INI-директивы `request_order` (по умолчанию: `"GP"` --- GET, затем POST). Он объединяет `$_GET` и `$_POST` (и, опционально, `$_COOKIE`) в указанном порядке. OxPHP не переопределяет это поведение.

## `$_FILES`

Загруженные файлы, отправленные через `multipart/form-data`, разбираются стандартным механизмом `read_post` PHP. Массив `$_FILES` заполняется автоматически, включая `name`, `type`, `tmp_name`, `error` и `size` для каждого загруженного файла.

```php
<?php
if ($_FILES['avatar']['error'] === UPLOAD_ERR_OK) {
    $tmp  = $_FILES['avatar']['tmp_name'];
    $name = $_FILES['avatar']['name'];
    move_uploaded_file($tmp, "/uploads/$name");
}
```

## Детали реализации

### Паттерн пакетных FFI-вызовов

OxPHP формирует все пары ключ-значение `$_SERVER` в одном Rust-векторе `Vec<(CString, CString)>` и сохраняет его в локальном хранилище потока до начала запроса PHP. Во время callback `register_server_variables` весь вектор обходится за один проход, вызывая `php_register_variable_safe()` для каждой записи. Это позволяет избежать FFI-вызовов на каждую переменную и сохраняет постоянные накладные расходы независимо от количества заголовков.

### Требования к времени жизни данных

Все данные, привязанные к запросу --- серверные переменные, строка cookie и тело запроса --- должны быть сохранены в локальном хранилище потока **до** вызова `php_request_startup()`. Эти значения должны оставаться действительными до `php_request_shutdown()`, поскольку движок PHP хранит сырые указатели на них. OxPHP хранит их в структуре `RequestData` внутри `thread_local! { RefCell }` и очищает только после полного завершения запроса.

### Мост с локальным хранилищем потока

C-библиотека моста (`liboxphp_bridge.so`) использует `__thread` TLS для передачи контекста запроса (идентификатор запроса, идентификатор воркера, время запроса) между Rust и PHP-расширением. И бинарный файл Rust, и PHP-расширение линкуются с одной и той же разделяемой библиотекой, получая доступ к одному и тому же локальному хранилищу потока. Это единственный надёжный механизм для обмена состоянием через границы `dlopen`.

## Смотрите также

- [Функции PHP-расширения](functions.md) --- встроенные функции для доступа к идентификаторам запросов, информации о воркерах и метаданным сервера
- [Совместимость с OPcache](opcache.md) --- как время запроса обеспечивает работу `file_update_protection` в OPcache
- [Мост SAPI](/architecture/sapi-bridge.md) --- C-библиотека моста и механизм локального хранилища потока
- [Жизненный цикл запроса](/architecture/request-lifecycle.md) --- как данные запроса передаются из Rust в PHP
