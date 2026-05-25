---
title: Shared\Flag
description: Атомарный boolean, разделяемый между PHP-воркерами — kill-switch'и, circuit breaker'ы и одноразовые маркеры с lock-free load/store/swap/compareAndSet и явным порядком памяти.
---

# Shared\Flag

`OxPHP\Shared\Flag` — это общепроцессный атомарный boolean, bool-двойник [`Shared\Atomic`](shared-atomic.md). Каждая операция lock-free; два воркера, переключающих флаг конкурентно, не могут наблюдать промежуточное состояние.

## Обзор

- **Атомарный bool.** Один бит состояния с `load` / `store` / `swap` / `compareAndSet`.
- **Явный порядок памяти.** Каждая операция принимает необязательный [`Ordering`](shared-atomic.md) со значением по умолчанию `SeqCst` — ровно как у `Shared\Atomic`.
- **Lock-free.** Все мутации — одна атомарная операция CPU. Безопасно под конкуренцией.
- **Shareable.** Экземпляры живут в реестре и могут храниться внутри `Shared\Map`, передаваться через захваты `use` и т. д.

## Справочник API

```php
namespace OxPHP\Shared;

final class Flag implements Shareable
{
    public function __construct(bool $initial = false);

    public function load(Ordering $order = Ordering::SeqCst): bool;             // Relaxed | Acquire | SeqCst
    public function store(bool $value, Ordering $order = Ordering::SeqCst): void; // Relaxed | Release | SeqCst
    public function swap(bool $value, Ordering $order = Ordering::SeqCst): bool;   // любой ordering; возвращает предыдущее
    public function compareAndSet(
        bool $expect,
        bool $new,
        Ordering $success = Ordering::SeqCst,
        Ordering $failure = Ordering::SeqCst,                                  // Relaxed | Acquire | SeqCst
    ): bool;

    public function id(): int;
}
```

| Метод           | Возвращает | Применение                                                       |
|-----------------|----------|------------------------------------------------------------------|
| `load`          | текущее  | Чистое чтение.                                                   |
| `store`         | void     | Безусловно установить явное значение.                            |
| `swap`          | предыдущее | Установить явное значение; возврат говорит, изменили ли вы его. `swap(true)` — это test-and-set («победил ли я?»). |
| `compareAndSet` | swap?    | Одноразовая инициализация: успех только если флаг был ожидаемым. |

## Примеры

### Kill-switch

```php
<?php
use OxPHP\Shared\Flag;

$maintenance = new Flag();

// В обработчике запроса
if ($maintenance->load()) {
    http_response_code(503);
    header('Retry-After: 60');
    echo 'under maintenance';
    return;
}

// В админском эндпоинте
$maintenance->store(true);   // включить
$maintenance->store(false);  // отключить
```

### Победитель одноразовой инициализации

```php
<?php
use OxPHP\Shared\Flag;

$migrated = new Flag();

if ($migrated->compareAndSet(expect: false, new: true)) {
    // Первый подоспевший воркер побеждает — запустить миграцию один раз.
    runSchemaMigration();
} else {
    // Кто-то другой уже её запустил.
}
```

### Срабатывание circuit breaker

```php
<?php
use OxPHP\Shared\Flag;

$tripped = new Flag();

try {
    callDownstream();
} catch (DownstreamFailedException $e) {
    $wasAlreadyTripped = $tripped->swap(true);   // установить true, узнать прежнее состояние
    if (!$wasAlreadyTripped) {
        alertOncall($e);        // посылать алерт только при первом срабатывании
    }
    throw $e;
}
```

Для полноценного circuit breaker обычно нужен `Shared\Counter` для окна отказов и `Shared\Flag` для состояния trip — сбрасывайте флаг через `store(false)`, когда окно остыло.

### Опубликовать данные, затем сигналить с более дешёвым ordering

```php
<?php
use OxPHP\Shared\Flag;
use OxPHP\Shared\Map;
use OxPHP\Shared\Ordering;

$ready = new Flag();
$config = new Map();

// Производитель: записать данные, затем опубликовать с Release.
$config->set('dsn', $dsn);
$ready->store(true, Ordering::Release);

// Потребитель: Acquire-загрузка, увидевшая `true`, видит и данные.
if ($ready->load(Ordering::Acquire)) {
    $dsn = $config->get('dsn');
}
```

## Семантика и подводные камни

- **`swap` возвращает *предыдущее* значение.** Это самый полезный возврат: «что-нибудь изменилось?» — это `$prev !== $new`. `swap(true)` — каноничный test-and-set.
- **`store` возвращает `void`.** Если нужно прежнее значение, используйте `swap`.
- **`compareAndSet` — это как выразить «побеждает первый».** Обычный `store(true)` всегда успешен, так что не может выразить «не перезаписывать, если уже установлено».
- **Порядок памяти совпадает с `Shared\Atomic`.** `load` отвергает `Release`/`AcqRel`, `store` отвергает `Acquire`/`AcqRel`, а `$failure` в `compareAndSet` отвергает `Release`/`AcqRel` — каждый случай бросает `InvalidOrderingException`. Значение по умолчанию `SeqCst` всегда безопасно.
- **Ожидания нет.** Flag не блокирует. Если нужно ждать перехода, используйте его в паре с `Shared\Channel` или `Shared\Once`.

## Исключения

| Исключение                 | Бросается                                                    |
|----------------------------|--------------------------------------------------------------|
| `StaleHandleException`     | Любой метод на хэндле, чья запись реестра была вытеснена.    |
| `UninitializedException`   | `id()` на обёртке, которая не завершила `__construct`.        |
| `InvalidOrderingException` | `Ordering`, недопустимый для операции (см. выше).            |

## Наблюдаемость

См. [Shared Observability](shared-observability.md). Краткие отсылки:

- `GET /__ox_shared/entry?id=N` показывает `{ value: true|false, type: "Flag" }`.
- Prometheus `oxphp_shared_flag_value{flag_id="…"}` gauge (0 или 1).
- Общереестровые метрики покрывают Flag через метку `type="Flag"`.

## Когда не использовать

- **Многосостоятельная логика.** Flag двухзначен. Если нужен idle/busy/done или любой трёхсостоятельный автомат, берите `Shared\Counter` (используйте целочисленные enum-значения) или `Shared\Mutex` над массивом в стиле enum.
- **Ожидание перехода.** Flag не блокирует. Парьте с `Shared\Channel` (или с `Shared\Counter`, опрашиваемым через `compareAndSet`), когда воркер должен ждать, пока флаг не перевернётся.
- **Подсчёт событий.** Flag — не счётчик. Используйте `Shared\Counter` для подсчётов.
- **Целочисленное состояние.** Если переключатель на самом деле небольшое целое, используйте напрямую [`Shared\Atomic`](shared-atomic.md).

## См. также

- [Разделяемое состояние](shared-state.md) — обзор и ментальная модель.
- [Shared\Atomic](shared-atomic.md) — int64-двойник с той же моделью ordering.
- [Shared\Counter](shared-counter.md) — когда нужно больше чем on/off.
- [Shared\Once](shared-once.md) — когда вычисленное один раз значение богаче, чем bool.
- [Shared\Mutex](shared-mutex.md) — когда переключение флага должно co-commit с другим состоянием.
