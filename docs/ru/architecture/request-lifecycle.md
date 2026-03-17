---
title: Жизненный цикл запроса
description: Пошаговый обзор обработки HTTP-запроса в OxPHP от приёма TCP до ответа
---

Каждый HTTP-запрос в OxPHP проходит через конвейер стадий, от приёма TCP до доставки ответа. На этой странице прослеживается этот конвейер на основе реального кода в `src/server/connection.rs`.

## Обзор конвейера

```
  Client
    │
    ▼
┌──────────────────┐
│ TCP Accept       │  main.rs: listener.accept()
│ + TLS Handshake  │  server/mod.rs: handle_connection()
└────────┬─────────┘
         ▼
┌──────────────────┐
│ HTTP Parse       │  hyper-util auto::Builder
│ (http1/http2)    │  service_fn → handle_request()
└────────┬─────────┘
         ▼
┌──────────────────┐
│ RequestReceived  │  Event dispatch (priority order):
│ Event            │    -100  RequestIdGenerator
│                  │    -95   TraceContextHandler
│                  │    -50   RateLimitHandler
│                  │      0   MetricsRequestHandler
└────────┬─────────┘
         │
    ┌────┴────┐
    │ Early   │──── Yes ──▶ 429 Too Many Requests
    │ Response│              (skip to RequestComplete)
    │ ?       │
    └────┬────┘
         │ No
         ▼
┌───────────────────┐
│ Plugin Cookie     │  plugin::cookies::strip_plugin_cookies()
│ Strip             │
└────────┬──────────┘
         ▼
┌───────────────────┐
│ Route Resolution  │  routing.rs: resolve_request()
│ Serve / Execute / │  sanitize, validate, file cache
│ NotFound          │
└────────┬──────────┘
         │
    ┌────┴─────────┐
    │              │
    ▼              ▼
┌────────┐  ┌──────────┐
│ Static │  │ PHP      │
│ File   │  │ Execution│
│ Serve  │  │ (worker) │
└───┬────┘  └────┬─────┘
    │            │
    ├── 304? ────┤  (If-None-Match / If-Modified-Since)
    │            │
    └─────┬──────┘
          ▼
┌───────────────────┐
│ ResponseBuilding  │  Event dispatch (priority order):
│ Event             │    -95   TraceContextHandler
│                   │     60   ErrorPagesHandler
│                   │    100   ServerHeaderHandler
└────────┬──────────┘
         ▼
┌───────────────────┐
│ Brotli            │  compression.rs: maybe_compress()
│ Compression       │  (if Accept-Encoding: br)
└────────┬──────────┘
         ▼
┌───────────────────┐
│ RequestComplete   │  Event dispatch (priority order):
│ Event             │      0   MetricsResponseHandler
│                   │    100   AccessLogHandler
└────────┬──────────┘
         ▼
  Response sent
```

## Детали по стадиям

### 1. Приём TCP и настройка соединения

Цикл приёма в `main.rs` вызывает `listener.accept()` для каждого входящего соединения. `Semaphore` с `max_connections` разрешениями ограничивает общую конкурентность. Каждое соединение порождает задачу Tokio:

```rust
let (stream, remote_addr) = listener.accept().await?;
let permit = semaphore.clone().acquire_owned().await?;
tokio::spawn(async move {
    let _permit = permit;
    server_clone.handle_connection(stream, remote_addr).await;
});
```

В `Server::handle_connection()` (`src/server/mod.rs`) сервер записывает соединение в метрики через `ConnectionGuard` (RAII — автоматически уменьшает счётчик при drop) и опционально выполняет TLS-рукопожатие:

```rust
self.metrics.connection_opened();
let _guard = ConnectionGuard(Arc::clone(&self.metrics));

if let Some(ref acceptor) = self.tls_acceptor {
    let tls_stream = acceptor.accept(stream).await?;
    // ... serve over TLS
} else {
    // ... serve over plaintext
}
```

### 2. Разбор HTTP

`auto::Builder` из `hyper-util` обрабатывает определение протокола HTTP/1.1 и HTTP/2. `header_read_timeout` защищает от атак медленных заголовков (требует установки `TokioTimer` на построителе). Построитель вызывает `service_fn`, которая вызывает `handle_request()` для каждого HTTP-запроса в соединении.

### 3. Декомпозиция запроса

В начале `handle_request()` запрос разделяется на части и тело:

```rust
let start = Instant::now();
let (parts, body) = req.into_parts();
```

Здесь же проверяется заголовок `Accept-Encoding` на поддержку Brotli — неаллоцирующая проверка через `is_some_and(compression::accepts_brotli)`.

### 4. Событие RequestReceived

Первая диспетчеризация событий запускает четыре обработчика в порядке приоритета:

| Приоритет | Обработчик | Действие |
|---|---|---|
| -100 | `RequestIdGenerator` | Генерирует `{timestamp_hex:08x}{counter:08x}` (16 hex-символов) или сохраняет входящий `X-Request-ID` |
| -95 | `TraceContextRequestHandler` | Разбирает/генерирует контекст трассировки W3C, записывает `trace_id`/`span_id` в метаданные |
| -50 | `RateLimitHandler` | Проверяет скользящее окно по IP; устанавливает `early_response` при превышении лимита |
| 0 | `MetricsRequestHandler` | Вызывает `metrics.record_request(&method)` |

Событие `RequestReceived` включает поле `metadata: Vec<(String, String)>`, которое обработчики плагинов могут использовать для прикрепления данных ключ-значение.

Идентификатор запроса извлекается с помощью `std::mem::take` (перемещение без копирования):

```rust
let request_id = std::mem::take(&mut received_event.request_id);
```

### 5. Короткое замыкание ранним ответом

Если какой-либо обработчик установил `early_response` в событии `RequestReceived` (обработчик ограничения частоты устанавливает ответ 429), конвейер перескакивает сразу к `RequestComplete`:

```rust
if let Some(early_resp) = received_event.early_response {
    // Dispatch RequestComplete for metrics/logging, then return
    return Ok(early_resp);
}
```

Это гарантирует, что запросы, ограниченные по частоте, всё равно учитываются в метриках и появляются в логе доступа. Строки метода и пути аллоцируются только здесь, в раннем пути (отложены от шага 3 для избежания ненужных аллокаций, когда `early_response` не установлен).

### 6. Удаление cookie плагинов и аллокация строк

После проверки раннего ответа конвейер:

1. Извлекает части запроса из события
2. Аллоцирует строки метода и пути (`method_str`, `path_str`) — отложено до этого момента для избежания аллокации при коротком замыкании запроса
3. Вызывает `plugin::cookies::strip_plugin_cookies()` для удаления внутренних cookie плагинов из заголовков запроса перед передачей в PHP

### 7. Тайм-аут запроса

Если настроен `REQUEST_TIMEOUT_SECONDS` (ненулевое значение), оставшаяся часть конвейера оборачивается в `tokio::time::timeout`. При срабатывании тайм-аута возвращается 504 Gateway Timeout:

```rust
match tokio::time::timeout(server.request_timeout, dispatch_request(...)).await {
    Ok(inner_result) => inner_result,
    Err(_) => Ok(Response::builder()
        .status(StatusCode::GATEWAY_TIMEOUT)
        .body(full_body(Bytes::from_static(b"504 Gateway Timeout")))
        .unwrap()),
}
```

### 8. Разрешение маршрута

`RouteConfig::resolve_request()` в `src/server/routing.rs` разрешает URI-путь в один из трёх результатов:

| Результат | Значение |
|---|---|
| `Serve(PathBuf)` | Отдать статический файл с диска |
| `Execute(PathBuf)` | Отправить в поток воркера PHP |
| `NotFound` | Вернуть 404 |

Процесс маршрутизации:

1. Процентное декодирование URI
2. Очистка пути (удаление сегментов `..` и `.`)
3. Блокировка прямого доступа к `INDEX_FILE` и `.php`-файлам в режиме framework
4. Проверка существования в файловом кэше
5. Откат к `INDEX_FILE`, если настроен (режим framework/SPA)
6. Проверка того, что разрешённый путь не выходит за пределы корня документов через символические ссылки

### 9a. Раздача статических файлов

Для результатов `Serve`, `static_file::serve()` обрабатывает ответ с поддержкой HTTP-кеширования:

1. **Условная проверка (попадание в кеш)** — если файл находится в кеше содержимого, проверяются заголовки `If-None-Match` / `If-Modified-Since` и возвращается `304 Not Modified`, если файл не изменился (без тела, без дискового ввода-вывода)
2. **Попадание в кеш** — возвращается закешированное содержимое с заголовками `Cache-Control`, `ETag` и `Last-Modified`
3. **Промах кеша** — читаются метаданные файла, проверяются условные заголовки до чтения тела файла, затем файл отдаётся с заголовками кеширования

При `STATIC_CACHE_TTL=off` заголовки кеширования опускаются и условные проверки не выполняются.

### 9b. Выполнение PHP (буферизованное или потоковое)

Для результатов `Execute` тело запроса собирается с **лимитом 10 МБ** (`MAX_POST_BODY`). Сбор тела происходит только для запросов POST, PUT и PATCH — все остальные методы (GET, HEAD, DELETE и т.д.) получают пустой `Bytes` без чтения из потока тела. Если тело превышает этот лимит, немедленно возвращается ответ 413 Payload Too Large.

```rust
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

let limited = Limited::new(body, MAX_POST_BODY);
let body_bytes = match BodyExt::collect(limited).await {
    Ok(collected) => collected.to_bytes(),
    Err(e) => {
        if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
            return Ok(Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(...)?);
        }
        return Err(e);
    }
};
```

Формируется `ScriptRequest` и отправляется исполнителю:

```rust
let script_request = ScriptRequest {
    request_id: request_id.to_string(),
    script_path,
    method: parts.method,
    uri: parts.uri,
    query_string,
    headers: parts.headers,
    body: body_bytes,
    remote_addr,
    document_root: ctx.route_config.document_root_arc(),
};

ctx.metrics.request_queued();
let response_rx = ctx.executor.execute(script_request);
```

Задача Tokio ожидает `oneshot::Receiver`. Когда воркер PHP завершает работу, он отправляет обратно `ScriptResponse`, содержащий код статуса, заголовки, тело и время выполнения. При разрыве канала воркера возвращается ошибка 500 и вызывается `metrics.request_dropped()`.

#### Потоковые ответы (SSE)

Когда PHP устанавливает `Content-Type: text/event-stream` (автоматически определяется в обработчике заголовков SAPI) или вызывает `oxphp_stream_flush()`, ответ переключается в потоковый режим:

1. **Доставка заголовков**: SAPI потребляет oneshot `EARLY_TX` для отправки `ScriptResponse` с `stream_rx: Some(receiver)` — заголовки доставляются на сторону Tokio немедленно.
2. **Чанки тела**: Каждый `flush()` или `oxphp_stream_flush()` очищает буфер вывода PHP и отправляет его как `Bytes`-чанк через канал `tokio::sync::mpsc` (ограниченный, ёмкость 64).
3. **StreamBody**: Слой соединения оборачивает приёмник канала в `StreamBody` для чанковой HTTP-доставки вместо использования `full_body()`.
4. **Завершение потока**: Когда PHP-скрипт завершается, воркер уничтожает отправитель `STREAM_TX`, закрывая канал. `StreamBody` возвращает `None`, завершая HTTP-ответ.

Обратное давление (backpressure) применяется естественным образом — если клиент читает медленно, ограниченный канал заполняется и `blocking_send()` блокирует поток PHP-воркера до появления свободного места.

Потоковые ответы пропускают сжатие (Brotli), так как `text/event-stream` не является сжимаемым типом контента.

### 10. Событие ResponseBuilding

После формирования ответа (из раздачи статического файла или выполнения PHP) срабатывает событие `ResponseBuilding`:

| Приоритет | Обработчик | Действие |
|---|---|---|
| -95 | `TraceContextResponseHandler` | Вставляет `traceparent`/`tracestate` в заголовки ответа |
| 60 | `ErrorPagesHandler` | Заменяет тело ответа пользовательской HTML-страницей для статуса >= 400 |
| 100 | `ServerHeaderHandler` | Добавляет заголовки `Server: OxPHP` и `X-Request-ID` |

Это единственная точка, где `request_id` клонируется (один раз), поскольку он нужен снова в событии `RequestComplete`.

### 11. Сжатие Brotli

Если клиент отправил `Accept-Encoding: br` и сжатие включено, `compression::maybe_compress()` выполняется после события ResponseBuilding:

- Проверяет, является ли тип контента сжимаемым (text/html, application/json и т.д.)
- Пропускает ответы, уже имеющие `Content-Encoding`
- Пропускает тела меньше 256 байт или больше 3 МБ
- Сжимает с качеством Brotli 4, размер окна 20
- Использует сжатую версию только если она действительно меньше
- Обновляет `Content-Encoding`, `Content-Length` и добавляет `Vary: Accept-Encoding`

### 12. Событие RequestComplete

Финальное событие несёт полные метаданные запроса:

```rust
let mut complete_event = RequestComplete {
    request_id,    // move, no clone
    method,        // http::Method (moved, no clone)
    path: path_str,
    status,
    duration: elapsed,
    remote_addr,
};
```

| Приоритет | Обработчик | Действие |
|---|---|---|
| 0 | `MetricsResponseHandler` | Вызывает `metrics.record_response(status, duration)` |
| 100 | `AccessLogHandler` | Выводит структурированную JSON-запись лога через `tracing::info!` |

### 13. Доставка ответа

`Ok(response)` возвращается в hyper-util, который сериализует его в сеть. Для keep-alive-соединений замыкание `service_fn` вызывается снова для следующего запроса в том же соединении.

## Обработка ошибок

Ошибки на каждой стадии производят соответствующие HTTP-коды статуса:

| Ошибка | Статус | Источник |
|---|---|---|
| Ограничение частоты | 429 | `RateLimitHandler` через ранний ответ |
| Тело слишком большое | 413 | Сбор тела через `Limited` |
| Тайм-аут запроса | 504 | `tokio::time::timeout` |
| Ошибка воркера PHP | 500 | Сломанный oneshot-канал |
| Очередь заполнена | 503 | `SapiExecutor::execute()` через `try_send` |
| Файл не найден | 404 | Разрешение маршрута |
| Внутренняя ошибка | 500 | Универсальный обработчик в `handle_request` |

## Бюджет аллокаций

Конвейер спроектирован для минимизации аллокаций на запрос:

- **0 клонирований** `request_id` через большую часть конвейера (`std::mem::take`)
- **1 клонирование** `request_id` в событии `ResponseBuilding` (нужен для повторного использования в `RequestComplete`)
- **0 клонирований** `method` (`http::Method`) и `path_str` (перемещаются через конвейер)
- Строки метода и пути **откладываются** до проверки раннего ответа — запросы, ограниченные по частоте, полностью избегают аллокации
- `Accept-Encoding` проверяется неаллоцирующим вызовом `is_some_and`
- `RouteConfig` использует предвычисленные индексные пути для корня `/`, чтобы избежать `PathBuf::join` на каждый запрос

## См. также

- [Обзор архитектуры](./overview.md) — Карта компонентов и высокоуровневый поток данных
- [Система событий](./event-system.md) — Типы событий, приоритеты и регистрация обработчиков
- [Пул воркеров](./worker-pool.md) — Как воркеры PHP обрабатывают `ScriptRequest`
- [SAPI и мост](./sapi-bridge.md) — Внутренний поток выполнения воркера PHP
- [Маршрутизация](../features/routing.md) — Три режима маршрутизации и очистка путей
- [Сжатие](../features/compression.md) — Конфигурация сжатия Brotli
- [Тайм-ауты](../features/timeouts.md) — Поведение тайм-аутов запросов и заголовков
- [Ограничение частоты запросов](../features/rate-limiting.md) — Ограничение частоты по IP и ответы 429
