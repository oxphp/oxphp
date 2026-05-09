---
title: PHP-функции
description: Полный справочник всех PHP-функций oxphp_*, предоставляемых OxPHP, включая асинхронные функции, потоковую передачу, режим worker и API декораторов.
---

# PHP-функции

OxPHP регистрирует свои функции через расширение `oxphp_sapi`, которое загружается автоматически для каждого PHP-скрипта, выполняемого сервером. Директива `extension=` и ручная загрузка не требуются — каждая функция из этого списка доступна с первой строки вашего PHP-кода.

## Содержание

- [oxphp_http_request()](#oxphp_http_request)
- [oxphp_superglobals_enabled()](#oxphp_superglobals_enabled)
- [oxphp_request_id()](#oxphp_request_id)
- [oxphp_worker_id()](#oxphp_worker_id)
- [oxphp_server_info()](#oxphp_server_info)
- [oxphp_finish_request()](#oxphp_finish_request)
- [oxphp_is_worker()](#oxphp_is_worker)
- [oxphp_worker()](#oxphp_worker)
- [oxphp_is_streaming()](#oxphp_is_streaming)
- [oxphp_stream_flush()](#oxphp_stream_flush)
- [oxphp_sleep()](#oxphp_sleep)
- [oxphp_usleep()](#oxphp_usleep)
- [oxphp_async()](#oxphp_async)
- [oxphp_async_await()](#oxphp_async_await)
- [oxphp_async_await_all()](#oxphp_async_await_all)
- [oxphp_async_await_any()](#oxphp_async_await_any)
- [oxphp_register_decorator()](#oxphp_register_decorator)
- [oxphp_apm_trace()](#oxphp_apm_trace)
- [oxphp_apm_start()](#oxphp_apm_start)
- [oxphp_apm_end()](#oxphp_apm_end)
- [oxphp_apm_attribute()](#oxphp_apm_attribute)
- [oxphp_apm_event()](#oxphp_apm_event)
- [oxphp_apm_error()](#oxphp_apm_error)
- [oxphp_apm_status()](#oxphp_apm_status)
- [oxphp_apm_trace_id()](#oxphp_apm_trace_id)
- [oxphp_apm_span_id()](#oxphp_apm_span_id)
- [oxphp_apm_header()](#oxphp_apm_header)
- [Классы и интерфейсы](#классы-и-интерфейсы)
- [Исключения](#исключения)

---

## oxphp_http_request()

```php
oxphp_http_request(): \OxPHP\Http\Request
```

Возвращает объект запроса для текущего HTTP-запроса. Объект предоставляет типизированный доступ к методу HTTP, URI, параметрам строки запроса, разобранному телу, заголовкам, кукам, загруженным файлам, IP-адресу клиента и времени запроса.

**Возвращает:** Экземпляр `\OxPHP\Http\Request`, подкреплённый данными запроса из текущего PHP-воркер-потока.

**Выбрасывает:** Исключение из пространства имён `OxPHP\Http\Exception` при вызове вне активного запроса:

| Исключение | Ситуация |
|------------|----------|
| `\OxPHP\Http\Exception\WorkerIdleException` | Режим worker, между запросами |
| `\OxPHP\Http\Exception\AsyncContextException` | Внутри колбэка `oxphp_async()` |
| `\OxPHP\Http\Exception\NoActiveRequestException` | Любой другой контекст без активного запроса |

В обычном коде обработки запросов обработка исключений не требуется.

**Пример:**

```php
<?php
$request = oxphp_http_request();

$method  = $request->method();             // "POST"
$path    = $request->path();               // "/api/users"
$email   = $request->payload('email');     // из JSON или тела формы
$token   = $request->header('Authorization');
$theme   = $request->cookie('theme', 'light');
```

Полный справочник по интерфейсу см. в документации [HTTP Object API](request-api.md).

---

## oxphp_superglobals_enabled()

```php
oxphp_superglobals_enabled(): bool
```

Возвращает, включено ли заполнение суперглобальных переменных для данного экземпляра сервера. Значение отражает переменную окружения `SUPERGLOBALS_ENABLED` и не изменяется в течение жизни сервера.

При значении `false` переменные `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES` и `$_SERVER` являются пустыми массивами. HTTP Object API (`oxphp_http_request()`), `php://input` и функции сессий PHP при этом не затрагиваются.

**Возвращает:** `true`, если `SUPERGLOBALS_ENABLED` равно `true` (по умолчанию), `false` в противном случае.

**Пример:**

```php
<?php
if (oxphp_superglobals_enabled()) {
    $query = $_GET['page'] ?? 1;
} else {
    $query = oxphp_http_request()->query('page', 1);
}
```

---

## oxphp_request_id()

```php
oxphp_request_id(): string
```

Возвращает уникальный идентификатор текущего запроса. Это то же значение, которое отправляется в заголовке ответа `X-Request-ID`. Если клиент передаёт заголовок `X-Request-ID`, OxPHP пропускает его без изменений вместо того, чтобы генерировать новый.

**Возвращает:** 20-символьную шестнадцатеричную строку, когда OxPHP генерирует идентификатор (например, `"67890abc12341a2b0042"`). Когда клиент отправляет заголовок `X-Request-ID`, это значение возвращается как есть (1–64 символа: буквы, цифры, `-`, `_`, `.`).

**Пример:**

```php
<?php
$id = oxphp_request_id();
error_log("[$id] Processing order #1234");

// Передаём идентификатор в downstream-сервисы
header("X-Correlation-ID: $id");
```

---

## oxphp_worker_id()

```php
oxphp_worker_id(): int
```

Возвращает индекс (начиная с нуля) PHP-воркера, обрабатывающего текущий запрос. Индексы воркеров находятся в диапазоне от `0` до `PHP_WORKERS - 1`.

**Возвращает:** Целое число, идентифицирующее текущий воркер-поток.

**Пример:**

```php
<?php
$workerId = oxphp_worker_id();

// Используем временные файлы для каждого воркера, чтобы избежать конфликтов
$tmp = "/tmp/worker_{$workerId}_buffer.dat";

error_log("Worker $workerId handling request");
```

---

## oxphp_server_info()

```php
oxphp_server_info(): array
```

Возвращает ассоциативный массив с метаданными сервера и запроса.

**Возвращает:** Массив со следующими ключами:

| Ключ | Тип | Описание |
|------|-----|----------|
| `version` | `string` | Версия сервера (например, `"0.1.0"`) |
| `worker_id` | `int` | То же значение, что и `oxphp_worker_id()` |
| `request_time` | `float` | Unix-timestamp с точностью до микросекунды в момент начала запроса |
| `worker_mode` | `bool` | Выполняется ли текущий процесс в режиме worker |

**Пример:**

```php
<?php
$info = oxphp_server_info();
// [
//     "version"      => "0.1.0",
//     "worker_id"    => 3,
//     "request_time" => 1738800000.123456,
//     "worker_mode"  => true,
// ]

$elapsed = microtime(true) - $info['request_time'];
echo "Processing took {$elapsed}s so far";
```

---

## oxphp_finish_request()

```php
oxphp_finish_request(): bool
```

Отправляет ответ клиенту и продолжает выполнение PHP в фоновом режиме. Клиент немедленно получает полный HTTP-ответ; скрипт продолжает работать до естественного завершения. Это аналог `fastcgi_finish_request()` из PHP-FPM для OxPHP.

**Возвращает:** `true` при успехе, `false` если уже был вызван для этого запроса.

> **Примечание:** Воркер-поток PHP остаётся занятым до завершения скрипта. Сохраняйте фоновую работу короткой или переносите тяжёлые операции в очередь.

**Пример:**

```php
<?php
http_response_code(202);
echo json_encode(['status' => 'accepted']);
oxphp_finish_request();

// Клиент уже получил ответ 202; продолжаем работу
send_notification_email($user);
update_analytics($event);
```

---

## oxphp_is_worker()

```php
oxphp_is_worker(): bool
```

Возвращает, работает ли сервер в режиме worker. Режим воркера активируется, когда `WORKER_MODE_ENABLED=true`.

**Возвращает:** `true` в режиме worker, `false` в традиционном режиме.

**Пример:**

```php
<?php
if (oxphp_is_worker()) {
    // Повторно использовать постоянные соединения между запросами
    $db = $GLOBALS['db'] ??= new PDO($dsn);
} else {
    // Традиционный режим: создавать новое соединение для каждого запроса
    $db = new PDO($dsn);
}
```

---

## oxphp_worker()

```php
oxphp_worker(callable $handler): bool
```

Запускает постоянный цикл обработки в режиме worker. OxPHP вызывает `$handler` один раз для каждого входящего HTTP-запроса. Между запросами происходит мягкий сброс: очищаются буферы вывода, заголовки и суперглобальные переменные — без уничтожения PHP-кучи, поэтому переменные, объявленные вне обработчика, сохраняются между запросами.

**Параметры:**
- `$handler` — Вызывается один раз для каждого запроса. Обработчик не получает аргументов. Используйте суперглобальные переменные (`$_SERVER`, `$_GET`, `$_POST` и т.д.) или `oxphp_http_request()` внутри обработчика для доступа к данным запроса.

**Возвращает:** `true` при штатном завершении работы, `false` если режим worker не активен.

Цикл worker завершается при выполнении одного из следующих условий:
- Штатное завершение работы сервера
- Обработчик генерирует 3 последовательных необработанных исключения или фатальные ошибки
- Воркер превышает `WORKER_MAX_MEMORY_MIB`
- Приложение вызвало [`Worker::scheduleExit()`](worker-class.md#scheduleexit)

> **Примечание:** `oxphp_worker()` работает только в режиме воркера (`WORKER_MODE_ENABLED=true`). В традиционном режиме функция выводит предупреждение в лог и возвращает `false`.

**Пример:**

```php
<?php
// worker.php — выполняется один раз за время жизни воркер-процесса

// Инициализация: выполняется один раз при запуске
require __DIR__ . '/vendor/autoload.php';
$app = new App();

// Обработка запросов в цикле
oxphp_worker(function () use ($app) {
    $app->handle();
});

// Код после oxphp_worker() выполняется при завершении работы
$app->terminate();
```

---

## oxphp_is_streaming()

```php
oxphp_is_streaming(): bool
```

Возвращает, активен ли режим потоковой передачи для текущего запроса. Режим потоковой передачи активируется при первом вызове `oxphp_stream_flush()` или автоматически, когда PHP устанавливает `Content-Type: text/event-stream`.

**Возвращает:** `true` если режим потоковой передачи активен, `false` в противном случае.

**Пример:**

```php
<?php
if (oxphp_is_streaming()) {
    echo "data: " . json_encode($event) . "\n\n";
    oxphp_stream_flush();
} else {
    echo json_encode($allData);
}
```

---

## oxphp_stream_flush()

```php
oxphp_stream_flush(): bool
```

Активирует режим потоковой передачи и отправляет клиенту буферизованный вывод в виде HTTP-чанка. При первом вызове HTTP-заголовки отправляются немедленно и начинается потоковая передача. Каждый последующий вызов отправляет вывод, накопленный с момента последнего сброса.

**Возвращает:** `true` при успехе, `false` если уже был вызван `oxphp_finish_request()`.

> **Примечание:** Режим потоковой передачи также активируется автоматически, когда PHP устанавливает `Content-Type: text/event-stream`. В этом случае можно использовать встроенную функцию PHP `flush()`, но сначала вызовите `ob_end_flush()`, чтобы обойти уровень буферизации вывода PHP.

**Пример:**

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

for ($i = 0; $i < 10; $i++) {
    echo "id: $i\n";
    echo "data: " . json_encode(['counter' => $i]) . "\n\n";
    oxphp_stream_flush();
    oxphp_sleep(1.0); // используйте oxphp_sleep вместо sleep — не блокирует воркер в режиме fiber
}
```

---

## oxphp_sleep()

```php
oxphp_sleep(float $seconds): void
```

Ожидает в течение указанного времени. Внутри обработчика режима worker, запущенного в файбере, этот вызов является кооперативным — он приостанавливает текущий файбер, позволяя обрабатывать другие запросы во время ожидания. Вне файбера функция откатывается к стандартному блокирующему `usleep()`.

**Параметры:**
- `$seconds` — Продолжительность ожидания в секундах. Допускаются дробные значения (например, `0.5` для 500 миллисекунд). Значения `0` и менее возвращаются немедленно.

**Возвращает:** `void`

**Пример:**

```php
<?php
oxphp_worker(function () {
    // В режиме worker с мультиплексированием файберов:
    // приостанавливает файбер, не блокируя поток
    oxphp_sleep(1.0);
    echo json_encode(['done' => true]);
});
```

---

## oxphp_usleep()

```php
oxphp_usleep(int $microseconds): void
```

Ожидает указанное количество микросекунд. Как и `oxphp_sleep()`, является кооперативным внутри файбера и откатывается к блокирующему `usleep()` в противном случае.

**Параметры:**
- `$microseconds` — Продолжительность ожидания в микросекундах. Значения `0` и менее возвращаются немедленно.

**Возвращает:** `void`

**Пример:**

```php
<?php
oxphp_worker(function () {
    // Проверяем условие каждые 100 мс, не блокируя другие запросы
    while (!$condition_met()) {
        oxphp_usleep(100_000);
    }
    echo "ready";
});
```

---

## oxphp_async()

```php
oxphp_async(Closure $closure, mixed ...$args): int
```

Отправляет замыкание на выполнение в выделенный асинхронный воркер-поток и немедленно возвращает идентификатор промиса. Вызывающий код продолжает выполнение, не ожидая завершения замыкания. Используйте `oxphp_async_await()` для получения результата.

**Параметры:**
- `$closure` — Пользовательское `Closure` для выполнения в асинхронном воркер-потоке
- `...$args` — Аргументы для передачи в замыкание. Принимаются только скалярные значения (`null`, `bool`, `int`, `float`, `string`) и массивы из скаляров. Объекты и ресурсы не могут передаваться между потоками.

**Возвращает:** Целочисленный идентификатор промиса. Передайте его в `oxphp_async_await()`, `oxphp_async_await_all()` или `oxphp_async_await_any()`.

**Выбрасывает:** `OxPHP\Async\AsyncException` в следующих случаях:
- Асинхронный пул отключён (`ASYNC_WORKERS=0`)
- Замыкание не является пользовательским (не определено пользователем)
- Пул асинхронных воркеров переполнен
- Аргументы содержат объекты или ресурсы

> **Примечание:** Переменные, захваченные через `use` в замыкании, подпадают под те же ограничения — объекты и ресурсы отклоняются.

**Пример:**

```php
<?php
// Запускаем две независимые задачи параллельно
$p1 = oxphp_async(function () {
    return fetch_from_api('/users');
});

$p2 = oxphp_async(function () {
    return fetch_from_api('/posts');
});

// Получаем оба результата
$users = oxphp_async_await($p1);
$posts = oxphp_async_await($p2);
```

---

## oxphp_async_await()

```php
oxphp_async_await(int $promise_id, float $timeout = 0.0): mixed
```

Блокирует выполнение до завершения указанного асинхронного промиса и возвращает его результат. Внутри файбера в режиме worker приостанавливает текущий файбер кооперативно, не блокируя поток.

**Параметры:**
- `$promise_id` — Идентификатор промиса, возвращённый `oxphp_async()`
- `$timeout` — Максимальное время ожидания в секундах. `0.0` означает ждать бесконечно. По умолчанию: `0.0`

**Возвращает:** Возвращаемое значение асинхронного замыкания.

**Выбрасывает:**
- `OxPHP\Async\AsyncException` если асинхронный пул отключён (`ASYNC_WORKERS=0`)
- `OxPHP\Async\AsyncException` если асинхронная задача выбросила исключение
- `OxPHP\Async\TimeoutException` если превышен `$timeout`

**Пример:**

```php
<?php
$promise = oxphp_async(function (int $n) {
    return array_sum(range(1, $n));
}, 1_000_000);

$result = oxphp_async_await($promise);
echo $result; // 500000500000

// С тайм-аутом
try {
    $result = oxphp_async_await($promise, 5.0);
} catch (\OxPHP\Async\TimeoutException $e) {
    echo "Task took too long";
}
```

---

## oxphp_async_await_all()

```php
oxphp_async_await_all(array $promise_ids, float $timeout = 0.0): array
```

Ожидает завершения всех промисов из массива и возвращает ассоциативный массив, где каждый идентификатор промиса сопоставлен с его результатом. Промисы ожидаются в порядке элементов массива.

**Параметры:**
- `$promise_ids` — Массив целочисленных идентификаторов промисов, возвращённых `oxphp_async()`
- `$timeout` — Максимальное время ожидания на каждый промис в секундах. `0.0` означает ждать бесконечно. По умолчанию: `0.0`

**Возвращает:** Ассоциативный массив, где каждый ключ — идентификатор промиса (целое число), а значение — результат этого промиса.

**Выбрасывает:**
- `OxPHP\Async\AsyncException` если асинхронный пул отключён (`ASYNC_WORKERS=0`)
- `OxPHP\Async\AsyncException` если какой-либо промис завершился с ошибкой
- `OxPHP\Async\TimeoutException` если какой-либо промис превысил `$timeout`

**Пример:**

```php
<?php
$promises = [
    oxphp_async(fn() => slow_query('users')),
    oxphp_async(fn() => slow_query('orders')),
    oxphp_async(fn() => slow_query('products')),
];

$results = oxphp_async_await_all($promises);

foreach ($results as $promiseId => $result) {
    // обрабатываем $result
}
```

---

## oxphp_async_await_any()

```php
oxphp_async_await_any(array $promise_ids, float $timeout = 0.0): array
```

Запускает гонку нескольких промисов и возвращает первый завершившийся. Остальные промисы не отменяются — они продолжают выполнение и остаются доступными через `oxphp_async_await()`.

**Параметры:**
- `$promise_ids` — Массив из как минимум одного целочисленного идентификатора промиса, возвращённого `oxphp_async()`. Не может быть пустым.
- `$timeout` — Максимальное время ожидания завершения любого промиса в секундах. `0.0` означает ждать бесконечно. По умолчанию: `0.0`

**Возвращает:** Ассоциативный массив с двумя ключами:
- `id` (`int`) — Идентификатор промиса-победителя
- `value` (`mixed`) — Возвращаемое значение промиса-победителя

**Выбрасывает:**
- `OxPHP\Async\AsyncException` если асинхронный пул отключён (`ASYNC_WORKERS=0`)
- `OxPHP\Async\AsyncException` если промис-победитель завершился с ошибкой
- `OxPHP\Async\TimeoutException` если ни один промис не завершился в течение `$timeout`

**Пример:**

```php
<?php
// Обращаемся к двум зеркальным эндпоинтам; используем тот, что ответит первым
$p1 = oxphp_async(fn() => fetch('https://mirror-1.example.com/data'));
$p2 = oxphp_async(fn() => fetch('https://mirror-2.example.com/data'));

$winner = oxphp_async_await_any([$p1, $p2], timeout: 10.0);
echo "Mirror {$winner['id']} won: " . json_encode($winner['value']);
```

---

## oxphp_register_decorator()

```php
oxphp_register_decorator(string $class): bool
```

Регистрирует PHP-класс в качестве декоратора, который оборачивает вызовы функций и методов. Класс должен реализовывать `OxPHP\Decorator\AttributeInterface`. После регистрации OxPHP вызывает хуки `before()` и `after()` декоратора вокруг каждого вызова функции или метода, соответствующего целям `#[Attribute]` декоратора.

**Параметры:**
- `$class` — Полное имя класса декоратора для регистрации

**Возвращает:** `true` при успехе, `false` если класс не существует или не реализует `OxPHP\Decorator\AttributeInterface`.

**Пример:**

```php
<?php
use OxPHP\Decorator\AttributeInterface;
use OxPHP\Decorator\Context;

#[\Attribute(\Attribute::TARGET_METHOD)]
class LogDecorator implements AttributeInterface
{
    public function before(Context $ctx): void
    {
        error_log("Calling {$ctx->target} (request {$ctx->requestId})");
    }

    public function after(Context $ctx): void
    {
        error_log("Finished {$ctx->target}");
    }
}

// Регистрируем один раз при инициализации (или при запуске воркера)
oxphp_register_decorator(LogDecorator::class);
```

---

## oxphp_apm_trace()

```php
oxphp_apm_trace(string $name, callable $callback, ?array $attributes = null): void
```

Выполняет колбэк внутри именованного спана. Спан открывается перед запуском колбэка и закрывается после его возврата. Зарезервирована для будущей расширенной интеграции с колбэками.

**Параметры:**
- `$name` — Имя спана
- `$callback` — Вызываемый объект для выполнения внутри спана
- `$attributes` — Необязательный ассоциативный массив строковых пар ключ-значение атрибутов

**Возвращает:** `void`

---

## oxphp_apm_start()

```php
oxphp_apm_start(string $name, ?array $attributes = null): int
```

Открывает новый спан и возвращает локальный идентификатор для последующего обращения. Спан становится дочерним по отношению к текущему активному спану (или корневому спану запроса, если нет активного спана). Используйте `oxphp_apm_end()` для его закрытия.

**Параметры:**
- `$name` — Имя спана (например, `"cache.warm"`, `"payment.process"`)
- `$attributes` — Необязательный ассоциативный массив строковых пар ключ-значение атрибутов для установки на спане при создании

**Возвращает:** Целочисленный локальный идентификатор спана. Передайте его в `oxphp_apm_end()`, `oxphp_apm_attribute()` или другие функции, принимающие `$span_id`. Возвращает `0`, когда APM отключён.

**Пример:**

```php
<?php
$spanId = oxphp_apm_start('order.validate', [
    'order.type' => 'subscription',
]);

validateOrder($order);

oxphp_apm_end($spanId);
```

---

## oxphp_apm_end()

```php
oxphp_apm_end(int $span_id): void
```

Закрывает спан, открытый `oxphp_apm_start()`. Записывается время окончания спана, и он перемещается из активного стека в список завершённых, готовый к экспорту.

**Параметры:**
- `$span_id` — Локальный идентификатор спана, возвращённый `oxphp_apm_start()`

**Возвращает:** `void`

> **Примечание:** Всегда закрывайте спаны в обратном порядке. Если вы открыли спан A, а затем спан B, закройте B перед A. Незакрытые спаны автоматически закрываются при завершении запроса и помечаются атрибутом `oxphp.span.leaked=true`.

---

## oxphp_apm_attribute()

```php
oxphp_apm_attribute(string $key, mixed $value, ?int $span_id = null): void
```

Устанавливает атрибут «ключ-значение» на спане. Значения преобразуются в строки. Если `$span_id` не указан, атрибут добавляется к текущему активному спану.

**Параметры:**
- `$key` — Ключ атрибута (например, `"user.id"`, `"cache.hit"`)
- `$value` — Значение атрибута (string, int, float, bool или null — преобразуется в строку)
- `$span_id` — Необязательный локальный идентификатор спана. При пропуске применяется к текущему спану

**Возвращает:** `void`

**Пример:**

```php
<?php
$spanId = oxphp_apm_start('db.query');

oxphp_apm_attribute('db.system', 'mysql');
oxphp_apm_attribute('db.statement', 'SELECT * FROM users WHERE id = ?');
oxphp_apm_attribute('db.row_count', $rowCount, $spanId);

oxphp_apm_end($spanId);
```

---

## oxphp_apm_event()

```php
oxphp_apm_event(string $name, ?array $attributes = null, ?int $span_id = null): void
```

Записывает событие с временной меткой на спане. События полезны для логирования дискретных происшествий в пределах жизни спана (например, промах кэша, повторная попытка, проверка авторизации).

**Параметры:**
- `$name` — Имя события (например, `"cache.miss"`, `"retry"`)
- `$attributes` — Необязательный ассоциативный массив строковых пар ключ-значение атрибутов события
- `$span_id` — Необязательный локальный идентификатор спана. При пропуске применяется к текущему спану

**Возвращает:** `void`

**Пример:**

```php
<?php
$spanId = oxphp_apm_start('payment.process');

oxphp_apm_event('payment.authorized', [
    'provider' => 'stripe',
    'amount' => '49.99',
]);

oxphp_apm_end($spanId);
```

---

## oxphp_apm_error()

```php
oxphp_apm_error(mixed $exception, ?int $span_id = null): void
```

Помечает статус спана как ошибку (код статуса 2). Используйте эту функцию для пометки спанов, в которых произошло исключение или сбой.

**Параметры:**
- `$exception` — Исключение или ошибка (используется для контекста; статус устанавливается вне зависимости от типа)
- `$span_id` — Необязательный локальный идентификатор спана. При пропуске применяется к текущему спану

**Возвращает:** `void`

**Пример:**

```php
<?php
$spanId = oxphp_apm_start('external.api');

try {
    $result = callExternalApi();
} catch (\Throwable $e) {
    oxphp_apm_error($e, $spanId);
    throw $e;
} finally {
    oxphp_apm_end($spanId);
}
```

---

## oxphp_apm_status()

```php
oxphp_apm_status(int $code, ?string $description = null, ?int $span_id = null): void
```

Устанавливает код статуса и необязательное описание на спане.

**Параметры:**
- `$code` — Код статуса: `0` = Unset, `1` = Ok, `2` = Error
- `$description` — Необязательное человекочитаемое описание статуса
- `$span_id` — Необязательный локальный идентификатор спана. При пропуске применяется к текущему спану

**Возвращает:** `void`

**Пример:**

```php
<?php
$spanId = oxphp_apm_start('validation');

if ($valid) {
    oxphp_apm_status(1, 'Validation passed', $spanId);
} else {
    oxphp_apm_status(2, 'Invalid input: missing email', $spanId);
}

oxphp_apm_end($spanId);
```

---

## oxphp_apm_trace_id()

```php
oxphp_apm_trace_id(): string
```

Возвращает W3C trace ID (32 шестнадцатеричных символа) для контекста трассировки текущего запроса. Это то же значение, что и `$_SERVER['OXPHP_TRACE_ID']`, доступное без суперглобальных переменных.

**Возвращает:** 32-символьную шестнадцатеричную строку trace ID. Возвращает пустую строку, когда APM отключён или контекст трассировки не активен.

**Пример:**

```php
<?php
$traceId = oxphp_apm_trace_id();
error_log("Processing request in trace {$traceId}");
```

---

## oxphp_apm_span_id()

```php
oxphp_apm_span_id(): string
```

Возвращает span ID (16 шестнадцатеричных символов) текущего активного спана. При наличии вложенных спанов возвращает идентификатор самого внутреннего открытого спана.

**Возвращает:** 16-символьную шестнадцатеричную строку span ID. Возвращает пустую строку, когда нет активного спана.

---

## oxphp_apm_header()

```php
oxphp_apm_header(): string
```

Возвращает значение заголовка W3C `traceparent` для текущего контекста спана. Используйте для передачи контекста трассировки в нисходящие HTTP-вызовы.

**Возвращает:** Строку в формате `00-{trace_id}-{span_id}-01`. Возвращает пустую строку, когда контекст трассировки не активен.

**Пример:**

```php
<?php
$spanId = oxphp_apm_start('http.call');

$traceparent = oxphp_apm_header();

$response = file_get_contents('https://api.example.com/data', false,
    stream_context_create([
        'http' => [
            'header' => "traceparent: {$traceparent}\r\n",
        ],
    ])
);

oxphp_apm_end($spanId);
```

---

## Классы и интерфейсы

Расширение `oxphp_sapi` регистрирует следующие классы:

### HTTP

| Класс | Описание |
|-------|----------|
| `OxPHP\Http\Request` | Объект запроса, возвращаемый `oxphp_http_request()`. `final` — нельзя наследовать. |
| `OxPHP\Http\Attributes` | Мутабельный контейнер атрибутов запроса (для middleware). `final`. |
| `OxPHP\Http\Session` | Объект сессии, доступный через `$request->session()`. `final`. |
| `OxPHP\Http\UploadedFile` | Объект загруженного файла из `$request->files()`. `final`. |

### Декораторы

| Класс / Интерфейс | Описание |
|--------------------|----------|
| `OxPHP\Decorator\AttributeInterface` | Интерфейс для декораторов. Требует методы `before(Context $ctx)` и `after(Context $ctx)`. |
| `OxPHP\Decorator\Context` | Объект контекста, передаваемый в хуки декоратора. `final`. Содержит `target`, `requestId`, аргументы и возвращаемое значение. |

### Трассировка

| Класс | Описание |
|-------|----------|
| `OxPHP\Apm\Trace` | Встроенный атрибут для автоматического создания спанов. Применяется к функциям или методам. |

### Async

| Класс | Описание |
|-------|----------|
| `OxPHP\Async\BorrowedProxy` | Прокси-объект для заимствованных значений между потоками. |

---

## Исключения

Все исключения, зарегистрированные расширением:

| Исключение | Наследует | Когда выбрасывается |
|------------|-----------|---------------------|
| `OxPHP\Async\AsyncException` | `\Exception` | Ошибка в асинхронной задаче (`oxphp_async_await()`) или невалидные аргументы в `oxphp_async()` |
| `OxPHP\Async\TimeoutException` | `OxPHP\Async\AsyncException` | Превышен таймаут в `oxphp_async_await()`, `oxphp_async_await_all()` или `oxphp_async_await_any()` |
| `OxPHP\Async\BorrowException` | `\Exception` | Ошибка заимствования значения между потоками |
| `OxPHP\Http\Exception\NoActiveRequestException` | `\RuntimeException` | Вызов `oxphp_http_request()` вне активного запроса |
| `OxPHP\Http\Exception\AsyncContextException` | `NoActiveRequestException` | Вызов `oxphp_http_request()` внутри колбэка `oxphp_async()` |
| `OxPHP\Http\Exception\WorkerIdleException` | `NoActiveRequestException` | Вызов `oxphp_http_request()` в режиме worker между запросами |
| `OxPHP\Decorator\RejectedException` | `\Exception` | Декоратор отклонил вызов функции/метода |

---

## Проверка расширения

Вы можете убедиться, что расширение OxPHP загружено, и просмотреть все зарегистрированные функции:

```php
<?php
if (extension_loaded('oxphp_sapi')) {
    echo "OxPHP extension is loaded\n";
}

$functions = get_extension_funcs('oxphp_sapi');
print_r($functions);
// Array
// (
//     [0]  => oxphp_http_request
//     [1]  => oxphp_superglobals_enabled
//     [2]  => oxphp_request_id
//     [3]  => oxphp_worker_id
//     [4]  => oxphp_server_info
//     [5]  => oxphp_finish_request
//     [6]  => oxphp_is_worker
//     [7]  => oxphp_is_streaming
//     [8]  => oxphp_stream_flush
//     [9]  => oxphp_sleep
//     [10] => oxphp_usleep
//     [11] => oxphp_worker
//     [12] => oxphp_async
//     [13] => oxphp_async_await
//     [14] => oxphp_async_await_all
//     [15] => oxphp_async_await_any
//     [16] => oxphp_register_decorator
//     [17] => oxphp_apm_trace
//     [18] => oxphp_apm_start
//     [19] => oxphp_apm_end
//     [20] => oxphp_apm_attribute
//     [21] => oxphp_apm_event
//     [22] => oxphp_apm_error
//     [23] => oxphp_apm_status
//     [24] => oxphp_apm_trace_id
//     [25] => oxphp_apm_span_id
//     [26] => oxphp_apm_header
// )
```

## Совместимость с PHP-FPM

> **Примечание:** `function_exists('oxphp_async')` возвращает `true` даже при отключённом асинхронном пуле (`ASYNC_WORKERS=0`). Функция зарегистрирована всегда — она лишь выбрасывает `OxPHP\Async\AsyncException` при вызове. Для проверки доступности фонового выполнения используйте `oxphp_server_info()['async_workers'] > 0`, а не `function_exists()`.

Если ваш код должен работать как в OxPHP, так и в PHP-FPM, используйте обёртки с откатом:

```php
<?php
function finish_request(): bool
{
    if (function_exists('oxphp_finish_request')) {
        return oxphp_finish_request();
    }
    if (function_exists('fastcgi_finish_request')) {
        return fastcgi_finish_request();
    }
    return false;
}

// Инициализация с поддержкой режима worker
if (function_exists('oxphp_is_worker') && oxphp_is_worker()) {
    // Режим worker OxPHP
} else {
    // PHP-FPM или традиционный режим OxPHP
}
```

## См. также

- [HTTP Object API](request-api.md) — объектно-ориентированный доступ к данным запроса через `oxphp_http_request()`
- [Режим Worker](../features/worker-mode.md) — постоянный цикл воркера и жизненный цикл запроса
- [Server-Sent Events](../features/sse.md) — потоковая передача в реальном времени с `oxphp_stream_flush()`
- [Ранний ответ](../features/early-response.md) — фоновая обработка с `oxphp_finish_request()`
- [Суперглобальные переменные](superglobals.md) — как OxPHP заполняет `$_SERVER`, `$_GET`, `$_POST` и другие суперглобальные переменные
- [Распределённая трассировка и APM](../features/distributed-tracing.md) — W3C Trace Context, экспорт OTel и SDK `oxphp_apm_*()`
- [Справочник по конфигурации](../operations/configuration.md) — `WORKER_MODE_ENABLED`, `ENTRY_FILE`, `PHP_WORKERS` и другие переменные окружения
