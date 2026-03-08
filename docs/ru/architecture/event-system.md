---
title: Система событий
description: Типизированный диспетчер событий с упорядочиванием по приоритету, безопасным затиранием типов и регистрацией обработчиков
---

OxPHP использует типизированную систему событий для отделения сквозной функциональности (метрики, логирование, ограничение частоты запросов, заголовки) от основного конвейера обработки запросов. Обработчики регистрируются для определённых типов событий и выполняются в порядке приоритета.

## Основные концепции

Система событий построена на трёх трейтах и одном перечислении, определённых в `src/events/mod.rs`:

### Трейт Event

```rust
pub trait Event: Any + Send + Sync + 'static {
    fn name(&self) -> &'static str;
}
```

Каждый тип события реализует `Event`. Ограничение `Any` обеспечивает затирание типов в диспетчере. Метод `name()` предоставляет читаемую строку для отладки (например, `"request.received"`).

### Трейт EventHandler

```rust
pub trait EventHandler<E: Event>: Send + Sync {
    fn handle(&self, event: &mut E) -> Propagation;

    fn priority(&self) -> Priority {
        0
    }
}
```

Обработчики являются обобщёнными для конкретного типа события `E`. Они получают изменяемую ссылку на событие и возвращают значение `Propagation`. Приоритет по умолчанию — 0.

### Priority

```rust
pub type Priority = i32;
```

Меньшие значения выполняются первыми. Отрицательные приоритеты выполняются до значения по умолчанию (0), положительные — после. Доступен весь диапазон `i32`.

### Propagation

```rust
pub enum Propagation {
    Continue,
    Stop,
}
```

- `Continue`: Следующий обработчик в порядке приоритета выполняется.
- `Stop`: Дальнейшие обработчики для данной диспетчеризации события не выполняются. Диспетчер возвращает `Propagation::Stop` вызывающей стороне.

## EventDispatcher

`EventDispatcher` в `src/events/dispatcher.rs` управляет регистрацией обработчиков и диспетчеризацией. Он имеет две фазы: **изменяемая** (регистрация) и **замороженная** (только диспетчеризация).

### Фаза регистрации

Во время запуска сервера обработчики регистрируются с помощью `on()`:

```rust
let mut dispatcher = EventDispatcher::new();
dispatcher.on(RequestIdGenerator);           // priority -100
dispatcher.on(RateLimitHandler::new(...));   // priority -50
dispatcher.on(MetricsRequestHandler::new(...)); // priority 0
dispatcher.freeze();
```

`on()` вызывает панику при вызове после `freeze()`.

### Заморозка

`freeze()` сортирует все списки обработчиков по приоритету (по возрастанию) и устанавливает флаг, предотвращающий дальнейшую регистрацию:

```rust
pub fn freeze(&mut self) {
    self.frozen = true;
    for handlers in self.handlers.values_mut() {
        handlers.sort_by_key(|(priority, _)| *priority);
    }
}
```

После заморозки диспетчер оборачивается в `Arc` и разделяется неизменяемо между всеми задачами Tokio.

### Диспетчеризация

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

Диспетчеризация имеет сложность `O(n)`, где `n` — количество обработчиков для данного типа события. Если для типа события не зарегистрировано ни одного обработчика, диспетчеризация сводится к одному поиску в хеш-таблице с немедленным возвратом.

## Затирание типов

Диспетчеру необходимо хранить обработчики для разных типов событий в одной коллекции. Это достигается безопасным затиранием типов — без блоков `unsafe`.

### Как это работает

Каждый обработчик оборачивается в замыкание, выполняющее приведение `dyn Any`:

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

Тип `ErasedFn`:

```rust
type ErasedFn = Box<dyn Fn(&mut dyn Any) -> Propagation + Send + Sync>;
```

Ключ `TypeId::of::<E>()` гарантирует, что обработчик, зарегистрированный для `RequestReceived`, будет вызван только с событием `RequestReceived`. Вызов `downcast_mut` — это проверка типа во время выполнения, но она может завершиться неудачей только при ошибке в самом диспетчере (события маршрутизируются по `TypeId`).

### Identity-хеширование

Карта обработчиков использует собственный `TypeIdHasher`, который избегает накладных расходов SipHash для ключей `TypeId`:

```rust
struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 { self.0 }
    fn write_u128(&mut self, i: u128) { self.0 = i as u64; }
    // ...
}
```

`TypeId` хеширует через `write_u128`. Identity-хешер берёт младшие 64 бита напрямую, что безопасно, поскольку значения `TypeId` уже хорошо распределены. Это устраняет накладные расходы двойного хеширования `HashMap<TypeId, V>` со стандартным `SipHash`.

## Типы событий

OxPHP определяет 18 типов событий в `src/events/types.rs`, организованных по стадиям жизненного цикла:

### Жизненный цикл сервера

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `ServerBooting` | `server.booting` | (нет) | Вызывается при загрузке сервера, до привязки |
| `ServerStarted` | `server.started` | `listen_addr: String` | Сервер слушает и готов |
| `ShutdownInitiated` | `server.shutdown_initiated` | (нет) | Начата плавная остановка |

### Конфигурация

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `ConfigLoading` | `config.loading` | (нет) | Загрузка конфигурации в процессе |

### Соединение

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `ConnectionAccepted` | `connection.accepted` | `remote_addr` | Принято новое TCP-соединение |
| `ConnectionClosed` | `connection.closed` | `remote_addr` | TCP-соединение закрыто |

### Запрос

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `RequestReceived` | `request.received` | `parts`, `remote_addr`, `request_id`, `early_response`, `metadata: Vec<(String, String)>` | HTTP-запрос получен, до маршрутизации |
| `RouteResolved` | `request.route_resolved` | `request_id`, `path` | Маршрут определён, до выполнения |
| `RequestComplete` | `request.complete` | `request_id`, `method: Method`, `path`, `status`, `duration`, `remote_addr` | Запрос полностью обработан |

### PHP

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `ScriptExecutionStarting` | `php.script_execution_starting` | `request_id`, `script_path` | Подготовка к выполнению PHP-скрипта |
| `PhpRequestStartup` | `php.request_startup` | `request_id` | Фаза PHP RINIT |
| `PhpRequestShutdown` | `php.request_shutdown` | `request_id` | Фаза PHP RSHUTDOWN |
| `ScriptExecutionComplete` | `php.script_execution_complete` | `request_id`, `execution_time_us` | Скрипт завершён |

### Ответ

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `ResponseBuilding` | `response.building` | `request_id`, `response` | Модификация ответа перед отправкой |

### Ошибка

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `RequestTimedOut` | `error.request_timed_out` | `request_id`, `timeout` | Запрос превысил тайм-аут |
| `RequestError` | `error.request_error` | `request_id`, `error` | Необработанная ошибка запроса |

### Служебные

| Событие | Имя | Поля | Описание |
|---|---|---|---|
| `HealthCheckRequested` | `service.health_check` | `executor_healthy` | Выполнена проверка состояния |
| `MetricsCollected` | `service.metrics_collected` | (нет) | Метрики собраны |

## Активные события в конвейере

Три события в настоящее время диспетчеризуются в конвейере обработки запросов (`src/server/connection.rs`):

```
RequestReceived ──▶ [route + execute] ──▶ ResponseBuilding ──▶ [compress] ──▶ RequestComplete
```

Остальные типы событий определены для использования системой плагинов и для регистрации пользовательских обработчиков.

### RequestReceived

Обработчики могут проверять/модифицировать части HTTP-запроса, назначать идентификатор запроса и прерывать конвейер, установив `early_response`:

```rust
pub struct RequestReceived {
    pub parts: Parts,
    pub remote_addr: SocketAddr,
    pub request_id: String,
    pub early_response: Option<Response<ResponseBody>>,
    pub metadata: Vec<(String, String)>,
}
```

Поле `metadata` позволяет обработчикам плагинов прикреплять пары ключ-значение, которые сопровождают запрос через конвейер.

Установка `early_response` **не** останавливает распространение. Обработчик ограничения частоты запросов возвращает `Propagation::Continue`, чтобы обработчик метрик (приоритет 0) всё равно записал запрос. Конвейер проверяет `early_response` после выполнения всех обработчиков `RequestReceived`.

### ResponseBuilding

Обработчики могут модифицировать ответ — заменить тело (страницы ошибок), добавить заголовки (Server, X-Request-ID):

```rust
pub struct ResponseBuilding {
    pub request_id: String,
    pub response: Response<ResponseBody>,
}
```

### RequestComplete

Событие только для чтения, предназначенное для логирования и метрик. Все поля являются владеющими значениями:

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

## Обработчики

OxPHP поставляется с семью обработчиками, определёнными в `src/handlers/`:

| Обработчик | Событие | Приоритет | Описание |
|---|---|---|---|
| `RequestIdGenerator` | `RequestReceived` | -100 | Генерирует `{ts:08x}{counter:08x}` или сохраняет заголовок `X-Request-ID` |
| `RateLimitHandler` | `RequestReceived` | -50 | Проверяет ограничение частоты по IP, устанавливает `early_response` с 429 |
| `MetricsRequestHandler` | `RequestReceived` | 0 | Записывает количество запросов и метод |
| `MetricsResponseHandler` | `RequestComplete` | 0 | Записывает класс статуса ответа и продолжительность |
| `ErrorPagesHandler` | `ResponseBuilding` | 60 | Заменяет тело ответа пользовательским HTML (статус >= 400) |
| `ServerHeaderHandler` | `ResponseBuilding` | 100 | Добавляет заголовки `Server: OxPHP` и `X-Request-ID` |
| `AccessLogHandler` | `RequestComplete` | 100 | Выводит структурированный JSON-лог доступа через `tracing::info!` (регистрируется только когда `config.access_log` включён) |

### Проектирование приоритетов

Назначение приоритетов следует продуманному порядку:

- **RequestIdGenerator (-100)**: Должен выполняться первым, чтобы все последующие обработчики могли использовать `request_id`
- **RateLimitHandler (-50)**: Выполняется после назначения идентификатора запроса, чтобы отклонённые запросы имели ID в логе доступа
- **MetricsRequestHandler (0)**: Считает все запросы, включая ограниченные по частоте (поскольку RateLimitHandler возвращает `Continue`)
- **ErrorPagesHandler (60)**: Выполняется до ServerHeaderHandler, чтобы тело страницы ошибки было на месте при добавлении заголовков
- **ServerHeaderHandler (100)**: Выполняется последним в ResponseBuilding — добавляет финальные заголовки после всех модификаций тела
- **MetricsResponseHandler (0)** и **AccessLogHandler (100)**: Выполняются при RequestComplete после полной сборки ответа

### Условная регистрация

Не все обработчики активны постоянно. В `main.rs`:

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

Обработчики плагинов регистрируются через `plugin_manager.init_all(&mut dispatcher)` до встроенных обработчиков, на раннем этапе запуска.

## См. также

- [Обзор архитектуры](./overview.md) — Карта компонентов и последовательность запуска
- [Жизненный цикл запроса](./request-lifecycle.md) — Как события вписываются в конвейер обработки запросов
- [Пул воркеров](./worker-pool.md) — Потоки воркеров PHP, формирующие ответы
- [Ограничение частоты запросов](../features/rate-limiting.md) — Конфигурация RateLimitHandler
- [Страницы ошибок](../features/error-pages.md) — Конфигурация ErrorPagesHandler
- [Идентификаторы запросов](../features/request-ids.md) — Формат и поведение RequestIdGenerator
- [Логирование доступа](../features/access-logging.md) — Формат вывода AccessLogHandler
