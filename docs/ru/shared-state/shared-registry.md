---
title: Shared\Registry
description: Именованные process-global хэндлы для Shared\* — одна Map / Counter / Channel, на которую сходятся все воркеры и все запросы, без внешнего хранилища.
---

# Shared\Registry

`OxPHP\Shared\Registry` — это **именованный** компаньон остальной части `OxPHP\Shared\*`. Там, где `new Shared\Map()` создаёт анонимную запись, разделяемую только по проброшенному хэндлу (`use`-захват, async-файберы, вложенность), `Registry::map('cache', fn() => new Shared\Map(...))` привязывает запись под строковым ключом — и любой вызывающий `Registry::map('cache', …)` на любом воркер-потоке, в любом запросе, получает ту же самую запись.

Это ответ на вопрос *«как разделить один `Shared\Map` между всеми воркерами или между всеми запросами в традиционном режиме?»*. Остальные `Shared\*`-типы по-прежнему являются правильной единицей изменяемого состояния; `Registry` — это просто как навесить имя на одну из них.

## Ментальная модель

```
 worker #1     worker #2     worker #3
   │              │              │
   └───── Registry::map('cache', $factory) ─────┐
                                                 ▼
                       ┌─────────────────────────────────────┐
                       │ SharedRegistry (process-global)     │
                       │                                     │
                       │   names: { "cache" → Bound(Arc<E>) }│
                       │   entries: { id=7: Map { … } }      │
                       └─────────────────────────────────────┘
```

- **Первый** вызывающий `Registry::map($key, $factory)` для непривязанного ключа выполняет фабрику и пинит полученную запись под именем.
- Каждый последующий вызывающий — тот же поток, другие воркеры, более поздние запросы — получает ту же запись. Фабрика **не** запускается повторно; на хите она игнорируется.
- Конкурентные первые касания блокируются на гейте ключа: ровно **один** поток выполняет фабрику, остальные ждут и получают запись победителя. Никакого двойного приобретения ресурсов для пулов соединений.

Идентичность-по-имени дополняет идентичность-по-хэндлу. Анонимные (`new Shared\*()`) и именованные записи сосуществуют в одном process-global реестре. Name-индекс просто добавляет строковый lookup поверх.

## Быстрый старт — один счётчик на все воркеры

```php
<?php
// worker.php — entry script в worker mode, выполняется один раз на воркер-поток
require __DIR__ . '/vendor/autoload.php';

$requests = OxPHP\Shared\Registry::counter(
    'request-counter',
    fn() => new OxPHP\Shared\Counter(),
);

oxphp_worker(function () use ($requests) {
    $n = $requests->add();          // атомарно между ВСЕМИ воркерами — один общий int64
    header('X-Request-Count: ' . $n);
    echo "hello\n";
});
```

Сравните с паттерном захваченного хэндла (`$x = new Shared\Counter()` в bootstrap). Тот паттерн создаёт один счётчик **на воркер-поток** — каждый воркер выполняет свой bootstrap и получает свою анонимную запись. Совокупные счёты расходятся в N раз (N = размер пула). `Registry::counter('request-counter', …)` вместо этого сводит все воркеры к одной записи, поэтому счёт — это настоящий итог.

Та же форма работает и в традиционном режиме (без `WORKER_MODE_ENABLED`). Первый запрос, который трогает `'request-counter'`, создаёт запись; каждый последующий запрос — на любом воркер-потоке — её видит. Это история замены APCu на одном хосте с типизированными примитивами и атомарными операциями вместо `apcu_fetch` / `apcu_store`.

## Справочник API

```php
namespace OxPHP\Shared;

final class Registry
{
    // Типизированный get-or-create. На хите фабрика игнорируется; на промахе
    // выполняется максимум один раз между всеми воркерами (block-losers) и
    // обязана вернуть свежий экземпляр совпадающего типа.
    public static function map(string $key, callable $factory): Map;
    public static function counter(string $key, callable $factory): Counter;
    public static function atomic(string $key, callable $factory): Atomic;
    public static function flag(string $key, callable $factory): Flag;
    public static function once(string $key, callable $factory): Once;
    public static function mutex(string $key, callable $factory): Mutex;
    public static function channel(string $key, callable $factory): Channel;
    public static function pool(string $key, callable $factory): Pool;

    // Untyped escape hatch — возвращает то, что привязано (без guard'а типа).
    public static function global(string $key, callable $factory): Shareable;

    // Управление namespace — оперирует name-индексом, НЕ объектами.
    public static function remove(string $key): bool;
    public static function keys(): array;            // list<string>

    // Layer-wide интроспекция.
    public static function memoryUsage(): int;       // оценочные байты, все Shared\* записи
    public static function count(): int;             // живые Shared\* записи (именованные + анонимные)
}
```

| Метод | Возвращает | Использование |
|---|---|---|
| `map` / `counter` / `atomic` / `flag` / `once` / `mutex` / `channel` / `pool` | запрошенный `Shared\*` тип | Основная поверхность. Type-guard на хите; валидация типа возврата фабрики. |
| `global` | `Shareable` | Untyped get-or-create. Берите только когда действительно не знаете тип заранее. |
| `remove` | `bool` | Снять привязку имени + pin. **Не уничтожает объект.** |
| `keys` | `list<string>` | Текущие привязанные ключи (только Bound; Creating-слоты в полёте не перечислены). |
| `memoryUsage` | `int` | Process-wide оценочные байты — см. [Память и интроспекция](#память-и-интроспекция). |
| `count` | `int` | Process-wide живые записи (именованные **и** анонимные). |

`Registry` — статический фасад: `new Registry()` бросает `Shared\SharedException`.

## Жизненный цикл — pinned по умолчанию

Привязанный ключ держит **сильную** ссылку на свою запись; запись жива на протяжении жизни процесса, если только вы явно не вызовете `remove(key)` или процесс не остановится. Это сделано осознанно: в традиционном режиме, где каждый запрос создаёт свои PHP-хэндлы и они умирают в конце запроса, pin name-индекса — это *единственная* причина, по которой запись переживает межзапросный интервал.

Инвалидируйте *содержимое* именованной записи мутацией in-place — `$cache->clear()`, `$counter->set(0)`, `$bucket->remove($k)` — а не удалением имени. Мутация разделяется по ссылке: каждый держатель того же ключа видит изменение мгновенно.

### `remove` — это управление namespace, а не уничтожение объекта

`remove($key)` снимает привязку и pin. Сама запись жива, пока на неё ссылается любой другой хэндл (захваченная bootstrap-переменная, значение, вложенное в другой `Shared\Map`, выполняющийся `oxphp_async`). Когда уходит последний хэндл, запись самодерегистрируется как обычно.

После `remove` ключ свободен. Следующий `Registry::map($key, …)` создаёт **новую** запись — другой id. **Захваченные хэндлы предыдущей привязки продолжают работать на старой (теперь анонимной) записи; они не сходятся автоматически на новой.**

```php
$cache = Registry::map('cache', fn() => new Shared\Map());
$id_a = $cache->id();

Registry::remove('cache');
$cache->set('x', 1);                        // всё ещё мутирует СТАРУЮ запись — норм, но это больше не "cache"

$fresh = Registry::map('cache', fn() => new Shared\Map());
$id_b = $fresh->id();                       // другой id — это новая запись

assert($id_a !== $id_b);
assert($cache->get('x') === 1);              // СТАРАЯ запись сохранила значение
assert($fresh->get('x') === null);           // НОВАЯ запись пуста
```

Если вы ротируете ключи (per-tenant записи, которые приходят и уходят, версионирование ключей), адресуйте их по имени на каждый вызов (`Registry::map($key, …)` per request), а не захватывайте хэндл один раз в bootstrap. Захваченные хэндлы + ротация ключей расходятся молча; адресация-по-имени сходится на текущей привязке.

`remove` возвращает `true`, если связанный ключ был снят, `false`, если ключ отсутствовал.

## Ошибки

| Исключение | Когда |
|---|---|
| `Shared\TypeException` | Типизированный метод на ключе, привязанном к другому типу; фабрика вернула неверный `Shared\*`-тип или не-`Shareable`. |
| `Shared\CapacityException` | Создание превысит капы `SHARED_MAX_ENTRIES` / `SHARED_MAX_BYTES`. |
| `Shared\DeadlockException` (реентрант) | `Registry::map($key, …)` для *того же* `$key` изнутри его же фабрики на том же потоке. |
| `Shared\DeadlockException` (cross-key cycle) | Ожидание дольше 30 секунд на чужом `Creating`-слоте — наиболее вероятная причина: фабрика A держит K1, ждёт K2, чья фабрика держит K2 и ждёт K1 у потока B. Отдельное сообщение от реентранта. |
| `Shared\SharedException` (draining) | Сервер завершает работу — реестр отказывает в новых `acquire` и `bind`. Ожидаемо при graceful shutdown; это не баг кода. |
| `Shared\SharedException` (bind race) | Параллельный creator уже встал в слот пока наша фабрика работала (наш entry НЕ запинен под ключом). Повторите вызов. |
| `\InvalidArgumentException` (SPL) | Пустой `$key`. Валидация аргумента, отдельно от доменных type-ошибок. |
| *(исключение фабрики)* | Если фабрика бросает, слот аборнут (Creating → absent, ждуны просыпаются на ретрай) и исходное исключение пробрасывается создателю. |

`Shared\DeadlockException` наследуется от `OxPHP\Async\AsyncException` — `catch (AsyncException)` ловит его вместе с bounded-wait таймаутами в других местах `Shared\*`. Два случая `DeadlockException` различаются по сообщению (`"reentrant get-or-create"` vs `"waited too long … cross-key cycle"`).

## Память и интроспекция

`Registry::memoryUsage()` и `Registry::count()` сообщают **весь Shared\*-слой**, а не только именованные записи. Анонимные записи, созданные через `new Shared\*()` (основная часть текущего использования `Shared\*` — захваты bootstrap, in-flight значения внутри `Map` и `Channel`, захваты async-файберов) — включены.

Это сделано осознанно. Эти два числа существуют для capacity / OOM мониторинга; такой мониторинг обязан видеть per-worker и in-flight анонимное состояние, а не только именованный namespace. Следствия:

- Оба числа **транзиентны** — они растут и падают вместе с in-flight запросами и per-worker хэндлами.
- `Registry::count()` **не равен** `count(Registry::keys())`. `keys()` — это только именованный namespace.
- `memoryUsage()` — это **статическая учётная оценка**, а не реальный RSS. Это то же число, которое ограничивает `SHARED_MAX_BYTES`. Для настоящего heap-следа используйте heap-профайлер (`heaptrack`, `jemalloc_stats_print`, `mi_stats_print`) или метрики памяти контейнера.

Per-entry детали — id, тип, refcount, стоимость байт — живут на [внутреннем интроспекционном эндпоинте](../operations/internal-server.md) по адресу `/__ox_shared/entries`. Намеренно нет per-entry PHP API, чтобы не дублировать эту поверхность.

## Когда не использовать

- **Cross-process, cross-host.** Реестр живёт внутри одного OxPHP-процесса. Несколько OxPHP-инстансов его не разделяют. Используйте Redis / NATS / ваш существующий брокер; см. [Миграция на внешнее хранилище](migrating-to-external-store.md).
- **Долговечность между рестартами.** Реестр испаряется при выходе процесса. Персистите через то же внешнее хранилище.
- **Высокая текучесть эфемерных ключей.** Pinned-по-умолчанию семантика означает, что динамические ключи, генерируемые на запрос, текут до `remove`. Ограничены капами `SHARED_MAX_*`, но всё равно дурной тон. Для короткоживущего per-request состояния используйте обычную PHP-переменную.
- **Примитив инвалидации кэша.** `remove($key)` — это для удаления имени, не для «очистить кэш». Инвалидируйте содержимое in-place (`$map->clear()`, `$map->remove($member_key)`); привязка имени переживает.

## См. также

- [Shared State](shared-state.md) — обзор слоя, идентичность-по-хэндлу, когда `new Shared\*()` — правильный инструмент.
- [Shared\Map](shared-map.md), [Shared\Counter](shared-counter.md), [Shared\Pool](shared-pool.md), … — типизированные примитивы, которые возвращает `Registry`.
- [Shared Observability](shared-observability.md) — `/__ox_shared/*` JSON API и Prometheus-метрики.
- [Миграция на внешнее хранилище](migrating-to-external-store.md) — когда перерастаете один процесс.
