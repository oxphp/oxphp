---
title: Жыццёвы цыкл запыту
description: Пакрокавы агляд таго, як OxPHP апрацоўвае HTTP-запыт ад прыёму TCP да адказу
---

Кожны HTTP-запыт у OxPHP праходзіць праз канвеер этапаў, ад прыёму TCP да дастаўкі адказу. Гэтая старонка прасочвае гэты канвеер праз рэальны код у `src/server/connection.rs`.

## Агляд канвеера

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
│ Event             │     60   ErrorPagesHandler
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

## Дэталі паэтапна

### 1. Прыём TCP і наладка злучэння

Цыкл прыёму ў `main.rs` выклікае `listener.accept()` для кожнага ўваходнага злучэння. `Semaphore` з `max_connections` дазволамі абмяжоўвае агульную канкурэнтнасць. Кожнае злучэнне запускае задачу Tokio:

```rust
let (stream, remote_addr) = listener.accept().await?;
let permit = semaphore.clone().acquire_owned().await?;
tokio::spawn(async move {
    let _permit = permit;
    server_clone.handle_connection(stream, remote_addr).await;
});
```

У `Server::handle_connection()` (`src/server/mod.rs`) сервер запісвае злучэнне ў метрыкі праз `ConnectionGuard` (RAII — аўтаматычна памяншае лічыльнік пры знішчэнні) і апцыянальна выконвае TLS-рукапацісканне:

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

`auto::Builder` з `hyper-util` апрацоўвае вызначэнне пратаколу HTTP/1.1 і HTTP/2. `header_read_timeout` абараняе ад атак з павольнымі загалоўкамі (патрабуе ўсталёўкі `TokioTimer` на зборшчыку). Зборшчык выклікае `service_fn`, якая выклікае `handle_request()` для кожнага HTTP-запыту на злучэнні.

### 3. Дэкампазіцыя запыту

У пачатку `handle_request()` запыт разбіваецца на часткі і цела:

```rust
let start = Instant::now();
let (parts, body) = req.into_parts();
```

Загаловак `Accept-Encoding` правяраецца тут на падтрымку Brotli — неалакацыйная праверка праз `is_some_and(compression::accepts_brotli)`.

### 4. Падзея RequestReceived

Першая дыспетчарызацыя падзей запускае тры апрацоўшчыкі ў парадку прыярытэту:

| Прыярытэт | Апрацоўшчык | Дзеянне |
|---|---|---|
| -100 | `RequestIdGenerator` | Генеруе `{timestamp_hex:08x}{counter:08x}` (16 hex-сімвалаў) або захоўвае ўваходны `X-Request-ID` |
| -50 | `RateLimitHandler` | Правярае слізгальнае акно па IP; усталёўвае `early_response`, калі ліміт перавышаны |
| 0 | `MetricsRequestHandler` | Выклікае `metrics.record_request(&method)` |

Падзея `RequestReceived` уключае поле `metadata: Vec<(String, String)>`, якое апрацоўшчыкі плагінаў могуць выкарыстоўваць для далучэння дадзеных ключ-значэнне.

Ідэнтыфікатар запыту здабываецца з дапамогай `std::mem::take` (перамяшчэнне без капіравання, без клона):

```rust
let request_id = std::mem::take(&mut received_event.request_id);
```

### 5. Скарочаны адказ (short-circuit)

Калі які-небудзь апрацоўшчык усталяваў `early_response` на падзеі `RequestReceived` (абмежавальнік хуткасці ўсталёўвае адказ 429), канвеер пераскоквае напрамую да `RequestComplete`:

```rust
if let Some(early_resp) = received_event.early_response {
    // Dispatch RequestComplete for metrics/logging, then return
    return Ok(early_resp);
}
```

Гэта забяспечвае, што запыты, абмежаваныя па хуткасці, усё яшчэ ўлічваюцца ў метрыках і з'яўляюцца ў логу доступу. Радкі метаду і шляху алакуюцца толькі тут, у ранняй галінцы (адкладзены з кроку 3, каб пазбегнуць непатрэбных алакацый, калі `early_response` не ўсталяваны).

### 6. Зняцце cookies плагінаў і алакацыя радкоў

Пасля праверкі ранняга адказу канвеер:

1. Здабывае часткі запыту з падзеі
2. Алакуе радкі метаду і шляху (`method_str`, `path_str`) — адкладзена да гэтага моманту, каб пазбегнуць алакацыі, калі запыт скарочана спынены
3. Выклікае `plugin::cookies::strip_plugin_cookies()` для выдалення ўнутраных cookies плагінаў з загалоўкаў запыту перад перадачай у PHP

### 7. Таймаўт запыту

Калі сканфігуравана `REQUEST_TIMEOUT_SECONDS` (не нуль), рэшта канвеера абгортваецца ў `tokio::time::timeout`. Калі таймаўт спрацоўвае, вяртаецца 504 Gateway Timeout:

```rust
match tokio::time::timeout(server.request_timeout, dispatch_request(...)).await {
    Ok(inner_result) => inner_result,
    Err(_) => Ok(Response::builder()
        .status(StatusCode::GATEWAY_TIMEOUT)
        .body(full_body(Bytes::from_static(b"504 Gateway Timeout")))
        .unwrap()),
}
```

### 8. Разрашэнне маршруту

`RouteConfig::resolve_request()` у `src/server/routing.rs` разрашае URI-шлях у адзін з трох вынікаў:

| Вынік | Значэнне |
|---|---|
| `Serve(PathBuf)` | Аддаць статычны файл з дыска |
| `Execute(PathBuf)` | Адправіць на паток воркера PHP |
| `NotFound` | Вярнуць 404 |

Працэс маршрутызацыі:

1. Працэнтнае дэкадаванне URI
2. Санітызацыя шляху (выдаленне сегментаў `..` і `.`)
3. Блакіроўка прамога доступу да `INDEX_FILE` і файлаў `.php` у рэжыме фрэймворка
4. Праверка кэша файлаў на існаванне
5. Адкат да `INDEX_FILE`, калі сканфігуравана (рэжым фрэймворка/SPA)
6. Праверка, што разрашаны шлях не выходзіць за межы каранёвага каталога праз сімвалічныя спасылкі

### 9a. Аддача статычных файлаў

Для вынікаў `Serve`, `static_file::serve()` апрацоўвае адказ з падтрымкай HTTP-кэшавання:

1. **Умоўная праверка (трапленне ў кэш)** — калі файл знаходзіцца ў кэшы змесціва, правяраюцца загалоўкі `If-None-Match` / `If-Modified-Since` і вяртаецца `304 Not Modified`, калі файл не змяніўся (без цела, без дыскавага ўводу-вываду)
2. **Трапленне ў кэш** — вяртаецца закэшаванае змесціва з загалоўкамі `Cache-Control`, `ETag` і `Last-Modified`
3. **Промах кэша** — чытаюцца метаданыя файла, правяраюцца ўмоўныя загалоўкі да чытання цела файла, затым файл аддаецца з загалоўкамі кэшавання

Калі `STATIC_CACHE_TTL=off`, загалоўкі кэшавання апускаюцца і ўмоўныя праверкі прапускаюцца.

### 9b. Выкананне PHP (буферызаванае або стрымінгавае)

Для вынікаў `Execute` цела запыту збіраецца з **абмежаваннем 10 МБ** (`MAX_POST_BODY`). Збор цела адбываецца толькі для запытаў POST, PUT і PATCH — усе іншыя метады (GET, HEAD, DELETE і інш.) атрымліваюць пусты `Bytes` без чытання патоку цела. Калі цела перавышае гэты ліміт, неадкладна вяртаецца адказ 413 Payload Too Large.

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

Ствараецца `ScriptRequest` і адпраўляецца ў экзекутар:

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

Задача Tokio чакае `oneshot::Receiver`. Калі воркер PHP завяршае працу, ён адпраўляе назад `ScriptResponse`, які змяшчае код стану, загалоўкі, цела і час выканання. Калі канал воркера разарваны, вяртаецца памылка 500 і выклікаецца `metrics.request_dropped()`.

#### Стрымінгавыя адказы (SSE)

Калі PHP усталёўвае `Content-Type: text/event-stream` (аўтаматычна вызначаецца ў апрацоўшчыку загалоўкаў SAPI) або выклікае `oxphp_stream_flush()`, адказ пераключаецца ў рэжым стрымінгу:

1. **Дастаўка загалоўкаў**: SAPI спажывае oneshot `EARLY_TX` для адпраўкі `ScriptResponse` з `stream_rx: Some(receiver)` — загалоўкі дастаўляюцца на бок Tokio неадкладна.
2. **Чанкі цела**: Кожны `flush()` або `oxphp_stream_flush()` ачышчае буфер вываду PHP і адпраўляе яго як `Bytes`-чанк праз канал `tokio::sync::mpsc` (абмежаваны, ёмістасць 64).
3. **StreamBody**: Слой злучэння абгортвае прыёмнік канала ў `StreamBody` для чанкавай HTTP-дастаўкі замест выкарыстання `full_body()`.
4. **Завяршэнне стрыму**: Калі PHP-скрыпт завяршаецца, воркер знішчае адпраўшчык `STREAM_TX`, зачыняючы канал. `StreamBody` вяртае `None`, завяршаючы HTTP-адказ.

Зваротны ціск (backpressure) ужываецца натуральна — калі кліент чытае павольна, абмежаваны канал запаўняецца і `blocking_send()` блакуе паток PHP-воркера да з'яўлення вольнага месца.

Стрымінгавыя адказы прапускаюць сціск (Brotli), бо `text/event-stream` не з'яўляецца тыпам кантэнту, які паддаецца сціску.

### 10. Падзея ResponseBuilding

Пасля пабудовы адказу (ці то ад аддачы статычных файлаў, ці то ад выканання PHP) выклікаецца падзея `ResponseBuilding`:

| Прыярытэт | Апрацоўшчык | Дзеянне |
|---|---|---|
| 60 | `ErrorPagesHandler` | Замяняе цела адказу на карыстальніцкую HTML-старонку для статусу >= 400 |
| 100 | `ServerHeaderHandler` | Дадае загалоўкі `Server: OxPHP` і `X-Request-ID` |

Гэта адзінае месца, дзе `request_id` кланіруецца (адзін раз), таму што ён патрэбны зноў у падзеі `RequestComplete`.

### 11. Brotli-кампрэсія

Калі кліент адправіў `Accept-Encoding: br` і кампрэсія ўключана, `compression::maybe_compress()` запускаецца пасля падзеі ResponseBuilding:

- Правярае, ці з'яўляецца тып кантэнту сціскальным (text/html, application/json і інш.)
- Прапускае адказы, якія ўжо маюць `Content-Encoding`
- Прапускае целы менш за 256 байтаў або больш за 3 МБ
- Сціскае з якасцю Brotli 4, памер акна 20
- Выкарыстоўвае сціснутую версію толькі калі яна сапраўды меншая
- Абнаўляе `Content-Encoding`, `Content-Length` і дадае `Vary: Accept-Encoding`

### 12. Падзея RequestComplete

Фінальная падзея нясе поўныя метададзеныя запыту:

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

| Прыярытэт | Апрацоўшчык | Дзеянне |
|---|---|---|
| 0 | `MetricsResponseHandler` | Выклікае `metrics.record_response(status, duration)` |
| 100 | `AccessLogHandler` | Выводзіць структураваны JSON-запіс логу праз `tracing::info!` |

### 13. Дастаўка адказу

`Ok(response)` вяртаецца ў hyper-util, які серыялізуе яго ў сетку. Для злучэнняў з keep-alive замыканне `service_fn` выклікаецца зноў для наступнага запыту на тым жа злучэнні.

## Апрацоўка памылак

Памылкі на кожным этапе выдаюць адпаведныя HTTP-коды стану:

| Памылка | Статус | Крыніца |
|---|---|---|
| Абмежаванне хуткасці | 429 | `RateLimitHandler` праз ранні адказ |
| Цела занадта вялікае | 413 | Збор цела `Limited` |
| Таймаўт запыту | 504 | `tokio::time::timeout` |
| Памылка воркера PHP | 500 | Разарваны канал oneshot |
| Чарга запоўнена | 503 | `SapiExecutor::execute()` праз `try_send` |
| Файл не знойдзены | 404 | Разрашэнне маршруту |
| Унутраная памылка | 500 | Агульная апрацоўка ў `handle_request` |

## Бюджэт алакацый

Канвеер спраектаваны для мінімізацыі алакацый на запыт:

- **0 кланаванняў** `request_id` праз большую частку канвеера (`std::mem::take`)
- **1 кланаванне** `request_id` на падзеі `ResponseBuilding` (патрэбна для паўторнага выкарыстання ў `RequestComplete`)
- **0 кланаванняў** `method` (`http::Method`) і `path_str` (перамяшчаюцца праз канвеер)
- Радкі метаду і шляху **адкладзены** да пасля праверкі ранняга адказу — запыты, абмежаваныя па хуткасці, цалкам пазбягаюць алакацыі
- `Accept-Encoding` правяраецца неалакацыйным выклікам `is_some_and`
- `RouteConfig` выкарыстоўвае загадзя вылічаныя шляхі індэкса для кораня `/`, каб пазбегнуць `PathBuf::join` на кожным запыце

## Гл. таксама

- [Агляд архітэктуры](./overview.md) — Карта кампанентаў і высокаўзроўневы паток дадзеных
- [Сістэма падзей](./event-system.md) — Тыпы падзей, прыярытэты і рэгістрацыя апрацоўшчыкаў
- [Пул воркераў](./worker-pool.md) — Як воркеры PHP апрацоўваюць `ScriptRequest`
- [SAPI і мост](./sapi-bridge.md) — Унутраны паток выканання воркера PHP
- [Маршрутызацыя](../features/routing.md) — Тры рэжымы маршрутызацыі і санітызацыя шляхоў
- [Кампрэсія](../features/compression.md) — Канфігурацыя Brotli-кампрэсіі
- [Таймаўты](../features/timeouts.md) — Паводзіны таймаўтаў запытаў і загалоўкаў
- [Абмежаванне хуткасці](../features/rate-limiting.md) — Абмежаванне хуткасці па IP і адказы 429
