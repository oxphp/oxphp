---
title: Миграция Shared\* на внешнее хранилище
description: Когда повышать in-process Shared\*-состояние до Redis, NATS или другого долговечного хранилища — паттерны, семантические разрывы и конкретная форма миграции для каждого Shared\*-типа.
---

# Миграция Shared\* на внешнее хранилище

`OxPHP\Shared\*` живёт в процессе. Это делает его быстрым и без зависимостей, но ограничивает одним хостом и одним временем жизни процесса. Этот документ — пожарный выход: когда вам нужна координация между несколькими хостами или долговечность между перезапусками, вот как переместить каждый Shared-тип на бэкенд Redis или NATS (или подобный) без переписывания приложения.

## Когда мигрировать

Скорее всего, вам не нужно мигрировать. Зона применимости `Shared\*` — single-host, эфемерная, координация с микросекундной задержкой — покрывает больше production-кейсов, чем кажется. Переходите на внешнее хранилище только когда верно одно из следующего:

1. **Вы запускаете больше одного процесса OxPHP.** Несколько хостов, blue/green деплои с пересечением или sidecar'ы, которым нужно видеть то же состояние. `Shared\*` — process-local; он не может пересекать границы процессов.
2. **Состояние должно пережить перезапуски.** Rolling deploy, креш или плановый перезапуск теряет каждую запись `Shared\*`. Если потеря неприемлема (счётчики биллинга, дневные квоты, позиции в очереди задач), вам нужна долговечность.
3. **Состояние должно пережить хост.** Если любой из ваших хостов может исчезнуть, а состояние всё ещё должно существовать, оно живёт где-то кроме этого хоста.
4. **Вы хотите кроссязычных читателей.** Внешнее хранилище можно прочитать фоновым джобом, написанным на Go, метрическим пайплайном или админским инструментом. `Shared\*` — только для PHP.

Если ничего из этого не применимо, in-process примитив почти наверняка правильный выбор. Держите план миграции в заднем кармане, а не на горячем пути.

## Абстракция

Большинство команд берут одну и ту же форму: интерфейс с двумя бэкендами, выбираемыми конфигурацией.

```php
<?php
interface CounterBackend
{
    public function inc(string $key, int $by = 1): int;
    public function get(string $key): int;
    public function reset(string $key): int;
}

final class SharedCounterBackend implements CounterBackend
{
    public function __construct(private OxPHP\Shared\Map $counters) {}

    public function inc(string $key, int $by = 1): int
    {
        $counter = $this->counters->getOrSet(
            $key,
            fn () => new OxPHP\Shared\Counter(),
        );
        return $counter->add($by);
    }

    public function get(string $key): int
    {
        $counter = $this->counters->get($key);
        return $counter?->get() ?? 0;
    }

    public function reset(string $key): int
    {
        $counter = $this->counters->get($key);
        return $counter?->set(0) ?? 0;
    }
}

final class RedisCounterBackend implements CounterBackend
{
    public function __construct(private Redis $redis) {}

    public function inc(string $key, int $by = 1): int
    {
        return (int) $this->redis->incrBy("counter:{$key}", $by);
    }

    public function get(string $key): int
    {
        return (int) ($this->redis->get("counter:{$key}") ?? 0);
    }

    public function reset(string $key): int
    {
        // GETSET атомарен: один round-trip, возвращает предыдущее значение.
        return (int) ($this->redis->getSet("counter:{$key}", 0) ?? 0);
    }
}
```

Подключите выбранный бэкенд один раз при bootstrap и используйте `CounterBackend` везде. Миграция тогда — это перещёлкивание конфигурации, а не переписывание.

## Примечания по типам

У каждого `Shared\*`-типа есть семантические особенности, которые тривиально не переводятся ни в одно внешнее хранилище. Заметки ниже подсвечивают различия и идиоматичные замены.

### `Shared\Counter` → Redis / NATS JetStream KV

- **Redis:** `INCR` / `INCRBY` / `GET`. Атомарно, долговечно и реплицируется в Redis Cluster.
- **NATS JetStream KV:** `KV.put` с CAS на основе ревизий покрывает и `set`, и `compareAndSet`. Инкременты требуют `KV.get` + `KV.update(revision)` в цикле.

Семантические разрывы:

- Массовая аккумуляция — это `add(array_sum($deltas))`, один FFI round trip в `Shared\*`. В Redis предвычислите сумму и сделайте один `INCRBY` (один RTT); в NATS — один `KV.update`.
- Целочисленное переполнение в Redis возвращает ошибку; `Shared\Counter` молча оборачивается.

### `Shared\Flag` → Redis / NATS feature-flag сервис

- **Redis:** `SET` / `GET` / `SETNX` для семантики, похожей на `compareAndSet`. Строковое значение `"1"` / `"0"` работает; boolean'ы чище через `GETSET` + сравнение строк.
- **Выделенный flag-сервис:** (LaunchDarkly, Unleash, ConfigCat) обрабатывает кэш, таргетинг раскатки и audit trail из коробки. Для операционных kill-switch'ей это обычно правильный шаг, как только вы переходите порог `Shared\*`.

Семантические разрывы:

- `swap($new)` → Redis `GETSET`. Атомарно.
- `compareAndSet($expect, $new)` → Lua-скрипт или `WATCH`/`MULTI`. Стоит обернуть в helper.
- Внешние flag-сервисы обычно кэшируют значение локально; ваше чтение не всегда сетевой round trip. Это обычно нормально, но ожидайте eventual consistency на изменениях.

### `Shared\Once` → Bootstrap-таблица в БД

- **Паттерн:** идемпотентный INSERT с unique-ограничением, затем SELECT при конфликте.
- **SQL:** `INSERT INTO once (key, value) VALUES (?, ?) ON CONFLICT (key) DO NOTHING; SELECT value FROM once WHERE key = ?`.
- **Redis:** `SETNX` + `GET`.

Семантические разрывы:

- `Shared\Once::getOrInit(callable)` запускает фабрику в процессе, когда побеждает. Во внешнем хранилище фабрика должна быть идемпотентной (два писателя могут оба её запустить, и побеждает только одно значение) или вам нужна обёртка leader-election.
- `DeadlockException` на реентрантности не имеет внешнего эквивалента — вы наследуете то, что делает хранилище, что обычно ничего.

### `Shared\Mutex` → распределённая блокировка Redis

- **Redis:** паттерн «Redlock» или более простая single-key блокировка `SET NX EX`, если ваши гарантии расслаблены. Библиотеки вроде `cheprasov/php-redis-lock` оборачивают это.
- **etcd / Consul / Zookeeper:** session-based блокировки с обновлением lease. Больше операционных накладных, но более сильные гарантии.

Семантические разрывы:

- **Это самая трудная миграция.** In-process mutex'ы мгновенны и корректны; распределённые блокировки медленны и предлагают только best-effort гарантии. Считайте, что семантика изменится — проектируйте под at-least-once, идемпотентные критические секции.
- `with($fn)` в `Shared\Mutex` атомарно фиксирует возвращаемое замыканием значение обратно в охраняемое хранилище. С Redis-блокировкой вы должны явно прочитать, вычислить, затем записать, и запись может гонять с несвязанной операцией.
- Отравление: внешние блокировки не имеют состояния «отравлено». Если ваше замыкание бросает в распределённой критической секции, вы отпускаете блокировку и позволяете следующему вызывающему увидеть полу-зафиксированное состояние. Обрабатывайте согласованность через компенсирующее действие, а не имитируя `isPoisoned()`.

### `Shared\Channel` → NATS JetStream / Redis Streams / SQS / Kafka

- **NATS JetStream:** ближайшее семантическое соответствие. Долговечный, ограниченный, MPMC, со смещениями consumer'а и доставкой at-least-once.
- **Redis Streams:** `XADD` / `XREADGROUP` покрывает базовый паттерн очереди. Consumer-группы соответствуют семантике multi-consumer у `Shared\Channel`.
- **SQS / Kafka:** индустриальные стандарты. Kafka — правильный выбор для высокопроизводительных потоков событий; SQS — для простых очередей задач.

Семантические разрывы:

- **Блокирующий `recv` заменяется long polling'ом.** Код consumer'а меняется с «вернуть null при close» на «опросить с таймаутом, обработать reconnect».
- **Пакетирование `sendMany`** соответствует linger/batch-конфигу Kafka или пайплайнингу Redis.
- **`close()`** не имеет внешнего аналога. Останавливайте producer'ы корректно и дайте consumer'ам слиться; нет сигнала, говорящего «больше элементов никогда».
- **In-process порядок** становится at-least-once доставкой по сети. Ключи идемпотентности на стороне consumer'а обязательны.

### `Shared\Map` → Redis hash / KV-сервис / БД

- **Redis hash:** `HGET` / `HSET` / `HDEL` / `HSCAN` покрывает форму keyed-map.
- **Ключевые строковые значения:** `SET key:<k> value` с `maxEntries`, обеспечиваемым через LRU-вытеснение.
- **Таблица БД с TTL-колонкой:** строки — записи; фоновый sweeper обрабатывает вытеснение. Это то, что вы хотите, когда значения больше нескольких сотен байт.

Семантические разрывы:

- `update($key, $fn)` должен стать server-side Lua-скриптом в Redis (чтобы сохранить RMW атомарным) или `SELECT ... FOR UPDATE` в SQL. Простой `HGET` + compute + `HSET` теряет атомарность.
- **Cycle safety** Map'а не существует снаружи. Вы никогда не замкнёте цикл, потому что нет графа Shareable, который можно замкнуть.
- **Вложенные Shareable** становятся «отдельным ключом с указателем, закодированным в значении». Бухгалтерию ведёте вы.

### `Shared\Pool` → Пулы клиентских библиотек

- **Предпочитайте собственный пул библиотеки.** PDO, Guzzle, HTTP-клиенты и большинство DB-драйверов имеют зрелый пуллинг. Не переизобретайте их с `Shared\Pool`.
- **Прокси-сервисы:** для per-host Postgres/MySQL пуллинга pgbouncer / proxysql терминируют границу пуллинга на инфраструктурном слое. Ваша PHP-сторона снова становится stateless.

Семантические разрывы:

- Idle-timeout вытеснение пула заменяется собственной проверкой здоровья библиотеки.
- Callback'и factory/destroy заменяются жизненным циклом соединения библиотеки.
- Между хостами вам могут понадобиться **per-service** пулы (один на downstream), а не один большой пул.

## Конкретный кейс: per-tenant rate limiter

Вот [rate-limiter пример](shared-state.md#канонический-пример--миграция-самописного-счётчика) из `shared-state.md`, переработанный за интерфейсом бэкенда:

```php
<?php
interface RateLimiterBackend
{
    public function allow(string $key, int $max, int $windowSecs): bool;
}

final class SharedRateLimiterBackend implements RateLimiterBackend
{
    public function __construct(private OxPHP\Shared\Map $buckets) {}

    public function allow(string $key, int $max, int $windowSecs): bool
    {
        $now = time();
        $state = $this->buckets->update($key, function ($current) use ($now, $windowSecs) {
            if ($current === null || $now - $current['start'] >= $windowSecs) {
                return ['count' => 1, 'start' => $now];
            }
            return ['count' => $current['count'] + 1, 'start' => $current['start']];
        });
        return $state['count'] <= $max;
    }
}

final class RedisRateLimiterBackend implements RateLimiterBackend
{
    /**
     * Atomic fixed-window counter. Load this script once at bootstrap
     * via `$redis->script('load', $lua)` and keep the resulting SHA.
     */
    private const SCRIPT = <<<'LUA'
        local current = redis.call('GET', KEYS[1])
        if current then
            local c = tonumber(current) + 1
            redis.call('SET', KEYS[1], c, 'KEEPTTL')
            return c
        end
        redis.call('SET', KEYS[1], 1, 'EX', ARGV[1])
        return 1
    LUA;

    public function __construct(
        private Redis $redis,
        private string $scriptSha,
    ) {}

    public static function withLoadedScript(Redis $redis): self
    {
        $sha = $redis->script('load', self::SCRIPT);
        return new self($redis, $sha);
    }

    public function allow(string $key, int $max, int $windowSecs): bool
    {
        $count = (int) $this->redis->evalSha($this->scriptSha, ["rl:{$key}"], [$windowSecs]);
        return $count <= $max;
    }
}
```

Единственное, что меняется между single-host и multi-host деплоями, — это какой бэкенд подключён при bootstrap. Остальное приложение разговаривает с `RateLimiterBackend`.

## Гибридные паттерны

### Локальный кэш перед внешним состоянием

Read-heavy нагрузки часто используют `Shared\Map` как TTL-кэш перед внешним хранилищем. Вы бьёте в Redis раз в N секунд; вы бьёте в `Shared\Map` тысячи раз в секунду.

```php
<?php
$cfg = $cache->getOrSet($tenantId, fn () => loadFromRedis($tenantId));
```

Инвалидируйте через канал Redis pub/sub, на который подписаны все процессы OxPHP, или через TTL в локальном Map.

### Write-through буфер

Write-heavy нагрузки буферизуются в `Shared\Channel`, а фоновый consumer сливает во внешнее хранилище. Вы поглощаете всплески в процессе и амортизируете сетевые накладные.

```php
<?php
$writes = new OxPHP\Shared\Channel(capacity: 10_000);

oxphp_async(function () use ($writes) {
    while (($batch = $writes->recvMany(max: 100, timeout: 0.5))) {
        writeBatchToRedis($batch);
    }
});

// Горячий путь
$writes->trySend([$key, $value]);
```

Trade-off: если процесс умирает до завершения слива, вы теряете буферизованные элементы. Подходит для аналитики, не для биллинга.

## Чеклист

Перед переключением:

- [ ] Определите один `Shared\*`-примитив за миграцией. Не мигрируйте «всё» сразу.
- [ ] Извлеките интерфейс; подключите оба бэкенда.
- [ ] Решите согласованность — at-most-once или at-least-once — и сделайте это явным в интерфейсе.
- [ ] Тестируйте оба бэкенда одним и тем же интеграционным набором тестов.
- [ ] Замерьте задержку. Внешние хранилища добавляют 0.1–5 мс на операцию — проверьте, что приложение поглотит это на горячих путях.
- [ ] Спланируйте отказ внешнего хранилища: fail open (пропустить запрос) или fail closed (отдать 503)? Правильный ответ зависит от домена.
- [ ] Включите метрики `oxphp_shared_*` на бэкенде `Shared\*` до и после переключения, чтобы можно было сравнить.

## См. также

- [Разделяемое состояние](shared-state.md) — обзор; когда оставаться in-process.
- [Shared Observability](../operations/shared-observability.md) — инструментируйте оба бэкенда одинаково.
- [Rate Limiting](rate-limiting.md) — встроенный per-IP лимитер (выполняется до PHP; ортогонально PHP-уровню лимитов).
