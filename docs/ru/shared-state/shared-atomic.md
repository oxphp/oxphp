---
title: Shared\Atomic
description: Универсальный атомарный int64, разделяемый между PHP-воркерами — load/store, swap, CAS, fetch-арифметика и fetch-битовые операции с явным контролем memory ordering.
---

# Shared\Atomic

`OxPHP\Shared\Atomic` — это общепроцессное атомарное 64-битное знаковое целое с полным набором примитивов: `load`, `store`, `swap`, `compareAndSet`, плюс `fetchAdd`/`Sub`/`And`/`Or`/`Xor`. Все операции lock-free; memory ordering задаётся явно и по умолчанию `SeqCst`.

## Обзор

- **Атомарный примитив int64.** Диапазон `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`. Переполнение оборачивается.
- **Lock-free.** Каждая операция компилируется в одну атомарную инструкцию CPU (`load`, `store`, `xchg`, `cmpxchg`, `xadd` и т.п.).
- **Memory ordering под вашим контролем.** Передавайте значение enum'а `OxPHP\Shared\Ordering`, когда нужен `Relaxed` / `Acquire` / `Release` / `AcqRel` / `SeqCst`. По умолчанию — `SeqCst`, поэтому те, кто не хочет думать про ordering, получают самые сильные гарантии.

Когда брать Atomic вместо `Shared\Counter`:

- **Конечные автоматы** — `compareAndSet` для `idle → busy → done`.
- **Версионные штампы / generation counter** — `fetchAdd(1)` возвращает предыдущую версию; читатели могут использовать её, чтобы детектировать гонки.
- **CAS-циклы** — читать через `load`, вычислить новое значение, повторять `compareAndSet` до успеха.
- **Битовые маски флагов** — `fetchOr` для установки, `fetchAnd` для сброса.

`Counter` подходит для аккумуляции (`add`); `Atomic` — для произвольного атомарного состояния.

## Справочник API

```php
namespace OxPHP\Shared;

final class Atomic implements Shareable
{
    public function __construct(int $initial = 0);

    public function load(Ordering $order = Ordering::SeqCst): int;
    public function store(int $value, Ordering $order = Ordering::SeqCst): void;
    public function swap(int $value, Ordering $order = Ordering::SeqCst): int;            // возвращает предыдущее

    public function compareAndSet(
        int $expect,
        int $new,
        Ordering $success = Ordering::SeqCst,
        Ordering $failure = Ordering::SeqCst,
    ): bool;

    public function fetchAdd(int $delta, Ordering $order = Ordering::SeqCst): int;        // возвращает предыдущее
    public function fetchSub(int $delta, Ordering $order = Ordering::SeqCst): int;        // возвращает предыдущее
    public function fetchAnd(int $mask,  Ordering $order = Ordering::SeqCst): int;        // возвращает предыдущее
    public function fetchOr (int $mask,  Ordering $order = Ordering::SeqCst): int;        // возвращает предыдущее
    public function fetchXor(int $mask,  Ordering $order = Ordering::SeqCst): int;        // возвращает предыдущее

    public function id(): int;
}
```

| Метод             | Возвращает    | Применение                                                     |
|-------------------|---------------|----------------------------------------------------------------|
| `load`            | текущее       | Чтение значения с выбранным ordering.                          |
| `store`           | void          | Запись нового значения, отбрасывая старое.                     |
| `swap`            | предыдущее    | Атомарная замена; `swap(0)` — паттерн snapshot-and-zero.       |
| `compareAndSet`   | swap?         | Оптимистичные переходы и CAS-циклы.                            |
| `fetchAdd`/`Sub`  | предыдущее    | Generation counter, ограниченные счётчики через CAS, дельты.   |
| `fetchAnd`/`Or`/`Xor` | предыдущее | Битовые маски: установить, сбросить, переключить.             |
| `id`              | id реестра    | Логи, трассировки, корреляция `/__ox_shared/entry?id=…`.       |

## Memory ordering

Краткий пример:

- **Relaxed** — только атомарность, без упорядочивания относительно других обращений к памяти.
- **Acquire** (для load) — образует пару с Release-store; чтения после этой операции видят записи, завершённые до соответствующего Release.
- **Release** (для store) — образует пару с Acquire-load; записи до этой операции видны acquirer'ам.
- **AcqRel** (для read-modify-write) — обе половины: Acquire-load и Release-store.
- **SeqCst** — единый глобальный тотальный порядок всех операций `SeqCst`.

Каждая операция принимает только осмысленные для неё ordering'и:

| Операция | Допустимые |
|---|---|
| `load` | `Relaxed`, `Acquire`, `SeqCst` |
| `store` | `Relaxed`, `Release`, `SeqCst` |
| `swap`, `fetchAdd`, `fetchSub`, `fetchAnd`, `fetchOr`, `fetchXor` | любые |
| `compareAndSet` `success` | любые |
| `compareAndSet` `failure` | `Relaxed`, `Acquire`, `SeqCst` |

По умолчанию везде `Ordering::SeqCst`, поэтому код, который не задумывается про ordering, получает безопасное поведение. Недопустимая комбинация бросает `OxPHP\Shared\InvalidOrderingException` ещё до FFI-вызова.

Глубокое погружение в C++/Rust модель памяти — см. [документацию `std::sync::atomic::Ordering` в Rust](https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html).

## Примеры

### Конечный автомат через compareAndSet

```php
<?php
use OxPHP\Shared\Atomic;

$state = new Atomic(initial: 0); // 0=idle, 1=busy, 2=done

if (!$state->compareAndSet(expect: 0, new: 1)) {
    throw new RuntimeException('another worker is already processing');
}

try {
    doWork();
    $state->store(2);
} catch (Throwable $e) {
    $state->store(0); // вернуть в idle при ошибке
    throw $e;
}
```

### Generation counter / версионный штамп

```php
<?php
$version = new OxPHP\Shared\Atomic();

// Каждый писатель бампит версию и получает ту, которую только что заменил.
$prev = $version->fetchAdd(1);
publishUpdate($prev + 1, $payload);
```

### Оптимистичное обновление через CAS-цикл

```php
<?php
use OxPHP\Shared\Atomic;
use OxPHP\Shared\Ordering;

$cell = new Atomic(initial: 100);

// Saturate-add: никогда не выше 1000.
do {
    $cur = $cell->load(Ordering::Acquire);
    $next = min($cur + 7, 1000);
    if ($cur === $next) {
        break; // уже на потолке
    }
} while (!$cell->compareAndSet($cur, $next, Ordering::AcqRel, Ordering::Acquire));
```

### Битовая маска флагов

```php
<?php
const FLAG_READY    = 1 << 0;
const FLAG_DRAINING = 1 << 1;
const FLAG_FAILED   = 1 << 2;

$flags = new OxPHP\Shared\Atomic();

$flags->fetchOr(FLAG_READY);                  // установить бит
$flags->fetchAnd(~FLAG_DRAINING);             // сбросить бит
$snapshot = $flags->load();
if ($snapshot & FLAG_FAILED) {
    raiseAlert();
}
```

## Семантика и подводные камни

- **`fetchAdd` возвращает предыдущее значение, а не новое.** Это намеренный контраст с `Counter::add`, который возвращает новый итог. Разные абстракции — разные конвенции; выбирайте класс по семантике, которую имеете в виду.
- **Переполнение оборачивается.** `i64::MIN.fetchSub(1)` даёт `i64::MAX`. Исключение не бросается.
- **По умолчанию ordering — `SeqCst`.** Это самый безопасный и самый медленный выбор. Опускайтесь до `Acquire`/`Release`/`Relaxed` только когда сможете объяснить, зачем.
- **Только один int64.** Для составного состояния (несколько связанных полей) — используйте `Shared\Mutex`.

## Исключения

| Исключение                   | Бросается                                                              |
|------------------------------|------------------------------------------------------------------------|
| `StaleHandleException`       | Любой метод на хэндле, чья запись реестра была вытеснена.              |
| `UninitializedException`     | `id()` на обёртке, не завершившей `__construct`.                       |
| `InvalidOrderingException`   | Операция получает недопустимый для неё memory ordering.                |

## Наблюдаемость

Полный тур — в [Shared Observability](shared-observability.md). Краткие отсылки:

- `GET /__ox_shared/entry?id=N` показывает `{ value, type: "Atomic" }`.
- Общереестровые счётчики (`oxphp_shared_ops_total`, `oxphp_shared_objects_total`) покрывают Atomic через метку `type="Atomic"`.

## Когда не использовать

- **Составное состояние.** Несколько полей, которые должны обновляться вместе → `Shared\Mutex`.
- **Подсчёт / аккумуляция.** Берите `Shared\Counter` — его `add`, возвращающий новый итог, соответствует домену.
- **Float'ы или десятичные.** Не поддерживаются; оберните структуру в `Shared\Mutex` или пару Counter'ов (числитель / знаменатель).
- **Межхостовая координация.** Atomic живёт только в процессе. Для многохостового состояния используйте Redis, БД или метрический пайплайн.
- **Долговечность.** Состояние Atomic испаряется при остановке сервера. Если значение должно пережить перезапуск, сохраняйте снимки где-то ещё.

## См. также

- [Разделяемое состояние](shared-state.md) — обзор и паттерны миграции.
- [Shared\Counter](shared-counter.md) — когда значение — это доменный аккумулятор.
- [Shared\Mutex](shared-mutex.md) — когда состояние шире одного int64.
- [Shared\Flag](shared-flag.md) — когда значение просто on/off.
