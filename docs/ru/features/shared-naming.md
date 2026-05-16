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

`Map::set()`, сброс значения `Mutex` через `with`, `Once::init()`.
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

`Channel::isClosed()`, `Mutex::isPoisoned()`, `Once::isInitialized()`,
`Flag::isSet()`.

Никаких голых глаголов (`test`, `check`) и доменных имён
(`poisoned`, `closed`). Префикс `is` отмечает чистое чтение
boolean-свойства.

### 5. Fallible-операция — префикс `try*()`

`Channel::trySend()`, `Channel::tryRecv()`, `Mutex::tryWith()`,
`Map::trySet()`.

Семантика: операция, которая может законно не выполниться — потому
что блокирующий вариант пришлось бы ждать, потому что не выполнено
логическое предусловие, или потому что исчерпана capacity — и
сообщает о неудаче возвращаемым `bool` / `null` вместо исключения.
Используйте `try*`, когда вызывающая сторона должна отличать «не
удалось» от «удалось» без `try`/`catch`.

Под одним префиксом живут два разных под-смысла; оба намеренные и
совпадают с использованием `try_*` в Rust stdlib:

- **Non-blocking вариант блокирующей операции.** `trySend` / `tryRecv`
  / `tryWith` эквивалентны блокирующему варианту с нулевым
  дедлайном (would-block → `false` / `null`, без `TimeoutException`).
  Параллель: `mpsc::Sender::try_send`, `Mutex::try_lock`.
- **Conditional-success операция.** `Map::trySet` успешен только
  если ключ отсутствовал; коллизия → `false`, без исключения.
  Параллель: `HashMap::try_insert`.

Объединяющий инвариант: `try*` возвращает значение, а не бросает
исключение. Не изобретайте альтернативных имён (`setIfAbsent`,
`lockNonblocking`, `pushIfRoom`).

### 6. Compare-and-swap — `compareAndSet()`

`Atomic::compareAndSet()`, `Flag::compareAndSet()`. Всегда возвращает
`bool` (обмен произошёл или нет).

### 7. Замена с возвратом предыдущего — `swap()`, `exchange()`

`Atomic::swap()` для int, `Flag::exchange()` для bool.

Асимметрия имён — историческая и намеренная: `swap` читается как
«поменять содержимое двух мест» в низкоуровневых контекстах;
`exchange` чаще встречается в PHP для «обменять на новое». Оба
возвращают предыдущее значение.

### 8. Atomic RMW с возвратом предыдущего — префикс `fetch*()`

`Atomic::fetchAdd()`, `fetchSub()`, `fetchAnd()`, `fetchOr()`,
`fetchXor()`.

Префикс `fetch` кодирует контракт возврата: **значение до операции**.
Это контрастирует с `Counter::add()` / `Counter::inc()` /
`Counter::dec()`, которые возвращают **новое** значение
(агрегирующий счётчик в стиле LongAdder).

При добавлении новых RMW-методов сначала выбирайте контракт, потом
имя:

- возврат prev → `fetchVerb(args)`
- возврат new → голый `verb(args)`

Не смешивайте.

### 9. Сброс к значению по умолчанию — `clear()`

`Map::clear()`, `Flag::clear()` (в смысле «выставить в false»).
Возвращает `void` для простого сброса; возвращает предыдущее значение,
когда вызывающая сторона разумно может его захотеть (`Flag::clear()`,
`Counter::reset()`).

`Counter::reset()` — задокументированное исключение, сохраняющее
`reset`: конвенция LongAdder — `sumThenReset`, переименование ввело
бы в заблуждение пользователей, знакомых с Java `LongAdder` или Go
`atomic.Int64.Swap(0)`.

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
| Boolean-свойство            | `is*(): bool`            | `Flag::isSet`, `Channel::isClosed`      |
| Fallible-операция           | `try*()`                 | `Channel::trySend`, `Map::trySet`       |
| Compare-and-swap            | `compareAndSet()`        | `Atomic::compareAndSet`                 |
| Замена с возвратом prev     | `swap()` / `exchange()`  | `Atomic::swap`, `Flag::exchange`        |
| Atomic RMW, возврат prev    | `fetch*()`               | `Atomic::fetchAdd`                      |
| Atomic RMW, возврат new     | голый глагол             | `Counter::inc`, `Counter::add`          |
| Сброс к значению по умолч.  | `clear()`                | `Map::clear`, `Flag::clear`             |
| Идентификатор в реестре     | `id(): int`              | каждый тип `Shared\*`                   |

## Добавление нового типа `Shared\*`

Предлагая новый примитив, пройдитесь по этому чеклисту перед мёрджем:

- [ ] Каждый метод соответствует строке в шпаргалке или имеет ADR,
  объясняющий исключение (см. `Atomic::load/store` и `Counter::reset`
  выше).
- [ ] Если тип содержит коллекцию значений — он реализует
  `\Countable` и предоставляет `count(): int`.
- [ ] Методы чтения — `get` или `load` (только для атомиков).
- [ ] Boolean-геттеры используют префикс `is*`.
- [ ] Fallible-варианты (non-blocking, conditional-success, capacity)
  используют префикс `try*` и возвращают `bool` / `null` вместо
  исключения.
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
