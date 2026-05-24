---
title: Shared\Mutex
description: Кросс-поточное взаимное исключение над хранимым значением — атомарные многошаговые обновления между PHP-воркерами через withLock / tryWithLock / withLockTimeout с явными wait-политиками.
---

# Shared\Mutex

`OxPHP\Shared\Mutex` — это общепроцессный блокирующий примитив взаимного исключения, оборачивающий хранимое значение. Вы никогда не трогаете блокировку напрямую — вы передаёте замыкание в один из трёх вариантов метода, и рантайм держит блокировку на протяжении замыкания, отпуская её даже если замыкание бросит.

## Обзор

- **Охраняет значение, а не просто секцию.** Обёрнутое значение передаётся в ваше замыкание **по ссылке**, поэтому прямое изменение внутри замыкания коммитится обратно при нормальном возврате.
- **Три явные wait-политики** вместо одного перегруженного `?float $timeout`:
  - `withLock($fn)` — ждать вечно (или пока request fiber не будет отменён).
  - `tryWithLock($fn)` — non-blocking; бросает `ContentionException`, если блокировка удерживается.
  - `withLockTimeout($fn, int $ms)` — ограниченное ожидание; бросает `OperationTimeoutException` при истечении дедлайна.
- **PHP-исключения свободно проходят.** Если замыкание бросает обычное PHP-исключение, блокировка отпускается, и исключение всплывает. Mutex **не** портится — частичная мутация допустима; вызывающий отвечает за восстановление инвариантов.
- **Rust-паники портят mutex.** Если Rust-паника пересекает границу FFI (баг сервера), mutex переходит в sticky-corrupted состояние, и каждый последующий захват бросает `CorruptedMutexException`. API восстановления нет — выбросьте инстанс и создайте новый.
- **Защита от дедлоков.** Повторный вход в тот же mutex на том же потоке (включая вложенные async-вызовы, захваченные на этом потоке) поднимает `DeadlockException` вместо зависания.

## Справочник API

```php
namespace OxPHP\Shared;

final class Mutex implements Shareable
{
    public function __construct(mixed $initial = null);

    public function withLock(callable $fn): mixed;
    public function tryWithLock(callable $fn): mixed;
    public function withLockTimeout(callable $fn, int $ms): mixed;

    public function id(): int;
}
```

Сигнатура замыкания — `function (mixed &$value): mixed` — `$value` передаётся по ссылке, так что вы мутируете его на месте. Нормальное возвращаемое значение замыкания пробрасывается вызывающему `withLock` / `tryWithLock` / `withLockTimeout`, но **возврат ограничен скалярами и `null`** (string, int, float, bool, byte-string, null). Возврат массива или экземпляра `Shared\*` поднимает `OxPHP\Shared\TypeException`. Чтобы пробросить структурированное состояние вызывающему, либо мутируйте `&$value` на месте и перечитайте его после вызова, либо подготовьте нужное в переменной через `use (&$captured)`. Снятие этого ограничения отслеживается отдельно.

| Метод                       | Поведение                                                       |
|-----------------------------|-----------------------------------------------------------------|
| `withLock($fn)`             | Блокирует до захвата, затем выполняет замыкание. Вечно / cancel. |
| `tryWithLock($fn)`          | Non-blocking. Бросает `ContentionException`, если удерживается. |
| `withLockTimeout($fn, $ms)` | Ограниченное ожидание. Требуется `$ms > 0`. Бросает `OperationTimeoutException` по дедлайну. |
| `id()`                      | Идентификатор реестра; полезен для логов / наблюдаемости.       |

`$ms` — строго положительное целое в миллисекундах. Ноль, отрицательные, не-int и отсутствующие значения поднимают `OxPHP\Shared\TypeException` на бридже — вместо попытки выразить эти политики через `$ms` вызывайте `withLock` (вечно) или `tryWithLock` (non-blocking).

## Почему Mutex бросает, а Channel возвращает Result

Конкуренция и таймаут — это **редкие события** для хорошо спроектированного mutex (блокировки должны удерживаться на короткие критические секции; устойчивая конкуренция — это запах). Это **рутинные события** для канала (fan-out диспетчер видит Full/Closed/Timeout в каждом нагруженном цикле). Поэтому:

- `Mutex` использует **exception-стиль** — редкий путь и есть исключительный.
- `Channel` использует **Result-стиль** — частый путь остаётся вне машинерии throw/catch.

Если вы оборачиваете каждый `withLock` в `try { … } catch (ContentionException) { … }`, вы используете не тот примитив. Возьмите `Shared\Channel` для очередевидных нагрузок или `Shared\Counter` / `Shared\Flag` для атомарности одного значения.

Та же структурная причина объясняет, почему `Pool::tryAcquire()` может вернуть `null` там, где `Mutex::tryWithLock()` бросает. `Pool` — **handle-first**: `tryAcquire(): ?Handle` несёт «насыщен» как `null`, а `Handle` сам по себе никогда не бывает пользовательским значением, так что неоднозначности нет. `Mutex` — **closure-only**: он намеренно не отдаёт guard блокировки наружу в PHP (чтобы удерживаемый лок не «утёк» за пределы замыкания), из-за чего нет объекта для возврата как nullable, а собственный результат `mixed` замыкания может уже быть `null`. Без свободного sentinel конкуренция всплывает как `ContentionException`. Две поверхности `try*` расходятся из-за того, что каждый тип может отдать наружу, а не из-за стилевых предпочтений.

## Примеры

### Атомарное обновление нескольких полей

Counter достаточно, когда значение — одно целое. Mutex выигрывает, когда несколько полей должны обновляться синхронно:

```php
<?php
$stats = new OxPHP\Shared\Mutex(['hits' => 0, 'bytes' => 0]);

$stats->withLock(function (array &$s) use ($responseBytes) {
    $s['hits']  += 1;
    $s['bytes'] += $responseBytes;
});
```

Другой воркер, наблюдающий значение, читает оба поля в одной критической секции:

```php
$snapshot = ['hits' => 0, 'bytes' => 0];
$stats->withLock(function (array &$s) use (&$snapshot) {
    $snapshot = $s;
});
// $snapshot видит оба поля из одного обновления или ни одно — никогда
// бампнутый 'hits' без соответствующих 'bytes'. (Мы захватываем через
// use(&$x), потому что собственный return замыкания сейчас scalar-only —
// см. заметку о сигнатуре замыкания выше.)
```

### Non-blocking проверка + деградация

```php
<?php
use OxPHP\Shared\{Mutex, ContentionException};

$budget = new Mutex(['tokens' => 100, 'refill_at' => time()]);

try {
    $budget->tryWithLock(function (array &$b) {
        if ($b['tokens'] <= 0) {
            // Нет токенов — оставляем состояние нетронутым.
            return;
        }
        $b['tokens'] -= 1;
    });
} catch (ContentionException) {
    // Блокировку держит другой воркер — сбрасываем запрос вместо очереди.
    http_response_code(503);
    return;
}
```

### Захват с таймаутом

```php
<?php
use OxPHP\Shared\{Mutex, OperationTimeoutException};

$counter = new Mutex(0);

try {
    // Возврат скалярный — int $next — поэтому return замыкания пробрасывается.
    $next = $counter->withLockTimeout(function (int &$c) {
        $c += 1;
        return $c;
    }, ms: 5000);
} catch (OperationTimeoutException) {
    // Кто-то держал блокировку дольше 5 с.
}
```

Именованные аргументы поощряются — `ms: 5000` читается как «5000 миллисекунд» без необходимости помнить порядок параметров.

### Ловить все условия конкурентности в одном месте

`OperationTimeoutException`, `ContentionException` и `DeadlockException` все наследуют `OxPHP\Async\AsyncException`. Один catch охватывает все исходы конкурентности по поверхностям Shared\* и Async\*:

```php
<?php
use OxPHP\Async\AsyncException;

try {
    $state->withLockTimeout($fn, 100);
} catch (AsyncException) {
    // timeout, contention, deadlock или любая ошибка конкурентности,
    // связанная с await
}
```

### Катастрофическое восстановление из порченого mutex

Rust-паника во время выполнения замыкания (баг сервера, а не PHP-кода) оставляет блокировку в sticky-corrupted состоянии. Эквивалента `clearPoison()` нет — выбросьте инстанс:

```php
<?php
use OxPHP\Shared\{Mutex, CorruptedMutexException};

try {
    $state->withLock($fn);
} catch (CorruptedMutexException) {
    // Старый инстанс мёртв. Пересоздаём из постоянного источника истины.
    $state = new Mutex($initialState);
}
```

## Семантика и подводные камни

- **Замыкание выполняется с удерживаемой блокировкой.** Держите его коротким. Не вызывайте `sleep`, не блокируйтесь на сетевом I/O, не входите повторно в другие типы Shared\*, которые могут позвать обратно в этот mutex.
- **PHP-throw больше не портит блокировку.** Это намеренное изменение по сравнению с прежней политикой «Poisoned на любой throw»: теперь политика частичной мутации — «вызывающий отвечает за восстановление инвариантов». Если нужен паттерн «попробовать-вычислить без мутации», делайте это вне mutex и вызывайте `withLock` только для коммита финального значения.
- **Сохранённое значение скаляр-подобное.** Строки, int, float, bool, `null` и вложенные массивы из них работают. Объекты, замыкания и ресурсы поднимают `TypeException`.
- **Return замыкания тоже scalar-only — *пока что*.** Сохранённое значение может быть массивом (мутируйте через `&$value`), но сам *return* замыкания поддерживает только string/int/float/bool/null/byte-string. Возврат массива или `Shared\*`-инстанса поднимает `OxPHP\Shared\TypeException`. Обход: захватите в `use (&$x)`-переменную или прочитайте состояние через последующий `withLock`, возвращающий скалярную проекцию.
- **Реентрантность на том же потоке бросает `DeadlockException`.** Используйте другой mutex или перестройте код — повторный вход на том же потоке — это баг, а не фича.
- **Отмена файбера всплывает как `Async\AsyncException`.** `withLock`, прерванный отменой запроса, поднимает это исключение; блокировка отпускается чисто.

## Исключения

| Исключение                   | Родитель                        | Бросается                                                            |
|------------------------------|---------------------------------|----------------------------------------------------------------------|
| `ContentionException`        | `Async\AsyncException`          | `tryWithLock` на удерживаемой блокировке.                            |
| `OperationTimeoutException`  | `Async\AsyncException`          | Дедлайн `withLockTimeout` истёк.                                     |
| `DeadlockException`          | `Async\AsyncException`          | Реентрантность на том же потоке или обнаружен wait-for цикл.         |
| `CorruptedMutexException`    | `Shared\SharedException`        | Предыдущее замыкание упало через Rust-панику; mutex непригоден.      |
| `TypeException`              | `Shared\SharedException`        | Конструктор или аргумент `$ms` нарушил свой контракт типа.           |
| `StaleHandleException`       | `Shared\SharedException`        | Вызов метода на хэндле, чья запись реестра была вытеснена.           |
| `UninitializedException`     | `Shared\SharedException`        | `id()` на обёртке, которая не завершила `__construct`.               |

## Наблюдаемость

См. [Shared Observability](../operations/shared-observability.md). Краткие отсылки:

- `GET /__ox_shared/entry?id=N` показывает `{ type: "Mutex", corrupted, waiters, last_acquire_ms, held_by_thread }`.
- Prometheus-метрики per instance:
  - `oxphp_shared_mutex_waiters{mutex_id="…"}` — текущее число ждущих.
  - `oxphp_shared_mutex_acquires_total{mutex_id="…"}` — захваты за время жизни.
  - `oxphp_shared_mutex_contended_total{mutex_id="…"}` — захваты, которым пришлось ждать.
  - `oxphp_shared_mutex_corrupted{mutex_id="…"}` — 0 / 1 (переименовано из `_poisoned`).

## Когда не использовать

- **Одно атомарное значение.** Если охраняемое значение — одно int или один bool, используйте `Shared\Counter` или `Shared\Flag` — оба lock-free и дешевле.
- **Долгая работа.** Не держите mutex через I/O, `sleep` или fiber-await'ы. Используйте вместо этого паттерн producer/consumer на `Shared\Channel`.
- **Горячий путь с высокой конкуренцией.** Если каждый запрос должен взять один и тот же mutex, вы сериализовали свою пропускную способность. Разбейте состояние на партиции (например, `Shared\Map<tenant_id, Mutex>`) или агрегируйте предварительно в per-worker локальных переменных и периодически сливайте.
- **Межхостовое взаимное исключение.** Только в процессе. Для межхостовой координации используйте распределённую блокировку (Redis `SET NX`, etcd).

## Миграция со старого API

| Было                                                    | Стало                                                                          |
|---------------------------------------------------------|--------------------------------------------------------------------------------|
| `$m->with($fn)` (вечно)                                 | `$m->withLock($fn)`                                                             |
| `$m->with($fn, $secs)`                                  | `$m->withLockTimeout($fn, $ms)` с `$ms` в миллисекундах                         |
| `$m->tryWith($fn)` → `null` при конкуренции             | `$m->tryWithLock($fn)` → бросает `ContentionException`                          |
| `$m->isPoisoned()` / `$m->clearPoison()`                | удалены; PHP-throw больше не портит mutex                                       |
| `PoisonedException` (путь Rust-паники)                  | `CorruptedMutexException` (публичного API очистки нет)                          |
| `Shared\TimeoutException`                               | `Shared\OperationTimeoutException` (теперь наследует `Async\AsyncException`)    |
| `DeadlockException extends Shared\TimeoutException`     | `DeadlockException extends Async\AsyncException`                                |

Сигнатура замыкания также сменилась с `function (mixed $value): mixed` (return-to-commit) на `function (mixed &$value): mixed` (мутация по ссылке, нормальный return — это возврат самого замыкания, а не нового состояния). Если замыкание ничего не возвращает, сохранённое значение остаётся таким, каким его оставила мутация по ссылке. **Одно ранее существовавшее ограничение переносится**: *return* замыкания должен быть скаляром (string / int / float / bool / null / byte-string) — возврат массива или `Shared\*`-инстанса бросает `OxPHP\Shared\TypeException`. Сохранённое значение по-прежнему может быть массивом; мутируйте его через `&$value` и используйте `use (&$x)`, чтобы пробросить структурированные данные наверх.

## См. также

- [Shared State](shared-state.md) — обзор и ментальная модель.
- [Shared\Counter](shared-counter.md) — когда охраняемое состояние — одно целое.
- [Shared\Flag](shared-flag.md) — когда охраняемое состояние — один bool.
- [Shared\Channel](shared-channel.md) — когда нужны ожидание + передача, а не взаимное исключение (и нужен Result-стиль вместо exception-стиля).
- [Shared\Map](shared-map.md) — партицируйте Mutex по ключу, чтобы избежать глобальной конкуренции.
