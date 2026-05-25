---
title: Shared\Counter
description: Атомарный int64-аккумулятор, разделяемый между PHP-воркерами — lock-free знаковое сложение, атомарный обмен, compare-and-set, оконный сброс через set(0).
---

# Shared\Counter

`OxPHP\Shared\Counter` — это общепроцессное атомарное 64-битное знаковое целое, специализированное под **аккумуляцию**: подсчёт событий, суммирование дельт, оконные итоги. Каждая операция lock-free; два воркера, складывающих конкурентно, никогда не теряют тик.

Для произвольного атомарного состояния, которое должно *синхронизировать другую память* — конечных автоматов, version stamps, seqlock'ов, bitflag-масок — используйте [`Shared\Atomic`](shared-atomic.md).

## Обзор

- **Атомарный int64.** Диапазон `−9_223_372_036_854_775_808 … 9_223_372_036_854_775_807`. Переполнение оборачивается.
- **Lock-free.** `add` компилируется в единственный `fetch_add`.
- **Всегда Relaxed.** Операции атомарны (тики не теряются, рваных чтений нет), но *не устанавливают happens-before* с другой памятью. Counter — это статистика, а не точка синхронизации; если нужен порядок, используйте `Shared\Atomic`.
- **Shareable.** Экземпляры можно хранить внутри `Shared\Map` / `Shared\Channel` и передавать файберам через захваты `use`.

## Справочник API

```php
namespace OxPHP\Shared;

final class Counter implements Shareable
{
    public function __construct(int $initial = 0);

    public function get(): int;                            // текущее
    public function set(int $value): int;                  // возвращает предыдущее; set(0) = сброс окна
    public function add(int $delta = 1): int;              // возвращает новое; add()=+1, add(-1)=декремент
    public function compareAndSet(int $expect, int $new): bool;

    public function id(): int;
}
```

| Метод            | Возвращает   | Применение                                                       |
|------------------|--------------|------------------------------------------------------------------|
| `get`            | текущее      | Чтение без мутации.                                              |
| `set`            | предыдущее   | Атомарный обмен; `set(0)` — чтение-и-обнуление на закрытии окна. |
| `add`            | новое        | `add()` инкрементирует на 1, `add(-1)` декрементирует, иначе любая дельта. |
| `compareAndSet`  | bool         | Ограниченные / насыщающиеся счётчики (потолок, пол) через CAS-цикл. |
| `id`             | id реестра   | Логи, трассировки, корреляция `/__ox_shared/entry?id=…`.         |

## Примеры

### Счётчик запросов на воркер

```php
<?php
$requests = new OxPHP\Shared\Counter();

oxphp_worker(function () use ($requests) {
    $count = $requests->add();          // +1, возвращает новый итог
    header("X-Request-Count: {$count}");
    echo "ok";
});
```

### Роллап по окну

```php
<?php
$hits = new OxPHP\Shared\Counter();

// Каждые N минут в вашем cron/worker loop:
$prev = $hits->set(0);                   // атомарно читает и обнуляет
logWindowMetric($prev);
```

### Ограниченный счётчик (CAS-цикл)

```php
<?php
$slots = new OxPHP\Shared\Counter();
$cap   = 100;

// Занять слот только пока не достигнут потолок.
do {
    $cur = $slots->get();
    if ($cur >= $cap) {
        // заполнено — отказ
        break;
    }
} while (!$slots->compareAndSet($cur, $cur + 1));
```

### Массовая аккумуляция

```php
<?php
$bytes = new OxPHP\Shared\Counter();

// Просуммировать пакет в PHP, затем один атомарный add (один FFI-вызов).
$deltas   = array_map(fn ($req) => strlen($req['body']), $batch);
$newTotal = $bytes->add(array_sum($deltas));
```

## Семантика и подводные камни

- **`set()` возвращает предыдущее значение, затем сохраняет — атомарно.** `set(0)` — это паттерн snapshot-and-zero (`LongAdder::sumThenReset`); `set($n)` задаёт любую новую базу.
- **Relaxed ordering.** Каждая операция атомарна, но Counter не публикует другую память. Если читатель должен увидеть данные, записанные писателем *до* инкремента целого, — это синхронизация, берите [`Shared\Atomic`](shared-atomic.md) с `Ordering::Release`/`Acquire`.
- **`compareAndSet` — Relaxed/Relaxed и не принимает аргументов ordering.** Он корректен для решений, принимаемых по собственному значению счётчика (потолок, пол, claim-by-value). CAS, публикующий другое состояние, относится к `Shared\Atomic`.
- **Переполнение оборачивается.** Добавление сверх `INT_MAX` возвращает к `INT_MIN`. Для счётчиков, работающих месяцами с тысячами тиков в секунду, держите значение в диапазоне десятков триллионов или периодически сбрасывайте.
- **Нет дробных значений.** Считаете байты и нужны усреднения с точностью float? Отслеживайте числитель (Counter) и знаменатель (Counter) отдельно и делите при чтении.

## Исключения

| Исключение              | Бросается                                                    |
|-------------------------|--------------------------------------------------------------|
| `StaleHandleException`  | Любой метод на хэндле, чья запись реестра была вытеснена.    |
| `UninitializedException`| `id()` на обёртке, которая не завершила `__construct`.        |

Counter'ы никогда не бросают на переполнение или экстремальные значения — они оборачиваются.

## Наблюдаемость

Полную экскурсию см. в [Shared Observability](shared-observability.md). Краткие отсылки:

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
- [Shared\Atomic](shared-atomic.md) — generic атомарный int64 с CAS, swap и полным контролем memory ordering.
- [Shared\Map](shared-map.md) — когда счёт ключевой (`Map<string, Counter>`).
- [Shared\Flag](shared-flag.md) — когда значение просто on/off.
- [Shared\Mutex](shared-mutex.md) — когда счётчик должен обновляться в ногу с другими полями.
