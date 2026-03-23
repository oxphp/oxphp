---
title: PHP-функции
description: Полный справочник всех PHP-функций oxphp_*, предоставляемых OxPHP, включая асинхронные функции, потоковую передачу, режим worker и API декораторов.
---

# PHP-функции

OxPHP регистрирует свои функции через расширение `oxphp_sapi`, которое загружается автоматически для каждого PHP-скрипта, выполняемого сервером. Директива `extension=` и ручная загрузка не требуются — каждая функция из этого списка доступна с первой строки вашего PHP-кода.

## Содержание

- [oxphp_request_id()](#oxphp_request_id)
- [oxphp_worker_id()](#oxphp_worker_id)
- [oxphp_server_info()](#oxphp_server_info)
- [oxphp_finish_request()](#oxphp_finish_request)
- [oxphp_request_heartbeat()](#oxphp_request_heartbeat)
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

---

## oxphp_request_id()

```php
oxphp_request_id(): string
```

Возвращает уникальный идентификатор текущего запроса. Это то же значение, которое отправляется в заголовке ответа `X-Request-ID`. Если клиент передаёт заголовок `X-Request-ID`, OxPHP пропускает его без изменений вместо того, чтобы генерировать новый.

**Возвращает:** 16-символьную шестнадцатеричную строку, когда OxPHP генерирует идентификатор (например, `"67b9a3c11a2b0042"`). Когда клиент отправляет заголовок `X-Request-ID`, это значение возвращается как есть (1–64 символа: буквы, цифры, `-`, `_`, `.`).

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
| `sapi` | `string` | Всегда `"oxphp"` |
| `version` | `string` | Версия сервера (например, `"0.1.0"`) |
| `worker_id` | `int` | То же значение, что и `oxphp_worker_id()` |
| `request_time` | `float` | Unix-timestamp с точностью до микросекунды в момент начала запроса |
| `worker_mode` | `bool` | Выполняется ли текущий процесс в режиме worker |

**Пример:**

```php
<?php
$info = oxphp_server_info();
// [
//     "sapi"         => "oxphp",
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

## oxphp_request_heartbeat()

```php
oxphp_request_heartbeat(int $time = 10): bool
```

Продлевает дедлайн `REQUEST_TIMEOUT_SECONDS` на `$time` секунд с момента вызова. Вызывайте эту функцию периодически в длительных циклах, чтобы OxPHP не прерывал запрос посреди обработки.

**Параметры:**
- `$time` — Количество секунд, на которое продлевается дедлайн тайм-аута. По умолчанию: `10`

**Возвращает:** `true` при успехе, `false` если `$time` равно нулю или отрицательно.

> **Примечание:** Каждый вызов устанавливает новый дедлайн относительно текущего момента, а не начала запроса. Вызов `oxphp_request_heartbeat(30)` на 100-й секунде при тайм-ауте 120 секунд устанавливает дедлайн через 130 секунд от текущего момента.

**Пример:**

```php
<?php
foreach ($large_dataset as $row) {
    oxphp_request_heartbeat(30); // продлить на 30 секунд от текущего момента
    process($row);
}
```

---

## oxphp_is_worker()

```php
oxphp_is_worker(): bool
```

Возвращает, работает ли сервер в режиме worker. Режим worker активируется, когда задана переменная `WORKER_FILE`.

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
- `$handler` — Вызывается один раз для каждого запроса. Обработчик не получает аргументов; используйте суперглобальные переменные (`$_SERVER`, `$_GET`, `$_POST` и т.д.) для данных запроса.

**Возвращает:** `true` при штатном завершении работы, `false` если режим worker не активен.

Цикл worker завершается при выполнении одного из следующих условий:
- Штатное завершение работы сервера
- Обработчик генерирует 3 последовательных необработанных исключения или фатальные ошибки
- Воркер достигает `WORKER_MAX_REQUESTS`
- Воркер превышает `WORKER_MAX_MEMORY_MIB`

> **Примечание:** `oxphp_worker()` работает только при настроенном `WORKER_FILE`. В традиционном режиме функция выводит предупреждение в лог и возвращает `false`.

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
    sleep(1);
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

**Выбрасывает:** `OxPHP\AsyncException` если замыкание не является пользовательским, если пул асинхронных воркеров переполнен или если аргументы содержат объекты или ресурсы.

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
- `OxPHP\AsyncException` если асинхронная задача выбросила исключение
- `OxPHP\AsyncTimeoutException` если превышен `$timeout`

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
} catch (\OxPHP\AsyncTimeoutException $e) {
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
- `OxPHP\AsyncException` если какой-либо промис завершился с ошибкой
- `OxPHP\AsyncTimeoutException` если какой-либо промис превысил `$timeout`

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
- `OxPHP\AsyncException` если промис-победитель завершился с ошибкой
- `OxPHP\AsyncTimeoutException` если ни один промис не завершился в течение `$timeout`

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
//     [0]  => oxphp_request_id
//     [1]  => oxphp_worker_id
//     [2]  => oxphp_server_info
//     [3]  => oxphp_request_heartbeat
//     [4]  => oxphp_finish_request
//     [5]  => oxphp_is_worker
//     [6]  => oxphp_is_streaming
//     [7]  => oxphp_stream_flush
//     [8]  => oxphp_sleep
//     [9]  => oxphp_usleep
//     [10] => oxphp_worker
//     [11] => oxphp_async
//     [12] => oxphp_async_await
//     [13] => oxphp_async_await_all
//     [14] => oxphp_async_await_any
//     [15] => oxphp_register_decorator
// )
```

## Совместимость с PHP-FPM

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

- [Режим Worker](../features/worker-mode.md) — постоянный цикл воркера и жизненный цикл запроса
- [Server-Sent Events](../features/sse.md) — потоковая передача в реальном времени с `oxphp_stream_flush()`
- [Ранний ответ](../features/early-response.md) — фоновая обработка с `oxphp_finish_request()`
- [Суперглобальные переменные](superglobals.md) — как OxPHP заполняет `$_SERVER`, `$_GET`, `$_POST` и другие суперглобальные переменные
- [Справочник по конфигурации](../operations/configuration.md) — `WORKER_FILE`, `PHP_WORKERS`, `REQUEST_TIMEOUT_SECONDS` и другие переменные окружения
