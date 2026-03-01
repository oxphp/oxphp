---
title: SAPI і мост
description: Карыстальніцкі PHP SAPI OxPHP, бібліятэка C-моста з __thread TLS і API PHP-пашырэння
---

OxPHP выкарыстоўвае карыстальніцкі SAPI (Server API) для інтэграцыі з PHP, а не стандартны `php-embed` SAPI. Агульная бібліятэка C-моста забяспечвае механізм абмену станам запыту паміж бінарнікам Rust і PHP-пашырэннем. Гэтая старонка тлумачыць, чаму гэтая архітэктура існуе і як кампаненты ўзаемадзейнічаюць.

## Чаму карыстальніцкі SAPI?

Узровень SAPI ў PHP — гэта інтэрфейс паміж вэб-серверам і рухавіком PHP. Стандартныя SAPI (cli, fpm, embed) робяць здагадкі пра жыццёвы цыкл працэсу, якія не адпавядаюць мадэлі OxPHP:

- **php-embed** чакае адзін запыт на працэс. Ён не падтрымлівае адначасовую апрацоўку запытаў на некалькіх патоках.
- **php-fpm** — гэта асобны менеджар працэсаў. OxPHP ліквідуе патрэбу ў міжпрацэсавай камунікацыі.
- **php-cli** не мае інтэграцыі з HTTP.

OxPHP рэгіструе ўласны `sapi_module_struct` з назвай `"oxphp"`. Гэта дае поўны кантроль над:

- Захопам вываду (перахоп буфера вываду PHP)
- Апрацоўкай загалоўкаў (збор выклікаў `header()`)
- `php://input` (прадастаўленне цела запыту)
- Запаўненнем `$_SERVER` (усталёўка суперглабальных з дадзеных запыту на баку Rust)
- Замерам часу запыту (праз `sapi_get_request_time`)

## Праблема моста

Калі бінарнік Rust OxPHP кампілюецца, ён звязваецца з `libphp.so`. PHP-пашырэнні загружаюцца `libphp.so` у час выканання праз `dlopen()`. Гэта стварае праблему бачнасці:

```
┌────────────────────┐         ┌───────────────────┐
│  Rust Binary       │         │  libphp.so        │
│                    │ links   │                   │
│  thread_local! {   │────────▶│  dlopen() ───────▶│ oxphp_sapi.so
│    // Rust TLS     │         │                   │  (PHP extension)
│  }                 │         └───────────────────┘
└────────────────────┘                             │
                                                   │
  Rust thread_local! vars are INVISIBLE            │
  to dlopen'd shared libraries ──────────────────▶ │
```

Макрас `thread_local!` у Rust выкарыстоўвае ELF TLS або платформа-спецыфічны механізм, які разрашаецца ў час звязвання. Агульныя бібліятэкі, загружаныя праз `dlopen()` у час выканання, не могуць бачыць гэтыя сімвалы. Гэта азначае, што PHP-пашырэнне не можа напрамую чытаць дадзеныя запыту, якія Rust захоўвае ў патокалакальным сховішчы.

## Бібліятэка моста

Рашэнне — `liboxphp_bridge.so` — невялікая агульная бібліятэка C, з якой звязваюцца і бінарнік Rust, і PHP-пашырэнне. Яна выкарыстоўвае C `__thread` TLS, які бачны ўсім бібліятэкам, загружаным праз `dlopen`, у адной адраснай прасторы.

```
┌────────────────────┐
│  Rust Binary       │──links──┐
└────────────────────┘         │
                               ▼
                    ┌──────────────────────┐
                    │  liboxphp_bridge.so  │
                    │                      │
                    │  static __thread     │
                    │    oxphp_ctx_t ctx;  │
                    │                      │
                    │  static (global)     │
                    │    plugin_functions  │
                    │    native_dispatch   │
                    └──────────────────────┘
                               ▲
┌────────────────────┐         │
│  oxphp_sapi.so     │──links──┘
│  (PHP extension)   │
└────────────────────┘
```

І бінарнік Rust, і PHP-пашырэнне выклікаюць функцыі ў `liboxphp_bridge.so` для чытання і запісу адной і той жа пераменнай `__thread`. Паколькі яны знаходзяцца ў адным працэсе і на адным патоку АС, яны дзеляць адзін і той жа TLS-слот.

### Кантэкст моста

Кантэкст запыту вызначаны ў `ext/bridge/oxphp_bridge.h`:

```c
typedef struct {
    char request_id[65];    // Hex request ID (64 chars + null)
    int32_t worker_id;      // Worker thread index
    double request_time;    // Unix epoch, microseconds
    bool stream_mode;       // Streaming mode active
    bool headers_sent;      // Headers sent (streaming)
    bool finished;          // oxphp_finish_request() called
} oxphp_ctx_t;
```

### API моста

Мост выстаўляе функцыі геттераў/сеттераў, якія працуюць з патокалакальнай пераменнай `__thread` `ctx`:

| Функцыя | Прызначэнне |
|---|---|
| `oxphp_bridge_init_ctx()` | Нулявая ініцыялізацыя кантэксту (выклікаць перад `php_request_startup`) |
| `oxphp_bridge_clear_ctx()` | Ачыстка кантэксту пасля завяршэння запыту |
| `oxphp_bridge_get_ctx()` | Атрымаць паказальнік на структуру кантэксту |
| `oxphp_bridge_set_request_id(id)` | Скапіраваць ідэнтыфікатар запыту (да 64 сімвалаў) |
| `oxphp_bridge_get_request_id()` | Атрымаць паказальнік на ідэнтыфікатар запыту |
| `oxphp_bridge_set_worker_id(id)` | Усталяваць індэкс патоку воркера |
| `oxphp_bridge_set_request_time(time)` | Усталяваць час пачатку запыту |
| `oxphp_bridge_get_request_time()` | Атрымаць час пачатку запыту |
| `oxphp_bridge_set_stream_mode(mode)` | Уключыць/выключыць рэжым стрымінгу |
| `oxphp_bridge_is_streaming()` | Праверыць, ці актыўны стрымінг |
| `oxphp_bridge_set_finished(bool)` | Пазначыць запыт як завершаны |
| `oxphp_bridge_is_finished()` | Праверыць, ці завершаны запыт |
| `oxphp_bridge_set_headers_sent(bool)` | Пазначыць загалоўкі як адпраўленыя |
| `oxphp_bridge_get_headers_sent()` | Праверыць, ці былі адпраўлены загалоўкі |

Рэалізацыя ў `ext/bridge/oxphp_bridge.c` простая — кожная функцыя чытае або запісвае поле пераменнай `static __thread oxphp_ctx_t ctx`.

### Крытычны інварыянт

**`init_ctx()` і `set_request_time()` павінны быць выкліканы ДА `php_request_startup()`.**

Апрацоўшчык RINIT OPcache чытае `sapi_get_request_time()` падчас `php_request_startup()`. Зваротны выклік `sapi_get_request_time` карыстальніцкага SAPI чытае з кантэксту моста. Калі мост вяртае 0 (неініцыялізаваны), праверка `file_update_protection` OPcache не праходзіць, што прыводзіць да 0% пападанняў у кэш.

Правільны парадак выклікаў на кожным патоку воркера:

```
1. oxphp_bridge_init_ctx()
2. oxphp_bridge_set_request_id(...)
3. oxphp_bridge_set_request_time(...)
4. sapi::set_request_data(request)    // server vars, cookies, body
5. php_request_startup()              // triggers RINIT for all extensions
6. php_execute_script(...)
7. php_request_shutdown()
8. oxphp_bridge_clear_ctx()
```

## Рэестр функцый плагінаў

Мост таксама прадастаўляе **глабальны** (не `__thread`) рэестр функцый плагінаў. Гэта дазваляе плагінам Rust рэгістраваць функцыі, якія PHP-скрыпты могуць выклікаць, і PHP-функцыі, якія Rust можа выклікаць.

### API рэестра

| Функцыя | Прызначэнне |
|---|---|
| `oxphp_bridge_register_plugin_fn(name, required, total)` | Зарэгістраваць функцыю плагіна (выклікаецца Rust падчас запуску) |
| `oxphp_bridge_get_plugin_fn_count()` | Атрымаць колькасць зарэгістраваных функцый плагінаў |
| `oxphp_bridge_get_plugin_fn_name(index)` | Атрымаць назву функцыі плагіна па індэксе |
| `oxphp_bridge_get_plugin_fn_required(index)` | Атрымаць колькасць абавязковых параметраў па індэксе |
| `oxphp_bridge_get_plugin_fn_total(index)` | Атрымаць агульную колькасць параметраў па індэксе |
| `oxphp_bridge_set_native_dispatch(fn)` | Усталяваць зваротны выклік натыўнай дыспетчарызацыі Rust |
| `oxphp_bridge_get_native_dispatch()` | Атрымаць зваротны выклік натыўнай дыспетчарызацыі Rust |

Рэестр глабальны (не для кожнага патоку), таму што ён запісваецца аднойчы з галоўнага патоку падчас запуску і чытаецца падчас MINIT — без канкурэнтнага доступу. Ён ніколі не вызваляецца; ён жыве на працягу ўсяго часу жыцця працэсу.

### Натыўны мост: выклікі паміж межамі без серыялізацыі

Rust і PHP камунікуюць праз прамы доступ да паказальнікаў `zval` — без JSON-серыялізацыі. C-функцыі доступу ў `liboxphp_bridge.so` забяспечваюць бяспечны, тыпізаваны інтэрфейс для чытання і запісу PHP-значэнняў:

**Чытанне аргументаў (PHP → Rust):**

| Функцыя | Прызначэнне |
|---|---|
| `oxphp_val_type(zval*)` | Атрымаць тып zval (IS_LONG, IS_DOUBLE, IS_STRING і інш.) |
| `oxphp_arg_long(zval*)` | Прачытаць цэлалікавы аргумент |
| `oxphp_arg_double(zval*)` | Прачытаць аргумент з плаваючай кропкай |
| `oxphp_arg_str(zval*, len*)` | Прачытаць радковы аргумент (паказальнік + даўжыня) |
| `oxphp_arg_bool(zval*)` | Прачытаць булевы аргумент |

**Запіс вяртаемых значэнняў (Rust → PHP):**

| Функцыя | Прызначэнне |
|---|---|
| `oxphp_ret_long(zval*, val)` | Запісаць цэлалікавае вяртаемае значэнне |
| `oxphp_ret_double(zval*, val)` | Запісаць вяртаемае значэнне з плаваючай кропкай |
| `oxphp_ret_str(zval*, str, len)` | Запісаць радковае вяртаемае значэнне |
| `oxphp_ret_bool(zval*, val)` | Запісаць булевае вяртаемае значэнне |
| `oxphp_ret_null(zval*)` | Запісаць null-вяртаемае значэнне |

**Паток натыўнай дыспетчарызацыі:**

`oxphp_bridge_set_native_dispatch(fn)` рэгіструе зваротны выклік Rust. Калі PHP-скрыпт выклікае функцыю плагіна, `ZEND_FUNCTION(oxphp_native_dispatch)` у пашырэнні выклікае гэты зваротны выклік, перадаючы сырыя паказальнікі `zval*` на аргументы і вяртаемае значэнне напрамую — без серыялізацыі.

**Выклік PHP з Rust:**

`oxphp_call_php_native(func_name, args, argc, result)` дазваляе Rust выклікаць PHP-функцыі прыкладнога ўзроўню. C-бок разрашае функцыю праз `zend_hash_str_find_ptr` і выклікае `zend_call_known_function` напрамую. Вынік-zval належыць Rust і вызваляецца праз `zval_ptr_dtor` пры выдаленні.

## PHP-пашырэнне

PHP-пашырэнне (`ext/oxphp_sapi.c`) выстаўляе сервер-спецыфічныя функцыі для PHP-скрыптоў. Яно звязваецца з `liboxphp_bridge.so` для чытання кантэксту моста.

### Даступныя функцыі

| Функцыя | Тып вяртання | Апісанне |
|---|---|---|
| `oxphp_request_id()` | `string` | Вяртае hex-ідэнтыфікатар запыту для бягучага запыту |
| `oxphp_worker_id()` | `int` | Вяртае індэкс патоку воркера (з 0) |
| `oxphp_server_info()` | `array` | Вяртае `sapi`, `version`, `worker_id`, `request_time`, `worker_mode` |
| `oxphp_request_heartbeat(int $time = 10)` | `bool` | Запас для падаўжэння таймаўту (зараз вяртае `true`) |
| `oxphp_finish_request()` | `bool` | Пазначае запыт як завершаны для фонавай апрацоўкі |
| `oxphp_is_worker()` | `bool` | Правярае, ці працуе сервер у рэжыме воркера |
| `oxphp_is_streaming()` | `bool` | Правярае, ці выкарыстоўвае бягучы запыт рэжым стрымінгу |

### Натыўная дыспетчарызацыя плагінаў

Пашырэнне рэгіструе `oxphp_native_dispatch` — апрацоўшчык з нулявой серыялізацыяй для ўсіх зарэгістраваных функцый плагінаў. Калі PHP-скрыпт выклікае функцыю плагіна (напр., `oxphp_example_info()`), рухавік Zend дыспетчарызуе да `oxphp_native_dispatch`, які:

1. Чытае назву функцыі з `execute_data->func->common.function_name`
2. Перадае ўказальнікі `zval*` на аргументы і вяртаемае значэнне напрамую ў Rust праз callback моста
3. Rust чытае/піша zval'ы праз C-функцыі доступу (`oxphp_arg_long`, `oxphp_ret_str` і інш.) — без серыялізацыі
4. Пры памылцы выдае PHP-папярэджанне `E_WARNING` і вяртае `NULL`

### Выклік PHP з Rust

Мост прадастаўляе `oxphp_call_php_native()` — функцыю, якую Rust можа выклікаць для выкліку PHP-функцый:

1. Rust выклікае `oxphp_call_php_native(func_name, args, argc, result)` з падрыхтаванымі zval-аргументамі
2. C-бок разрашае функцыю праз `zend_hash_str_find_ptr` і выклікае `zend_call_known_function` напрамую
3. Вынік-zval належыць Rust і вызваляецца праз `zval_ptr_dtor` пры выдаленні

### Прыклад выкарыстання

```php
<?php
// Get the request ID assigned by the server
$requestId = oxphp_request_id();
header("X-Debug-Worker: " . oxphp_worker_id());

// Examine SAPI details
$info = oxphp_server_info();
// $info = [
//     'sapi' => 'oxphp',
//     'version' => '0.1.0',
//     'worker_id' => 3,
//     'request_time' => 1707609600.123456,
// ]

// Finish the response but continue processing
oxphp_finish_request();
// ... background work here (logging, cleanup, etc.)
```

### Рэгістрацыя пашырэння

Пашырэнне рэгіструецца як стандартны модуль PHP з хукам MINIT, які наладжвае мост функцый плагінаў:

```c
zend_module_entry oxphp_sapi_module_entry = {
    STANDARD_MODULE_HEADER,
    "oxphp_sapi",
    oxphp_sapi_functions,
    PHP_MINIT(oxphp_sapi),  // sets call_php callback, registers plugin fns
    NULL,                    // MSHUTDOWN
    NULL,                    // RINIT
    NULL,                    // RSHUTDOWN
    PHP_MINFO(oxphp_sapi),
    "0.1.0",
    STANDARD_MODULE_PROPERTIES
};
```

**MINIT** выконвае дзве задачы:

1. Усталёўвае `oxphp_bridge_set_native_dispatch(oxphp_native_dispatch)`, каб мост ведаў, якую функцыю выклікаць, калі функцыі плагінаў выклікаюцца з PHP
2. Чытае рэестр функцый плагінаў з моста і рэгіструе кожную функцыю ў Zend праз `zend_register_functions()` — гэта павінна адбыцца пры запуску модуля (а не пры запуску запыту), каб аптымізацыя `function_exists()` часу кампіляцыі OPcache магла бачыць функцыі

## Зводка патоку дадзеных

```
Rust (Tokio task)                     PHP Worker Thread
─────────────────                     ──────────────────
ScriptRequest ──crossbeam_channel::bounded──▶ recv()
                                      │
                                      ├── bridge::init_ctx()
                                      ├── bridge::set_request_id()
                                      ├── bridge::set_request_time()
                                      ├── sapi::set_request_data()
                                      │     ├── server vars → TLS
                                      │     ├── cookies → TLS
                                      │     └── body → TLS
                                      │
                                      ├── php_request_startup()
                                      │     ├── RINIT for all extensions
                                      │     └── OPcache reads request_time
                                      │
                                      ├── php_execute_script()
                                      │     ├── PHP reads $_SERVER, $_GET, etc.
                                      │     ├── PHP calls oxphp_request_id()
                                      │     │     └── bridge::get_request_id()
                                      │     ├── PHP calls plugin function
                                      │     │     └── bridge::dispatch() → Rust
                                      │     └── Output captured by SAPI
                                      │
                                      ├── php_request_shutdown()
                                      │
                                      ├── sapi::take_response()
                                      │     ├── output buffer
                                      │     ├── response headers
                                      │     └── status code
                                      │
                                      └── bridge::clear_ctx()
                                      │
ScriptResponse ◀──oneshot──────────── tx.send()
```

## Зборка моста і пашырэння

Бібліятэка моста і PHP-пашырэнне збіраюцца як частка Docker-вобраза. Для лакальнай распрацоўкі:

```bash
# Build the bridge library
cd ext/bridge
make
sudo make install  # installs liboxphp_bridge.so

# Build the PHP extension
cd ext
phpize
./configure --enable-oxphp-sapi
make
sudo make install  # installs oxphp_sapi.so
```

Абодва артэфакты павінны быць даступны ў час выканання:
- `liboxphp_bridge.so` у шляху пошуку бібліятэк (`LD_LIBRARY_PATH=/usr/local/lib`)
- `oxphp_sapi.so` у каталогу пашырэнняў PHP (або загружаны праз `extension=oxphp_sapi.so` у `php.ini`)

## Гл. таксама

- [Агляд архітэктуры](./overview.md) — Карта кампанентаў і паслядоўнасць запуску
- [Пул воркераў](./worker-pool.md) — Жыццёвы цыкл патоку воркера, які выклікае мост
- [Жыццёвы цыкл запыту](./request-lifecycle.md) — Поўны канвеер запыту ад TCP да адказу
- [Функцыі PHP](../php/functions.md) — Даведнік па функцыях, выклікальных з PHP
- [Суперглабальныя](../php/superglobals.md) — Як запаўняюцца `$_SERVER`, `$_GET` і інш.
- [OPcache](../php/opcache.md) — Інтэграцыя OPcache і інварыянт `request_time`
