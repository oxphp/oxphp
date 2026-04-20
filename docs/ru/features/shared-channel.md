---
title: Shared\Channel
description: Ограниченный MPMC-канал, разделяемый между PHP-воркерами, с fiber-aware send и recv для кооперативных producer/consumer пайплайнов.
---

# Shared\Channel

`OxPHP\Shared\Channel` — это ограниченный multi-producer multi-consumer канал, живущий в общем реестре и видимый каждому PHP-воркеру в процессе. Используйте его, когда обработчик запроса и фоновый воркер — или два воркера — должны обмениваться элементами работы в порядке FIFO. Внутри файбера `send` и `recv` кооперативно приостанавливаются, так что нижележащий поток воркера остаётся свободным для обработки других запросов.

## Обзор

- **Ограниченный.** Ёмкость фиксируется при конструировании. Когда полон, `send` блокирует или приостанавливает; `trySend` возвращает `false`.
- **MPMC.** Произвольное число отправителей и получателей между потоками. Доставка FIFO.
- **Fiber-aware.** В worker mode с async-пулом `send`/`recv` приостанавливают файбер вместо блокировки потока воркера. В традиционном режиме блокируют OS-поток.
- **Подкреплён реестром.** Каналы переживают границы запросов и разделяются по ID. Close распространяется на всех держателей.

## Справочник API

```php
namespace OxPHP\Shared;

final class Channel implements Shareable
{
    public function __construct(int $capacity);

    public function send(mixed $value, float $timeout = 0.0): void;
    public function trySend(mixed $value): bool;

    public function recv(float $timeout = 0.0): mixed;
    public function tryRecv(): mixed;

    public function close(): void;
    public function isClosed(): bool;
    public function pending(): int;

    public function sendMany(array $values, float $timeout = 0.0): int;
    public function recvMany(int $max, float $timeout = 0.0): array;

    public function id(): int;
}
```

| Метод         | Применение                                                                           |
|---------------|--------------------------------------------------------------------------------------|
| `send`        | Положить один элемент, ожидая (или приостанавливая файбер) до `$timeout` свободного места. |
| `trySend`     | Положить один элемент без ожидания; возвращает `false`, если полон или закрыт.       |
| `recv`        | Забрать один элемент, ожидая до `$timeout`. Возвращает `null` при closed+empty или таймауте. |
| `tryRecv`     | Забрать один элемент без ожидания; возвращает `null`, если пуст; бросает при closed+empty. |
| `close`       | Пометить канал закрытым. Идемпотентно. Будит всех заблокированных отправителей/получателей. |
| `isClosed`    | Сообщает, был ли канал закрыт.                                                       |
| `pending`     | Ориентировочное количество буферизованных элементов прямо сейчас. Полезно для метрик/backpressure. |
| `sendMany`    | Положить массив элементов; возвращает, сколько реально вошло до full/closed/timeout. |
| `recvMany`    | Забрать до `$max` элементов (`0` = слить то, что буферизовано сейчас, без ожидания). |
| `id`          | Числовой идентификатор реестра; полезен для логов и корреляции в наблюдаемости.      |

## Выбор между вариантами send/recv

Блокирующие и неблокирующие пары различаются в **том, что они возвращают, и в том, что они бросают**, и поведение намеренно асимметрично.

| Исход               | `send(v, t)`         | `trySend(v)` | `recv(t)`       | `tryRecv()`           |
|---------------------|----------------------|--------------|------------------|-----------------------|
| Успех               | возвращает `void`    | `true`       | элемент          | элемент               |
| Полон / пуст, открыт | ждёт до `t`          | `false`      | ждёт до `t`      | `null`                |
| Таймаут             | `TimeoutException`   | —            | `null`           | —                     |
| Закрыт (empty recv) | `ClosedException`    | `false`      | `null`           | `ClosedException`     |
| Закрыт (ещё есть элементы) | `ClosedException` | `false`    | возвращает элемент | возвращает элемент  |

Два следствия стоит запомнить:

1. **`recv` никогда не бросает на closed+empty.** Он возвращает `null`. Циклы должны проверять на null.
2. **`recv` также возвращает `null` на таймаут**, тогда как `send` бросает `TimeoutException`. Если нужно отличить «никто не отправил вовремя» от «канал закрыт», проверьте `isClosed()` после `null`-recv.

```php
<?php
$ch = new OxPHP\Shared\Channel(4);

// Неблокирующая проверка.
if (!$ch->trySend('job-1')) {
    // Очередь полна; сбросить, повторить или применить backpressure.
}

// Блокирующий send с дедлайном.
try {
    $ch->send('job-2', timeout: 1.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    // Ни один потребитель не подхватил за 1 с.
} catch (OxPHP\Shared\ClosedException $e) {
    // Канал закрыли, пока мы ждали.
}
```

## Поведение fiber vs блокирующее

Одни и те же вызовы методов ведут себя по-разному в зависимости от того, выполняется ли PHP сейчас внутри файбера:

- **Внутри файбера** (worker mode + `oxphp_async(...)`): `send` / `recv` выделяют синтетический promise, регистрируют waker в канале и приостанавливают файбер. Поток воркера возвращается к планировщику и обрабатывает другие файберы, пока канал не уведомит waker.
- **Вне файбера** (традиционный режим или неасинхронный путь вызова): `send` / `recv` блокируют OS-поток воркера через `crossbeam_channel`. Никакая другая работа на этом потоке не выполняется до возврата.

Традиционный режим всё равно получает семантику канала — он просто платит заблокированным потоком. Worker mode — рекомендуемый способ развёртывания для любого пайплайна, опирающегося на ожидание.

```php
<?php
// Традиционный режим: этот recv блокирует поток воркера до 2 секунд.
$ch = new OxPHP\Shared\Channel(16);
$item = $ch->recv(timeout: 2.0);

// Worker mode: обернуть в oxphp_async, и recv приостанавливается кооперативно.
oxphp_worker(function () use ($ch) {
    $consumer = oxphp_async(function () use ($ch) {
        while (($item = $ch->recv(timeout: 5.0)) !== null) {
            process($item);
        }
    });
    oxphp_async_await($consumer);
});
```

## Семантика close

`close()` идемпотентен — второй вызов — no-op. После close:

- `send` / `sendMany` бросают `ClosedException`.
- `trySend` возвращает `false`.
- `recv` продолжает сливать буферизованные элементы, затем возвращает `null`, когда пуст.
- `tryRecv` возвращает буферизованные элементы, затем бросает `ClosedException` на пустом.
- `isClosed()` возвращает `true`.
- Заблокированные отправители просыпаются с `ClosedException`; заблокированные получатели просыпаются с `null`.

```php
<?php
$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->close();

// Сливаем остатки.
while (($item = $ch->recv()) !== null) {
    echo $item, "\n"; // one, two
}

// Дальнейшие send отклоняются.
try {
    $ch->send('three');
} catch (OxPHP\Shared\ClosedException $e) {
    // ожидаемо
}
```

Паттерн для graceful-остановки пайплайна: producers останавливаются, сторона producer вызывает `close()`, consumers сливают в цикле `while (($item = $ch->recv()) !== null)` и естественно выходят.

## Слив при shutdown

Когда процесс OxPHP останавливается, реестр `OxPHP\Shared` вызывает `close()` на каждой записи, включая каналы. С точки зрения PHP это выглядит идентично явному `close()`:

- Заблокированные вызовы `recv` возвращают `null`.
- Заблокированные вызовы `send` бросают `ClosedException`.

> **Всегда проверяйте `recv` на null.** Вызывающий, трактующий возврат как non-null, упадёт при shutdown или когда другой держатель закроет канал. Стандартная идиома — `while (($item = $ch->recv(timeout: T)) !== null) { ... }`.

## Пакетные операции

`sendMany` и `recvMany` существуют для пайплайнов, перемещающих элементы группами. Предпочитайте их, когда вы регулярно обрабатываете 10+ элементов за раз: каждый пакет — один FFI round trip вместо N, что заметно снижает накладные расходы на элемент в циклах, упирающихся в пропускную способность.

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

// Отправить массив одним вызовом; возвращает, сколько реально буферизовано.
$sent = $ch->sendMany([1, 2, 3, 4, 5]);   // 5

// Слить до 10 элементов с дедлайном 100 мс.
$batch = $ch->recvMany(10, 0.1);

// max = 0 означает «слить то, что сейчас буферизовано, без ожидания».
$snapshot = $ch->recvMany(0);
```

Семантика, которую стоит отметить:

- `sendMany` на закрытом канале возвращает `0` (без исключения). Он не отправляет частичный пакет.
- `recvMany(0)` никогда не блокирует. Возвращает то, что сейчас буферизовано.
- Частичный возврат — нормально: если таймаут истекает во время приёма, вызов возвращает элементы, которые уже получил.

## Наблюдаемость

Внутренний сервер (по умолчанию `INTERNAL_ADDR=127.0.0.1:9090`) показывает каналы в общих эндпоинтах shared-реестра:

- **`GET /__ox_shared/summary`** включает бакет `Channel` с count, bytes, ops и `pending_total`.
- **`GET /__ox_shared/entries?type=Channel`** перечисляет записи каналов с их ID реестра.
- **`GET /__ox_shared/entries/:id`** возвращает per-channel состояние: `capacity`, `pending`, `closed`, `senders_blocked`, `receivers_blocked`.

Prometheus-экспозиция на `/metrics`:

```text
oxphp_shared_channel_pending{channel_id="<id>"}             gauge
oxphp_shared_channel_senders_blocked{channel_id="<id>"}     gauge
oxphp_shared_channel_receivers_blocked{channel_id="<id>"}   gauge
oxphp_shared_channel_items_sent_total{channel_id="<id>"}    counter
oxphp_shared_channel_items_dropped_total{channel_id="<id>"} counter
```

`items_dropped_total` инкрементируется для хвоста частичного `sendMany`, который не поместился.

## Распространённые паттерны

### HTTP-producer, async-consumer

Выставите очередь на канале и запустите воркер внутри async-пула:

```php
<?php
// worker.php (WORKER_FILE)
$work = new OxPHP\Shared\Channel(256);

$consumer = oxphp_async(function () use ($work) {
    while (($job = $work->recv(timeout: 30.0)) !== null) {
        process_job($job);
    }
});

oxphp_worker(function () use ($work) {
    $work->send(['url' => $_POST['url'], 'tries' => 3]);
    echo "queued";
});
```

### Fan-out на нескольких consumer'ов

Запустите N async-consumer'ов на одном канале; реестр гарантирует, что ровно один из них получит каждый элемент.

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

for ($i = 0; $i < 4; $i++) {
    oxphp_async(function () use ($ch, $i) {
        while (($job = $ch->recv(timeout: 60.0)) !== null) {
            handle($i, $job);
        }
    });
}
```

### Ограниченный пайплайн с backpressure

Использование `trySend` плюс счётчика сбросов позволяет producer'у сбрасывать нагрузку вместо блокировки при перегрузке:

```php
<?php
if (!$ch->trySend($event)) {
    increment_dropped_metric();
}
```

## Подводные камни

- **`timeout = 0.0` означает ждать бесконечно**, а не «вернуться немедленно». Используйте `trySend` / `tryRecv` для неблокирующих проверок. Это соответствует семантике `oxphp_async_await`.
- **Значения должны быть shareable.** Скаляры, `null` и вложенные массивы shareable-объектов разрешены. Передача объекта, который не является экземпляром `Shared\*`, поднимает `TypeException` при send.
- **Clone запрещён.** `clone $channel` бросает; передавайте канал через `use` замыкания — `oxphp_async(function () use ($ch) { ... })` — чтобы обе стороны видели одну и ту же запись реестра.
- **Всегда проверяйте `recv` на null.** Трактовка возврата как non-null ломается при shutdown, когда другой держатель закрывает канал, и при таймауте.
- **Неоднозначность таймаута vs close.** `recv` возвращает `null` для обоих. Если нужно их различить, вызывайте `isClosed()` после `null`-возврата.
- **Отменённые ожидающие с in-flight пэйлоадом.** Если много файберов ждут на `send` / `recv` и отменяются, когда их пэйлоад уже почти перешёл, пэйлоад может остаться со ссылкой до следующего пробуждения. Держите число ожидающих ограниченным (например, кэпайте конкуренцию через `Shared\Counter` или семафор из channel-capacity).

## Связанные возможности

- [Worker Mode](worker-mode.md) — предпосылка для fiber-приостанавливающих `send` / `recv`.
- [Async Promises](async-promises.md) — замыкание `oxphp_async()` — обычный способ передать `Channel` фоновому файберу.
- [Fiber Multiplexing](fiber-multiplexing.md) — объясняет, как приостановка держит поток воркера продуктивным, пока операции канала ждут.
