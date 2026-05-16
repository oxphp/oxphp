---
title: Наблюдаемость Shared\*
description: Эндпоинты интроспекции, метрики Prometheus и диагностические playbook'и для примитивов OxPHP\Shared\* — инспекция живых записей реестра, обход графов достижимости и алерты по насыщению.
---

# Наблюдаемость Shared\*

Каждый экземпляр `OxPHP\Shared\*` — это запись реестра, которую рантайм уже отслеживает по refcount и ёмкости. Это отслеживание открыто для операторов как JSON-интроспекция под `/__ox_shared/*` и как метрики Prometheus под `oxphp_shared_*`. Этот документ — справочник и полевой гайд.

## Включение

Наблюдаемость садится поверх [внутреннего сервера](../features/internal-server.md). Установите `INTERNAL_ADDR`, чтобы запустить его:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

И JSON-эндпоинты, и `/metrics` тогда доступны по этому адресу. Никакой дополнительной конфигурации не требуется.

Вы можете отключить любое из этого независимо:

| Переменная                       | По умолчанию | Эффект                                                       |
|----------------------------------|---------|--------------------------------------------------------------|
| `SHARED_INTROSPECTION_ENABLED`   | `true`  | Переключает JSON API `/__ox_shared/*`.                       |
| `SHARED_INTROSPECTION_PREVIEW_ENABLED` | `true` | Переключает `/preview` (превью формы значений могут утекать данные). |
| `SHARED_METRICS_ENABLED`         | `true`  | Переключает метрики Prometheus `oxphp_shared_*`.             |

Отключайте интроспекцию в развёртываниях с враждебными тенантами; метрики только агрегатные и безопасны для оставления.

## Эндпоинты интроспекции

Все ответы — `Content-Type: application/json; charset=utf-8`. Параметры запроса — стандартный URL-encoded.

### `GET /__ox_shared/summary`

Снимок верхнего уровня: агрегированные счётчики per-type, память, скорость операций и насыщение против настроенных cap'ов.

```json
{
  "total_entries": 127,
  "total_bytes": 2_481_664,
  "by_type": {
    "Counter": { "count": 48, "bytes": 3_072,   "ops": 1_402_391 },
    "Map":     { "count": 12, "bytes": 1_638_400, "ops":    48_201 },
    "Pool":    { "count":  4, "bytes":   16_384, "ops":    67_014 }
  },
  "limits":   { "max_entries": 100_000, "max_bytes": 1073741824, "soft_ratio": 0.7 },
  "saturation": { "entries": 0.00127, "bytes": 0.00231 },
  "diagnostics": {
    "lock_diagnostics_level": "warn",
    "cycle_detect_depth": 16,
    "poison_strict": false
  }
}
```

Используйте `summary` в дашбордах и cron-алертах. Один scrape даёт per-type здоровье и запас по ёмкости.

### `GET /__ox_shared/entries?limit=N`

Перечисляет живые записи (ограничено `limit`, по умолчанию 100, максимум 500). Одна строка на запись:

```json
{
  "items": [
    { "id": 42, "type": "Map",    "refcount": 2, "ops":  1820, "mem_bytes": 204_800, "age_sec": 612 },
    { "id": 43, "type": "Counter", "refcount": 3, "ops": 48_014, "mem_bytes":     64, "age_sec": 612 }
  ],
  "next_cursor": null,
  "total_matching": 127
}
```

`refcount` — внешний счётчик удержаний — сколько PHP-обёрток и вложенных Shared-записей держат эту. Когда вы ожидаете, что запись GC-нется, но этого не происходит, это поле для проверки.

### `GET /__ox_shared/entry?id=N`

Типоспецифичные детали одной записи:

```json
{
  "id": 42,
  "type": "Map",
  "refcount": 2,
  "ops": 1820,
  "mem_bytes": 204_800,
  "age_sec": 612,
  "type_specific": {
    "key_count": 1_240,
    "max_entries": 50_000,
    "saturation": 0.0248,
    "sample_keys": ["tenant:acme", "tenant:beta", "..."]
  }
}
```

`type_specific` варьируется по типу — Pool показывает `{ size, in_use, idle, waiting, idle_by_thread, max_size }`, Channel показывает `{ capacity, pending, closed, senders_blocked, receivers_blocked }`, Counter показывает `{ value }` и т. д.

### `GET /__ox_shared/preview?id=N`

Превью формы значения для скаляров и небольших массивов. Строковые значения обрезаются до `SHARED_PREVIEW_STRING_LIMIT` (по умолчанию 256 байт); массивы показывают первые `SHARED_PREVIEW_ARRAY_LIMIT` записей (по умолчанию 20). Шлюзовано `SHARED_INTROSPECTION_PREVIEW_ENABLED`.

```json
{ "id": 42, "type": "Counter", "preview": "1420" }
```

Используйте `preview` во время разработки; отключайте в production, когда значения могут содержать пользовательские данные.

### `GET /__ox_shared/types`

Перечисляет каталог типов v1 — полезно для генерируемого тулинга, которому нужна mapping тег → класс:

```json
{
  "types": [
    { "tag": 10, "name": "Counter", "php_class": "OxPHP\\Shared\\Counter" },
    { "tag": 11, "name": "Flag",    "php_class": "OxPHP\\Shared\\Flag" },
    { "tag": 12, "name": "Once",    "php_class": "OxPHP\\Shared\\Once" },
    { "tag": 20, "name": "Map",     "php_class": "OxPHP\\Shared\\Map" },
    { "tag": 30, "name": "Mutex",   "php_class": "OxPHP\\Shared\\Mutex" },
    { "tag": 31, "name": "Channel", "php_class": "OxPHP\\Shared\\Channel" },
    { "tag": 50, "name": "Pool",    "php_class": "OxPHP\\Shared\\Pool" }
  ]
}
```

### `GET /__ox_shared/graph?id=N[&depth=D][&edges=E]`

BFS-обход исходящих ссылок `Shareable`, начинающийся с `id=N`. Возвращает узлы и рёбра достижимого подграфа. По умолчанию: `depth=16`, `edges=500`. Достижение бюджета обходчика выставляет `truncated: true` в ответе.

```json
{
  "root": 42,
  "nodes": [
    { "id": 42, "type": "Map",     "refcount": 2, "mem_bytes": 204_800 },
    { "id": 51, "type": "Counter", "refcount": 1, "mem_bytes":     64 }
  ],
  "edges": [
    { "from": 42, "to": 51, "key": "hits" }
  ],
  "truncated": false
}
```

Берите `graph` после `CycleException`, чтобы увидеть достижимый путь, по которому пошёл обходчик, или при диагностике «почему этот Counter не GC-ится» — граф показывает каждого родителя, удерживающего на нём retain.

## Метрики Prometheus

Все метрики выставляются на `GET /metrics` рядом с метриками ядра сервера.

### Общереестровые

| Метрика                                         | Тип     | Метки           | Описание                                            |
|------------------------------------------------|---------|-----------------|----------------------------------------------------|
| `oxphp_shared_objects_total`                   | gauge   | `type`          | Число живых записей per type.                      |
| `oxphp_shared_operations_total`                | counter | `type`          | Совокупно операций, диспетчированных в каждый тип. |
| `oxphp_shared_bytes`                           | gauge   | `type`          | Приблизительные байты per type (±30% против `mallinfo`). |
| `oxphp_shared_total_bytes`                     | gauge   | —               | Сумма по типам.                                    |
| `oxphp_shared_capacity_saturation`             | gauge   | `kind`          | `entries` и `bytes` как доли от их cap'ов.         |
| `oxphp_shared_deadlock_detected_total`         | counter | —               | Обнаружено межпоточных циклов wait-for.            |

### Channel

| Метрика                                          | Тип     | Метки         |
|--------------------------------------------------|---------|---------------|
| `oxphp_shared_channel_count`                     | gauge   | `channel_id`  |
| `oxphp_shared_channel_pending` *(устаревшая)*    | gauge   | `channel_id`  |
| `oxphp_shared_channel_senders_blocked`           | gauge   | `channel_id`  |
| `oxphp_shared_channel_receivers_blocked`         | gauge   | `channel_id`  |
| `oxphp_shared_channel_items_sent_total`          | counter | `channel_id`  |
| `oxphp_shared_channel_items_dropped_total`       | counter | `channel_id`  |

`oxphp_shared_channel_pending` — устаревшее написание
`oxphp_shared_channel_count`; обе серии возвращают одно значение в
течение цикла deprecation и разойдутся, когда alias будет удалён в
одном из будущих релизов. Новые дашборды настраивайте на `_count`.

### Map

| Метрика                                 | Тип     | Метки      |
|-----------------------------------------|---------|------------|
| `oxphp_shared_map_entries`              | gauge   | `map_id`   |
| `oxphp_shared_map_max_entries`          | gauge   | `map_id`   |
| `oxphp_shared_map_saturation`           | gauge   | `map_id`   |

### Pool

| Метрика                                   | Тип       | Метки                               |
|-------------------------------------------|-----------|-------------------------------------|
| `oxphp_shared_pool_count`                 | gauge     | `pool_id`                           |
| `oxphp_shared_pool_size` *(устаревшая)*   | gauge     | `pool_id`                           |
| `oxphp_shared_pool_in_use`                | gauge     | `pool_id`                           |
| `oxphp_shared_pool_idle`                  | gauge     | `pool_id`                           |
| `oxphp_shared_pool_waiting`               | gauge     | `pool_id`                           |
| `oxphp_shared_pool_acquire_total`         | counter   | `pool_id`                           |
| `oxphp_shared_pool_evicted_total`         | counter   | `pool_id`, `reason`                 |
| `oxphp_shared_pool_wait_seconds`          | histogram | `pool_id`                           |

`oxphp_shared_pool_size` — устаревшее написание
`oxphp_shared_pool_count`; обе серии возвращают одно значение в
течение цикла deprecation и разойдутся, когда alias будет удалён в
одном из будущих релизов. Новые дашборды настраивайте на `_count`.

Метки `oxphp_shared_pool_evicted_total`: `reason=idle_timeout | manual | shutdown | dead_owner`. Метка `dead_owner` считает события chaos-reclaim.

### Counter / Flag / Once / Mutex

Per-instance счётчики, флаги, once'ы и mutex'ы не поставляются с индивидуальными метрическими сериями — это раздуло бы кардинальность меток. Используйте общереестровый `oxphp_shared_operations_total{type=...}` counter и JSON `/__ox_shared/entry?id=…` для per-instance инспекции.

> **Mutex-метрики — кандидат на v1.x.** Отслеживается как follow-up работа; сегодняшняя видимость — через `/__ox_shared/entry`.

## Диагностические playbook'и

### Pool насыщен (429 с неуспешными retry)

Симптомы: HTTP-вызывающие видят таймауты, `oxphp_shared_pool_waiting` растёт, `oxphp_shared_pool_count` упирается в `maxSize`.

Проверьте:

```bash
curl -s http://localhost:9090/__ox_shared/entry?id=<pool_id> | jq .type_specific
```

Посмотрите на `idle_by_thread`. Если он `{}` или сильно несбалансирован (worker 0 имеет 8 idle, worker 3 имеет 0), захват конкурирует за потоки, которые случайно заняты в другом месте — per-thread аффинность в v1 не перебалансирует. Либо поднимайте `maxSize`, либо снижайте per-thread acquire-хотспот.

Если `idle_by_thread` сбалансирован, но всё в `in_use`, поднимайте `maxSize`.

### Насыщение памяти

Проверьте `oxphp_shared_total_bytes` и `oxphp_shared_capacity_saturation{kind="bytes"}`. Если что-то из этого высоко:

1. `curl /__ox_shared/entries?limit=500` и отсортируйте по `mem_bytes`, чтобы найти топ-контрибьюторов.
2. `curl /__ox_shared/entry?id=<N>` на каждый, чтобы проверить форму. Для Map смотрите `key_count` vs `max_entries`.
3. Самая частая причина: неограниченный `Shared\Map`, ключуемый пользовательским вводом. Лекарство — cap `maxEntries` и политика удержания.

### Обёртка не собирается мусорщиком

`refcount` в `/__ox_shared/entries` говорит, сколько внешних удержаний есть. Если он остаётся выше 1 после того, как PHP-обёртка покинула область, другая Shared-запись держит её живой.

```bash
curl -s http://localhost:9090/__ox_shared/graph?id=<N> | jq .nodes
```

Пройдите граф назад — любой узел, достигающий застрявшей записи, держит retain. Удалите ссылку (`$map->remove($key)`, закройте канал, сбросьте Mutex-запись), и refcount упадёт.

### `CycleException` сработал в production

Сообщение исключения включает достижимый путь, который исследовал детектор циклов. Сопоставьте эти ID обратно с типами через `/__ox_shared/entries` и спросите `/__ox_shared/graph?id=<root>` за полной формой:

```bash
# Сообщение исключения: "cycle would form: #42 → #51 → #42"
curl -s http://localhost:9090/__ox_shared/graph?id=42 | jq
```

Результат визуализирует цепочку, чтобы можно было увидеть, где была введена непреднамеренная back-reference.

### Сработал детектор дедлоков

`oxphp_shared_deadlock_detected_total` тикает. Проверьте логи сервера — детектор эмитит запись лога per cycle с задействованными ID mutex'ов и владеющими потоками. Восстановление:

1. `curl /__ox_shared/entry?id=<mutex_id>` на каждый — подтвердите `poisoned=false`. Если отравлено, детектор уже отменил цикл.
2. Если цикл — настоящий баг реентрантности, рефакторите, чтобы использовать отдельные mutex'ы на каждую область блокировки.
3. Поднимите `SHARED_LOCK_DIAGNOSTICS=strict` в staging, чтобы превратить будущую реентрантность в fast-fail вместо обнаруженного цикла.

## Long-running soak harness

`tests/soak/pool_soak.sh` — это ручной (не CI) harness для проверки истории стабильности Shared\Pool на часах или днях непрерывной нагрузки. Он:

1. Поднимает dev-образ с динамическим масштабированием воркеров (`PHP_WORKERS=4:40` по умолчанию) и коротким `idleTimeout` пула, чтобы планировщик вытеснения срабатывал непрерывно.
2. Загружает `tests/soak/workload.php` как бутстрап воркера, конструирующий 10 пулов × `maxSize=8` и обслуживающий acquire/release на каждом запросе.
3. Прогоняет трафик через `wrk` в течение `SOAK_DURATION_MIN` минут (по умолчанию 1440 = 24ч).
4. Скрейпит `/metrics` и RSS контейнера каждые 60 с в `tests/soak/out/<timestamp>/metrics.csv`.
5. Записывает `verify.txt` в конце с pass/fail для пяти release-критериев (дрейф RSS в пределах ±5%, ноль stale-handle паник, ноль утёкших записей при shutdown, плавный рост idle-timeout вытеснений, ноль срабатываний детектора дедлоков).

Предпосылки на хосте: `docker`, `wrk`, `curl`, `awk`.

Типичные вызовы:

```bash
# 24ч полный soak перед релизом
tests/soak/pool_soak.sh

# 1ч smoke для валидации самого harness'а
SOAK_DURATION_MIN=60 tests/soak/pool_soak.sh

# Более тяжёлая конкурентность
SOAK_CONCURRENCY=400 SOAK_THREADS=8 tests/soak/pool_soak.sh
```

Артефакты падают в `tests/soak/out/<timestamp>/`:

- `metrics.csv` — одна строка в минуту (unix ts, RSS, per-type счётчики записей, per-pool счётчики вытеснения, deadlock count, ops).
- `server.log` — stdout/stderr контейнера, включая любые stale-handle или panic трейсы.
- `wrk.out` / `wrk.err` — сырой вывод нагрузочного генератора.
- `metrics.final` — последний скрейп `/metrics`, взятый прямо перед teardown контейнера. Используется для чтения `oxphp_shared_leaked_entries_at_shutdown_total`.
- `verify.txt` — pass/fail отчёт по пяти exit-критериям.

**Не** подключайте это в CI. 24ч прогон не дёшев, и его цель — pre-release уверенность, а не непрерывная валидация.

## Каденс scrape

Эндпоинты реестра обходят живое состояние под read-блокировками, так что скрейпинг дёшев, но не бесплатен. Рекомендуемые каденсы:

- `/metrics` — **каждые 15 с** (типичное значение Prometheus по умолчанию). Только агрегатно; накладные расходы пренебрежимы.
- `/__ox_shared/summary` — **каждые 60 с** для дашбордов. Чуть тяжелее `/metrics`.
- `/__ox_shared/entries` — **по требованию** только. Итерирует все шарды; не скрейпите каждый тик.
- `/__ox_shared/entry` / `/preview` / `/graph` — **по требованию** во время расследований.

## См. также

- [Разделяемое состояние](../features/shared-state.md) — ментальная модель и обзор примитивов.
- [Метрики Prometheus](metrics.md) — метрики ядра сервера на том же эндпоинте `/metrics`.
- [Внутренний сервер](../features/internal-server.md) — как эндпоинты `/__ox_shared/*` подключаются к `INTERNAL_ADDR`.
- [Миграция на внешнее хранилище](../features/migrating-to-external-store.md) — когда насыщение структурно, а не настраиваемо.
