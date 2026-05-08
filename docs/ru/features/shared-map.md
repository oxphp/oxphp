---
title: Shared\Map
description: Общепроцессный конкурентный hash-map для координации состояния между PHP-воркерами — атомарные чтения, пакетные записи, cycle-safe вложенные Shareable-значения.
---

# Shared\Map

`OxPHP\Shared\Map` — это конкурентный map со строковыми ключами, живущий в общем реестре и видимый каждому PHP-воркеру в процессе. Это примитив первого выбора, когда двум воркерам — или обработчику запроса и фоновой задаче — нужно разделять изменяемое состояние, переживающее жизненный цикл запроса.

## Обзор

- **String → mixed.** Ключи — PHP-строки. Значения — любой скаляр, массив (включая вложенные массивы) или другой экземпляр `Shareable`.
- **Конкурентный.** Записи от разных воркеров не требуют внешней блокировки. Операции per-key атомарны на уровне шарда.
- **Cycle-safe.** Сохранение `Shareable`, который в итоге достигнет обратно этого Map, отклоняется с `CycleException` до какой-либо мутации — нет утечек на отклонённом пути.
- **Опциональный cap per-instance.** Передайте `maxEntries` при конструировании для строгого потолка; перезаписи существующих ключей всегда разрешены, новые ключи отклоняются с `CapacityException` при достижении потолка.
- **Подкреплён реестром.** У каждого Map есть стабильный числовой `id()`; он переживает границы запросов и разделяется по хэндлу.

## Справочник API

```php
namespace OxPHP\Shared;

final class Map implements Shareable
{
    public function __construct(?int $maxEntries = null);

    public function get(string $key, mixed $default = null): mixed;
    public function set(string $key, mixed $value): void;
    public function has(string $key): bool;
    public function remove(string $key): mixed;
    public function clear(): void;
    public function count(): int;
    public function keys(): array;
    public function maxEntries(): ?int;

    public function setIfAbsent(string $key, mixed $value): bool;

    public function setMany(array $kv): int;
    public function getMany(array $keys): array;
    public function removeMany(array $keys): int;

    public function id(): int;
}
```

| Метод          | Применение                                                                           |
|----------------|--------------------------------------------------------------------------------------|
| `__construct`  | Создать с опциональным `maxEntries` cap (null = без ограничения).                    |
| `get`          | Получить по ключу; возвращает `$default`, когда отсутствует (по умолчанию `null`).   |
| `set`          | Вставить или заменить; перезаписывает существующие значения.                         |
| `has`          | Проверка наличия без получения значения.                                             |
| `remove`       | Удалить ключ и вернуть его предыдущее значение (`null`, если отсутствует).            |
| `clear`        | Сбросить каждую запись и отпустить удержание Map на любых вложенных `Shareable`.      |
| `count`        | Текущее количество записей.                                                          |
| `keys`         | Снимок всех ключей на момент вызова. Порядок итерации не определён (порядок шардов). |
| `maxEntries`   | Сообщает настроенный cap (или `null`, если без ограничения).                         |
| `setIfAbsent`  | Атомарная вставка-если-отсутствует. Возвращает `true` при сохранении, `false`, если ключ был. |
| `setMany`      | Массовая вставка; возвращает число пар, сохранённых до любой ошибки.                  |
| `getMany`      | Массовое чтение; отсутствующие ключи возвращаются как `null` в результирующем массиве. |
| `removeMany`   | Массовое удаление; возвращает число фактически удалённых ключей.                      |
| `id`           | Числовой идентификатор реестра; полезен для логов + `/__ox_shared/entry?id=…`.        |

## Примеры

### Разделяемый кэш конфигурации

```php
<?php
$config = new OxPHP\Shared\Map(maxEntries: 1024);

// Прогреть один раз при bootstrap приложения.
$config->setMany([
    'rate_limit.default_rpm' => 600,
    'feature.new_checkout'   => true,
    'timeout.downstream_ms'  => 250,
]);

// Любой обработчик запроса читает без конкуренции.
$rpm = $config->get('rate_limit.default_rpm', 60);
```

### Per-tenant rate limiter

```php
<?php
$buckets = new OxPHP\Shared\Map(maxEntries: 50_000);

$key = "tenant:{$tenantId}";
$created = $buckets->setIfAbsent($key, ['tokens' => 100, 'refill_at' => time() + 60]);
// Если другой запрос опередил, $created равен false — побеждает существующий bucket.

$state = $buckets->get($key);
if ($state['tokens'] === 0) {
    throw new RateLimitException();
}
```

### Координация счётчиков между воркерами

```php
<?php
$counters = new OxPHP\Shared\Map();

// Сохранить Shareable-счётчик под ключом; обработчики во всех воркерах его мутируют.
$counters->set('requests_handled', new OxPHP\Shared\Counter());

// Любой воркер может инкрементировать через сохранённый Shareable.
$counters->get('requests_handled')->inc();
```

## Семантика и подводные камни

### Массивы копируются при чтении

```php
<?php
$m = new OxPHP\Shared\Map();
$m->set('cfg', ['timeout' => 5, 'retries' => 3]);

$cfg = $m->get('cfg');
$cfg['timeout'] = 10;     // мутирует только возвращённую копию
// $m->get('cfg')['timeout'] всё ещё 5
```

Чтобы атомарно обновить значение-массив, сделайте remove + set новой формы или используйте вложенный `Shared\Counter` / `Shared\Map` для полей, меняющихся независимо. `update($key, fn)` на основе замыкания появится в следующем коммите.

### Удержания вложенных Shareable автоматичны

Когда вы делаете `set($key, $shareable)`, Map удерживает Shareable столько, сколько живёт запись. `remove`, `clear` или вытеснение отпускают это удержание. PHP-обёртка, которую вы передали, остаётся валидной независимо:

```php
<?php
$map     = new OxPHP\Shared\Map();
$counter = new OxPHP\Shared\Counter(10);
$map->set('c', $counter);

$retrieved = $map->get('c');           // та же идентичность Shareable
$retrieved->inc();                      // мутация видна и через $counter
echo $counter->get();                   // 11

$map->remove('c');                      // Map отпускает своё удержание
$counter->inc();                        // $counter всё ещё жив через PHP-переменную
```

### Обнаружение циклов отклоняет до мутации

```php
<?php
$a = new OxPHP\Shared\Map();
$b = new OxPHP\Shared\Map();
$a->set('b', $b);                       // ок

try {
    $b->set('a', $a);                   // замыкает петлю
} catch (OxPHP\Shared\CycleException $e) {
    // message: "cycle would form: #… → #… (inserting into #…)"
}

// $b не тронут — частичного состояния нет, утёкших удержаний нет.
$b->has('a');                           // false
```

Вложенные ссылки внутри массивов тоже проверяются:

```php
try {
    $b->set('shape', ['self' => $a]);
} catch (OxPHP\Shared\CycleException $e) { /* отклонено */ }
```

Обходчик ограничен `SHARED_CYCLE_DETECT_DEPTH` (по умолчанию 16) и `SHARED_CYCLE_DETECT_EDGES` (по умолчанию 10 000). Очень большие графы могут всплыть как `CycleException` с `bounds exceeded` в сообщении; поднимайте env-настройки или разбивайте граф.

### Cap per-instance vs перезаписи

```php
<?php
$m = new OxPHP\Shared\Map(maxEntries: 3);
$m->set('a', 1);
$m->set('b', 2);
$m->set('c', 3);

try {
    $m->set('d', 4);                    // 4-й *новый* ключ
} catch (OxPHP\Shared\CapacityException $e) { /* … */ }

$m->set('a', 99);                       // перезапись всегда OK на cap
```

Нарушения cap бросают `CapacityException`. Сообщение называет лимит, чтобы операторы могли поднять его через конструктор.

### Пакетные операции атомарны per-key, а не per-batch

`setMany`, `getMany` и `removeMany` применяют операции по одному ключу за раз. Если `setMany` упирается в `CapacityException` или `CycleException` посреди пути, предыдущие ключи остаются сохранёнными — частичный успех намеренный, соответствует спеке. Оборачивайте весь batch в `Mutex<Map>` (появится в следующем релизе), если нужна семантика «всё-или-ничего».

## Исключения

Все методы, которые могут отказать, бросают подклассы `OxPHP\Shared\SharedException`:

| Исключение             | Бросается                               |
|------------------------|-----------------------------------------|
| `CapacityException`    | `set` / `setIfAbsent` / `setMany` за пределом `maxEntries`. |
| `CycleException`       | Любая запись, которая замкнула бы цикл достижимости (`extends TypeException`). |
| `TypeException`        | Конструктор получает неположительный `maxEntries`; несериализуемые значения (замыкания, ресурсы); нестроковые пакетные ключи. |
| `StaleHandleException` | Вызов метода на хэндле, чья запись реестра была вытеснена. |
| `UninitializedException` | `id()` на обёртке, которая не завершила `__construct`. |

## Наблюдаемость

Каждый Map виден через внутренний API:

- `GET /__ox_shared/summary` — агрегированные счётчики по типам, включая `Map`.
- `GET /__ox_shared/entries` — список всех записей с id / type / refcount / mem_bytes.
- `GET /__ox_shared/entry?id=N` — детали per-instance для Map включают `key_count`, `max_entries`, `saturation` и `sample_keys` (обрезано preview-лимитом).
- `GET /__ox_shared/graph?id=N[&depth=D][&edges=E]` — BFS-обход исходящих Shareable-ссылок. Удобно, когда срабатывает `CycleException` и вы хотите увидеть путь обходчика.

Prometheus показывает per-Map gauge'ы на `/metrics`:

| Метрика                                | Смысл                                     |
|----------------------------------------|-------------------------------------------|
| `oxphp_shared_map_entries{map_id="…"}` | Текущее число ключей.                     |
| `oxphp_shared_map_max_entries{map_id="…"}` | Настроенный cap (0, если без ограничения). |
| `oxphp_shared_map_saturation{map_id="…"}` | `entries / max_entries`, 0 без ограничения. |

Общереестровые gauge'ы (`oxphp_shared_objects_total`, `oxphp_shared_bytes`, `oxphp_shared_capacity_saturation`) покрывают Map автоматически через метку `type="Map"`.

## Конфигурация

| Переменная                       | По умолчанию | Эффект                                                                |
|----------------------------------|---------|-----------------------------------------------------------------------|
| `SHARED_MAX_ENTRIES`             | 100 000 | Глобальный cap на все Shared-записи вместе.                            |
| `SHARED_MAX_BYTES`               | 1 GiB   | Глобальный cap на оценочную память по всем Shared-записям.             |
| `SHARED_CYCLE_DETECT_DEPTH`      | 16      | Максимальная глубина BFS при проверке циклов. Поднимайте для глубоких легитимных графов. |
| `SHARED_CYCLE_DETECT_EDGES`      | 10 000  | Максимум пройденных рёбер при проверке циклов. Поднимайте для плотных легитимных графов. |
| `SHARED_PREVIEW_ARRAY_LIMIT`     | 20      | Число записей, семплируемых в `/entry?id=…` `sample_keys`.             |
| `SHARED_INTROSPECTION_ENABLED`   | true    | Переключает API `/__ox_shared/*`.                                      |

## См. также

- [`Shared\Counter`](shared-counter.md) — атомарное целое; храните внутри Map для per-key hit count.
- [`Shared\Channel`](shared-channel.md) — MPMC-очередь; комплементарно, когда нужны FIFO-пайплайны, а не lookup по ключу.
- [`Shared\Mutex`](shared-mutex.md) — когда нужно строгое взаимное исключение вокруг сохранённого значения.
