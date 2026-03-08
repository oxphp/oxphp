---
title: Сістэма падзей
description: Тыпізаваны дыспетчар падзей з упарадкаваннем па прыярытэце, бяспечным сціраннем тыпаў і рэгістрацыяй апрацоўшчыкаў
---

OxPHP выкарыстоўвае тыпізаваную сістэму падзей для адмежавання скразных задач (метрыкі, лагіраванне, абмежаванне хуткасці, загалоўкі) ад асноўнага канвеера запытаў. Апрацоўшчыкі рэгіструюцца для пэўных тыпаў падзей і выконваюцца ў парадку прыярытэту.

## Асноўныя канцэпцыі

Сістэма падзей пабудавана на трох трэйтах і адным пералічэнні, вызначаных у `src/events/mod.rs`:

### Трэйт Event

```rust
pub trait Event: Any + Send + Sync + 'static {
    fn name(&self) -> &'static str;
}
```

Кожны тып падзеі рэалізуе `Event`. Абмежаванне `Any` дазваляе сціранне тыпаў у дыспетчары. Метад `name()` дае зразумелы радок для адладкі (напр., `"request.received"`).

### Трэйт EventHandler

```rust
pub trait EventHandler<E: Event>: Send + Sync {
    fn handle(&self, event: &mut E) -> Propagation;

    fn priority(&self) -> Priority {
        0
    }
}
```

Апрацоўшчыкі генерычныя адносна пэўнага тыпу падзеі `E`. Яны атрымліваюць зменную спасылку на падзею і вяртаюць значэнне `Propagation`. Прыярытэт па змаўчанні — 0.

### Priority

```rust
pub type Priority = i32;
```

Меншыя значэнні выконваюцца першымі. Адмоўныя прыярытэты выконваюцца перад стандартным (0), дадатныя — пасля. Даступны поўны дыяпазон `i32`.

### Propagation

```rust
pub enum Propagation {
    Continue,
    Stop,
}
```

- `Continue`: Наступны апрацоўшчык у парадку прыярытэту выконваецца.
- `Stop`: Ніякіх далейшых апрацоўшчыкаў для гэтай дыспетчарызацыі падзеі не выконваецца. Дыспетчар вяртае `Propagation::Stop` выклікаючаму коду.

## EventDispatcher

`EventDispatcher` у `src/events/dispatcher.rs` кіруе рэгістрацыяй апрацоўшчыкаў і дыспетчарызацыяй. Ён мае дзве фазы: **зменная** (рэгістрацыя) і **замарожаная** (толькі дыспетчарызацыя).

### Фаза рэгістрацыі

Падчас запуску сервера апрацоўшчыкі рэгіструюцца з дапамогай `on()`:

```rust
let mut dispatcher = EventDispatcher::new();
dispatcher.on(RequestIdGenerator);           // priority -100
dispatcher.on(RateLimitHandler::new(...));   // priority -50
dispatcher.on(MetricsRequestHandler::new(...)); // priority 0
dispatcher.freeze();
```

`on()` выклікае паніку, калі выклікаецца пасля `freeze()`.

### Замарозка

`freeze()` сартуе ўсе спісы апрацоўшчыкаў па прыярытэце (па ўзрастанні) і ўсталёўвае сцяг, які прадухіляе далейшую рэгістрацыю:

```rust
pub fn freeze(&mut self) {
    self.frozen = true;
    for handlers in self.handlers.values_mut() {
        handlers.sort_by_key(|(priority, _)| *priority);
    }
}
```

Пасля замарозкі дыспетчар абгортваецца ў `Arc` і размяркоўваецца нязменна паміж усімі задачамі Tokio.

### Дыспетчарызацыя

```rust
pub fn dispatch<E: Event>(&self, event: &mut E) -> Propagation {
    let type_id = TypeId::of::<E>();
    let Some(handlers) = self.handlers.get(&type_id) else {
        return Propagation::Continue;
    };

    for (_, handler_fn) in handlers {
        if handler_fn(event) == Propagation::Stop {
            return Propagation::Stop;
        }
    }

    Propagation::Continue
}
```

Дыспетчарызацыя мае складанасць `O(n)`, дзе `n` — колькасць апрацоўшчыкаў для дадзенага тыпу падзеі. Калі для тыпу падзеі не зарэгістравана ніводнага апрацоўшчыка, дыспетчарызацыя — гэта адзін пошук у хэш-табліцы, які вяртаецца неадкладна.

## Сціранне тыпаў

Дыспетчару трэба захоўваць апрацоўшчыкі для розных тыпаў падзей у адной калекцыі. Ён дасягае гэтага з дапамогай бяспечнага сцірання тыпаў — без блокаў `unsafe`.

### Як гэта працуе

Кожны апрацоўшчык абгортваецца ў замыканне, якое выконвае зваротнае прывядзенне `dyn Any`:

```rust
pub fn on<E: Event>(&mut self, handler: impl EventHandler<E> + 'static) {
    let priority = handler.priority();
    let f: ErasedFn = Box::new(move |event: &mut dyn Any| {
        handler.handle(event.downcast_mut::<E>().expect("event type mismatch"))
    });

    self.handlers
        .entry(TypeId::of::<E>())
        .or_default()
        .push((priority, f));
}
```

Тып `ErasedFn`:

```rust
type ErasedFn = Box<dyn Fn(&mut dyn Any) -> Propagation + Send + Sync>;
```

Ключ `TypeId::of::<E>()` гарантуе, што апрацоўшчык, зарэгістраваны для `RequestReceived`, будзе выкліканы толькі з падзеяй `RequestReceived`. Выклік `downcast_mut` — гэта праверка тыпу ў час выканання, але яна можа не спрацаваць толькі пры памылцы ў самім дыспетчары (падзеі маршрутызуюцца па `TypeId`).

### Identity-хэшаванне

Карта апрацоўшчыкаў выкарыстоўвае карыстальніцкі `TypeIdHasher`, які пазбягае накладных выдаткаў SipHash для ключоў `TypeId`:

```rust
struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 { self.0 }
    fn write_u128(&mut self, i: u128) { self.0 = i as u64; }
    // ...
}
```

`TypeId` хэшуецца праз `write_u128`. Identity-хэшар бярэ малодшыя 64 біты напрамую, што бяспечна, таму што значэнні `TypeId` ужо добра размеркаваны. Гэта пазбягае падвойнага хэшавання `HashMap<TypeId, V>` са стандартным `SipHash`.

## Тыпы падзей

OxPHP вызначае 18 тыпаў падзей у `src/events/types.rs`, арганізаваных па этапах жыццёвага цыклу:

### Жыццёвы цыкл сервера

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `ServerBooting` | `server.booting` | (няма) | Выклікаецца падчас загрузкі сервера, да прывязкі |
| `ServerStarted` | `server.started` | `listen_addr: String` | Сервер слухае і гатовы |
| `ShutdownInitiated` | `server.shutdown_initiated` | (няма) | Пачата плаўная спынка |

### Канфігурацыя

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `ConfigLoading` | `config.loading` | (няма) | Загрузка канфігурацыі ў працэсе |

### Злучэнне

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `ConnectionAccepted` | `connection.accepted` | `remote_addr` | Прынята новае TCP-злучэнне |
| `ConnectionClosed` | `connection.closed` | `remote_addr` | TCP-злучэнне закрыта |

### Запыт

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `RequestReceived` | `request.received` | `parts`, `remote_addr`, `request_id`, `early_response`, `metadata: Vec<(String, String)>` | HTTP-запыт атрыманы, да маршрутызацыі |
| `RouteResolved` | `request.route_resolved` | `request_id`, `path` | Маршрут разрашаны, да выканання |
| `RequestComplete` | `request.complete` | `request_id`, `method: Method`, `path`, `status`, `duration`, `remote_addr` | Запыт цалкам апрацаваны |

### PHP

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `ScriptExecutionStarting` | `php.script_execution_starting` | `request_id`, `script_path` | Збіраецца выконваць PHP-скрыпт |
| `PhpRequestStartup` | `php.request_startup` | `request_id` | Фаза PHP RINIT |
| `PhpRequestShutdown` | `php.request_shutdown` | `request_id` | Фаза PHP RSHUTDOWN |
| `ScriptExecutionComplete` | `php.script_execution_complete` | `request_id`, `execution_time_us` | Скрыпт завершаны |

### Адказ

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `ResponseBuilding` | `response.building` | `request_id`, `response` | Мадыфікацыя адказу перад адпраўкай |

### Памылка

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `RequestTimedOut` | `error.request_timed_out` | `request_id`, `timeout` | Запыт перавысіў таймаўт |
| `RequestError` | `error.request_error` | `request_id`, `error` | Неапрацаваная памылка запыту |

### Сэрвіс

| Падзея | Назва | Палі | Апісанне |
|---|---|---|---|
| `HealthCheckRequested` | `service.health_check` | `executor_healthy` | Правераны endpoint спраўнасці |
| `MetricsCollected` | `service.metrics_collected` | (няма) | Метрыкі сабраны |

## Актыўныя падзеі ў канвееры

Тры падзеі зараз дыспетчарызуюцца ў канвееры запытаў (`src/server/connection.rs`):

```
RequestReceived ──▶ [route + execute] ──▶ ResponseBuilding ──▶ [compress] ──▶ RequestComplete
```

Астатнія тыпы падзей вызначаны для выкарыстання сістэмай плагінаў і для карыстальніцкай рэгістрацыі апрацоўшчыкаў.

### RequestReceived

Апрацоўшчыкі могуць інспектаваць/мадыфікаваць часткі HTTP-запыту, прызначыць ідэнтыфікатар запыту і скарочана спыніць канвеер, усталяваўшы `early_response`:

```rust
pub struct RequestReceived {
    pub parts: Parts,
    pub remote_addr: SocketAddr,
    pub request_id: String,
    pub early_response: Option<Response<ResponseBody>>,
    pub metadata: Vec<(String, String)>,
}
```

Поле `metadata` дазваляе апрацоўшчыкам плагінаў далучаць пары ключ-значэнне, якія ідуць з запытам праз увесь канвеер.

Усталёўка `early_response` **не** спыняе распаўсюджванне. Абмежавальнік хуткасці вяртае `Propagation::Continue`, каб апрацоўшчык метрык (прыярытэт 0) усё яшчэ запісваў запыт. Канвеер правярае `early_response` пасля выканання ўсіх апрацоўшчыкаў `RequestReceived`.

### ResponseBuilding

Апрацоўшчыкі могуць мадыфікаваць адказ — замяніць цела (старонкі памылак), дадаць загалоўкі (Server, X-Request-ID):

```rust
pub struct ResponseBuilding {
    pub request_id: String,
    pub response: Response<ResponseBody>,
}
```

### RequestComplete

Падзея толькі для чытання для лагіравання і метрык. Усе палі — уласныя значэнні:

```rust
pub struct RequestComplete {
    pub request_id: String,
    pub method: Method,  // http::Method
    pub path: String,
    pub status: u16,
    pub duration: Duration,
    pub remote_addr: SocketAddr,
}
```

## Апрацоўшчыкі

Сем апрацоўшчыкаў пастаўляюцца з OxPHP, вызначаныя ў `src/handlers/`:

| Апрацоўшчык | Падзея | Прыярытэт | Апісанне |
|---|---|---|---|
| `RequestIdGenerator` | `RequestReceived` | -100 | Генеруе `{ts:08x}{counter:08x}` або захоўвае загаловак `X-Request-ID` |
| `RateLimitHandler` | `RequestReceived` | -50 | Правярае абмежаванне хуткасці па IP, усталёўвае `early_response` з 429 |
| `MetricsRequestHandler` | `RequestReceived` | 0 | Запісвае колькасць запытаў і метад |
| `MetricsResponseHandler` | `RequestComplete` | 0 | Запісвае клас статусу адказу і працягласць |
| `ErrorPagesHandler` | `ResponseBuilding` | 60 | Замяняе цела адказу на карыстальніцкі HTML (статус >= 400) |
| `ServerHeaderHandler` | `ResponseBuilding` | 100 | Дадае загалоўкі `Server: OxPHP` і `X-Request-ID` |
| `AccessLogHandler` | `RequestComplete` | 100 | Выводзіць структураваны JSON-лог доступу праз `tracing::info!` (рэгіструецца толькі калі ўключаны `config.access_log`) |

### Дызайн прыярытэтаў

Прызначэнне прыярытэтаў мае наўмысны парадак:

- **RequestIdGenerator (-100)**: Павінен выконвацца першым, каб усе наступныя апрацоўшчыкі маглі выкарыстоўваць `request_id`
- **RateLimitHandler (-50)**: Выконваецца пасля прызначэння ідэнтыфікатара запыту, каб адхіленыя запыты мелі ідэнтыфікатары ў логу доступу
- **MetricsRequestHandler (0)**: Лічыць усе запыты, уключаючы абмежаваныя па хуткасці (бо RateLimitHandler вяртае `Continue`)
- **ErrorPagesHandler (60)**: Выконваецца да ServerHeaderHandler, каб цела старонкі памылкі было на месцы, калі дадаюцца загалоўкі
- **ServerHeaderHandler (100)**: Выконваецца апошнім у ResponseBuilding — дадае фінальныя загалоўкі пасля ўсіх мадыфікацый цела
- **MetricsResponseHandler (0)** і **AccessLogHandler (100)**: Выконваюцца на RequestComplete пасля поўнай пабудовы адказу

### Умоўная рэгістрацыя

Не ўсе апрацоўшчыкі заўсёды актыўныя. У `main.rs`:

```rust
// Always registered
dispatcher.on(RequestIdGenerator);
dispatcher.on(MetricsRequestHandler::new(...));
dispatcher.on(MetricsResponseHandler::new(...));
dispatcher.on(ServerHeaderHandler);

// Only if configured
if let Some(ref limiter) = rate_limiter {
    dispatcher.on(RateLimitHandler::new(Arc::clone(limiter)));
}
if let Some(ref pages) = error_pages {
    dispatcher.on(ErrorPagesHandler::new(Arc::clone(pages)));
}
if config.access_log {
    dispatcher.on(AccessLogHandler);
}

dispatcher.freeze();
```

Апрацоўшчыкі плагінаў рэгіструюцца праз `plugin_manager.init_all(&mut dispatcher)` да ўбудаваных апрацоўшчыкаў, падчас ранняга запуску.

## Гл. таксама

- [Агляд архітэктуры](./overview.md) — Карта кампанентаў і паслядоўнасць запуску
- [Жыццёвы цыкл запыту](./request-lifecycle.md) — Як падзеі ўпісваюцца ў канвеер запытаў
- [Пул воркераў](./worker-pool.md) — Патокі воркераў PHP, якія фармуюць адказы
- [Абмежаванне хуткасці](../features/rate-limiting.md) — Канфігурацыя RateLimitHandler
- [Старонкі памылак](../features/error-pages.md) — Канфігурацыя ErrorPagesHandler
- [Ідэнтыфікатары запытаў](../features/request-ids.md) — Фармат і паводзіны RequestIdGenerator
- [Лагіраванне доступу](../features/access-logging.md) — Фармат вываду AccessLogHandler
