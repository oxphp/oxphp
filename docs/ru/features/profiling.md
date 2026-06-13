---
title: Профилирование PHP-кода
description: Максимально подробный гайд — от первого запуска до производственного использования. Триггеры, PHP SDK, атрибуты, форматы вывода, интеграция со speedscope/xhgui/pprof, метрики, решение проблем.
---

# Профилирование PHP-кода в OxPHP

OxPHP содержит встроенный профилировщик уровня запроса. В отличие от xdebug или
отдельных расширений, он работает внутри самого сервера, не требует перезапуска
PHP и не прибавляет значимых накладных расходов, когда профилирование выключено
(ветка `mode=Off` выходит раньше, чем ищется что-либо в кеше фильтров).

Этот документ — **практический гайд**: от нулевой конфигурации до поиска
медленных запросов на проде, чтения flamegraph и сравнения «до / после»
оптимизации.

---

## 1. TL;DR — поднять за 60 секунд

```yaml
# compose.yml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.8.0
    environment:
      INTERNAL_ADDR: 0.0.0.0:9090
      PROFILER_ENABLED: "true"
      PROFILER_AUTH_TOKEN: "dev-secret"
      PROFILER_OUTPUT_FORMATS: "xhprof,speedscope,collapsed"
    volumes:
      - ./www:/var/www/html
      - profiles:/tmp/oxphp-profiles
    ports:
      - "80:80"
      - "9090:9090"

volumes:
  profiles:
```

```bash
# 1. Запросить страницу с триггером профилирования.
curl -H "X-OxPHP-Profile: dev-secret" http://localhost/slow-endpoint

# 2. Посмотреть список захваченных запусков.
curl -H "Authorization: Bearer dev-secret" http://localhost:9090/__profiler/runs \
  | jq '.runs[0]'

# 3. Открыть профиль в speedscope (браузерный flamegraph).
open "http://localhost:9090/__profiler/runs/<run_id>/speedscope"
```

Всё. Ниже — подробно, что именно происходит, и как использовать это в проде.

---

## 2. Что умеет профилировщик

- **Захватывает каждый вызов PHP-функции** через Zend Observer API — без
  патчинга байткода и без изменения пользовательского кода.
- **Строит дерево спанов** с wall-time, CPU-time, памятью на входе/выходе,
  атрибутами и событиями.
- **Экспортирует в 4 формата** одновременно: `xhprof.json`, `speedscope.json`,
  `pprof` (protobuf + gzip), `collapsed` (для `flamegraph.pl`).
- **Хранит запуски**: LRU-кеш в памяти + файлы на диске + опциональный
  HTTP push (xhgui, собственный коллектор).
- **Отдаёт 8 внутренних HTTP-маршрутов** на `INTERNAL_ADDR` для просмотра
  и скачивания профилей.
- **Считает метрики Prometheus** — runs per source, spans collected,
  bytes written, drops, push failures.
- **Не требует перезапуска** — активируется по триггеру на конкретный запрос.

---

## 3. Как это работает внутри

```
 ┌─ Запрос ──────────────────────────────────────────────────────────┐
 │  1. Tokio-поток: ProfilerRequestHandler проверяет триггер          │
 │     (header/cookie/query/sample_rate, constant-time compare)       │
 │                                                                    │
 │  2. Решение записывается в PluginRequestActions → передаётся       │
 │     воркеру через SAPI-канал                                       │
 │                                                                    │
 │  3. Воркер перед RINIT устанавливает ProfilingMode = ProfileAll    │
 │     и регистрирует Observer handlers на begin/end каждой функции   │
 │                                                                    │
 │  4. Каждый вызов PHP-функции → C hook → bridge-буфер → Rust        │
 │     SpanTree (span_id = монотонный BE-счётчик; имена интернятся    │
 │     в thread-local interner — без лишних аллокаций)                │
 │                                                                    │
 │  5. После ответа: ProfilerCompleteHandler получает Arc<SpanTree>,  │
 │     запускает 4 экспортёра, кладёт в LRU-кеш, спавнит задачи на    │
 │     disk-write и HTTP push (семафоры ограничивают fan-out)         │
 └────────────────────────────────────────────────────────────────────┘
```

### Три режима работы за запрос

| Режим | Когда активируется | Что захватывается |
|-------|--------------------|-------------------|
| `Off` | По умолчанию. Ни один плагин не запросил профилирование. | Ничего. Нулевые накладные расходы. |
| `ApmOnly` | `plugin-apm` включён, но триггер профилировщика не сработал. | Только явные хуки APM: `#[Trace]`, PDO/cURL emitter'ы, `oxphp_trace_*()`. |
| `ProfileAll` | Сработал триггер профилировщика (или вызван `OxPHP\Profile\start()`). | **Каждый** вызов PHP-функции через Observer API + всё, что собирает APM. |

`ProfileAll` «поглощает» `ApmOnly`: если оба плагина включены и триггер сработал,
используется одно общее `Arc<SpanTree>` — дублирования нет.

---

## 4. Установка и сборка

Плагин `plugin-profiler` входит в **cargo-фичу по умолчанию**. Стандартная сборка
через `docker compose build` уже содержит его.

Отключить:

```dockerfile
ARG OXPHP_WITH_PROFILER=0
# или
ARG CARGO_FEATURES="plugin-apm,plugin-otel"  # без plugin-profiler
```

Проверить, что плагин скомпилирован:

```bash
docker compose exec app cat /proc/self/maps | grep -i profiler
# или: oxphp --list-plugins (если доступна команда)
```

---

## 5. Активация — 4 способа триггеров

Порядок проверки (приоритетный): **header → cookie → query → sample_rate**.
Любое совпадение активирует `ProfileAll`. Токен сравнивается за **константное
время** (`subtle::ConstantTimeEq`).

### 5.1. HTTP-заголовок (для разработки и скриптов)

```bash
curl -H "X-OxPHP-Profile: dev-secret" https://app.local/checkout
```

Идеально для CI-бенчмарков, Postman-коллекций, curl-скриптов.

### 5.2. Cookie (для браузерной отладки)

Установите cookie в браузере через DevTools / расширение:

```
OXPROF=dev-secret; Domain=app.local; Path=/
```

Пока cookie живёт — каждый запрос профилируется. Удалили cookie — отключились.
Полезно, чтобы пройти пользовательский сценарий (открыть карточку → добавить
в корзину → оформить) и получить **серию** профилей.

### 5.3. Query-параметр (для шаринга ссылок)

```
https://app.local/admin/report?__oxprof=dev-secret
```

Самый грубый способ, но удобен, когда надо отправить коллеге ссылку
«открой, это воспроизводит баг». Осторожно: параметр попадёт в access-логи
и Referer — не используйте с продовым токеном.

### 5.4. Случайная выборка (для прода)

```bash
PROFILER_SAMPLE_RATE=0.001   # ≈ 1 из 1000 запросов
```

Работает **без токена**. Включайте в проде, чтобы накопить статистику по реальному
трафику. Рекомендуемая начальная цифра — `0.0005..0.002`; выше — заметные
накладные расходы при `PROFILER_INTERNAL=true`.

---

## 6. Конфигурация — полный справочник

| Переменная | По умолч. | Описание |
|---|---|---|
| `PROFILER_ENABLED` | `false` | Главный включатель. `true` → плагин загружается. |
| `PROFILER_AUTH_TOKEN` | *(нет)* | Секрет для триггеров и Bearer-токен для `/__profiler/*` маршрутов. Пустая строка = «токен не требуется» (любой непустой триггер подходит). **Никогда не коммитьте токен в репо.** |
| `PROFILER_SAMPLE_RATE` | `0.0` | `[0.0; 1.0]`. Случайная выборка. |
| `PROFILER_INTERNAL` | `false` | Наблюдать за **внутренними** C-функциями (`strlen`, `json_encode` и т.д.). Даёт полное покрытие, но **2–5× накладных расходов**. Включайте только точечно. |
| `PROFILER_MAX_SPANS` | `50000` | Hard-cap на размер дерева за запрос. При превышении — новые спаны маркируются `truncated` и не пишутся. |
| `PROFILER_MAX_DEPTH` | `256` | Hard-cap на глубину стека. |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | Абсолютный путь. Должен быть writable для `www-data`. |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,speedscope` | CSV из `xhprof`, `speedscope`, `pprof`, `collapsed`. |
| `PROFILER_RETENTION_COUNT` | `100` | Сколько запусков держать (и на диске, и в LRU). Фоновая обрезка — каждые 5 секунд. |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | Token-bucket для защиты диска. Лишние сбрасываются в метрику `oxphp_profiler_disk_drops_total`. |
| `PROFILER_EXPORT_URL` | *(нет)* | POST-URL для отправки каждого запуска (xhgui, свой коллектор). |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | Один из 4 форматов для HTTP push. |
| `PROFILER_EXPORT_AUTH_TOKEN` | *(нет)* | Bearer-токен для целевого URL. |
| `PROFILER_EXPORT_XHGUI` | `auto` | Принудительный режим xhgui-конверта. Auto: URL содержит `xhgui` или `/run/import`. |

### Пример продовой конфигурации

```yaml
environment:
  PROFILER_ENABLED: "true"
  PROFILER_AUTH_TOKEN: "${PROFILER_TOKEN_FROM_VAULT}"
  PROFILER_SAMPLE_RATE: "0.001"             # ~0.1% трафика
  PROFILER_INTERNAL: "false"
  PROFILER_OUTPUT_DIR: /var/lib/oxphp/profiles
  PROFILER_OUTPUT_FORMATS: "xhprof,collapsed"
  PROFILER_RETENTION_COUNT: "500"
  PROFILER_DISK_MAX_PER_SEC: "20"
  PROFILER_EXPORT_URL: "http://xhgui.monitoring.svc.cluster.local/run/import"
  PROFILER_EXPORT_FORMAT: "xhprof"
```

---

## 7. PHP SDK — семь функций

Все функции в пространстве имён `OxPHP\Profile`. Их можно вызывать всегда:
если профилирование для данного запроса не активировано, мутаторы —
безопасные no-op, `is_active()` вернёт `false`.

### 7.1. Точное включение/выключение участка кода

```php
use function OxPHP\Profile\{start, stop, is_active};

function heavy_report(): array
{
    start();                       // активировать ProfileAll внутри запроса
    $result = build_report();      // это попадёт в дерево
    stop();                        // остановить capture
    return $result;
}
```

`start()` идемпотентен. `stop()` тоже — вызов дважды подряд безопасен.

> ⚠️ `start()` в середине запроса **сбрасывает** текущее дерево (см.
> `PROFILING_CONTEXT.reset()` в `php_sdk.rs`). Это согласуется со спецификацией:
> режим ставится **один раз** за запрос — либо триггером в RINIT, либо первым
> вызовом `start()`.

### 7.2. Пауза и возобновление

```php
use function OxPHP\Profile\{pause, resume};

pause();
noisy_helper_we_dont_care_about();  // не попадёт в дерево
resume();
```

В отличие от `stop()`, pause/resume семантически сигнализируют «временно».
Внутри это один и тот же флаг, но разделение полезно для читателя кода.

### 7.3. Пометка точек — mark()

```php
use function OxPHP\Profile\mark;

mark('cache_miss');
mark('got_auth_token', ['user_id' => (string) $user->id]);
```

Прикрепляет событие `SpanEventKind::Mark` к **текущему открытому спану**.
Если ни одного спана не открыто — no-op. Удобно для временных меток в
длинной функции или для обозначения веток if/else.

### 7.4. Числовые метрики — metric()

```php
use function OxPHP\Profile\metric;

$rows = $pdo->query('SELECT ...')->fetchAll();
metric('db.rows', (float) count($rows));
metric('payload.kb', strlen($body) / 1024.0);
```

Прикрепляет пару `metric.<name>=<value>` к **атрибутам текущего спана**.
В отличие от `mark()`, это просто ключ-значение (без timestamp).
В speedscope / xhgui показывается в панели свойств спана.

### 7.5. Проверка статуса — is_active()

```php
if (OxPHP\Profile\is_active()) {
    // можно позволить себе дорогой debug-dump —
    // этот запрос всё равно профилируется
    error_log(json_encode($debug_state));
}
```

Две TLS-чтения, без FFI. Безопасно вызывать в горячем коде.

---

## 8. Атрибуты (PHP 8) — декларативный контроль

Семь атрибутов делятся на две категории: **observer-фильтры** работают
**до** создания спана; **декораторы** работают **после** его закрытия.

| Атрибут | Категория | Эффект |
|---|---|---|
| `#[Profile]` | фильтр | Принудительно включить функцию в дерево (даже если по общим правилам была бы исключена). |
| `#[Exclude]` | фильтр | Пропустить функцию; её дети перепривязываются к ближайшему включённому предку. |
| `#[Sample(rate: 0.1)]` | фильтр | Сохранить только долю вызовов (`rate ∈ [0.0; 1.0]`). Вероятностно — без блокировок. |
| `#[Tag(key, value)]` | фильтр | Прикрепить лейбл к спану. Атрибут repeatable: несколько `#[Tag]` аккумулируются. |
| `#[Mark(label?)]` | декоратор | Сгенерировать `Mark`-событие на входе в функцию. |
| `#[SlowThreshold(ms)]` | декоратор | Сгенерировать `Slow`-событие + выставить статус, если wall-time ≥ `ms`. |
| `#[MemoryThreshold(kb)]` | декоратор | Сгенерировать `MemorySpike` + статус, если чистое выделение памяти ≥ `kb`. |

### Композиция классов и методов

```php
use OxPHP\Profile\{Tag, Profile, Exclude};

#[Tag(key: 'layer', value: 'domain')]
#[Profile]                                    // класс всегда профилируется
class OrderService
{
    #[Tag(key: 'op', value: 'create')]
    public function create(array $data): Order { /* ... */ }

    #[Exclude]                                 // метод исключён, несмотря на #[Profile] на классе
    public function debug_dump(): void { /* ... */ }

    public function find(int $id): ?Order { /* ... */ }   // наследует #[Profile] и #[Tag(layer)]
}
```

- Атрибуты класса **распространяются** на все его методы.
- Атрибут метода **дополняет** атрибуты класса (теги аккумулируются).
- `#[Exclude]` на методе **переопределяет** `#[Profile]` класса.

### Порог медленных функций

```php
use OxPHP\Profile\SlowThreshold;

#[SlowThreshold(ms: 250)]
function render_dashboard(User $u): string
{
    // если выполняется ≥ 250 мс — в спан добавляется Slow-событие
    // и status_code=2 (error). В xhgui / speedscope сразу видно.
}
```

### Порог памяти

```php
use OxPHP\Profile\MemoryThreshold;

#[MemoryThreshold(kb: 512)]
function import_csv(string $path): int
{
    // если за время выполнения функция выделила ≥ 512 КБ —
    // MemorySpike-событие + status=error
}
```

### Семплирование отдельных функций

```php
use OxPHP\Profile\Sample;

#[Sample(rate: 0.01)]
function log_event(string $evt, array $ctx): void
{
    // в дерево попадёт ≈ 1% вызовов; остальные полностью пропускаются —
    // ни сам спан, ни его дети не создаются. Полезно для функций,
    // вызываемых миллионы раз за запрос.
}
```

> **Когда фильтры vs декораторы?** Если функция вызывается **очень часто**
> и вы хотите уменьшить cost capture — используйте `#[Sample]` или `#[Exclude]`
> (они работают до создания спана). Если надо добавить event при
> превышении порога — используйте `#[Slow/MemoryThreshold]` (они смотрят на
> уже собранный спан).

---

## 9. Что содержит захваченный спан

```ruby
FinishedSpan {
  span_id         # Arc<str>, W3C-совместимый
  parent_span_id  # Arc<str>
  trace_id        # Arc<str>, общий с APM
  name            # Полное имя PHP-функции/метода
  start_ns        # wall-clock, нс от эпохи профилировщика
  end_ns
  cpu_ns          # CLOCK_THREAD_CPUTIME_ID (0, если платформа не поддерживает)
  memory_start    # zend_memory_usage(0) на входе
  memory_end      # zend_memory_usage(0) на выходе
  attributes      # Vec<(Arc<str>, Arc<str>)> — из #[Tag], metric(), APM SQL/HTTP
  events          # Vec<SpanEvent { ts, kind, label, attrs }>
  status_code     # 0 = unset, 1 = ok, 2 = error
  status_message
  leaked          # true, если спан был force-closed при finalize (PHP кинул исключение)
}
```

**Виды событий** (`SpanEvent::kind`):

| Kind | Кто генерирует |
|---|---|
| `Mark` | `mark()`, `metric()`, `#[Mark]` |
| `Slow` | `#[SlowThreshold]` |
| `MemorySpike` | `#[MemoryThreshold]` |
| `Sql` | APM-хуки PDO/mysqli |
| `Http` | APM-хуки cURL/HTTP streams |
| `Exception` | APM exception handler |
| `Alloc` | (зарезервировано под heap sampling) |
| `Other` | fallback |

---

## 10. Форматы экспорта — когда какой использовать

Файлы лежат в `PROFILER_OUTPUT_DIR`, имя = `<run_id>.<ext>`, где `run_id` =
`<ts_ms>-<req_id_prefix>-<rand4>` (например, `1713600000000-a1b2c3d4-0f5e`).

### 10.1. speedscope (🏆 по умолчанию для интерактивного анализа)

Расширение: `.speedscope.json`

- Браузерный flamegraph с zoom, поиском, переключением CPU / time / memory.
- Нулевой setup — открывайте прямо в [speedscope.app](https://www.speedscope.app/).
- OxPHP отдаёт 302-редирект: `/__profiler/runs/{id}/speedscope` → speedscope.app
  с параметром `profileURL=…`, которое загрузит профиль прямо из вашего сервера.

```bash
# Ctrl-click в терминале macOS / xdg-open в Linux
open "http://localhost:9090/__profiler/runs/<run_id>/speedscope"
```

### 10.2. xhprof (для xhgui — timeline + историческое сравнение)

Расширение: `.xhprof.json`

- Формат, совместимый с xhgui (UI с поиском по URL, trends, дифом двух прогонов).
- Идеально для **продового накопления**: запускаете xhgui-контейнер рядом с
  приложением, указываете `PROFILER_EXPORT_URL=http://xhgui/run/import` — и
  в UI копится история.
- Готовый docker-compose: `tests/compose.xhgui.yml`.

### 10.3. pprof (для Google pprof tooling, Grafana pprof-plugin, Pyroscope)

Расширение: `.pprof` (protobuf + gzip, уровень `fast`, zlib backend)

```bash
# сохранить и открыть
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.pprof > profile.pprof

go tool pprof -http=:8080 profile.pprof
# или
pyroscope-cli adhoc --input profile.pprof
```

### 10.4. collapsed (для flamegraph.pl Бренда Грегга)

Расширение: `.collapsed`

- Текстовый формат `func;child;grandchild <count>`.
- Инструмент де-факто для SVG-flamegraph.
- Три варианта метрики: wall-time, CPU, memory. OxPHP пишет `.collapsed`
  (wall), внутренние пути также дают `.collapsed.cpu` и `.collapsed.mem`
  (см. `tests/fixtures/profiler_exports/`).

```bash
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.collapsed \
  | flamegraph.pl --title "Checkout $run_id" > flame.svg
```

---

## 11. Хранение и очистка

```
/tmp/oxphp-profiles/
├── index.json                                # NDJSON — одна запись на строку
├── 1713600000000-a1b2c3d4-0f5e.xhprof.json
├── 1713600000000-a1b2c3d4-0f5e.speedscope.json
└── 1713600001234-b2c3d4e5-4a2b.xhprof.json
```

### Схема записи в `index.json`

```json
{
  "run_id": "1713600000000-a1b2c3d4-0f5e",
  "request_id": "a1b2c3d4e5f67890",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "timestamp_ms": 1713600000000,
  "duration_ms": 123,
  "method": "GET",
  "url": "/checkout",
  "status": 200,
  "user_agent": "Mozilla/5.0 …",
  "client_ip": "10.0.0.42",
  "source": "Header",                 // Header | Cookie | Query | SampleRate
  "span_count": 4821,
  "event_count": 7,
  "error_count": 0,
  "leaked_count": 0,
  "truncated": false,                 // true — превысили PROFILER_MAX_SPANS
  "oxphp_version": "0.8.0",
  "formats": ["xhprof.json", "speedscope.json"]
}
```

`index.json` парсится маршрутом `/__profiler/runs`, сортируется от новых к
старым, пагинируется через `?limit=N&offset=M`.

### Retention

- Фоновая задача каждые 5 секунд удаляет записи старше
  `PROFILER_RETENTION_COUNT` (атомарный `rename` → `index.json`).
- Файлы без записи в `index.json` (осиротевшие после падения сервера)
  удаляются sweep'ом.
- Token-bucket `PROFILER_DISK_MAX_PER_SEC` защищает диск: если нагрузка
  выше — запуски не пишутся и инкрементируется
  `oxphp_profiler_disk_drops_total`.

---

## 12. Внутренние HTTP-маршруты

При `INTERNAL_ADDR=0.0.0.0:9090` плагин регистрирует 8 эндпоинтов на
префиксе `/__profiler/`. Все требуют `Authorization: Bearer
<PROFILER_AUTH_TOKEN>`, когда токен настроен. Сравнение — **constant-time**.

| Маршрут | Метод | Назначение |
|---|---|---|
| `/__profiler/` | GET | HTML landing page со списком эндпоинтов. |
| `/__profiler/runs` | GET | JSON-массив запусков. `?limit=N&offset=M`. |
| `/__profiler/runs/{id}` | GET | JSON-метаданные одного запуска. |
| `/__profiler/runs/{id}.{format}` | GET | Сырые байты профиля. `format` ∈ `xhprof.json`, `speedscope.json`, `pprof`, `collapsed`. |
| `/__profiler/runs/{id}/speedscope` | GET | 302 → speedscope.app с `profileURL=…`. |
| `/__profiler/runs/{id}` | DELETE | Удалить все файлы форматов + запись в индексе (возвращает 204). |
| `/__profiler/config` | GET | Текущая конфигурация плагина (токены скрыты). |
| `/__profiler/stats` | GET | JSON-snapshot счётчиков. |

### Примеры скриптов

```bash
# Список последних 20 запусков, отсортированный по длительности
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=20" \
  | jq '.runs | sort_by(.duration_ms) | reverse | .[:5]'

# Все профили по конкретному URL
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=500" \
  | jq '.runs[] | select(.url == "/checkout")'

# Удалить все runs старше 1 часа (кроме ретенции плагина)
NOW=$(date +%s%3N)
CUTOFF=$((NOW - 3600000))
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=1000" \
  | jq -r --arg c "$CUTOFF" '.runs[] | select(.timestamp_ms < ($c|tonumber)) | .run_id' \
  | xargs -I{} curl -X DELETE -H "Authorization: Bearer $TOK" \
       "http://localhost:9090/__profiler/runs/{}"
```

---

## 13. HTTP push + xhgui

Отправка каждого запуска на удалённый коллектор:

```yaml
environment:
  PROFILER_EXPORT_URL: "http://xhgui/run/import"
  PROFILER_EXPORT_FORMAT: "xhprof"
  PROFILER_EXPORT_AUTH_TOKEN: "shared-secret"   # опционально
```

- Автоопределение xhgui-конверта: URL содержит `xhgui` **или** заканчивается
  на `/run/import`. Принудительно — `PROFILER_EXPORT_XHGUI=true|false`.
- Retry-план: 3 попытки с экспонентой `100/200/400 мс`, общий бюджет — 5 с
  wall-clock. Тело запроса разделяется между попытками как `bytes::Bytes`
  (ноль аллокаций на retry).
- Ошибки инкрементируют `oxphp_profiler_http_push_failures_total`.

### Полный демо-стек

```bash
docker compose -f tests/compose.xhgui.yml up -d
# приложение: :80, xhgui: :8142 (UI), :27017 (mongo)
```

E2E smoke: `tests/php/profiler/test_xhgui_import.php`.

---

## 14. Метрики Prometheus

Экспортируются на `/metrics`:

```
oxphp_profiler_runs_total{source="header"|"cookie"|"query"|"sample"}
oxphp_profiler_spans_collected_total
oxphp_profiler_bytes_written_total{format="xhprof"|"speedscope"|"pprof"|"collapsed"}
oxphp_profiler_disk_drops_total
oxphp_profiler_http_push_failures_total
oxphp_profiler_truncated_total
oxphp_profiler_in_memory_runs
```

Базовые алерты Prometheus:

```yaml
- alert: ProfilerDiskDrops
  expr: rate(oxphp_profiler_disk_drops_total[5m]) > 0
  annotations:
    summary: "Профилировщик сбрасывает запуски на диск — проверьте PROFILER_DISK_MAX_PER_SEC"

- alert: ProfilerPushFailing
  expr: rate(oxphp_profiler_http_push_failures_total[5m]) > 0
  annotations:
    summary: "xhgui/коллектор недоступен"

- alert: ProfilerTruncatingTrees
  expr: rate(oxphp_profiler_truncated_total[5m]) > 0
  annotations:
    summary: "Запросы дают > PROFILER_MAX_SPANS — поднимите лимит или расследуйте"
```

---

## 15. Рабочие кейсы — пошагово

### 15.1. Найти медленный endpoint

1. Включите `PROFILER_SAMPLE_RATE=0.001` в проде. Дождитесь накопления.
2. Отсортируйте запуски по `duration_ms`:
   ```bash
   curl -s -H "Authorization: Bearer $TOK" \
        "http://INT_ADDR/__profiler/runs?limit=500" \
     | jq '.runs | sort_by(.duration_ms) | reverse | .[:10]
            | map({run_id, url, duration_ms, span_count})'
   ```
3. Откройте топ-1 в speedscope: `.../__profiler/runs/<id>/speedscope`.
4. Включите режим **Left Heavy** (в speedscope) — увидите функции, которые
   занимают больше всего суммарного времени.
5. Кликните на самый широкий «кирпич» — получите file:line и список детей.

### 15.2. Проверить гипотезу «до / после»

1. Запустите бенчмарк ДО изменений:
   ```bash
   for i in $(seq 1 20); do
     curl -s -H "X-OxPHP-Profile: dev-secret" http://localhost/api/report > /dev/null
   done
   curl -s -H "Authorization: Bearer dev-secret" \
        "http://localhost:9090/__profiler/runs?limit=20" \
     | jq '.runs | map(.duration_ms) | add / length' > /tmp/p50_before.txt
   ```
2. Внесите изменения, пересоберите, повторите. Сравните медианы.
3. Для глубинного дифа скачайте два xhprof-профиля и загрузите в xhgui —
   у него есть встроенный diff-view.

### 15.3. Поиск утечки памяти

1. Запрос, который «растёт»:
   ```bash
   curl -H "X-OxPHP-Profile: dev-secret" http://localhost/import?file=big.csv
   ```
2. Открыть в speedscope, переключиться на **memory metric** (речь про `.collapsed.mem`
   или speedscope memory-view).
3. Добавьте `#[MemoryThreshold(kb: 1024)]` на подозрительные функции —
   получите явные `MemorySpike`-события в следующем запуске.
4. Используйте `metric('mem.after', memory_get_usage())` для точечного
   инструментирования.

### 15.4. Постоянный мониторинг холодного пути

```php
#[Profile]
#[SlowThreshold(ms: 500)]
public function chargeCard(PaymentRequest $r): PaymentResult
{
    // всегда захватывается + если тормозит — явная метка Slow
}
```

В Grafana добавьте панель из `oxphp_profiler_runs_total{source="sample"}` —
и альерт на выбросы `duration_ms` из `index.json` (через log-based metric
или side-car экспортёр).

### 15.5. Воспроизвести баг по ссылке

Коллега пишет «у меня на /admin/report 500». Ответ:

```
https://app.local/admin/report?__oxprof=<одноразовый-токен>
```

После её визита — `/__profiler/runs?limit=5`, открываете профиль, видите,
где произошло исключение (`status_code=2` + `Exception`-event).

---

## 16. Взаимодействие с APM (`plugin-apm`, OpenTelemetry)

- Оба плагина держат **одно общее** `Arc<SpanTree>`. Двойного сбора нет.
- Без триггера профайлера + APM включён → `mode=ApmOnly`. В дереве только
  явно отмеченные спаны (`#[Trace]`, SQL/HTTP-хуки APM).
- С триггером профайлера → `mode=ProfileAll`. Дерево содержит **всё** +
  APM-метки.
- APM отправляет в OTLP **только** свои спаны (Jaeger/Tempo ограничены
  ~10k спанов на трейс). Для полноты — `/__profiler/runs/<id>`.

---

## 17. Best practices

1. **Никогда не коммитьте `PROFILER_AUTH_TOKEN`**. Читайте из Vault /
   Docker secrets / Kubernetes secrets.
2. **В проде — только `SAMPLE_RATE`**. Header/Cookie/Query — инструменты
   разработчика. Если нужен on-demand в проде — отдельный токен, ротируемый
   ежедневно.
3. **Не включайте `PROFILER_INTERNAL=true` глобально**. 2–5× оверхед
   превращает прод в лабораторию. Используйте точечно в изоляции.
4. **Держите `PROFILER_RETENTION_COUNT` реалистичным** — каждый запуск
   может весить от сотен КБ (маленький запрос) до мегабайт (большое дерево).
   500 runs × 2 МБ = 1 ГБ. Планируйте диск.
5. **`#[Exclude]` на шумных helpers** (логирование, i18n, автолоадер) —
   дерево становится читаемым без потери смысла.
6. **Связывайте профили с трейсами**: `trace_id` общий. В Grafana / Kibana
   ссылайтесь на `/__profiler/runs/<id>` из trace-вью.
7. **Используйте Git-friendly identifiers**. В этой сборке `span_id` — это
   детерминированный big-endian монотонный счётчик. Diff двух сохранённых
   профилей — чистый.
8. **Плагин APM + профайлер — бесплатно**. Можно держать оба включёнными;
   дерево общее, оверхед только по накопленному APM-покрытию.

---

## 18. Решение проблем

### «Профили не появляются»

1. Плагин скомпилирован? `docker compose build` по умолчанию включает.
   Проверьте, не ставили ли вы `--build-arg OXPHP_WITH_PROFILER=0` или
   кастомный `CARGO_FEATURES` без `plugin-profiler`.
2. `PROFILER_ENABLED=true`?
3. Триггер реально подходит под `PROFILER_AUTH_TOKEN`?
   - Проверьте на лишний `\n` в переменной окружения.
   - Для query — URL-encoded корректно?
4. Сервер видит ваш запрос? Проверьте access-log.

### 401 от `/__profiler/runs`

Bearer-токен в заголовке не совпадает с `PROFILER_AUTH_TOKEN`. Частые
грабли: `echo "secret" > secret.txt` → попадает `\n`. Используйте
`printf` или прокиньте из env.

### «xhgui не показывает новые runs»

1. Проверьте reachability:
   ```bash
   docker compose exec app curl -v $PROFILER_EXPORT_URL
   ```
2. Посмотрите `oxphp_profiler_http_push_failures_total`.
3. Проверьте логи: `tracing::warn!` с `run_id` и HTTP-статусом пишется
   при каждой неудаче.

### Файлы на диске не создаются

- `PROFILER_OUTPUT_DIR` — **абсолютный**? Относительные игнорируются.
- Writable для `www-data`?
  ```bash
  docker compose exec app ls -la /tmp/oxphp-profiles
  ```
- `PROFILER_DISK_MAX_PER_SEC` не слишком низок? Смотрите
  `oxphp_profiler_disk_drops_total`.

### Слишком большой оверхед в проде

- `PROFILER_INTERNAL=false` (это default).
- `PROFILER_SAMPLE_RATE` в разумных пределах (0.0005..0.002).
- `PROFILER_MAX_SPANS` разумный — при превышении дерево усекается,
  но сам capture идёт. Для очень больших запросов лучше использовать
  точечный `start()`/`stop()` вокруг интересующего участка.

### `truncated=true` в `index.json`

Запрос превысил `PROFILER_MAX_SPANS` (default 50 000). Варианты:
1. Поднимите лимит (жертвуя памятью).
2. Добавьте `#[Exclude]` / `#[Sample(rate: 0.01)]` на функции,
   которые зовутся десятки тысяч раз.
3. Оборачивайте только подозрительный участок в `start()`/`stop()`.

---

## 19. Шпаргалка команд

```bash
# Активация на один запрос
curl -H "X-OxPHP-Profile: $TOK" http://localhost/endpoint

# Список запусков, топ-10 по длительности
curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/runs \
  | jq '.runs | sort_by(.duration_ms) | reverse | .[:10]'

# Открыть в speedscope
open "http://localhost:9090/__profiler/runs/$RUN_ID/speedscope"

# Скачать как xhprof для xhgui import
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.xhprof.json > run.xhprof.json

# Скачать как pprof и открыть
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.pprof > run.pprof
go tool pprof -http=:8080 run.pprof

# flamegraph.pl
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.collapsed \
  | flamegraph.pl > flame.svg

# Удалить запуск
curl -X DELETE -H "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID

# Метрики
curl -s http://localhost:9090/metrics | grep oxphp_profiler_

# Текущая конфигурация плагина (безопасно — токены редактированы)
curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/config | jq
```

---

## 20. Практические примеры (полный код)

Ниже — готовые PHP-сценарии, которые можно положить в `www/public/` и
вызвать из curl.

### 20.1. Простой контроллер с ручным управлением

```php
<?php
// www/public/report.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric, is_active};

function fetch_rows(PDO $db, int $user_id): array
{
    $stmt = $db->prepare('SELECT * FROM orders WHERE user_id = ? LIMIT 1000');
    $stmt->execute([$user_id]);
    return $stmt->fetchAll(PDO::FETCH_ASSOC);
}

function render_report(array $rows): string
{
    $sum = array_sum(array_column($rows, 'amount'));
    return json_encode(['count' => count($rows), 'total' => $sum]);
}

$db = new PDO('mysql:host=db;dbname=app', 'app', 'secret');
$user_id = (int) ($_GET['user_id'] ?? 1);

// Явно профилируем только тяжёлый блок — даже если триггер был не задан.
start();

mark('report.begin', ['user_id' => (string) $user_id]);

$rows = fetch_rows($db, $user_id);
metric('db.rows', (float) count($rows));

$body = render_report($rows);
metric('response.bytes', (float) strlen($body));

mark('report.done');
stop();

header('Content-Type: application/json');
echo $body;

// Опционально — подсказка для фронта, что запрос профилировался:
if (is_active()) {
    header('X-Profiled: 1');
}
```

Вызов:

```bash
curl -H "X-OxPHP-Profile: dev-secret" 'http://localhost/report.php?user_id=42'
```

### 20.2. Сервисный класс с атрибутами

```php
<?php
// www/lib/OrderService.php
declare(strict_types=1);

use OxPHP\Profile\{Profile, Tag, Exclude, Sample, SlowThreshold, MemoryThreshold};

#[Profile]
#[Tag(key: 'layer', value: 'domain')]
#[Tag(key: 'svc',   value: 'orders')]
final class OrderService
{
    public function __construct(
        private readonly PDO $db,
        private readonly Mailer $mailer,
    ) {}

    #[SlowThreshold(ms: 250)]
    #[Tag(key: 'op', value: 'create')]
    public function create(array $payload): int
    {
        $this->db->beginTransaction();
        try {
            $id = $this->insertOrder($payload);
            $this->insertLines($id, $payload['items']);
            $this->db->commit();
            $this->mailer->sendReceipt($id);
            return $id;
        } catch (\Throwable $e) {
            $this->db->rollBack();
            throw $e;
        }
    }

    #[MemoryThreshold(kb: 2048)]
    #[Tag(key: 'op', value: 'export')]
    public function exportCsv(int $user_id): string
    {
        $stmt = $this->db->prepare('SELECT * FROM orders WHERE user_id = ?');
        $stmt->execute([$user_id]);

        $buf = fopen('php://temp', 'r+');
        fputcsv($buf, ['id', 'created_at', 'total']);
        while ($row = $stmt->fetch(PDO::FETCH_ASSOC)) {
            fputcsv($buf, [$row['id'], $row['created_at'], $row['total']]);
        }
        rewind($buf);
        return stream_get_contents($buf);
    }

    // Тривиальный getter — не засоряем дерево.
    #[Exclude]
    public function find(int $id): ?array
    {
        $stmt = $this->db->prepare('SELECT * FROM orders WHERE id = ?');
        $stmt->execute([$id]);
        return $stmt->fetch(PDO::FETCH_ASSOC) ?: null;
    }

    // Очень частый аудит — семплируем, чтобы не раздувать дерево.
    #[Sample(rate: 0.05)]
    private function audit(string $event, array $ctx): void
    {
        $this->db->prepare('INSERT INTO audit (event, ctx) VALUES (?, ?)')
                 ->execute([$event, json_encode($ctx)]);
    }

    private function insertOrder(array $p): int { /* ... */ return 0; }
    private function insertLines(int $id, array $items): void { /* ... */ }
}
```

### 20.3. Batch-job: профилируем только первую итерацию из N

```php
<?php
// www/bin/import.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, pause, resume, mark};

$files = glob('/data/incoming/*.csv');
$i = 0;

foreach ($files as $path) {
    if ($i === 0) {
        start();                       // профилируем только первый файл целиком
        mark('batch.begin', ['path' => $path]);
    } else {
        pause();                       // остальные — no-op для capture
    }

    import_one($path);

    if ($i === 0) {
        mark('batch.first_done');
        stop();
    }
    $i++;
}

function import_one(string $path): void { /* ... */ }
```

### 20.4. Сравнение двух реализаций — микро-бенчмарк с профилями

```php
<?php
// www/public/bench.php — сравнение наивной vs потоковой реализации
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric};

function naive_sum(string $path): int
{
    $rows = array_map('str_getcsv', file($path));           // всё в память
    return array_sum(array_column($rows, 1));
}

function streaming_sum(string $path): int
{
    $h = fopen($path, 'r');
    $total = 0;
    while (($row = fgetcsv($h)) !== false) {
        $total += (int) ($row[1] ?? 0);
    }
    fclose($h);
    return $total;
}

$path = '/data/big.csv';
$which = $_GET['impl'] ?? 'naive';

start();
mark('bench.begin', ['impl' => $which]);
$t0 = hrtime(true);

$result = $which === 'naive' ? naive_sum($path) : streaming_sum($path);

$elapsed_ms = (hrtime(true) - $t0) / 1e6;
metric('bench.elapsed_ms', $elapsed_ms);
metric('bench.result',     (float) $result);
mark('bench.done');
stop();

echo json_encode(['impl' => $which, 'elapsed_ms' => $elapsed_ms, 'result' => $result]);
```

Воркфлоу:

```bash
# Наивная реализация
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=naive"

# Потоковая
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=streaming"

# Диф в xhgui (два последних xhprof-запуска)
curl -sH "Authorization: Bearer dev-secret" \
     "http://localhost:9090/__profiler/runs?limit=2" | jq '.runs[] | .run_id'
```

### 20.5. Условное профилирование в «боевом» коде

```php
<?php
// Типичный случай: подозреваемая функция иногда тормозит на конкретном пользователе.
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, is_active};

function charge(User $user, Money $amount): PaymentResult
{
    // Аудит: если трассу этого запроса активно профилируют,
    // включим повышенный логгинг в стороннем вызове.
    $verbose = is_active();

    $gateway = new StripeClient(verbose: $verbose);
    return $gateway->charge($user->id, $amount);
}

function oncall_path(Order $order): void
{
    // Включаем профиль только для VIP-пользователей, даже без внешнего триггера.
    if ($order->user->tier === 'vip') {
        start();
    }
    process($order);
    if ($order->user->tier === 'vip') {
        stop();
    }
}
```

### 20.6. Интеграционный тест, сам себя профилирующий

```php
<?php
// tests/php/profile_smoke.php
declare(strict_types=1);
require __DIR__ . '/test_helper.php';

use function OxPHP\Profile\{start, stop, mark, is_active};

$t = new TestCase('profile_smoke', 'my-app');

// Включаем профайлер вручную (триггер не нужен для теста SDK).
$t->assertFalse('initially not active', is_active());
start();
$t->assertTrue('active after start', is_active());

mark('test.midpoint');

// имитация работы
$sum = 0;
for ($i = 0; $i < 100_000; $i++) { $sum += $i; }

stop();
$t->assertFalse('inactive after stop', is_active());
$t->assertSame('computation OK', $sum, 4999950000);

$t->done();
```

### 20.7. Поиск медленного места по серии запросов из Postman

Сценарий: «у нас иногда тормозит /api/search, но не всегда».

```javascript
// Pre-request Script в Postman
pm.request.headers.add({
    key: 'X-OxPHP-Profile',
    value: pm.environment.get('PROFILE_TOKEN')
});
```

После 100 прогонов коллекции — `jq`-однострочник для топ-аномалий:

```bash
curl -sH "Authorization: Bearer $TOK" \
     "http://int.app.local:9090/__profiler/runs?limit=200" \
  | jq -r '.runs
          | map(select(.url | startswith("/api/search")))
          | sort_by(-.duration_ms)
          | .[:5]
          | map("\(.duration_ms)ms  \(.run_id)  \(.url)")
          | .[]'
```

### 20.8. Кастомный декоратор вокруг профайлера

Собственный декоратор `#[ProfileDb]` — логирует число rows и автоматически
делает `metric('db.rows', …)`:

```php
<?php
use OxPHP\Decorator\{AttributeInterface, Context};
use function OxPHP\Profile\metric;

#[Attribute(Attribute::TARGET_METHOD)]
class ProfileDb implements AttributeInterface
{
    public function before(Context $ctx): void {}

    public function after(Context $ctx): void
    {
        $result = $ctx->returnValue;
        if (is_array($result)) {
            metric('db.rows', (float) count($result));
        } elseif ($result instanceof PDOStatement) {
            metric('db.rows', (float) $result->rowCount());
        }
    }
}

oxphp_register_decorator(ProfileDb::class);

class UserRepository
{
    #[ProfileDb]
    public function findAll(): array { /* ... */ return []; }
}
```

Связка декоратор + профайлер работает «из коробки»: `metric()` автоматически
прикрепится к спану функции, которую сейчас наблюдает Observer API.

---

## 21. Ссылки

- Спецификация в коде: `src/profiling/mod.rs`, `src/plugins/ox_profiler/`
- Bridge (C): `ext/bridge/oxphp_bridge.c`, `ext/oxphp_sapi.c`
- PHP-тесты: `tests/php/profiler/`
- Fixture-примеры форматов: `tests/fixtures/profiler_exports/`
- xhgui демо: `tests/compose.xhgui.yml`
- speedscope: <https://www.speedscope.app/>
- xhgui: <https://github.com/perftools/xhgui>
- Google pprof: <https://github.com/google/pprof>
- flamegraph.pl: <https://github.com/brendangregg/FlameGraph>
