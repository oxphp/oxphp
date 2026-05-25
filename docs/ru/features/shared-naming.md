---
title: Naming-конвенции Shared\*
description: Конвенции именования для API конкурентности OxPHP\Shared\* — канонический словарь методов (get/set, политики ожидания try*/timeout, is*, fetch*, compareAndSet), которому следует каждый общий примитив.
---

# Naming-конвенции `OxPHP\Shared\*`

Пространство имён `OxPHP\Shared\*` — это API конкурентности уровня
приложения: `Atomic`, `Counter`, `Flag`, `Map`, `Channel`, `Mutex`,
`Once`, `Pool`. Имена методов подчиняются единому набору правил,
чтобы пользователи могли предсказывать API, не заглядывая в документацию
каждого типа.

Этот документ — канонический справочник. Новые примитивы и изменения
существующих ОБЯЗАНЫ ему следовать.

## Правила

### 1. Чтение значения — `get()`

PHP-конвенция. Используется в `Map::get()`, `Counter::get()`,
`Once::get()`.

`Atomic::load(?Ordering $order = null)` — намеренное исключение: само
наличие метода с аргументом ordering сообщает, что это часть контракта
memory model, а не обычный геттер.

### 2. Запись значения — `set()`, для атомиков `store()`

`Map::set()`, сброс значения `Mutex` через `with`, `Once::getOrInit()`.
`Atomic::store($value, ?Ordering)` симметричен `load` по той же причине.

### 3. Количество элементов — `count(): int` + `\Countable`

Каждый контейнер предоставляет `count(): int` и реализует `\Countable`.
Это позволяет `count($obj)` работать нативно:

```php
$ch  = new OxPHP\Shared\Channel(1024);
$map = new OxPHP\Shared\Map();
$pool = new OxPHP\Shared\Pool($factory);

count($ch);    // буферизованные элементы
count($map);   // записи
count($pool);  // всего живых слотов (in-use + idle)
```

Никаких `size()`, `len()` или `pending()` — они запрещены в публичном
API независимо от того, из какого языка взялась мышечная память
автора.

### 4. Boolean-геттер — префикс `is*()`

`Channel::isClosed()`.

Никаких голых глаголов (`test`, `check`) и доменных имён
(`closed`). Префикс `is` отмечает чистое чтение
boolean-свойства.

Тип, чьё состояние богаче одного булева, выставляет его методом
`status()`, возвращающим enum, а не геттером `is*()` — так сделаны
`RecvResult::status()` у `Channel` и `Once::status(): Once\Status`
(Uninitialized/Pending/Ready/Poisoned). Берите `status()`, когда у
ответа больше двух вариантов.

`Mutex` намеренно **не** предоставляет `isCorrupted()` — порча
sticky, невосстановима и всплывает через `CorruptedMutexException`
на следующем захвате. Никакого полезного действия по результату
проверки, кроме повторного захвата и catch, нет.

### 5. Wait-policy трихотомия — `try*` / голое имя / `*Timeout`

Блокирующие примитивы (Channel, Mutex) выражают **wait policy**
через имя метода, а не через перегруженный аргумент `?float $timeout`:

| Суффикс       | Поведение                                                     | Примеры                                                  |
|---------------|---------------------------------------------------------------|-----------------------------------------------------------|
| `try*`        | Non-blocking; сразу сообщает вариант неудачи.                 | `Channel::trySend`, `Channel::tryRecv`, `Mutex::tryWithLock` |
| (голое имя)   | Ждать вечно (или пока request fiber не будет отменён).        | `Channel::send`, `Channel::recv`, `Mutex::withLock`       |
| `*Timeout`    | Ограниченное ожидание. Принимает обязательный `int $ms > 0`.   | `Channel::sendTimeout`, `Channel::recvTimeout`, `Mutex::withLockTimeout` |

Трихотомия выносит три неоднозначные политики (`null` = вечно, `0`
= try, положительное = ограниченное) из одного параметра в три
метода с самодокументирующимися именами. Аргумент `$ms` в `*Timeout`-
методах **строго положительный** — ноль, отрицательные значения,
non-int и отсутствие поднимают `OxPHP\Shared\TypeException` на
бридже.

`try*` несёт ещё один под-смысл, существовавший до трихотомии:

- **Conditional-success операция.** `Map::trySet` успешен только
  если ключ отсутствовал; коллизия → `false`, без исключения.
  Параллель: `HashMap::try_insert`.

Объединяющий инвариант для `try*`: он либо возвращает Result со
значением (Channel), либо бросает `ContentionException` (Mutex). Он
никогда не возвращает `null`, чтобы закодировать «не удалось» — это
был старый API и порождал ту самую null-coalescing неоднозначность,
которую трихотомия устраняет.

### 6. Compare-and-swap — `compareAndSet()`

`Atomic::compareAndSet()`, `Flag::compareAndSet()`. Всегда возвращает
`bool` (обмен произошёл или нет).

### 7. Замена с возвратом предыдущего — `swap()`

`Atomic::swap()` для int, `Flag::swap()` для bool. Возвращает
предыдущее значение.

### 8. Atomic RMW с возвратом предыдущего — префикс `fetch*()`

`Atomic::fetchAdd()`, `fetchSub()`, `fetchAnd()`, `fetchOr()`,
`fetchXor()`.

Префикс `fetch` кодирует контракт возврата: **значение до операции**.
Это контрастирует с `Counter::add()`, который возвращает **новое**
значение (агрегирующий счётчик в стиле LongAdder).

При добавлении новых RMW-методов сначала выбирайте контракт, потом
имя:

- возврат prev → `fetchVerb(args)`
- возврат new → голый `verb(args)`

Не смешивайте.

### 9. Сброс к значению по умолчанию — `clear()`

`Map::clear()` — опустошить контейнер; возвращает `void`.

У `Counter` нет `clear()` — оконный сброс это `set(0)`.
`Counter::set()` — задокументированное исключение, возвращающее
**предыдущее** значение (а не `void`): это атомарный обмен, а
`set(0)`, читающий прежний итог, — идиома LongAdder `sumThenReset`.
(`Atomic` называет ту же операцию `swap()`; Counter
сохраняет `set`, потому что `set($n)` естественно читается для
инициализации и оконного сброса.)

### 10. Идентификация в реестре — `id(): int`

Каждый инстанс `Shared\*` предоставляет `id(): int` для логов и
observability-эндпоинта `/__ox_shared/entries/:id`.

## Шпаргалка

| Концепт                     | Каноническое имя         | Примеры                                 |
| --------------------------- | ------------------------ | --------------------------------------- |
| Чтение значения             | `get()`                  | `Map::get`, `Counter::get`              |
| Чтение атомика              | `load($order)`           | `Atomic::load`                          |
| Запись значения             | `set()`                  | `Map::set`                              |
| Запись атомика              | `store($v, $order)`      | `Atomic::store`                         |
| Количество элементов        | `count(): int`           | `Map::count`, `Channel::count`, `Pool::count` |
| Наличие ключа/элемента      | `has($key): bool`        | `Map::has`                              |
| Boolean-свойство            | `is*(): bool`            | `Channel::isClosed`                     |
| Non-blocking ожидание       | `try*()`                 | `Channel::trySend`, `Mutex::tryWithLock`, `Map::trySet` |
| Бесконечное ожидание        | голый глагол             | `Channel::send`, `Channel::recv`, `Mutex::withLock`     |
| Ограниченное ожидание       | `*Timeout(int $ms)`      | `Channel::sendTimeout`, `Mutex::withLockTimeout`        |
| Compare-and-swap            | `compareAndSet()`        | `Atomic::compareAndSet`                 |
| Замена с возвратом prev     | `swap()`                 | `Atomic::swap`, `Flag::swap`            |
| Atomic RMW, возврат prev    | `fetch*()`               | `Atomic::fetchAdd`                      |
| Atomic RMW, возврат new     | голый глагол             | `Counter::add`                          |
| Сброс к значению по умолч.  | `clear()`                | `Map::clear`                            |
| Идентификатор в реестре     | `id(): int`              | каждый тип `Shared\*`                   |

## Добавление нового типа `Shared\*`

Предлагая новый примитив, пройдитесь по этому чеклисту перед мёрджем:

- [ ] Каждый метод соответствует строке в шпаргалке или имеет ADR,
  объясняющий исключение (см. `Atomic::load/store` и `Counter::set`
  выше).
- [ ] Если тип содержит коллекцию значений — он реализует
  `\Countable` и предоставляет `count(): int`.
- [ ] Методы чтения — `get` или `load` (только для атомиков).
- [ ] Boolean-геттеры используют префикс `is*`.
- [ ] Wait-policy варианты следуют трихотомии `try*` / голое имя /
  `*Timeout(int $ms)`. Вариант `*Timeout` принимает `int $ms > 0` и
  отклоняет ноль / отрицательные / non-int значения через
  `TypeException`. Conditional-success операции (`Map::trySet`)
  сохраняют префикс `try*` и могут возвращать `bool`; новые
  wait-policy `try*`-методы возвращают либо Result со значением,
  либо бросают доменное исключение — никогда `null`-в-роли-кода.
- [ ] Никаких `len`, `size`, `pending`, `test`, `setIfAbsent` и других
  ad-hoc имён.
- [ ] Доменные глаголы (`evict`, `drain`, `flush` и т. п.) появляются
  только когда у концепта нет канонического аналога в шпаргалке.

## Observability-имена отстают от PHP-API

Operator-facing поверхность — имена Prometheus-метрик и JSON в
`/__ox_shared/entries/:id` — это отдельный контракт от PHP-API. Его
переименование ломает дашборды и алерты. Чтобы не оставлять тихую
несогласованность, затронутые имена эмитятся **дважды** в течение
одного релизного цикла:

| Поверхность   | Deprecated (всё ещё эмитится)  | Каноническое            |
| ------------- | ------------------------------ | ----------------------- |
| Prometheus    | `oxphp_shared_channel_pending` | `oxphp_shared_channel_count` |
| Prometheus    | `oxphp_shared_pool_size`       | `oxphp_shared_pool_count`    |
| JSON entry    | `Channel.pending`              | `Channel.count`             |
| JSON entry    | `Pool.size`                    | `Pool.count`                |

`# HELP`-строки старых метрик помечены префиксом `(deprecated, removed
in a future release; use *_count)`, а плагин `ox_shared` пишет
startup-`WARN` всякий раз, когда включены introspection или metrics.

Переключите дашборды и правила алертов на `_count`-имена до закрытия
цикла deprecation. После удаления будут эмититься только канонические
имена, и панели Prometheus/Grafana, ссылающиеся на старые, начнут
возвращать пустые ряды.

## Стабильность

Эти правила — часть 1.0-контракта `OxPHP\Shared\*`. После релиза 1.0
переименования становятся breaking-изменениями и требуют цикла
deprecation. До 1.0 правила всё равно обязательны — новые методы,
нарушающие их, будут отклонены на ревью.
