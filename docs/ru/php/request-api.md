---
title: HTTP Object API
description: Объектно-ориентированный API для доступа к данным HTTP-запроса в OxPHP — типобезопасный интерфейс с ленивой загрузкой вместо суперглобальных переменных PHP.
---

# HTTP Object API

OxPHP предоставляет объектно-ориентированный API для доступа к данным HTTP-запроса. Вместо того чтобы читать `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES` и `$_SERVER`, вы вызываете методы объекта `Request`, который возвращает именно то, что нужно — не больше и не меньше.

## Содержание

- [Обзор](#обзор)
- [Получение объекта запроса](#получение-объекта-запроса)
- [Методы RequestInterface](#методы-requestinterface)
  - [URI и метод](#uri-и-метод)
  - [Протокол](#протокол)
  - [Параметры строки запроса](#параметры-строки-запроса)
  - [Разобранное тело](#разобранное-тело)
  - [Заголовки](#заголовки)
  - [Куки](#куки)
  - [Сырое тело](#сырое-тело)
  - [Загруженные файлы](#загруженные-файлы)
  - [Клиент](#клиент)
  - [Время](#время)
  - [Атрибуты](#атрибуты)
  - [Сессия](#сессия)
- [SessionInterface](#sessioninterface)
- [UploadedFileInterface](#uploadedfileinterface)
- [AttributesInterface](#attributesinterface)
- [Исключения](#исключения)
- [SUPERGLOBALS_ENABLED](#superglobals_enabled)
- [Режим Worker](#режим-worker)
- [Поддержка IDE](#поддержка-ide)
- [Примеры](#примеры)

---

## Обзор

`oxphp_http_request()` возвращает прокси только для чтения к данным HTTP-запроса, хранящимся в текущем воркер-потоке. Данные загружаются лениво — единственный вызов вроде `$request->header('Accept')` обращается напрямую к структуре данных на стороне Rust и возвращает только это значение. Вызовы, возвращающие полный массив, например `$request->headers()`, строят массив один раз и кэшируют его в PHP-объекте на время жизни запроса.

**Зачем использовать вместо суперглобальных переменных?**

- **Разбор JSON-тела встроен.** `$request->payload()` разбирает `application/json`, `application/x-www-form-urlencoded` и `multipart/form-data` без дополнительного кода.
- **Никаких опечаток в ключах массива.** `$request->method()` сложнее написать неправильно, чем `$_SERVER['REQUEST_METHOD']`.
- **Типобезопасные загрузки файлов.** `$request->file('avatar')->type()` возвращает MIME-тип, определённый по реальному содержимому файла, а не по значению, переданному клиентом.
- **Тестируемость.** Поскольку поведение определено интерфейсами, в модульных тестах можно подставлять тестовые реализации.
- **Суперглобальные переменные остаются доступными.** Установка `SUPERGLOBALS_ENABLED=false` — опциональна. Object API работает в любом случае.

---

## Получение объекта запроса

```php
<?php
$request = oxphp_http_request();
```

Вызывайте `oxphp_http_request()` в любом месте скрипта, выполняющегося в контексте активного HTTP-запроса. В режиме worker запрос можно также получить как параметр колбэка `oxphp_worker()`:

```php
<?php
oxphp_worker(function (\OxPHP\Http\RequestInterface $request) {
    $method = $request->method();
    // ...
});
```

Оба варианта возвращают один и тот же объект — параметр в колбэке является удобным сокращением.

---

## Методы RequestInterface

### URI и метод

```php
$request->method(): string
```

Возвращает HTTP-метод в верхнем регистре: `"GET"`, `"POST"`, `"PUT"`, `"PATCH"`, `"DELETE"` и т.д.

```php
$request->isMethod(string $method): bool
```

Проверка метода без учёта регистра.

```php
$request->path(): string
```

Путь URI без строки запроса: `"/users/42"`.

```php
$request->fullUri(): string
```

Полный URI со схемой, хостом, необязательным нестандартным портом, путём и строкой запроса: `"https://example.com:8080/users/42?page=2"`. Стандартные порты (80 для HTTP, 443 для HTTPS) опускаются.

```php
$request->scheme(): string
```

`"https"` или `"http"`.

```php
$request->isSecure(): bool
```

`true`, если схема — `"https"`.

```php
$request->host(): string
```

Имя хоста из заголовка `Host`. Возвращает пустую строку, если заголовок отсутствует (запросы HTTP/1.0 без заголовка `Host`).

```php
$request->port(): int
```

Порт из заголовка `Host`. Если явно не указан, возвращается порт по умолчанию для схемы: `80` для HTTP, `443` для HTTPS.

```php
$request->queryString(): ?string
```

Сырая строка запроса без ведущего `?`. Возвращает `null`, если строки запроса нет.

---

### Протокол

```php
$request->httpProtocol(): string
```

Полная строка протокола: `"HTTP/1.1"` или `"HTTP/2"`.

```php
$request->httpProtocolVersion(): string
```

Только номер версии: `"1.1"` или `"2"`.

---

### Параметры строки запроса

```php
$request->query(?string $key = null, mixed $default = null): mixed
```

Доступ к параметрам строки запроса.

| Вызов | Возвращает |
|-------|-----------|
| `$request->query()` | Все параметры в виде массива, включая вложенные |
| `$request->query('page')` | Значение `page` или `null`, если отсутствует |
| `$request->query('page', 1)` | Значение `page` или `1`, если отсутствует |

Скобочная нотация (`?tags[]=php&tags[]=async`) разбирается в вложенные массивы:

```php
// Запрос: GET /search?q=oxphp&tags[]=php&tags[]=async
$q    = $request->query('q');      // "oxphp"
$tags = $request->query('tags');   // ["php", "async"]
$all  = $request->query();         // ["q" => "oxphp", "tags" => ["php", "async"]]
```

Найденные значения всегда являются строками. `$default` возвращается как есть, если ключ отсутствует.

---

### Разобранное тело

```php
$request->payload(?string $key = null, mixed $default = null): mixed
```

Возвращает разобранное тело запроса. Тело разбирается в соответствии с заголовком `Content-Type`:

| Content-Type | Возвращает |
|---|---|
| `application/x-www-form-urlencoded` | Ассоциативный массив значений полей |
| `multipart/form-data` | Ассоциативный массив значений текстовых полей |
| `application/json` | Декодированный массив или скалярное значение; `null` при невалидном JSON |
| Любое другое значение | `null` |

`payload()` не ограничен запросами POST — метод работает с PUT, PATCH и любым другим методом, передающим тело. Разобранный результат кэшируется при первом вызове и переиспользуется в течение всего времени жизни запроса.

| Вызов | Возвращает |
|-------|-----------|
| `$request->payload()` | Всё разобранное тело целиком |
| `$request->payload('email')` | Значение одного поля или `null`, если отсутствует |
| `$request->payload('email', '')` | Значение одного поля или `''`, если отсутствует |

```php
<?php
// JSON-запрос: POST /api/users
// Content-Type: application/json
// Body: {"name": "Alice", "role": "admin"}

$name = $request->payload('name');  // "Alice"
$role = $request->payload('role');  // "admin"
$data = $request->payload();        // ["name" => "Alice", "role" => "admin"]
```

---

### Заголовки

```php
$request->header(string $name, ?string $default = null): ?string
```

Возвращает сырое значение заголовка. Имена заголовков нечувствительны к регистру. Для заголовков с несколькими значениями (`Accept`, `X-Forwarded-For`) вся строка заголовка возвращается как единая строка — разбор остаётся на ваше усмотрение.

```php
$request->hasHeader(string $name): bool
```

Возвращает `true`, если указанный заголовок присутствует.

```php
$request->headers(): array
```

Возвращает все заголовки в виде ассоциативного массива. Каждый ключ — имя заголовка в том виде, в котором оно получено (без нормализации), каждое значение — сырая строка заголовка.

```php
<?php
$accept = $request->header('Accept');
// "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"

if ($request->hasHeader('Authorization')) {
    $token = $request->header('Authorization');
}

$all = $request->headers();
// ["Content-Type" => "application/json", "Accept" => "...", ...]
```

---

### Куки

```php
$request->cookie(string $name, ?string $default = null): ?string
```

Возвращает значение одной куки или `$default`, если кука отсутствует.

```php
$request->cookies(): array
```

Возвращает все куки в виде ассоциативного массива пар «имя-значение».

```php
<?php
$theme   = $request->cookie('theme', 'light');   // "dark" или "light"
$session = $request->cookie('session');           // null, если отсутствует
$all     = $request->cookies();                   // ["theme" => "dark", ...]
```

---

### Сырое тело

```php
$request->body(): string
```

Возвращает сырые байты тела запроса. Это аналог `file_get_contents('php://input')` для OxPHP. В отличие от `payload()`, результат `body()` не кэшируется — каждый вызов обращается к базовой структуре данных.

```php
$request->contentType(): ?string
```

Возвращает значение заголовка `Content-Type` или `null`, если заголовок отсутствует.

```php
<?php
// Чтение сырого тела для проверки подписи
$raw       = $request->body();
$signature = $request->header('X-Hub-Signature-256');
$valid     = hash_hmac('sha256', $raw, $secret) === $signature;
```

`body()` и `payload()` независимы друг от друга. Оба можно вызывать в одном запросе.

---

### Загруженные файлы

```php
$request->file(string $name): ?UploadedFileInterface
```

Возвращает загруженный файл для указанного имени поля или `null`, если поле отсутствует. Для полей-массивов (`name="photos[]"`) возвращает первый файл.

```php
$request->files(?string $name = null): array
```

| Вызов | Возвращает |
|-------|-----------|
| `$request->files()` | Все загруженные файлы в виде плоского массива `UploadedFileInterface` |
| `$request->files('photos')` | Все файлы для поля `photos` (поддерживает `name="photos[]"`) |

```php
<?php
$avatar = $request->file('avatar');

if ($avatar && $avatar->isValid()) {
    $mime = $avatar->type();    // Определён по содержимому файла, а не по заявлению клиента
    $name = $avatar->name();    // Оригинальное имя файла
    $avatar->moveTo('/var/uploads/' . basename($name));
}

// Несколько файлов
$photos = $request->files('photos');  // UploadedFileInterface[]
foreach ($photos as $photo) {
    if ($photo->isValid()) {
        $photo->moveTo('/var/uploads/' . basename($photo->name()));
    }
}
```

---

### Клиент

```php
$request->ip(): string
```

Возвращает IP-адрес клиента (`REMOTE_ADDR`). Если ваше приложение работает за обратным прокси, читайте заголовок `X-Forwarded-For` напрямую через `$request->header('X-Forwarded-For')` и применяйте собственную логику доверия.

---

### Время

```php
$request->startTime(bool $asFloat = false): int|float
```

Возвращает Unix-timestamp момента получения запроса.

| Вызов | Возвращает |
|-------|-----------|
| `$request->startTime()` | Целые секунды: `1711234567` |
| `$request->startTime(true)` | Float с долями секунды: `1711234567.3412` |

```php
<?php
$elapsed = microtime(true) - $request->startTime(true);
error_log(sprintf("Request took %.3fs so far", $elapsed));
```

---

### Атрибуты

```php
$request->attributes(): AttributesInterface
```

Возвращает изменяемый контейнер атрибутов для текущего запроса. Используйте атрибуты для передачи данных между промежуточными слоями, обработчиками маршрутов и другим кодом в рамках одного запроса без использования глобальных переменных.

```php
<?php
// В промежуточном слое аутентификации
$request->attributes()->set('user', $authenticatedUser);

// В обработчике маршрута
$user = $request->attributes()->get('user');
```

Атрибуты привязаны к конкретному запросу и сбрасываются с каждым новым запросом в режиме worker. При использовании Fibers атрибуты разделяются между всеми Fibers, выполняющимися в одном воркер-потоке для одного запроса — но поскольку PHP Fibers кооперативны, параллельный доступ невозможен.

---

### Сессия

```php
$request->session(): ?SessionInterface
```

Возвращает представление `$_SESSION` только для чтения. Возвращает `null`, если `session_start()` не был вызван. Управление сессией (запуск, сохранение, уничтожение, запись значений) выполняется через стандартные функции PHP для работы с сессиями.

```php
<?php
session_start();
$session = $request->session();

$userId  = $session->get('user_id');
$isAdmin = $session->get('is_admin', false);

// Запись данных сессии через стандартные функции PHP
$_SESSION['last_seen'] = time();
```

---

## SessionInterface

`SessionInterface` — представление активной сессии только для чтения.

```php
namespace OxPHP\Http;

interface SessionInterface
{
    public function id(): string;
    public function name(): string;
    public function get(string $key, mixed $default = null): mixed;
    public function has(string $key): bool;
    public function all(): array;
}
```

| Метод | Описание |
|-------|----------|
| `id()` | Идентификатор сессии |
| `name()` | Имя сессии (по умолчанию: `"PHPSESSID"`) |
| `get(key, default)` | Одно значение сессии или `$default`, если ключ отсутствует |
| `has(key)` | `true`, если ключ существует в `$_SESSION` |
| `all()` | Все данные сессии в виде массива |

Значения сессии отражают текущее состояние `$_SESSION` на момент вызова, а не состояние при первом вызове `session()`.

---

## UploadedFileInterface

`UploadedFileInterface` представляет один загруженный файл.

```php
namespace OxPHP\Http;

interface UploadedFileInterface
{
    public function name(): string;
    public function clientType(): string;
    public function type(): string;
    public function size(): int;
    public function tmpPath(): string;
    public function error(): int;
    public function isValid(): bool;
    public function moveTo(string $path): bool;
}
```

| Метод | Описание |
|-------|----------|
| `name()` | Оригинальное имя файла, переданное клиентом |
| `clientType()` | MIME-тип, объявленный клиентом — не используйте это значение для принятия решений, связанных с безопасностью |
| `type()` | MIME-тип, определённый по реальному содержимому файла с помощью распознавания magic bytes. Возвращает `"application/octet-stream"`, если тип не удаётся определить. Кэшируется при первом вызове. |
| `size()` | Размер файла в байтах |
| `tmpPath()` | Путь к временному файлу на диске |
| `error()` | Одна из констант `UPLOAD_ERR_*` |
| `isValid()` | `true`, если `error()` равен `UPLOAD_ERR_OK` |
| `moveTo(path)` | Перемещает файл по указанному `$path`. Вызывает `type()` перед перемещением. Возвращает `false`, если файл невалиден или перемещение не удалось. |

Всегда проверяйте `isValid()` перед работой с загруженным файлом. Используйте `type()` вместо `clientType()` при принятии решений, связанных с безопасностью:

```php
<?php
$file = $request->file('document');

if (!$file || !$file->isValid()) {
    http_response_code(400);
    echo json_encode(['error' => 'Upload failed or missing']);
    return;
}

$detectedMime = $file->type();
$allowed = ['application/pdf', 'image/jpeg', 'image/png'];

if (!in_array($detectedMime, $allowed, true)) {
    http_response_code(415);
    echo json_encode(['error' => "File type not allowed: $detectedMime"]);
    return;
}

$file->moveTo('/var/uploads/' . bin2hex(random_bytes(8)) . '.pdf');
```

---

## AttributesInterface

`AttributesInterface` — единственная изменяемая часть объекта запроса. Предназначена для хранения метаданных конкретного запроса — аутентифицированного пользователя, параметров разобранного маршрута, локали, флагов функций — к которым обращаются несколько частей приложения.

```php
namespace OxPHP\Http;

interface AttributesInterface
{
    public function get(string $key, mixed $default = null): mixed;
    public function set(string $key, mixed $value): void;
    public function has(string $key): bool;
    public function remove(string $key): void;
    public function all(): array;
}
```

| Метод | Описание |
|-------|----------|
| `get(key, default)` | Возвращает значение для `$key` или `$default`, если отсутствует |
| `set(key, value)` | Сохраняет значение |
| `has(key)` | `true`, если ключ был установлен |
| `remove(key)` | Удаляет ключ |
| `all()` | Все атрибуты в виде ассоциативного массива |

---

## Исключения

Вызов `oxphp_http_request()` вне контекста активного запроса выбрасывает исключение из пространства имён `OxPHP\Http\Exception`.

```php
namespace OxPHP\Http\Exception;

class NoActiveRequestException extends \RuntimeException {}
class AsyncContextException extends NoActiveRequestException {}
class WorkerIdleException extends NoActiveRequestException {}
```

| Исключение | Когда выбрасывается |
|------------|---------------------|
| `NoActiveRequestException` | Нет активного HTTP-запроса: CLI, MINIT, после завершения работы или Fiber, переживший свой запрос |
| `AsyncContextException` | Внутри колбэка `oxphp_async()` — асинхронные воркеры выполняются в отдельных потоках без контекста запроса |
| `WorkerIdleException` | Режим worker, между запросами — воркер ожидает следующего запроса |

`AsyncContextException` и `WorkerIdleException` оба расширяют `NoActiveRequestException`, поэтому перехват базового класса покрывает все случаи.

```php
<?php
try {
    $request = oxphp_http_request();
} catch (\OxPHP\Http\Exception\AsyncContextException $e) {
    // Внутри oxphp_async() — здесь нет контекста запроса
} catch (\OxPHP\Http\Exception\WorkerIdleException $e) {
    // Воркер находится между запросами — не вызывайте oxphp_http_request() здесь
} catch (\OxPHP\Http\Exception\NoActiveRequestException $e) {
    // Любой другой случай без активного запроса
}
```

В обычном коде обработки запросов этот блок try/catch не нужен. Он полезен в коде инициализации, который может выполняться вне контекста запроса.

---

## SUPERGLOBALS_ENABLED

```bash
SUPERGLOBALS_ENABLED=true    # по умолчанию — полная обратная совместимость
SUPERGLOBALS_ENABLED=false   # суперглобальные переменные — пустые массивы
```

По умолчанию OxPHP заполняет `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES` и `$_SERVER` как обычно. В этом режиме HTTP Object API доступен наряду с суперглобальными переменными.

Установка `SUPERGLOBALS_ENABLED=false` делает эти массивы пустыми, что устраняет затраты на их построение для каждого запроса. Следующее работает независимо от этой настройки:

| Возможность | Поведение при `SUPERGLOBALS_ENABLED=false` |
|-------------|-------------------------------------------|
| `oxphp_http_request()` | Всегда доступен |
| `php://input` | Доступен (это поток, а не суперглобальная переменная) |
| `$_SESSION` | Доступен (управляется модулем сессий PHP) |
| `header()`, `headers_list()` | Доступны (функции вывода SAPI) |
| `session_start()`, `session_*()` | Доступны (встроенные функции PHP) |
| `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_SERVER` | Пустые массивы |

Используйте `oxphp_superglobals_enabled()` для проверки текущей настройки во время выполнения:

```php
<?php
if (!oxphp_superglobals_enabled()) {
    $method = oxphp_http_request()->method();
} else {
    $method = $_SERVER['REQUEST_METHOD'];
}
```

---

## Режим Worker

В режиме worker для каждого входящего запроса создаётся новый объект Request. Объект предыдущего запроса становится недействительным после завершения запроса — не сохраняйте ссылку на него между запросами.

```php
<?php
// worker.php

require __DIR__ . '/vendor/autoload.php';
$app = new MyApp\Application();

oxphp_worker(function (\OxPHP\Http\RequestInterface $request) use ($app) {
    $app->handle($request);
});
```

Объект Request, передаваемый в колбэк, идентичен результату вызова `oxphp_http_request()` внутри колбэка — оба читают из одного и того же потока-локального состояния.

Все кэши объекта Request (разобранные заголовки, куки, параметры запроса, payload) автоматически очищаются в начале следующего запроса.

---

## Поддержка IDE

Установите пакет стабов для автодополнения и проверки типов в PhpStorm, VS Code и любом редакторе с поддержкой LSP:

```bash
composer require --dev oxphp/stubs
```

Пакет стабов предоставляет:

```
oxphp-stubs/
├── OxPHP/Http/
│   ├── RequestInterface.php
│   ├── SessionInterface.php
│   ├── UploadedFileInterface.php
│   ├── AttributesInterface.php
│   ├── Request.php
│   ├── Session.php
│   ├── UploadedFile.php
│   ├── Attributes.php
│   └── Exception/
│       ├── NoActiveRequestException.php
│       ├── AsyncContextException.php
│       └── WorkerIdleException.php
└── functions.php
```

Зависимость времени выполнения не добавляется. Пакет устанавливается только в `require-dev`.

---

## Примеры

### Традиционный режим

```php
<?php
$request = oxphp_http_request();

$method = $request->method();         // "GET"
$path   = $request->path();           // "/api/articles"
$page   = $request->query('page', 1); // "2" или 1 (по умолчанию)

// Заголовок авторизации
if (!$request->hasHeader('Authorization')) {
    http_response_code(401);
    echo json_encode(['error' => 'Unauthorized']);
    exit;
}

$token = $request->header('Authorization');

// Структурированное логирование с метаданными запроса
error_log(sprintf(
    '[%s] %s %s from %s',
    oxphp_request_id(),
    $method,
    $path,
    $request->ip()
));
```

### POST с JSON-телом

```php
<?php
$request = oxphp_http_request();

if (!$request->isMethod('POST')) {
    http_response_code(405);
    exit;
}

$email    = $request->payload('email');
$password = $request->payload('password');

if (!$email || !$password) {
    http_response_code(400);
    echo json_encode(['error' => 'email and password are required']);
    exit;
}

// payload() обрабатывает JSON, form-urlencoded и multipart
// Ручной json_decode() и проверка $_POST не нужны
$user = authenticate($email, $password);

header('Content-Type: application/json');
echo json_encode(['token' => $user->generateToken()]);
```

### Атрибуты промежуточного слоя

```php
<?php
// auth-middleware.php
function authenticate_request(\OxPHP\Http\RequestInterface $request): void
{
    $token = $request->header('Authorization');
    if (!$token) {
        http_response_code(401);
        exit;
    }

    $user = verify_token(str_replace('Bearer ', '', $token));
    if (!$user) {
        http_response_code(403);
        exit;
    }

    $request->attributes()->set('user', $user);
}

// route-handler.php
$request = oxphp_http_request();
authenticate_request($request);

$user = $request->attributes()->get('user');
echo json_encode(['id' => $user->id, 'name' => $user->name]);
```

### Режим Worker с сессией

```php
<?php
// worker.php
require __DIR__ . '/vendor/autoload.php';

oxphp_worker(function (\OxPHP\Http\RequestInterface $request) {
    if ($request->path() === '/login' && $request->isMethod('POST')) {
        $username = $request->payload('username');
        $password = $request->payload('password');

        if (verify_credentials($username, $password)) {
            session_start();
            $_SESSION['user'] = $username;
            $_SESSION['authenticated'] = true;
            header('Location: /dashboard');
        } else {
            http_response_code(401);
            echo 'Invalid credentials';
        }
        return;
    }

    if ($request->path() === '/dashboard') {
        session_start();
        $session = $request->session();

        if (!$session || !$session->get('authenticated')) {
            header('Location: /login');
            return;
        }

        echo 'Welcome, ' . htmlspecialchars($session->get('user'));
    }
});
```

---

## См. также

- [Суперглобальные переменные](superglobals.md) — как OxPHP заполняет `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE` и `$_FILES`
- [PHP-функции](functions.md) — полный справочник по `oxphp_http_request()`, `oxphp_superglobals_enabled()` и всем остальным встроенным функциям
- [Режим Worker](../features/worker-mode.md) — постоянные PHP-процессы и жизненный цикл запроса
- [Справочник конфигурации](../operations/configuration.md) — `SUPERGLOBALS_ENABLED` и другие переменные окружения
