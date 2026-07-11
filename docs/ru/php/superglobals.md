---
title: Суперглобальные переменные
description: Как OxPHP заполняет $_SERVER, $_GET, $_POST, $_COOKIE, $_FILES и php://input для каждого запроса.
---

# Суперглобальные переменные

OxPHP заполняет все стандартные суперглобальные переменные PHP до выполнения вашего скрипта, воспроизводя поведение, которое PHP-разработчики ожидают от традиционных серверных окружений. Каждое значение доступно с первой строки вашего кода — никакой инициализации не требуется.

## $_SERVER

OxPHP формирует `$_SERVER` из входящего HTTP-запроса в соответствии со спецификацией CGI/1.1. Переменные окружения процесса импортируются первыми; CGI-переменные устанавливаются после, поэтому значения, специфичные для запроса, всегда перекрывают совпадающие ключи окружения.

### Стандартные переменные

| Переменная | Описание | Пример |
|-----------|---------|--------|
| `SCRIPT_FILENAME` | Абсолютный путь в файловой системе к выполняемому PHP-скрипту | `/var/www/html/public/index.php` |
| `DOCUMENT_ROOT` | Корневая директория веб-сервера, настроенная через переменную окружения `DOCUMENT_ROOT` | `/var/www/html/public` |
| `SERVER_SOFTWARE` | Идентификатор сервера (содержит текущую версию OxPHP) | `OxPHP/0.10.0` |
| `SERVER_PROTOCOL` | Согласованная версия протокола HTTP | `HTTP/2` |
| `REQUEST_METHOD` | HTTP-метод | `GET` |
| `REQUEST_URI` | Полный URI со строкой запроса | `/app?page=2` |
| `SCRIPT_NAME` | Путь исполняемого скрипта относительно `DOCUMENT_ROOT` — фронт-контроллер в режиме Framework, а **не** URI запроса | `/index.php` |
| `DOCUMENT_URI` | Псевдоним `SCRIPT_NAME` для совместимости с nginx/PHP-FPM | `/index.php` |
| `PHP_SELF` | `SCRIPT_NAME` плюс `PATH_INFO` при наличии, иначе равно `SCRIPT_NAME` | `/index.php/user/42` |
| `QUERY_STRING` | Часть URI со строкой запроса (пустая строка при отсутствии) | `page=2` |
| `SERVER_NAME` | Имя хоста из заголовка `Host` | `example.com` |
| `SERVER_PORT` | Порт из заголовка `Host` | `8080` |
| `REMOTE_ADDR` | IP-адрес клиента | `172.17.0.1` |
| `REMOTE_PORT` | Номер порта клиента | `54321` |
| `HTTPS` | Устанавливается в `"on"` при соединении через TLS; отсутствует в других случаях | `on` |
| `REQUEST_SCHEME` | `"https"` для TLS-соединений, `"http"` в остальных случаях | `https` |
| `CONTENT_TYPE` | Значение заголовка `Content-Type` (без префикса `HTTP_`) | `application/json` |
| `CONTENT_LENGTH` | Значение заголовка `Content-Length` (без префикса `HTTP_`) | `128` |
| `REQUEST_TIME` | Unix-timestamp (целое число) в момент начала запроса | `1738800000` |
| `REQUEST_TIME_FLOAT` | Unix-timestamp с точностью до микросекунды | `1738800000.123456` |
| `GATEWAY_INTERFACE` | Строка версии CGI | `CGI/1.1` |

При отсутствии заголовка `Host` значение `SERVER_NAME` по умолчанию равно `localhost`, а `SERVER_PORT` — `80` (или `443` для TLS).

### Заголовки HTTP-запроса

Все заголовки HTTP-запроса добавляются в `$_SERVER` с префиксом `HTTP_`. Имена заголовков преобразуются в верхний регистр, а дефисы заменяются подчёркиваниями согласно соглашениям CGI/1.1:

```text
Accept: text/html            -> HTTP_ACCEPT
X-Forwarded-For: 1.2.3.4    -> HTTP_X_FORWARDED_FOR
Authorization: Bearer abc    -> HTTP_AUTHORIZATION
Cookie: session=xyz          -> HTTP_COOKIE
```

> **Примечание:** `Content-Type` и `Content-Length` присутствуют без префикса `HTTP_` — как `CONTENT_TYPE` и `CONTENT_LENGTH` — согласно требованиям спецификации CGI.

### За reverse proxy

Когда настроен `TRUSTED_PROXIES` и пир запроса входит в доверенное множество, OxPHP переписывает следующие ключи `$_SERVER` из forwarded-заголовков (`X-Forwarded-*` или RFC 7239 `Forwarded`):

| Переменная | Значение, когда пир доверенный | Значение в остальных случаях |
|------------|--------------------------------|------------------------------|
| `REMOTE_ADDR` | Самый правый недоверенный адрес из `X-Forwarded-For` / `Forwarded` | IP прямого пира |
| `REMOTE_PORT` | Исходный порт клиента из `Forwarded: for=ip:port`, иначе `0` | Порт прямого пира |
| `HTTPS` | `"on"`, когда `X-Forwarded-Proto: https` | Устанавливается только если пир-соединение использует TLS |
| `REQUEST_SCHEME` | `"https"` / `"http"` из `X-Forwarded-Proto` | По фактическому состоянию TLS |
| `SERVER_NAME` | Hostname из `X-Forwarded-Host` | Hostname из заголовка `Host` |
| `SERVER_PORT` | `X-Forwarded-Port`, иначе порт из `X-Forwarded-Host`, иначе 443/80 по схеме | Порт из `Host` или 443/80 |

Сырые ключи `HTTP_X_FORWARDED_FOR`, `HTTP_X_FORWARDED_PROTO`, `HTTP_X_FORWARDED_HOST`, `HTTP_X_FORWARDED_PORT` и `HTTP_FORWARDED` остаются в `$_SERVER` без изменений — доступны и переписанные значения, и оригинальные заголовки.

`REMOTE_PORT` за доверенным прокси равен `"0"`, если только прокси не передаёт RFC 7239 `Forwarded: for=ip:port` — ни `X-Forwarded-For`, ни выбор самого правого недоверенного адреса не несут исходный порт клиента, поэтому синтетическое значение обнуляется, а не угадывается.

Когда `TRUSTED_PROXIES` **не** задано, переписывание не выполняется и `REMOTE_ADDR` всегда содержит IP прямого пира — как правило, вашего балансировщика, а не конечного клиента. Разбирать `X-Forwarded-For` вручную легко ошибиться (leftmost против rightmost, отсутствие CIDR-проверки доверия); предпочтительно настроить `TRUSTED_PROXIES`. Алгоритм доверия и синтаксис конфигурации — в разделе [Доверенные прокси](../security/trusted-proxies.md).

### Переменные контекста трассировки

При включённой распределённой трассировке OxPHP добавляет в `$_SERVER` переменные контекста трассировки:

| Переменная | Описание | Пример |
|-----------|---------|--------|
| `OXPHP_TRACE_ID` | W3C trace ID для текущего запроса | `4bf92f3577b34da6a3ce929d0e0e4736` |
| `OXPHP_SPAN_ID` | Span ID для серверного спана OxPHP | `00f067aa0ba902b7` |
| `OXPHP_PARENT_SPAN_ID` | Parent span ID от вышестоящего сервиса (пусто для корневого спана) | `b9c7c989f97918e1` |

Эти переменные присутствуют только при получении корректного заголовка `traceparent` или при генерации OxPHP нового трейса. Если трассировка не настроена, эти ключи отсутствуют.

### Отличия от PHP-FPM

Следующие переменные ведут себя иначе по сравнению со стандартной конфигурацией PHP-FPM:

| Переменная | Поведение |
|-----------|---------|
| `SERVER_ADDR` | Не устанавливается. OxPHP не заполняет локальный IP-адрес сервера. |
| `PATH_INFO` | Устанавливается автоматически — см. [Поведение PATH_INFO](#поведение-path_info) ниже. |
| `PATH_TRANSLATED` | Не устанавливается. |
| `PHP_AUTH_USER` / `PHP_AUTH_PW` / `AUTH_TYPE` | Не извлекаются из заголовка `Authorization`. Читайте `$_SERVER['HTTP_AUTHORIZATION']` напрямую. |
| `REDIRECT_STATUS` | Не устанавливается. OxPHP не использует механизм внутреннего перенаправления. |

### Пример

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$scheme  = $_SERVER['REQUEST_SCHEME'];  // "http" или "https"

// Читаем произвольный заголовок
$token = $_SERVER['HTTP_AUTHORIZATION'] ?? '';

// REMOTE_ADDR уже содержит реальный IP клиента, когда настроен TRUSTED_PROXIES.
// Без него REMOTE_ADDR — это прямой пир (обычно балансировщик нагрузки).
$clientIp = $_SERVER['REMOTE_ADDR'];

// Проверяем TLS без проверки порта
if (isset($_SERVER['HTTPS']) && $_SERVER['HTTPS'] === 'on') {
    // Защищённое соединение
}
```

### Поведение PATH_INFO

`$_SERVER['PATH_INFO']` заполняется автоматически в зависимости от активного режима маршрутизации. Никакого feature flag нет — переменная окружения `SPLIT_PATH_INFO_ENABLED` удалена.

| Режим маршрутизации | Когда установлен | Значение |
|---|---|---|
| **Traditional** (`ENTRY_FILE` не задано) | Только когда URI содержит `.php/` и префикс скрипта существует на диске | Хвост после сегмента скрипта |
| **Framework** (`ENTRY_FILE=index.php`) | Только когда запрос явно называет entry-файл с хвостовым сегментом (`/index.php/extra`) | Хвост после entry-файла, напр. `/news` |
| **SPA** (`ENTRY_FILE=index.html`) | Никогда — PHP вызывается только для точных `.php`-файлов, PATH_INFO нет | — |

`SCRIPT_NAME` всегда указывает на реально исполняемый скрипт (путь файла относительно document root), поэтому при обычной маршрутизации `PATH_INFO` присутствует только когда `SCRIPT_NAME` — буквальный префикс пути запроса. Если запрос переписан на фронт-контроллер, которого нет в URL (маршрут приложения, directory-index, фолбэк отсутствующей статики), `PATH_INFO` отсутствует, а оригинальный путь живёт в `REQUEST_URI`. (Фолбэк `PHP_DENY_PATHS` — намеренное исключение: он кладёт в `PATH_INFO` исходный санитизированный URI, чтобы fallback-скрипт мог по нему роутить.)

#### Примеры в режиме Traditional

OxPHP сканирует URI слева направо в поисках первого сегмента `.php`, соответствующего реальному файлу на диске. Всё, что следует после него, становится `PATH_INFO`:

| URI запроса | Файл на диске | `SCRIPT_NAME` | `PATH_INFO` | `PHP_SELF` |
|---|---|---|---|---|
| `/app.php/user/42` | `app.php` существует | `/app.php` | `/user/42` | `/app.php/user/42` |
| `/index.php/api/v2/users` | `index.php` существует | `/index.php` | `/api/v2/users` | `/index.php/api/v2/users` |
| `/app.php` | `app.php` существует | `/app.php` | *(отсутствует)* | `/app.php` |
| `/missing.php/foo` | файл не найден | фолбэк на `/index.php` | — | зависит от фолбэка |

#### Примеры в режиме Framework

Каждый нестатический запрос переписывается на `index.php`. `PATH_INFO` устанавливается только когда запрос явно называет entry-файл с хвостовым сегментом; для маршрутов приложения оригинальный путь читается из `REQUEST_URI`.

| URI запроса | `SCRIPT_NAME` | `PATH_INFO` |
|---|---|---|
| `/api/users` | `/index.php` | *(отсутствует)* |
| `/about.php` | `/index.php` | *(отсутствует)* |
| `/index.php/news/local` | `/index.php` | `/news/local` |
| `/index.php` | `/index.php` | *(отсутствует)* |

> **Примечание:** `PATH_TRANSLATED` не заполняется. На практике используется крайне редко и не устанавливается ни nginx, ни PHP-FPM по умолчанию.

---

## $_GET

Параметры строки запроса автоматически разбираются из URI запроса.

```php
<?php
// Запрос: GET /search?q=oxphp&page=2
$query = $_GET['q'];     // "oxphp"
$page  = $_GET['page'];  // "2"
```

Синтаксис массивов работает ожидаемым образом:

```php
<?php
// Запрос: GET /filter?tags[]=php&tags[]=async
$tags = $_GET['tags'];  // ["php", "async"]
```

---

## $_POST

OxPHP поддерживает два стандартных типа содержимого для отправки форм:

- `application/x-www-form-urlencoded` — стандартные данные HTML-формы
- `multipart/form-data` — загрузка файлов вместе с полями формы

```php
<?php
// Запрос: POST /login
// Content-Type: application/x-www-form-urlencoded
// Body: username=admin&password=secret

$username = $_POST['username'];  // "admin"
$password = $_POST['password'];  // "secret"
```

Для JSON или других типов содержимого используйте `php://input`:

```php
<?php
// Запрос: POST /api/users
// Content-Type: application/json
// Body: {"name":"Alice","email":"alice@example.com"}

$data  = json_decode(file_get_contents('php://input'), true);
$name  = $data['name'];   // "Alice"
$email = $data['email'];  // "alice@example.com"
```

---

## $_COOKIE

Куки разбираются из заголовка запроса `Cookie`.

```php
<?php
// Запрос с: Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

> **Примечание:** Куки с префиксом `__oxp_` зарезервированы для внутренних плагинов OxPHP. Они удаляются из заголовка `Cookie` до того, как тот поступает в PHP, и не появятся в `$_COOKIE`.

---

## $_FILES

Загруженные файлы, отправленные через `multipart/form-data`, заполняют массив `$_FILES` со стандартной структурой PHP:

```php
<?php
// Структура $_FILES['avatar']:
// [
//     'name'     => 'photo.jpg',       // Оригинальное имя файла, отправленное клиентом
//     'type'     => 'image/jpeg',      // MIME-тип, указанный клиентом
//     'tmp_name' => '/tmp/phpAb12Cd',  // Путь к временному файлу на сервере
//     'error'    => 0,                 // UPLOAD_ERR_OK (0 означает отсутствие ошибки)
//     'size'     => 204800,            // Размер файла в байтах
// ]

if ($_FILES['avatar']['error'] === UPLOAD_ERR_OK) {
    $tmp  = $_FILES['avatar']['tmp_name'];
    $name = basename($_FILES['avatar']['name']);
    move_uploaded_file($tmp, "/uploads/$name");
}
```

---

## $_REQUEST

`$_REQUEST` — это объединённый массив `$_GET`, `$_POST` и при необходимости `$_COOKIE`, формируемый PHP согласно INI-директиве `request_order` (по умолчанию: `"GP"` — GET, затем POST). OxPHP не изменяет это поведение.

```php
<?php
// GET /form?action=preview с телом POST: action=submit
$action = $_REQUEST['action'];  // "submit" (POST перекрывает GET при порядке по умолчанию)
```

---

## php://input

Сырое тело запроса доступно через поток `php://input`. Это стандартный способ чтения JSON-нагрузки, XML или любого другого типа содержимого, кроме отправки форм.

```php
<?php
$body = file_get_contents('php://input');
$data = json_decode($body, true);
```

`php://input` поддерживает перемотку и может читаться несколько раз в рамках одного запроса.

> **Примечание:** `php://input` пуст для запросов с `multipart/form-data`. Для таких запросов используйте `$_POST` и `$_FILES`.

---

## Отключение суперглобалов

Установите `SUPERGLOBALS_ENABLED=false` чтобы отключить заполнение `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES` и `$_SERVER`. При отключении эти массивы будут пустыми. Используйте [HTTP Request API](request-api.md) (`oxphp_http_request()`) для доступа к данным запроса.

```bash
SUPERGLOBALS_ENABLED=false   # суперглобалы — пустые массивы
```

Следующее остаётся доступным независимо от этой настройки:

| Что | Почему |
|-----|--------|
| `$_SESSION` | Управляется модулем сессий PHP, а не SAPI |
| `php://input` | Поток, а не суперглобал |
| `header()`, `headers_list()` и т.д. | SAPI-функции, не суперглобалы |
| `session_start()` и другие `session_*()` | Нативные PHP-функции |
| `oxphp_http_request()` | Доступен всегда — рекомендуемая альтернатива |

Текущую настройку можно проверить во время выполнения:

```php
if (!oxphp_superglobals_enabled()) {
    $request = oxphp_http_request();
    $page = $request->query('page', 1);
}
```

---

## См. также

- [HTTP Request API](request-api.md) — типизированный объект запроса с ленивой загрузкой как альтернатива суперглобалам
- [PHP-функции](functions.md) — `oxphp_request_id()`, `oxphp_worker_id()` и другие функции расширения
- [Режим Worker](../features/worker-mode.md) — как суперглобальные переменные обновляются между запросами воркера
- [Справочник по конфигурации](../operations/configuration.md) — `DOCUMENT_ROOT` и другие переменные конфигурации сервера
