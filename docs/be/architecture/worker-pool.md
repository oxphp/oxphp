---
title: Пул воркераў
description: Архітэктура пула патокаў воркераў PHP — статычнае/дынамічнае маштабаванне, абмежаваныя каналы, зваротны ціск і трэйт ScriptExecutor
---

OxPHP выконвае PHP-скрыпты на пуле вылучаных патокаў АС, ізаляваных ад асінхроннага рантайму ўводу-вываду. Гэтая старонка апісвае трэйт `ScriptExecutor`, дызайн абмежаванага канала, паводзіны зваротнага ціску і аўтаматычнае маштабаванне воркераў.

## Трэйт ScriptExecutor

Усе бэкенды выканання PHP рэалізуюць трэйт `ScriptExecutor`, вызначаны ў `src/executor/mod.rs`:

```rust
pub trait ScriptExecutor: Send + Sync {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult;

    fn shutdown(&self);

    fn is_healthy(&self) -> bool {
        true
    }

    fn start_scale_manager(&self) {}
}
```

| Метад | Прызначэнне |
|---|---|
| `execute()` | Прыняць запыт і вярнуць `ExecuteResult` (неадкладны або адкладзены адказ) |
| `shutdown()` | Падаць сігнал экзекутару спыніць прыём працы |
| `is_healthy()` | Праверка спраўнасці для ўнутранай канцавой кропкі `/health` |
| `start_scale_manager()` | Запусціць фонавую задачу маштабавання (у stub не робіць нічога; статычны рэжым запускае манітор здароўя) |

Трэйт вяртае `ExecuteResult`, а не сыры `Future` або `oneshot::Receiver`. Гэта дазваляе экзекутару вярнуць адказ з памылкай неадкладна (напр., 503 калі чарга запоўнена) без удзелу патоку воркера, пры гэтым падтрымліваючы адкладзены выпадак, калі задача Tokio чакае `oneshot::Receiver` для адказу.

```rust
pub enum ExecuteResult {
    Immediate(ScriptResponse),
    Deferred(tokio::sync::oneshot::Receiver<ScriptResponse>),
}
```

## Тыпы дадзеных

Запыты і адказы вызначаны ў `src/types.rs`:

```rust
pub struct ScriptRequest {
    pub request_id: String,
    pub script_path: PathBuf,
    pub method: Method,
    pub uri: Uri,
    pub query_string: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub remote_addr: SocketAddr,
    pub document_root: Arc<PathBuf>,
}

pub struct ScriptResponse {
    pub status: u16,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub body: Bytes,
    pub execution_time_us: u64,
}
```

`document_root` абгорнуты ў `Arc<PathBuf>` для танніга сумеснага выкарыстання паміж запытамі. `headers` у адказе выкарыстоўвае `Vec` (не `HeaderMap`), таму што воркер PHP загадзя разбірае радкі загалоўкаў у тыпізаваныя пары `HeaderName`/`HeaderValue` на патоку воркера, пазбягаючы выдаткаў разбору на рантайме Tokio.

## SapiExecutor

Прадукцыйны экзекутар (`src/executor/sapi.rs`, абгароджаны feature-сцягом `--features php`) кіруе пулам патокаў воркераў PHP ZTS, злучаных праз абмежаваны `crossbeam_channel`. Ён падтрымлівае два рэжымы працы: **статычны** (фіксаваны памер пула) і **дынамічны** (аўтаматычнае маштабаванне паміж мінімальнай і максімальнай мяжой).

### Архітэктура

```
                         ┌──────────────────────┐
                         │   crossbeam_channel  │
Tokio tasks ──try_send──▶│   bounded(CAPACITY)  │──recv──▶ php-worker-0
             (non-block) │                      │──recv──▶ php-worker-1
                         │                      │──recv──▶ php-worker-N
                         └──────────────────────┘
                                                          ▲
                         ┌──────────────────────┐         │
                         │   ScaleManager       │─────────┘
                         │   (tokio task,       │  spawn/retire workers
                         │    dynamic mode only)│  based on idle count
                         └──────────────────────┘

Each worker:
  ┌──────────────────────────────────────────────────┐
  │  recv(WorkerRequest)                             │
  │    ├── sapi::clear_buffers()                     │
  │    ├── sapi::set_request_data(request)           │
  │    ├── php_request_startup()                     │
  │    ├── zend_stream_init_filename(file_handle)    │
  │    ├── php_execute_script(file_handle)           │
  │    ├── zend_destroy_file_handle(file_handle)     │
  │    ├── php_request_shutdown()                    │
  │    ├── sapi::take_response() → (output, headers) │
  │    └── tx.send(ScriptResponse)                   │
  └──────────────────────────────────────────────────┘
```

### Рэжымы воркераў

Пераменная асяроддзя `PHP_WORKERS` кіруе рэжымам пула воркераў:

| Фармат | Рэжым | Прыклад | Паводзіны |
|--------|-------|---------|-----------|
| `N` | Статычны | `PHP_WORKERS=8` | Фіксаваны пул з 8 воркераў |
| `0` або не ўсталяваны | Статычны | `PHP_WORKERS=0` | Фіксаваны пул з колькасці ядраў CPU * 2 воркераў |
| `MIN:MAX` | Дынамічны | `PHP_WORKERS=2:16` | Пачынае з MIN, маштабуецца да MAX пад нагрузкай |
| `MIN:0` | Дынамічны | `PHP_WORKERS=2:0` | MIN зададзены яўна, MAX вызначаецца аўтаматычна (CPU * 2) |
| `0:0` | Дынамічны | `PHP_WORKERS=0:0` | MIN аўтаматычна (CPU/2, мін. 2), MAX аўтаматычна (CPU * 2) |

У **статычным рэжыме** памер пула ніколі не змяняецца пасля запуску. Воркеры выкарыстоўваюць блакуючы `recv()` з нулявым спажываннем CPU у рэжыме чакання.

У **дынамічным рэжыме** фонавая задача ScaleManager перыядычна правярае выкарыстанне воркераў і запускае або спыняе воркераў. Воркеры выкарыстоўваюць `recv_timeout(200ms)` для перыядычнай праверкі сцяга спынкі.

### Паслядоўнасць запуску

Канструктар `SapiExecutor::new(metrics)` выконвае ініцыялізацыю PHP на галоўным патоку да запуску любых патокаў воркераў:

1. **Запуск TSRM**: `php_tsrm_startup()` ініцыялізуе Zend Thread Safety. Гэта павінна адбыцца на галоўным патоку да ўсталёўкі любых апрацоўшчыкаў сігналаў асінхроннага рантайму.
2. **Рэгістрацыя SAPI**: `sapi_startup()` рэгіструе карыстальніцкі модуль SAPI `oxphp`.
3. **Запуск рухавіка PHP**: `php_module_startup()` ініцыялізуе рухавік PHP, загружае пашырэнні і разбірае `php.ini`. Гэта запускае MINIT для ўсіх пашырэнняў, у тым ліку пашырэнне OxPHP, якое рэгіструе функцыі плагінаў у Zend.
4. **Зваротны выклік памылак**: `sapi::install_error_cb()` замяняе стандартны апрацоўшчык памылак структураваным JSON-лагіраваннем.
5. **Разбор рэжыму воркераў**: `parse_php_workers()` чытае `PHP_WORKERS` і вяртае `WorkerMode::Static(n)` або `WorkerMode::Dynamic { min, max }`.
6. **Стварэнне канала**: `crossbeam_channel::bounded(queue_capacity)` стварае абмежаваную чаргу працы. Ёмістасць па змаўчанні — `worker_count * 128` (з выкарыстаннем min для дынамічнага рэжыму).
7. **Запуск воркераў**: Запускаюцца пачатковыя воркеры — поўная колькасць для статычнага рэжыму або `min` для дынамічнага. Кожны абгорнуты ў структуру `ManagedWorker`.
8. **Ініцыялізацыя метрык**: Усталёўваюцца `metrics.set_workers_min/max/current` для адлюстравання пачатковага стану пула.

### ManagedWorker

Кожны воркер адсочваецца структурай `ManagedWorker`:

```rust
struct ManagedWorker {
    id: usize,                       // Унікальны ID (для адладачнага вываду)
    handle: JoinHandle<()>,          // Хэндл патоку АС
    shutdown: Arc<AtomicBool>,       // Сцяг спынкі для кожнага воркера
    last_active: Arc<AtomicU64>,     // Epoch у мілісекундах апошняга запыту (толькі дынамічны)
}
```

Сцяг `shutdown` дазваляе спыняць асобных воркераў без закрыцця агульнага канала. Часовая адзнака `last_active` выкарыстоўваецца ScaleManager для ідэнтыфікацыі бяздзейных воркераў пры маштабаванні ўніз.

### Жыццёвы цыкл патоку воркера

Кожны паток воркера:

1. Ініцыялізуе патокалакальнае сховішча TSRM праз `ts_resource_ex()`
2. Уваходзіць у цыкл прыёму (залежыць ад рэжыму):
   - **Статычны рэжым**: Блакуючы `while let Ok(wr) = request_rx.recv()` — нулявы CPU у рэжыме чакання
   - **Дынамічны рэжым**: `recv_timeout(200ms)` з перыядычнай праверкай сцяга `shutdown` і абнаўленнем `last_active`
3. Для кожнага запыту:
   - Ачышчае буферы вываду праз `sapi::clear_buffers()`
   - Усталёўвае дадзеныя запыту (стан SAPI, суперглабальныя) праз `sapi::set_request_data()`
   - Стварае `RequestDataGuard` (RAII — ачышчае дадзеныя SAPI пры знішчэнні, нават пры паніцы)
   - Выклікае `php_request_startup()` (запускае RINIT для ўсіх пашырэнняў)
   - Адкрывае файл скрыпту з `zend_stream_init_filename()`
   - Выконвае з `php_execute_script()`
   - Знішчае файлавы хэндл з `zend_destroy_file_handle()`
   - Выклікае `php_request_shutdown()` (запускае RSHUTDOWN)
   - Збірае адказ: буфер вываду, загалоўкі, код стану праз `sapi::take_response()`
   - Разбірае сырыя радкі загалоўкаў у тыпізаваныя пары `HeaderName`/`HeaderValue` на патоку воркера
   - Адпраўляе адказ праз канал oneshot
4. Умовы выхаду:
   - **Статычны рэжым**: Адпраўшчык канала закрыты (спынка), `recv()` вяртае `Err`
   - **Дынамічны рэжым**: Сцяг `shutdown` усталяваны ScaleManager, або канал адключаны

### ScaleManager (дынамічны рэжым)

У **статычным рэжыме** `start_scale_manager()` запускае задачу маніторынгу здароўя воркераў, а не пустую аперацыю. Манітор здароўя перыядычна правярае наяўнасць ўпаўшых воркераў (воркераў, чый паток АС завяршыўся нечакана) і перазапускае іх, каб падтрымліваць наладжаную мэтавую колькасць. Гэта прадухіляе сітуацыю, калі збой воркера назаўжды скарачае магутнасць пула.

Калі сканфігуравана `PHP_WORKERS=MIN:MAX`, `start_scale_manager()` замест гэтага запускае задачу аўтамасштабавання ScaleManager. ScaleManager працуе на рантайме Tokio і правярае выкарыстанне воркераў кожныя 500 мс:

**Маштабаванне ўверх** (усе ўмовы павінны быць выкананы):
- Выяўлена нуль бяздзейных воркераў (бяздзейны = last_active > 200 мс таму)
- Бягучая колькасць воркераў ніжэй MAX
- Мінула мінімум 500 мс з апошняга маштабавання ўверх

**Маштабаванне ўніз** (усе ўмовы павінны быць выкананы):
- Бягучая колькасць воркераў вышэй MIN
- Воркер быў бяздзейным даўжэй за `PHP_WORKERS_IDLE_SEC` (па змаўчанні 30 с)
- Мінула мінімум 5 секунд з апошняга маштабавання ўніз

ScaleManager здымае блакіроўку Mutex перад запускам новых патокаў АС, каб пазбегнуць блакіроўкі рантайму Tokio. Спыненыя воркеры далучаюцца ў фонавым патоку.

### Канфігурацыя

| Пераменная | Па змаўчанні | Апісанне |
|---|---|---|
| `PHP_WORKERS` | `0` (CPU * 2, статычны) | Рэжым пула воркераў. `N` для статычнага, `MIN:MAX` для дынамічнага |
| `PHP_WORKERS_IDLE_SEC` | `30` | Таймаўт бяздзейнасці перад спыненнем дынамічнага воркера |
| `QUEUE_CAPACITY` | `PHP_WORKERS * 128` | Ёмістасць абмежаванага канала (выкарыстоўвае пачатковую колькасць для дынамічнага) |

## Абмежаваная чарга і зваротны ціск

Канал паміж Tokio і воркерамі PHP выкарыстоўвае `crossbeam_channel::bounded(QUEUE_CAPACITY)`. Экзекутар выклікае `try_send()` (неблакуючы) для дадання запытаў у чаргу:

```rust
if let Err(e) = self.request_tx.as_ref().unwrap().try_send(worker_request) {
    let (status, body) = match e {
        TrySendError::Full(_) => (503, "Service Unavailable: queue full"),
        TrySendError::Disconnected(_) => (500, "PHP worker pool unavailable"),
    };
    return ExecuteResult::Immediate(ScriptResponse {
        status,
        headers: vec![],
        body: Bytes::from_static(body.as_bytes()),
        execution_time_us: 0,
    });
}
```

| Умова | Паводзіны |
|---|---|
| Чарга мае месца | Запыт дадаецца ў чаргу, задача Tokio чакае адказу праз oneshot |
| Чарга запоўнена | 503 Service Unavailable вяртаецца неадкладна з загалоўкам `Retry-After: 1` |
| Воркеры адключаны | 500 Internal Server Error (пул воркераў непрацаздольны) |

Гэты дызайн забяспечвае зваротны ціск: калі воркеры PHP не паспяваюць, новыя запыты адхіляюцца неадкладна, а не ставяцца ў чаргу бясконца. Загаловак `Retry-After: 1` сігналізуе кліентам паўтарыць спробу праз кароткі прамежак часу.

### Метрыкі

Апрацоўшчык злучэнняў адсочвае стан чаргі праз структуру `Metrics`:

| Метад | Калі |
|---|---|
| `metrics.request_queued()` | Непасрэдна перад `executor.execute()` |
| `metrics.request_dequeued()` | Калі прыбывае адказ праз oneshot |
| `metrics.request_dropped()` | Калі канал oneshot разарваны (воркер упаў) |

Гэта экспартуецца як gauge/counter Prometheus: `oxphp_pending_requests`, `oxphp_busy_workers`, `oxphp_dropped_requests_total`.

## StubExecutor

`StubExecutor` (`src/executor/stub.rs`) — гэта бэкенд для тэсціравання і бенчмаркаў з нулявымі накладнымі выдаткамі. Ён вяртае жорстка зададзены адказ 200 OK сінхронна без запуску якіх-небудзь патокаў:

```rust
impl ScriptExecutor for StubExecutor {
    fn execute(&self, _request: ScriptRequest) -> ExecuteResult {
        ExecuteResult::Immediate(ScriptResponse {
            status: 200,
            headers: vec![(
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("text/plain"),
            )],
            body: Bytes::from_static(b"OK"),
            execution_time_us: 0,
        })
    }
}
```

Выкарыстоўвайце stub-экзекутар, усталяваўшы `EXECUTOR=stub`. Ён актывуецца аўтаматычна, калі бінарнік скампіляваны без `--features php`.

## Выбар экзекутара

Фабрыка `create_executor()` у `src/executor/mod.rs` выбірае бэкенд на аснове пераменнай асяроддзя `EXECUTOR` і feature-сцягоў часу кампіляцыі:

| `EXECUTOR` | `--features php` | Вынік |
|---|---|---|
| `sapi` (па змаўчанні) | так | `SapiExecutor` (пул воркераў PHP) |
| `sapi` (па змаўчанні) | не | `StubExecutor` (адкат з папярэджаннем) |
| `stub` | любы | `StubExecutor` (рэжым бенчмарку) |

## Спынка

`SapiExecutor` рэалізуе `Drop` для ўпарадкаванай спынкі:

1. **Глабальны сцяг спынкі**: `global_shutdown.store(true)` — спыняе ScaleManager (калі працуе)
2. **Закрыццё адпраўшчыка канала**: Воркеры бачаць, што `recv()` вяртае `Err` (статычны) або адключаны (дынамічны), і выходзяць са сваіх цыклаў
3. **Спынка кожнага воркера**: Усталёўвае сцяг `shutdown` кожнага воркера, забяспечваючы выхад дынамічных воркераў з іх цыклаў з таймаўтам
4. **Далучэнне ўсіх патокаў воркераў**: Блакуе, пакуль кожны воркер не завершыць бягучы запыт
5. **Ачыстка PHP**: `php_module_shutdown()`, `sapi_shutdown()`, `tsrm_shutdown()` паслядоўна

Гэта гарантуе, што ніводны PHP-запыт не будзе перарваны падчас выканання пры спынцы.

## Гл. таксама

- [Агляд архітэктуры](./overview.md) — Высокаўзроўневая карта кампанентаў і паслядоўнасць запуску
- [SAPI і мост](./sapi-bridge.md) — Як воркеры PHP ўзаемадзейнічаюць з бібліятэкай моста
- [Жыццёвы цыкл запыту](./request-lifecycle.md) — Як запыты ідуць ад Tokio да воркераў PHP
- [Канфігурацыя](../operations/configuration.md) — `PHP_WORKERS`, `QUEUE_CAPACITY` і іншыя пераменныя асяроддзя
- [Метрыкі](../operations/metrics.md) — Метрыкі пула воркераў (чакаючыя, занятыя, адкінутыя)
- [Плаўная спынка](../operations/graceful-shutdown.md) — Паводзіны дрэнажу і разбурэнне воркераў
