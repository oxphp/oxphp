---
title: Shared\Channel
description: Ограниченный MPMC-канал, разделяемый между PHP-воркерами, с fiber-aware send и recv, возвращающими типизированные Result-значения для явной диспетчеризации.
---

# Shared\Channel

`OxPHP\Shared\Channel` — это ограниченный multi-producer multi-consumer канал, живущий в общем реестре и видимый каждому PHP-воркеру в процессе. Используйте его, когда обработчик запроса и фоновый воркер — или два воркера — должны обмениваться элементами работы в порядке FIFO. Внутри файбера блокирующие вызовы кооперативно приостанавливаются, так что нижележащий поток воркера остаётся свободным для обработки других запросов.

## Обзор

- **Ограниченный.** Ёмкость фиксируется при конструировании. Когда полон, `send` блокирует (или приостанавливает файбер); `trySend` сообщает результат без ожидания.
- **MPMC.** Произвольное число отправителей и получателей между потоками. Доставка FIFO.
- **Fiber-aware.** В worker mode с async-пулом блокирующие варианты приостанавливают файбер вместо OS-потока. В традиционном режиме блокируют OS-поток.
- **Типизированный результат.** Каждый send/recv возвращает [`Channel\SendResult`](#Справочник-api) / [`Channel\RecvResult`](#Справочник-api) — closed / full / timeout это нормальные исходы для fan-out диспетчеров, поэтому они представлены вариантами результата, а не исключениями.

## Справочник API

```php
namespace OxPHP\Shared;

final class Channel implements Shareable, \Countable
{
    public function __construct(int $capacity);

    // ── Receive ──
    public function tryRecv(): Channel\RecvResult;                  // non-blocking
    public function recv(): Channel\RecvResult;                     // ждать вечно / fiber-cancel
    public function recvTimeout(int $ms): Channel\RecvResult;       // ограниченное ($ms > 0)

    // ── Send ──
    public function trySend(mixed $value): Channel\SendResult;
    public function send(mixed $value): Channel\SendResult;
    public function sendTimeout(mixed $value, int $ms): Channel\SendResult;

    // ── Batch ──
    public function sendMany(array $values, int $ms): int;          // частичный счёт, без throw
    public function recvMany(int $max, int $ms): array;             // частичный массив, без throw

    // ── Lifecycle ──
    public function close(): bool;
    public function isClosed(): bool;
    public function count(): int;
    public function id(): int;
}

namespace OxPHP\Shared\Channel;

enum RecvStatus { case Ok; case Empty; case Timeout; case Closed; }
enum SendStatus { case Ok; case Full;  case Timeout; case Closed; }

final class RecvResult {
    public function isOk(): bool;
    public function isEmpty(): bool;
    public function isTimeout(): bool;
    public function isClosed(): bool;
    public function value(): mixed;                // бросает SharedException, если не Ok
    public function valueOr(mixed $default): mixed;
    public function status(): RecvStatus;
}

final class SendResult {
    public function isOk(): bool;
    public function isFull(): bool;
    public function isTimeout(): bool;
    public function isClosed(): bool;
    public function status(): SendStatus;
}
```

## Wait-политики

Для каждого направления есть три варианта метода — суффикс кодирует wait-политику:

| Суффикс       | Поведение                                                            |
|---------------|----------------------------------------------------------------------|
| `try*`        | Non-blocking. Сообщает `Empty` / `Full`, если вызов не может продолжиться. |
| (голое имя)   | Ждать вечно или пока request fiber не будет отменён.                 |
| `*Timeout`    | Ограниченное ожидание. `$ms` обязан быть `> 0` — для «вечно» и «без ожидания» используйте другие формы. |

`$ms` — **всегда положительное целое в миллисекундах** для `recvTimeout` / `sendTimeout` / `sendMany` / `recvMany`. Ноль, отрицательные, не-int и отсутствующие значения поднимают `OxPHP\Shared\TypeException` на бридже — это ограничение вынесено из тела метода, чтобы трихотомия была самодокументирующейся.

### Достижимые варианты результата по методам

`SendResult::Full` и `RecvResult::Empty` эмитятся только non-blocking `try*`-вызовами — блокирующий вариант либо получает слот/элемент, либо исчерпывает бюджет, и тогда результат — `Timeout`, а не `Full` / `Empty`. Полная матрица достижимости:

| Метод           | `Ok` | `Full` / `Empty` | `Timeout` | `Closed` |
|-----------------|------|------------------|-----------|----------|
| `trySend`       | ✓    | `Full` ✓         | —         | ✓        |
| `send`          | ✓    | —                | —         | ✓        |
| `sendTimeout`   | ✓    | —                | ✓         | ✓        |
| `tryRecv`       | ✓    | `Empty` ✓        | —         | ✓        |
| `recv`          | ✓    | —                | —         | ✓        |
| `recvTimeout`   | ✓    | —                | ✓         | ✓        |

Проверки `isFull()` / `isEmpty()` на результате блокирующего вызова — мёртвый код. `match`-примеры ниже оставляют эти ветви с комментариями `unreachable`, чтобы читатель сразу увидел асимметрию.

## Диспетчеризация результата

`RecvResult` и `SendResult` несут дискриминант `status()` плюс (только для `RecvResult`) полезную нагрузку. Две эквивалентные идиомы:

```php
use OxPHP\Shared\Channel;
use OxPHP\Shared\Channel\RecvStatus;

$ch = new Channel(capacity: 64);

// Boolean-аксессоры — кратко, когда важен только один исход.
$result = $ch->tryRecv();
if ($result->isOk()) {
    process($result->value());
} elseif ($result->isEmpty()) {
    backoff();
} else {
    break; // closed
}

// Исчерпывающий match — проверка исчерпываемости ловит новые варианты.
$r = $ch->recvTimeout(1500);
match ($r->status()) {
    RecvStatus::Ok      => process($r->value()),
    RecvStatus::Timeout => $logger->debug('idle'),
    RecvStatus::Closed  => break,
    RecvStatus::Empty   => /* unreachable: только tryRecv возвращает Empty */ ,
};

// Симметричная диспетчеризация на send-стороне — достижимы только Ok / Timeout / Closed.
use OxPHP\Shared\Channel\SendStatus;
$s = $ch->sendTimeout($value, 1500);
match ($s->status()) {
    SendStatus::Ok      => /* доставлено */ ,
    SendStatus::Timeout => $logger->debug('backpressure'),
    SendStatus::Closed  => break,
    SendStatus::Full    => /* unreachable: только trySend возвращает Full */ ,
};

// Безопасный аксессор — никогда не бросает.
$value = $ch->tryRecv()->valueOr('fallback');
```

`RecvResult::value()` бросает `OxPHP\Shared\SharedException`, если вызван на не-Ok варианте — сначала используйте `isOk()` / `valueOr()` / `status()`. Это сделано намеренно: ошибка «забыл проверить isOk» превращается в громкий сбой, а не в тихий `null`.

## Поведение fiber vs блокирующее

Одни и те же вызовы методов ведут себя по-разному в зависимости от того, выполняется ли PHP сейчас внутри файбера:

- **Внутри файбера** (worker mode + `oxphp_async(...)`): блокирующие варианты (`send`, `recv`, `sendTimeout`, `recvTimeout`) выделяют синтетический promise, регистрируют waker в канале и приостанавливают файбер. Поток воркера возвращается к планировщику и обрабатывает другие файберы, пока канал не уведомит waker.
- **Вне файбера** (традиционный режим или неасинхронный путь вызова): блокирующие варианты блокируют OS-поток воркера через `crossbeam_channel`. Никакая другая работа на этом потоке не выполняется до возврата.

Традиционный режим всё равно получает семантику канала — он просто платит заблокированным потоком. Worker mode — рекомендуемый способ развёртывания для любого пайплайна, опирающегося на ожидание.

```php
<?php
// Традиционный режим: этот recvTimeout блокирует поток воркера до 2 секунд.
$ch = new OxPHP\Shared\Channel(16);
$r = $ch->recvTimeout(2000);

// Worker mode: обернуть в oxphp_async, и recv приостанавливается кооперативно.
oxphp_worker(function () use ($ch) {
    $consumer = oxphp_async(function () use ($ch) {
        for (;;) {
            $r = $ch->recvTimeout(5000);
            if ($r->isOk()) { process($r->value()); continue; }
            if ($r->isClosed()) { break; }
            // Timeout — продолжаем ждать.
        }
    });
    oxphp_async_await($consumer);
});
```

Отмена файбера (abort запроса, истёкший дедлайн на уровне SAPI) всё равно всплывает как `OxPHP\Async\AsyncException` — она **не** транслируется в `RecvResult::Closed`. Канал не закрывался; вас отменил рантайм. Перебрасывайте или ловите по обстоятельствам.

## Семантика close

`close()` идемпотентен — второй вызов — no-op. После close:

- `send`, `sendTimeout`, `trySend` возвращают `SendResult::Closed`.
- `sendMany` возвращает число, реально принятое до close (`0`, если уже закрыт).
- `recv`, `recvTimeout`, `tryRecv` продолжают сливать буферизованные элементы как `RecvResult::Ok($value)`, затем возвращают `RecvResult::Closed`, когда пуст.
- `recvMany` возвращает элементы, которые смог слить, возможно пустой массив.
- `isClosed()` возвращает `true`.
- Заблокированные отправители просыпаются с `SendResult::Closed`; заблокированные получатели — с `RecvResult::Closed`.

```php
<?php
$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->close();

// Сливаем остатки.
for (;;) {
    $r = $ch->recv();
    if ($r->isClosed()) { break; }
    echo $r->value(), "\n"; // one, two
}

// Дальнейшие send возвращают Closed без исключения.
$result = $ch->send('three');
assert($result->isClosed());
```

Паттерн для graceful-остановки пайплайна: producers останавливаются, сторона producer вызывает `close()`, consumers сливают в цикле `for (;;) { $r = $ch->recv(); if ($r->isClosed()) break; … }` и естественно выходят. Ни один consumer не увидит `null`-payload по ошибке — вариант несёт этот сигнал явно.

## Слив при shutdown

Когда процесс OxPHP останавливается, реестр `OxPHP\Shared` вызывает `close()` на каждой записи, включая каналы. С точки зрения PHP это выглядит идентично явному `close()`:

- Заблокированные вызовы `recv*` возвращают `RecvResult::Closed`.
- Заблокированные вызовы `send*` возвращают `SendResult::Closed`.

> Всегда проверяйте вариант результата. Вызывающий, считающий `recv()->value()` всегда безопасным, бросит `SharedException` при shutdown или когда другой держатель закроет канал.

## Пакетные операции

`sendMany` и `recvMany` существуют для пайплайнов, перемещающих элементы группами. Предпочитайте их, когда вы регулярно обрабатываете 10+ элементов за раз: каждый пакет — один FFI round trip вместо N, что заметно снижает накладные расходы на элемент в циклах, упирающихся в пропускную способность.

```php
<?php
$ch = new OxPHP\Shared\Channel(1024);

// Отправить массив; возвращает, сколько реально принято.
$sent = $ch->sendMany([1, 2, 3, 4, 5], 100);  // 5 в пределах бюджета 100 мс

// Слить до 10 элементов с дедлайном 100 мс.
$batch = $ch->recvMany(10, 100);
```

Семантика, которую стоит отметить:

- Оба пакетных метода **возвращают частичный результат** при таймауте или при close посреди пакета — никогда исключение. Проверяйте длину int / массива, чтобы обнаружить частичный прогресс.
- `sendMany` на уже закрытом канале возвращает `0`.
- `recvMany` на closed+empty возвращает `[]`.
- `$ms` обязан быть `> 0`. Перегрузки «слить то, что буферизовано» нет — вызывайте `tryRecv()` в плотном цикле, если паттерн важен; добавим отдельный batch-вариант, когда появится реальный callsite.

## Наблюдаемость

Внутренний сервер (по умолчанию `INTERNAL_ADDR=127.0.0.1:9090`) показывает каналы в общих эндпоинтах shared-реестра:

- **`GET /__ox_shared/summary`** включает бакет `Channel` с count, bytes, ops и `pending_total`.
- **`GET /__ox_shared/entries?type=Channel`** перечисляет записи каналов с их ID реестра.
- **`GET /__ox_shared/entries/:id`** возвращает per-channel состояние: `capacity`, `count`, `pending` *(устаревший alias `count`)*, `closed`, `senders_blocked`, `receivers_blocked`.

Prometheus-экспозиция на `/metrics`:

```text
oxphp_shared_channel_count{channel_id="<id>"}               gauge
oxphp_shared_channel_pending{channel_id="<id>"}             gauge (устаревшая, alias _count)
oxphp_shared_channel_senders_blocked{channel_id="<id>"}     gauge
oxphp_shared_channel_receivers_blocked{channel_id="<id>"}   gauge
oxphp_shared_channel_items_sent_total{channel_id="<id>"}    counter
oxphp_shared_channel_items_dropped_total{channel_id="<id>"} counter
```

`items_dropped_total` инкрементируется для хвоста частичного `sendMany`, который не поместился.

## Распространённые паттерны

### HTTP-producer, async-consumer

```php
<?php
$work = new OxPHP\Shared\Channel(256);

$consumer = oxphp_async(function () use ($work) {
    for (;;) {
        $r = $work->recvTimeout(30000);
        if ($r->isOk()) { process_job($r->value()); continue; }
        if ($r->isClosed()) { break; }
        // Timeout — health-check и продолжаем ждать.
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
        for (;;) {
            $r = $ch->recvTimeout(60000);
            if ($r->isOk()) { handle($i, $r->value()); continue; }
            if ($r->isClosed()) { break; }
        }
    });
}
```

### Ограниченный пайплайн с backpressure

`trySend` плюс счётчик сбросов позволяет producer'у сбрасывать нагрузку вместо блокировки при перегрузке:

```php
<?php
if ($ch->trySend($event)->isFull()) {
    increment_dropped_metric();
}
```

## Подводные камни

- **`recvTimeout(0)` — это TypeException, а не «без блокировки».** Для non-blocking используйте `tryRecv()`. Отдельные имена методов убрали неоднозначность timeout-перегрузки, которая была у старого `?float $timeout`.
- **Значения должны быть shareable.** Скаляры, `null` и вложенные массивы shareable-объектов разрешены. Передача объекта, который не является экземпляром `Shared\*`, поднимает `TypeException` при send.
- **Clone запрещён.** `clone $channel` бросает `SharedException`; передавайте канал через `use` замыкания — `oxphp_async(function () use ($ch) { ... })` — чтобы обе стороны видели одну и ту же запись реестра.
- **`value()` на не-Ok бросает.** Это фича, а не баг — она превращает ошибку «забыл проверить isOk» в громкий сбой. Используйте `valueOr($default)`, если default действительно подходит.
- **Отмена файбера всплывает как исключение**, а не вариант Result. `recv()`, прерванный отменой запроса, поднимает `OxPHP\Async\AsyncException`. Каналы, закрытые сами по себе, по-прежнему дают `RecvResult::Closed`.
- **Отменённые ожидающие с in-flight payload.** Если много файберов ждут на `send` / `recv` и отменяются, когда их payload почти перешёл, payload может остаться со ссылкой до следующего пробуждения. Держите число ожидающих ограниченным (например, кэпайте конкурентность через `Shared\Counter` или семафор из channel-capacity).

## Миграция со старого API

| Было                                                                | Стало                                                                  |
|---------------------------------------------------------------------|------------------------------------------------------------------------|
| `$ch->tryRecv()` → `null` при empty, бросает `ClosedException`      | `$ch->tryRecv()` → `RecvResult::Empty` / `Closed`                       |
| `$ch->recv($secs)` → `null` при timeout/close                       | `$ch->recv()` (вечно) / `$ch->recvTimeout($ms)` → `RecvResult`          |
| `$ch->trySend($v): bool`                                            | `$ch->trySend($v): SendResult`                                          |
| `$ch->send($v, $secs): bool` (бросает TimeoutException/ClosedException) | `$ch->send($v)` / `$ch->sendTimeout($v, $ms)` → `SendResult`        |
| `$ch->sendMany($vs, $secs)` (бросает TimeoutException при частичном) | `$ch->sendMany($vs, $ms): int` (частичный счёт, без throw)             |
| `?float $timeout` (секунды, NaN/INF допускались)                    | `int $ms` (миллисекунды, обязан быть `> 0`)                            |

## Связанные возможности

- [Worker Mode](worker-mode.md) — предпосылка для fiber-приостанавливающих блокирующих вариантов.
- [Async Promises](async-promises.md) — замыкание `oxphp_async()` — обычный способ передать `Channel` фоновому файберу.
- [Fiber Multiplexing](fiber-multiplexing.md) — объясняет, как приостановка держит поток воркера продуктивным, пока операции канала ждут.
