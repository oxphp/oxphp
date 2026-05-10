---
title: Shared\Counter
description: Атомарное int64, разделяемое между PHP-воркерами — lock-free инкремент/декремент, compare-and-set и bulk-add для высокопроизводительного счёта.
---

# Shared\Counter

`OxPHP\Shared\Counter` — это общепроцессное атомарное 64-битное знаковое целое. Каждая операция lock-free и линеаризуема; два воркера, инкрементирующих конкурентно, никогда не теряют тик.

## Обзор

- **Атомарный int64.** Диапазон `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`. Переполнение оборачивается.
- **Lock-free.** `inc` / `dec` / `add` компилируются в единственный `fetch_add`; `compareAndSet` — это один CAS.
- **Shareable.** Экземпляры можно хранить внутри `Shared\Map` / `Shared\Channel` и передавать файберам через захваты `use`.

## Справочник API

```php
namespace OxPHP\Shared;

final class Counter implements Shareable
{
    public function __construct(int $initial = 0);

    public function get(): int;
    public function swap(int $value): int;            // возвращает предыдущее
    public function inc(int $by = 1): int;            // возвращает новое
    public function dec(int $by = 1): int;            // возвращает новое
    public function add(int $delta): int;             // возвращает новое
    public function compareAndSet(int $expect, int $new): bool;
    public function addBatch(array $deltas): int;     // возвращает новое

    public function id(): int;
}
```

| Метод            | Возвращает | Применение                                                     |
|------------------|-----------|-----------------------------------------------------------------|
| `get`            | текущее   | Чтение без мутации.                                             |
| `swap`           | предыдущее | Атомарная замена; `swap(0)` — паттерн snapshot-and-zero.       |
| `inc` / `dec`    | новое     | Подсчёт событий; `$by` позволяет шагнуть на N одной атомарной операцией. |
| `add`            | новое     | Любая дельта, положительная или отрицательная.                  |
| `compareAndSet`  | swap?     | Оптимистичные конечные автоматы (idle → busy → done).           |
| `addBatch`       | новое     | Массовая аккумуляция за один FFI round trip.                    |
| `id`             | id реестра | Логи, трассировки, корреляция `/__ox_shared/entry?id=…`.        |

## Примеры

### Счётчик запросов на воркер

```php
<?php
$requests = new OxPHP\Shared\Counter();

oxphp_worker(function () use ($requests) {
    $count = $requests->inc();
    header("X-Request-Count: {$count}");
    echo "ok";
});
```

### Оптимистичный конечный автомат

```php
<?php
$state = new OxPHP\Shared\Counter(initial: 0); // 0=idle, 1=busy, 2=done

if (!$state->compareAndSet(expect: 0, new: 1)) {
    throw new RuntimeException('another worker is already processing');
}

try {
    doWork();
    $state->swap(2);
} catch (Throwable $e) {
    $state->swap(0); // вернуть в idle при ошибке
    throw $e;
}
```

### Роллап по окну

```php
<?php
$hits = new OxPHP\Shared\Counter();

// Каждые N минут в вашем cron/worker loop:
$prev = $hits->swap(0);                // атомарно читает-и-обнуляет
logWindowMetric($prev);
```

### Массовая аккумуляция

```php
<?php
$bytes = new OxPHP\Shared\Counter();

// Подсчитать байты пакета за один FFI-вызов вместо N.
$deltas = array_map(fn ($req) => strlen($req['body']), $batch);
$newTotal = $bytes->addBatch($deltas);
```

## Семантика и подводные камни

- **`swap` возвращает предыдущее значение, а не новое.** Это соответствует `std::atomic<T>::exchange` и `AtomicI64::swap` в Rust. Используйте `get()` после `swap()`, если хотите получить то, что только что записали.
- **`addBatch` не атомарен между элементами.** Под капотом это цикл `fetch_add` — итоговое значение корректно, но другие воркеры видят промежуточные итоги во время пакета. Используйте `Shared\Mutex`, оборачивающий Counter, если нужна видимость всего пакета.
- **Переполнение оборачивается.** Добавление `INT_MAX + 1` возвращает к `INT_MIN`. Для монотонных счётчиков, которые могут работать месяцами с тысячами тиков в секунду, держите значение в диапазоне десятков триллионов или периодически сбрасывайте.
- **Нет дробных значений.** Если вы считаете байты и вам нужны усреднения с точностью float, отслеживайте числитель (Counter) и знаменатель (Counter) отдельно и делите при чтении.

## Исключения

| Исключение              | Бросается                                                    |
|-------------------------|--------------------------------------------------------------|
| `StaleHandleException`  | Любой метод на хэндле, чья запись реестра была вытеснена.    |
| `UninitializedException`| `id()` на обёртке, которая не завершила `__construct`.        |

Counter'ы никогда не бросают на переполнение или экстремальные значения — они оборачиваются.

## Наблюдаемость

Полную экскурсию см. в [Shared Observability](../operations/shared-observability.md). Краткие отсылки:

- `GET /__ox_shared/entry?id=N` показывает `{ value, type: "Counter" }`.
- Prometheus `oxphp_shared_counter_value{counter_id="…"}` gauge отслеживает текущее значение.
- Общереестровые счётчики (`oxphp_shared_ops_total`, `oxphp_shared_objects_total`) покрывают Counter через метку `type="Counter"`.

## Когда не использовать

- **Float'ы или десятичные.** Используйте пару Counter'ов (числитель / знаменатель) или `Shared\Mutex<array{total_cents: int, count: int}>`.
- **Нечисловые события с богатым контекстом.** Если нужно `{count, last_actor, last_reason}` с привязкой к одному ключу, берите `Shared\Map` или `Shared\Mutex`.
- **Межхостовые итоги.** Counter живёт только в процессе. Для агрегации между хостами используйте метрический пайплайн (Prometheus + `rate()` или центральный Redis `INCR`).
- **Долговечность.** Состояние Counter испаряется при остановке сервера. Если итог должен пережить перезапуск, сохраняйте снимки где-то ещё.

## См. также

- [Разделяемое состояние](shared-state.md) — обзор и паттерны миграции.
- [Shared\Map](shared-map.md) — когда счёт ключевой (`Map<string, Counter>`).
- [Shared\Flag](shared-flag.md) — когда значение просто on/off.
- [Shared\Mutex](shared-mutex.md) — когда счётчик должен обновляться в ногу с другими полями.
