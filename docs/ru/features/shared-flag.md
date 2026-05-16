---
title: Shared\Flag
description: Атомарный boolean, разделяемый между PHP-воркерами — kill-switch'и, circuit breaker'ы и одноразовые маркеры инициализации с lock-free set/clear/isSet.
---

# Shared\Flag

`OxPHP\Shared\Flag` — это общепроцессный атомарный boolean. Каждая операция lock-free; два воркера, переключающих флаг конкурентно, не могут наблюдать промежуточное состояние.

## Обзор

- **Атомарный bool.** Один бит состояния с `isSet` / `set` / `clear` / `exchange` / `compareAndSet`.
- **Lock-free.** Все мутации — одна атомарная операция CPU. Безопасно под конкуренцией.
- **Shareable.** Экземпляры живут в реестре и могут храниться внутри `Shared\Map`, передаваться через захваты `use` и т. д.

## Справочник API

```php
namespace OxPHP\Shared;

final class Flag implements Shareable
{
    public function __construct(bool $initial = false);

    public function isSet(): bool;
    public function set(): bool;                                 // возвращает предыдущее
    public function clear(): bool;                               // возвращает предыдущее
    public function exchange(bool $new): bool;                   // возвращает предыдущее
    public function compareAndSet(bool $expect, bool $new): bool;

    public function id(): int;
}
```

| Метод           | Возвращает | Применение                                                       |
|-----------------|----------|------------------------------------------------------------------|
| `isSet`         | текущее  | Чистое чтение.                                                   |
| `set`           | предыдущее | Безусловно включить. Предыдущее значение говорит, победили ли вы. |
| `clear`         | предыдущее | Безусловно выключить.                                            |
| `exchange`      | предыдущее | Обмен на явное значение; полезно, когда переключение условное.   |
| `compareAndSet` | swap?    | Одноразовая инициализация: успех только если флаг был ожидаемым. |

## Примеры

### Kill-switch

```php
<?php
$maintenance = new OxPHP\Shared\Flag();

// В обработчике запроса
if ($maintenance->isSet()) {
    http_response_code(503);
    header('Retry-After: 60');
    echo 'under maintenance';
    return;
}

// В админском эндпоинте
$maintenance->set();     // включить
$maintenance->clear();   // отключить
```

### Победитель одноразовой инициализации

```php
<?php
$migrated = new OxPHP\Shared\Flag();

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
$tripped = new OxPHP\Shared\Flag();

try {
    callDownstream();
} catch (DownstreamFailedException $e) {
    $wasAlreadyTripped = $tripped->set();
    if (!$wasAlreadyTripped) {
        alertOncall($e);        // посылать алерт только при первом срабатывании
    }
    throw $e;
}
```

Для полноценного circuit breaker обычно нужен `Shared\Counter` для окна отказов и `Shared\Flag` для состояния trip — сбрасывайте флаг через `clear()`, когда окно остыло.

## Семантика и подводные камни

- **`set` / `clear` / `exchange` возвращают *предыдущее* значение.** Это сознательно самый полезный возврат: «что-нибудь изменилось?» — это `$prev !== $new`.
- **`compareAndSet` — это как выразить «побеждает первый».** Обычный `set()` всегда успешен, так что не может выразить «не перезаписывать, если уже установлено».
- **Ожидания нет.** Flag не блокирует. Если нужно ждать перехода, используйте его в паре с `Shared\Channel` или `Shared\Once`.

## Исключения

| Исключение               | Бросается                                                    |
|--------------------------|--------------------------------------------------------------|
| `StaleHandleException`   | Любой метод на хэндле, чья запись реестра была вытеснена.    |
| `UninitializedException` | `id()` на обёртке, которая не завершила `__construct`.        |

## Наблюдаемость

См. [Shared Observability](../operations/shared-observability.md). Краткие отсылки:

- `GET /__ox_shared/entry?id=N` показывает `{ value: true|false, type: "Flag" }`.
- Prometheus `oxphp_shared_flag_value{flag_id="…"}` gauge (0 или 1).
- Общереестровые метрики покрывают Flag через метку `type="Flag"`.

## Когда не использовать

- **Многосостоятельная логика.** Flag двухзначен. Если нужен idle/busy/done или любой трёхсостоятельный автомат, берите `Shared\Counter` (используйте целочисленные enum-значения) или `Shared\Mutex` над массивом в стиле enum.
- **Ожидание перехода.** Flag не блокирует. Парьте с `Shared\Channel` (или с `Shared\Counter`, опрашиваемым через `compareAndSet`), когда воркер должен ждать, пока флаг не перевернётся.
- **Подсчёт событий.** Flag — не счётчик. Используйте `Shared\Counter` для подсчётов.

## См. также

- [Разделяемое состояние](shared-state.md) — обзор и ментальная модель.
- [Shared\Counter](shared-counter.md) — когда нужно больше чем on/off.
- [Shared\Once](shared-once.md) — когда вычисленное один раз значение богаче, чем bool.
- [Shared\Mutex](shared-mutex.md) — когда переключение флага должно co-commit с другим состоянием.
