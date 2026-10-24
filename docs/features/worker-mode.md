---
title: Worker Mode
description: Persistent PHP workers that bootstrap once and handle multiple requests, eliminating per-request startup overhead in OxPHP.
---

# Worker Mode

Worker mode runs persistent PHP workers that bootstrap once and handle multiple requests, eliminating per-request startup overhead. Instead of tearing down and rebuilding PHP state on every request, your application loads its autoloader, configuration, and database connections a single time and reuses them across the lifetime of the worker.

## How It Works

1. **Set `WORKER_MODE_ENABLED=true`** and point **`ENTRY_FILE`** at your bootstrap script. This enables worker mode for all PHP workers in the pool.
2. **PHP starts and runs the outer scope once** — autoloader registration, configuration loading, database connections, and any other initialization code execute a single time.
3. **Call `oxphp_worker(callback)`** to enter the request loop. OxPHP begins dispatching incoming HTTP requests to your callback.
4. **Between requests**, superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`, `$_REQUEST`, `php://input`), output buffers, response headers, the session a request opened, the ini directives a request changed, and the environment variables a request set with `putenv()` are reset automatically — see [What Gets Reset Between Requests](#what-gets-reset-between-requests) for what that covers and where it stops. A soft reset cleans per-request state while preserving bootstrapped resources in the outer scope. `$_ENV` is a deliberate exception: with PHP's default `auto_globals_jit=1` it is **not** reset — see [Superglobals](../php/superglobals.md#_env).
5. **Outer scope persists** — variables defined before `oxphp_worker()`, static properties, database connections, and autoloaders remain available across all requests handled by that worker.

> **Note:** Worker mode changes routing behavior. All requests that do not match a static file on disk are dispatched to the worker instead of returning 404. See [Routing](routing.md) for details.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `WORKER_MODE_ENABLED` | `false` | Enable persistent worker mode. Accepts `true`, `1`, `yes`. Requires `ENTRY_FILE` to point at a `.php` script |
| `ENTRY_FILE` | *(unset)* | Path to the worker bootstrap script. Resolved against `DOCUMENT_ROOT` when relative; `..` segments and absolute paths are allowed (worker bootstraps living outside the public document root are a supported layout) |
| `WORKER_MAX_MEMORY_MIB` | `0` | Maximum PHP memory per worker in MiB before recycling. `0` = unlimited |

For application-driven recycling, call [`OxPHP\Server\Worker::scheduleExit()`](../php/worker-class.md#scheduleexit) from inside a request handler. The worker exits cleanly after the current request completes.

## Writing a Worker Script

A worker script has two parts: the outer scope that runs once at startup, and the callback passed to `oxphp_worker()` that runs on every request.

```php
<?php
// Outer scope: runs once at startup
require __DIR__ . '/../vendor/autoload.php';

$config = parse_ini_file(__DIR__ . '/../config/app.ini');
$db = new PDO($config['dsn'], $config['user'], $config['pass'], [
    PDO::ATTR_PERSISTENT => true,
]);

$app = new MyApp\Application($config, $db);

// Request loop: runs for every request
oxphp_worker(function () use ($app) {
    $app->handle();
});

// Teardown: runs when this worker's loop ends, which is not
// only at server shutdown — see Recycling below
$app->terminate();
```

## What Gets Reset Between Requests

OxPHP performs a soft reset between requests. The following state is cleaned automatically:

- **Superglobals** — `$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`, `$_REQUEST`, and `php://input` are repopulated with the new request data
- **Output buffers** — all output buffers are flushed and cleaned
- **Response headers** — HTTP status code and headers are reset to defaults
- **Error state** — last error information (message, file, line, type) and connection status are cleared. User-registered error handlers (`set_error_handler()`) and exception handlers (`set_exception_handler()`) persist across requests
- **Session state** — the session a request opened is closed as that request ends, after its shutdown functions and before its ini changes are put back — the order PHP's own request shutdown uses, so the write runs under that request's settings: the data is handed to the save handler, the session id is released, and `$_SESSION` leaves the symbol table. The next request therefore starts a session under its own cookie, reads `''` from `session_id()` until it does, and finds `$_SESSION` undefined until then. Two boundaries here as well:
    - **A session travels with its request, and a worker has one open at a time.** A worker takes new requests, and resumes the ones it has paused, whenever the request it is running suspends — in `oxphp_async_await()`, `oxphp_sleep()`, `oxphp_usleep()`, or a `sleep()`, `usleep()`, `stream_select()` or socket wait taken over by `RUNTIME_HOOKS` (for a socket, a read; with the `net` category also a connect — `new PDO('mysql:…')` and `new mysqli(...)` included — a TLS handshake, or a write that has to wait for room), or a streaming response's flush that waits for its client to make room (see [Server-Sent Events](sse.md#how-it-works)). PHP's session module, though, keeps one session per thread, and `session_start()` on a thread where a session is open answers `true` without looking at the caller's cookie. So a request's session is taken off the worker when the request suspends and put back when it resumes, as its superglobals are: its id and `$_SESSION`, open or closed, and while it is open the save handler and serializer it was opened with, so a request run in the meantime that registers a handler of its own or changes `session.serialize_handler` does not change how the paused session is written. With the id carried in URLs (`session.use_trans_sid` on, `session.use_only_cookies` off — both deprecated since PHP 8.4), the id PHP's URL rewriter appends to the request's links and forms, and the `SID` constant, travel with it too, as do variables added with `output_add_rewrite_var()`, and all of them are given back when the request ends. A request run in that window finds no session — `session_id()` reads `''`, `$_SESSION` is undefined — and its own `session_start()` reads its own cookie. It does not open a session while another is open on the worker, though: the save handler can hold a lock for the open one — the default `files` handler locks the session file with a blocking call — and a second open on the same thread that waited for that lock would hold the very thread the first needs to let it go, while nothing says in advance which session a `session_start()` will open. So from `session_start()` until `session_write_close()`, `session_abort()`, `session_destroy()` or the end of the request — and after a `session_regenerate_id()` that fails part way, which can leave the save handler open without a session, at the latest until the end of the request — while that request is paused the worker takes no new request — those wait in the queue for another worker, or for this one, as they do when every worker is busy — and `session_start()` in any other request it is running waits until the session is closed, and until every request that called it earlier has had its turn, before it opens one. Everything else on the worker goes on running meanwhile. A request waiting in `session_start()` is paused the way a sleep is, a quarter of a second at a time, and a drain or a cancellation ends it there as it ends a sleep; so does its time limit, within that quarter of a second, without opening the session. The window includes a wait inside `session_start()` itself, such as a save handler reading its store over a connection `RUNTIME_HOOKS` suspends. Three consequences follow. Sessions on one worker are open one after another, even for different clients, so a request that keeps its session open across a suspension point holds up the sessions of every other request on its worker, and every request the worker has not taken yet, for that long. A request that, with its session open, waits for something only a request the worker has not taken yet, or one waiting in `session_start()`, could provide does not get it until the session is closed: with `PHP_WORKERS=1`, an HTTP call back to the server is not served until then, and fails once the caller's timeout or the server's queue wait runs out, and a wait with no timeout of its own does not end. And `session_start()` cannot wait inside a fiber a userland scheduler such as Revolt or AMPHP started, or where switching is blocked (see `RUNTIME_HOOKS` in [Configuration](../operations/configuration.md)), because what it would wait for runs on the same thread: there, while another request on the worker has a session open, it raises a warning and returns `false`. The save handler also keeps whatever it locked until the session is closed. In the default `files` handler that is an exclusive lock on the session file, taken with a blocking call — so a request for the same session id on another worker, or in another process, waits inside the save handler, and because that wait is a blocking system call on that worker's thread, it stalls every request that worker is carrying. Call `session_write_close()` as soon as the session data is final, and before any suspension point: both the write and the unlock then happen where your application put them, and the worker's other sessions and its queue are no longer held up.
    - **A closed session stays with its request.** After `session_write_close()`, the request that closed the session can still read its id from `session_id()` and its data from `$_SESSION`, as under PHP-FPM, and both go on travelling with it as above, so it resumes with its own id and data while a request the worker took in between starts its session from its own cookie.
- **ini directives changed by the request** — anything a request altered with `ini_set()`, `ignore_user_abort()` or `error_reporting()` belongs to that request, as it would under PHP-FPM: it is put back when the request ends. Most directives are also the request's own while it is paused — on an await, a sleep or a socket wait: they are taken off the worker, so the requests the worker runs in the meantime see the baseline rather than the paused request's values, and the paused request resumes with its own. `ini_set('default_socket_timeout', 5)` around one HTTP call and `ini_set('display_errors', '1')` in a debugging branch apply to the request that made them and not to the next one. What your **bootstrap** set is the baseline they are restored to, not the value in `php.ini` — an `ini_set()` in the outer scope is application configuration and survives every request, and every request starts with it. That includes `ignore_user_abort(true)`: called in the bootstrap, it applies to every request the worker serves, streams included (see [Server-Sent Events](sse.md)). See [What Persists](#what-persists). Seven boundaries are worth knowing:
    - **`memory_limit` and the `session.*` settings stay with the worker while it has other requests in flight.** The heap belongs to the worker thread, not to a request, and so does the session module the `session.*` settings configure (see **Session state** above) — including the save handler `session_set_save_handler()` installs. So these are not taken off the worker when a request pauses, and they are put back only when the worker takes its next request with nothing else running on it. While a worker has work in flight — the case whenever a request is paused, and also while a fire-and-forget promise is still being reclaimed — a value one request set here applies to the requests the worker takes in that window. A session already open is the exception: it keeps the save handler and serializer it was opened with until it is closed, whatever another request installs meanwhile. A request waiting in `session_start()` for another request's session to close is not: it opens its session with the save handler and `session.*` settings in force when its turn comes, which a request run while it waited may have changed. Configure sessions the same way in every request, or once in the bootstrap.
    - **`set_time_limit()` sets the limit of the request that calls it, but `ini_get('max_execution_time')` reports the worker's.** Each request has a time limit of its own (see [Request Deadlines](#request-deadlines)), and `set_time_limit()` — or `ini_set()` or `ini_restore()` on `max_execution_time` — changes that one and no other. The directive itself stays with the worker, as `memory_limit` does: `ini_get('max_execution_time')` answers with the last value any request set since the worker last had nothing in flight — or the bootstrap's, if none did — which on a worker carrying several requests need not be the caller's own limit, and it is put back when the worker takes its next request with nothing else running on it.
    - **A directive whose handler does more than store its value stays on the worker while its request is paused.** Moving a directive runs its handler, and some handlers act as they store: `default_charset` and the encoding settings reset mbstring's encodings, `zlib.output_compression` starts an output handler, `open_basedir` resolves a relative path against the current directory, and the deprecated `assert.*` settings raise a deprecation each time. Run on every pause and resume, they would undo what the request did in between, so these directives are not taken off the worker: the requests it runs while one is paused see the paused request's value, and are put back when the last request in flight ends. The directives that travel are the ones stored by one of PHP's standard setters — `ignore_user_abort`, `precision`, `display_errors`, `error_reporting`, `default_socket_timeout`, `user_agent` and most others. A few with a handler of their own stay on the worker too although they only validate the value: `include_path` while OPcache is enabled (OPcache installs its own handler for it, so `set_include_path()` included; with OPcache off it travels), `error_log`, `date.timezone` and `default_mimetype`. What a request sets through a function rather than an ini directive — `mb_internal_encoding()`, `date_default_timezone_set()`, `setlocale()` — is not an ini change at all: it is the worker's, and outlives the request unless the request puts it back — except the environment that `putenv()` writes, which the worker puts back itself (see below).
    - **A directive whose handler refuses to be put back stays as the request left it.** Putting a directive back runs its handler, and a handler can refuse. Such a directive keeps the request's value until the worker next takes a request with nothing else running on it, and a paused request whose change is refused when it resumes continues without that change.
    - **A request ended while one of its directives is being moved recycles the worker.** A handler can run application code: putting `assert.callback` back drops the callback, and with it whatever the callback held, whose destructors run there. Anything that ends the request from inside one of them — a fatal error, or output written by a stream whose client has already left — leaves the handler part-way through: the callback still set, and pointing at a closure already half torn down. Nothing short of a fresh worker puts that right, so the worker exits under `oxphp_worker_recycles_by_reason_total{reason="scheduled"}`, and the requests still in flight on it are ended as they are on any other recycle. The request itself is filed as its ending says: a fatal is answered `500` and counts toward the error breaker, a client that left is a cancellation and does not. An exception such a destructor throws is reported as the request's uncaught exception instead, and costs nothing more.
    - **`memory_limit` is restored as a value before it is restored as a limit.** PHP refuses to lower the allocator's ceiling while more than the new limit is still held, so a worker left holding what the request allocated reports the restored `memory_limit` from `ini_get()` while the allocator is still enforcing the raised one. The ceiling follows as soon as the worker's own footprint leaves room for it.
    - **`opcache.enable` is not restored at all.** Turning OPcache off is the only thing a request can do to it — PHP refuses to switch it back on mid-request — and what raises it again is OPcache's own per-request startup, which a worker runs once, when it boots. So a worker whose request turned OPcache off compiles every file from source for the rest of its life, and the directive is left reading `0` to say so. Restoring it would make `ini_get('opcache.enable')` and `opcache_get_status()` report an enabled cache that is not running, which is the worse of the two: an application that asks in order to decide something would be told the opposite of what is happening.
- **Environment variables set with `putenv()`** — `putenv()` writes the environment of the whole server process, the one `getenv()` reads and that a child process started by `exec()`, `shell_exec()`, `system()` or `proc_open()` without an `$env` array inherits. What a request sets there is put back when the request ends, as PHP's own request shutdown puts it back in traditional mode: a variable the request added is removed, and one it overwrote gets back the value it had before. What your **bootstrap** set before `oxphp_worker()` is the baseline: a request that overwrites such a variable ends it back to the bootstrap's value, so the next request starts with it again unless something else has set it since, as the boundaries below describe. Laravel's `Env`, which by default writes `.env` values with `putenv()` as well, keeps them for every request. Four boundaries:
    - **Until the request ends, the value is the process's.** There is one environment per process, and it is not taken off the worker when a request pauses, so other requests read what a request set while that request is still running: requests on other workers, and requests the same worker runs while the request that set it is paused at a [suspension point](fiber-multiplexing.md#suspension-points). Traditional mode has the same window for requests running at the same time on other workers, since every worker is a thread of one process — under PHP-FPM each is a process of its own. A value that belongs to one request, such as one client's credentials, belongs in a variable, or in the `$env` array of the `proc_open()` call that needs it, not in the environment.
    - **The last request to set a name owns it.** A request that sets a name another request on the same worker set and has not finished yet replaces that request's value, and the name is put back when the later request ends — to the value it had before either of them, so the earlier request does not get its own back.
    - **Every `putenv()` made while a request runs is that request's.** A bootstrap that runs lazily, inside the first request rather than before `oxphp_worker()`, has its `putenv()` calls undone when that request ends; call `putenv()` before `oxphp_worker()` for values every request is to see. A `putenv()` made outside every request — by a destructor or a callback the worker runs between requests — is put back when the next request on that worker ends.
    - **Not in `oxphp_async()` tasks.** A task runs on an async worker thread, not in the request that started it, and what it sets with `putenv()` stays for the life of that thread.

## What Persists

The following state survives across requests within the same worker:

- **Variables in outer scope** — anything defined before `oxphp_worker()` and captured via `use`
- **Static properties** — class static properties retain their values
- **Database connections** — PDO, MySQLi, and other persistent connections remain open
- **Autoloaders** — registered autoloaders (Composer, custom) remain active
- **Loaded classes and functions** — all previously loaded classes, interfaces, traits, and functions
- **ini directives set during bootstrap** — an `ini_set()` in the outer scope holds for the life of the worker and is what per-request changes are rolled back to
- **Environment variables set during bootstrap** — a `putenv()` in the outer scope is what a request's `putenv()` of the same name is put back to, for as long as the worker runs. The environment is the whole process's, though: with several workers, a name every worker's bootstrap sets goes back to what it was before the server started — or is removed — for every worker when the worker that set it first exits, until another worker's bootstrap sets it again (see [Superglobals](../php/superglobals.md#_env))
- **`$_ENV`** — deliberately not reset, with PHP's default `auto_globals_jit=1`. It describes the worker rather than the request, which is what keeps a `.env` loader's values alive past the boot that wrote them — and it keeps a write made from inside a request handler just as long, for every later request on that worker and for any request multiplexed alongside it. See [Superglobals](../php/superglobals.md#_env)

## Recycling

Workers are automatically recycled — the worker's loop ends and its PHP state is torn down; whether a fresh worker comes up in its place is decided by the pool, under the rules below — when any of the following conditions are met:

- **Max memory exceeded** — the worker's PHP memory usage exceeds `WORKER_MAX_MEMORY_MIB` MiB
- **Application requested exit** — the handler called [`Worker::scheduleExit()`](../php/worker-class.md#scheduleexit). Useful for app-controlled hot reload, file-mtime-based reload, or per-request bootstrap re-execution
- **Consecutive errors** — the worker took 3 consecutive requests that came apart: a fatal error, an out-of-memory, a stack overflow, or one of two failures around the fiber the request runs in. Three in a row say the worker rather than any one request is at fault, and a worker that keeps coming apart is replaced rather than patched up again — see below for what does and does not count
- **A request ended inside an internal function left a lot behind** — a fatal error or a cancellation (`max_execution_time` elapsing, say) that ends a request inside an internal function leaves behind what that function had allocated for itself, and the worker cannot give that back: `usort()` sorts a copy of the array, and the copy keeps every element alive. The worker compares its memory usage between two moments when no request is in flight; if a request was ended with an internal function's call on its stack in between and the usage grew by more than 4 MiB, the worker is recycled, with a warning in the log that names the reason. A request ended just as an internal function such as `curl_exec()` returned counts as well, because a time limit is noticed while that call is still on the stack. The stacks of fibers the worker created in that time are not counted, since it keeps them for as long as it lives. Garbage that only the cycle collector can free is collected first, but only what the collector already knew about: a cycle that only the cleanup after the bailout released is never handed to it, stays allocated, is counted, and recycles the worker. No request is in flight at that moment, so none is ended by it, and the recycle is counted under `oxphp_worker_recycles_by_reason_total{reason="scheduled"}`. It does not depend on `WORKER_MAX_MEMORY_MIB`. What decides is the growth, not the call, so anything else that grew the worker's memory in the same stretch counts too — a cache filled for the first time, a large request body not yet released. What it does not catch: a leak under about 4 MiB, which adds up until `WORKER_MAX_MEMORY_MIB` is passed or, without it, until requests start failing against PHP's `memory_limit`; a leak in the first stretch in which a worker is busy, which is not judged because what a fresh worker builds for itself in its first requests would otherwise recycle every replacement in turn while timeouts keep coming; anything while the server is shutting down; and a worker that is never without a request in flight, where the check waits for the first such moment.
- **An out-of-memory error left more than `memory_limit` in use** — PHP builds the `Allowed memory size … exhausted` report with the limit set aside, since the report needs memory of its own, and on PHP 8.5.0 to 8.5.11 that report includes a backtrace of every frame on the stack (`fatal_error_backtraces`, on by default; from 8.5.12 the backtrace of this error stops at its first frame). After a runaway recursion that backtrace can be larger than the limit itself: a recursion 400,000 calls deep under `memory_limit=128M` left about 290 MiB counted as in use. Up to PHP 8.4.26 / 8.5.11, while the allocator counts more than the limit as in use it refuses nothing, for any request on the worker. PHP-FPM gets that memory back by shutting the request down; a worker shuts no request down, so it hands the memory back and puts the limit back in force as the failing request ends — after its shutdown functions, so a request the worker serves while one of them is suspended (in `oxphp_async_await()`, `oxphp_sleep()`/`oxphp_usleep()`, or a `sleep()`, `usleep()` or socket wait taken over by `RUNTIME_HOOKS`) runs with no limit as well. Where what the worker still holds is more than the limit even then — a shutdown function of that request kept that much, say — the worker is recycled at its next moment with no request in flight, with a warning in the log that names the reason, under `oxphp_worker_recycles_by_reason_total{reason="scheduled"}`, and the requests it serves until then run without `memory_limit` in force — as a rule with no limit at all. From PHP 8.4.27 / 8.5.12 the allocation that takes a request past `memory_limit` is granted and the error is reported just after it, so a request can also leave more than the limit behind in what outlives it — the worker's shared state or a static variable it was filling, say — and the worker is recycled for that the same way; on those releases, while more than the limit is counted as in use, a request that needs memory the worker does not already hold fails with `Allowed memory size … exhausted` instead of running with no limit. Those releases also have a hard limit, `memory_hard_limit_percent` of `memory_limit` (by default twice the limit, or the limit plus 1 GiB for a limit above 1 GiB): an allocation that would take the worker's memory past it ends the whole server process with exit code 137 (see [Memory limits](../getting-started/docker.md#memory-limits) for the setting that moves it), and a worker that already holds more than `memory_limit` is that much closer to it. Unlike the check above, this recycle applies from a worker's first request, but not while the server is shutting down. A thread of the [async pool](async-promises.md) hands the memory back as the failing task ends and, where that is not enough, tries again as each later task ends; it is not recycled. `fatal_error_backtraces=0` leaves the backtrace out, and with it most of what the report takes.

Not every failed request counts, because not every failure says the worker is unfit to serve:

| Outcome | Effect on the count |
|---|---|
| Fatal error, out-of-memory, stack overflow — raised in the request handler, in a shutdown function, or in a destructor that runs as those shutdown functions are released | Counts |
| The fiber the request runs in would not start — the engine could not allocate its C stack, so the handler never ran | Counts |
| The request would not stop suspending that fiber from userland, and the server ended it | Counts |
| Uncaught exception (answered `500`) — from the request handler or from a shutdown function | Neutral |
| Cancelled request — `max_execution_time` elapsed, the server is shutting down, the client of a streaming response that has not called `ignore_user_abort(true)` hung up | Neutral |
| Request completed, `exit()`/`die()` included | Clears the count |

"Neutral" means exactly that: one of those in the middle of a run of fatals neither adds to the count nor clears it, so `fatal, exception, fatal, fatal` still recycles the worker. A failure in a shutdown function is read the same way as one in the request handler. PHP runs shutdown functions under protection of its own, so the worker sees a request that failed inside one return normally — but a fatal there is a request coming apart on its own, exactly as one in the handler is, so it counts the same, and an exception there unwinds as cleanly as one from the handler, so it is neutral the same way. What decides is why a request ended and not what it left behind: whatever raised the bailout, the worker picks the abandoned frames back up and rewinds the VM stack the same way, so a deadline that expires while a shutdown function runs is cleaned up exactly as a fatal there is and leaves the worker in the same state — and it is still neutral, because the question the count answers is whether this worker is at fault, and a request the server itself cut short is not evidence against it. So a cancellation is neutral whether it lands on the handler — a deadline expiring mid-render, the server shutting down — or in a shutdown function, provided the request had not already hit a fatal. A client hanging up is not an outcome of its own in worker mode, outside a streaming response: it does not end the request, which is free to finish and run its own cleanup, and the request is then read by how it ended like any other — reaching its end clears the count, a fatal counts, an uncaught exception is neutral. That is deliberate. A request that ran its handler to the end is evidence the worker can serve whether or not anyone was left to read the response, and a worker that fatals on every request fatals on this one too, so a client leaving cannot keep such a worker alive. A deadline that expires in a shutdown function of a handler that came apart leaves that failure counted: what the worker answers for is having come apart, and a deadline afterwards does not take that back. Shutting the server down is the one cancellation that is neutral unconditionally — a worker on its way out is not a worker being judged.

The last two rows are about the fiber a request runs in rather than the code inside it, and they count for the reason a fatal does: the request did not reach the end of its own work, and it was not the server cutting it short on a client's or a deadline's behalf. A fiber does not start when the engine cannot allocate the C stack it runs on — `fiber.stack_size` set below the platform minimum of two memory pages, or the process unable to map more memory — and the handler is never reached at all. The other is the application's own doing and leaves the response looking ordinary: the server drives a request's fiber itself, so a userland `Fiber::suspend()` on it is refused with a `FiberError`, and a handler that catches that and retries in a loop is ended rather than argued with indefinitely — an ending userland cannot catch, which still runs the request's `finally` blocks, destructors and shutdown functions, and still sends what the request had produced under the status the handler had set. The `WARN` line for a counted failure names which of the two it was, as `fiber would not start` or `fiber refused to park`.

What none of this does is diagnose a worker that has wedged rather than failed: a request stuck in a syscall is reported through `oxphp_worker_stuck_total` for an operator to act on, not cancelled and not counted. Nothing in the server ends such a request automatically — no recycle, no cancellation — so a wedged worker stays wedged until the call returns or the process is restarted. `max_execution_time` is not a backstop for it either: a time limit ends a request only where PHP code runs, which a request stuck in a syscall does not reach until the call returns — see [Request Deadlines](#request-deadlines).

When a worker is recycled, that worker's loop ends and its replacement re-executes the outer scope of the worker script. Workers are OS threads inside the single OxPHP process rather than separate processes, so nothing the operating system sees restarts and `getmypid()` returns the same value in every worker, before and after. Whether a replacement arrives at once depends on the pool: a static pool (`PHP_WORKERS=N`) refills to `N` on its next scan, so the replacement is one-for-one, while a dynamic pool (`PHP_WORKERS=MIN:MAX`) refills only to `MIN` — a worker recycled above the minimum leaves the pool one smaller until ordinary scale-up grows it again. For memory-based and scheduled exit, the current request completes normally before the worker exits. For error-based recycling, the worker exits after the failed request.

A memory-based or scheduled recycle is logged at `INFO`: the worker writes `Worker mode thread recycled` with `reason` set to the label `oxphp_worker_recycles_by_reason_total` counts it under — `max_memory`, together with `memory_bytes` (the usage that passed the limit) and `max_memory_bytes` (the limit), or `scheduled`, which covers `Worker::scheduleExit()` and the recycles the worker schedules for itself — and the pool follows with `Recycled workers detected`. A worker recycled after consecutive errors is logged at `WARN` as `Worker mode thread retired after consecutive request failures`, and the pool reports it, like any worker thread that ends without choosing to, as `Dead workers detected, respawning`. So at `LOG_LEVEL=warn` a recycle on `WORKER_MAX_MEMORY_MIB` or on `Worker::scheduleExit()` leaves no line of its own: a worker that recycles on memory often is a leak worth chasing, and the place to see that is the rate of `oxphp_worker_recycles_by_reason_total{reason="max_memory"}`.

Retirement is not recycling and is not counted as such. A dynamic pool retiring an idle worker (see [Dynamic Pool](../architecture/overview.md#dynamic-pool)) also ends that worker's loop and also runs the code after `oxphp_worker()`, but it is scaling down rather than replacing: nothing is spawned to replace it, neither `oxphp_worker_recycles_total` nor `oxphp_worker_recycles_by_reason_total` moves, and it is `oxphp_workers_retired_total` that counts it instead. Application teardown placed after `oxphp_worker()` therefore runs on a healthy, serving pool as well — see [`oxphp_worker()`](../php/functions.md#oxphp_worker).

Other requests the same worker was serving concurrently — suspended in `oxphp_async_await()`, `oxphp_sleep()`, or a socket wait under `RUNTIME_HOOKS` — do not get to finish: each is cancelled where it is suspended and answered with `503 Service Unavailable` and a `Retry-After`, after running its own shutdown functions. Recycling is therefore visible to clients whose requests happen to be in flight, which is worth knowing when choosing `WORKER_MAX_MEMORY_MIB` or calling `scheduleExit()` on a worker that serves concurrent requests. A full server shutdown is different: there, in-flight requests get the drain window to finish normally.

## Request Deadlines

Every request a worker serves is bounded by `max_execution_time` on its own, whether the worker took it with nothing else running or while it was carrying others — with [fiber multiplexing](fiber-multiplexing.md) a worker runs several requests on one thread, and each still gets the whole of its limit and no more:

- **A request's limit counts from the moment the worker takes it.** It starts at the value of `max_execution_time` the worker's bootstrap left — the `php.ini` value, or whatever the outer scope of the worker script set — and, as everywhere in OxPHP's thread-safe PHP build, it counts wall-clock time, time spent suspended included (see [Timeouts](timeouts.md#wall-clock-not-cpu-time)).
- **`set_time_limit()` applies to the request that calls it.** It restarts that request's limit from the call, as it does under PHP-FPM, and leaves every other request on the worker as it was. Only the request's own deadline moves with it; the directive the call writes stays with the worker (see the ini boundaries under [What Gets Reset Between Requests](#what-gets-reset-between-requests)).
- **When a limit runs out, it ends the request it belongs to.** That request is read as having run out of time: a `504`, the timeout bit set in its own `connection_status()` for its shutdown functions, neutral for the recycle count. The requests running beside it carry on.
- **A suspended request is ended when it resumes, not at the moment its deadline passes.** A request waiting in `oxphp_async_await()`, `oxphp_sleep()`/`oxphp_usleep()`, or a `sleep()` or a socket wait (a read, and with `net` a connect, a TLS handshake or a write that waits for room) taken over by `RUNTIME_HOOKS` runs no PHP code, and a time limit can only end code that runs. A request whose limit passed while it waited is ended as soon as it is back in PHP code: once the call it was waiting in returns, the request ends there as at any other timeout, its shutdown functions included. A call that waits once — a sleep, a single read — returns when it is resumed; one that waits again and again inside, such as `stream_get_contents()` reading a stream to its end, returns only when it is done as a whole. `session_start()` waiting for another request's session to close (see [What gets reset between requests](#what-gets-reset-between-requests)) is the exception: it is resumed every quarter of a second, and returns, without opening the session, the first time it is resumed out of time. So is a streaming response's flush that waits for its client to make room (see [Server-Sent Events](sse.md#how-it-works)): it is resumed whenever the client takes a chunk and otherwise at least every 16 milliseconds, and returns the first time it is resumed out of time, having sent what it was waiting to send only if the client made room for it by then. It resumes when what it waits for arrives — the promise settles, the sleep elapses, the socket becomes readable (or writable, for a write waiting for room, or connected, for a connect) or the wait reaches its timeout — the stream's own (`default_socket_timeout` unless `stream_set_timeout()` set another) for a read or a write, the timeout passed to `stream_socket_client()` or `fsockopen()` for a connect — or when the server cancels it. A server shutdown that passes its drain deadline, and a recycle of the worker, cancel every request suspended at that moment, and one whose limit had already passed is ended by that cancellation like the others: a `503`, with its shutdown functions run and not cut short by the spent limit.
- **What a worker runs between requests is under no limit.** The destructors of objects the worker's periodic garbage collection frees run outside any request, and a time limit bounds none of that.

Nothing else ends a suspended request for you. A client disconnecting does not end a non-streaming request, which runs to its end by design (see [Recycling](#recycling)), and a request running longer than the supervisor's threshold is reported through `oxphp_worker_stuck_total`, not cancelled. Put the limit on the wait itself: pass a timeout to `oxphp_async_await()` and its variants, and set timeouts on sockets and database clients.

## Development Reloading

Worker Mode persists bootstrap state (autoloader, DI container, DB connections) in memory, so `opcache.validate_timestamps=1` alone is not enough to pick up changes to code that ran during the outer scope. For development loops there are two options:

- **Recycle every request.** Call `OxPHP\Server\Worker::current()->scheduleExit()` at the end of every handler invocation (gated on a `OXPHP_DEV` env flag, for example). The current request completes normally, then the worker exits and a replacement re-executes the outer scope — immediately on a static pool, which is what a development setup normally runs. This trades the worker-mode performance win for FPM-style reload semantics — simplest and most reliable for active development.
- **Keep the worker warm, reload request handlers.** Skip `scheduleExit()` entirely, enable `opcache.validate_timestamps=1`, and keep your bootstrap minimal. Code loaded inside the request callback will be refreshed by OPcache on the next request; code loaded once in the outer scope will not. See [OPcache and JIT → Development Settings](../php/opcache.md#development-settings) for the full list of caveats.

## Troubleshooting

### Requests hang and never complete

If `oxphp_worker()` is never called in the bootstrap script, no requests are dispatched and every request waits indefinitely. Verify that your script calls `oxphp_worker()` unconditionally in the normal code path.

### State leaks between requests

Variables defined inside the `oxphp_worker()` callback are cleaned up by PHP's garbage collector, but static properties and globals defined in the outer scope persist. If you see data from one request appearing in another, check for static properties or global variables that accumulate state across calls.

**Fix:** Reset static state explicitly at the start of each request callback, or avoid storing per-request state in statics.

### Worker recycles immediately (memory limit)

The worker memory limit is checked after each request using PHP's reported memory usage. If your bootstrap phase allocates a large amount of memory (e.g. loading a large cache), the initial memory footprint may already be close to the limit.

**Fix:** Increase `WORKER_MAX_MEMORY_MIB` or defer large allocations to the first request.

**Check:** At `LOG_LEVEL=info`, each such recycle logs the usage that passed the limit as `memory_bytes`, next to the limit as `max_memory_bytes`. Every earlier check found the usage within the limit, so the difference between the two is at most what the worker grew by since the last check before it: a small one fits gradual growth, a large one a single jump. On a worker that recycles after its first request, that jump includes everything the bootstrap allocated.

```bash
docker logs <container> 2>&1 | grep '"reason":"max_memory"'
```

### Worker recycles immediately (error limit)

Three consecutive counted failures trigger a recycle. A fatal error in the request callback is the usual one, so check your application logs for those first — not for uncaught exceptions, and not for cancelled requests, unless a fatal was reported on the cancelled request as well, before or after the cancellation. A recycle with no fatal behind it is one of the two fiber failures instead, and the `WARN` line below names which: `fiber would not start` points at `fiber.stack_size` or at the host — that C stack is mapped from the operating system rather than allocated from PHP's heap, so neither `memory_limit` nor the `WORKER_MAX_MEMORY_MIB` above is the knob for it — and `fiber refused to park` points at a `Fiber::suspend()` your own code keeps retrying.

The server says so itself at `WARN`: one line per counted failure, naming the worker, where the failure came from and how many have run consecutively, and one line for the recycle that follows the third. The worker is named because the count is per worker — three lines from one worker are a recycle, three lines from three workers are not.

**Check:** Look for errors in the access log or structured log output:

```bash
docker logs <container> 2>&1 | grep -E '"level":"(ERROR|WARN)"'
```

### Database connection drops after idle

If your database server closes idle connections, reconnect attempts in the next request may fail. Use a connection pool that handles reconnection, or catch the exception and reconnect manually.

## Docker Example

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.13.0
    ports:
      - "8080:80"
    volumes:
      - ./src:/var/www/html
    environment:
      - DOCUMENT_ROOT=/var/www/html/public
      - WORKER_MODE_ENABLED=true
      - ENTRY_FILE=/var/www/html/worker.php
      - WORKER_MAX_MEMORY_MIB=128
```

## PHP API

Worker introspection and the worker entry point are exposed through the
[`OxPHP\Server\Worker`](../php/worker-class.md) class.

```php
<?php
$worker = OxPHP\Server\Worker::current();
$worker->serve(function () {
    handleRequest();
});
```

**Legacy free functions** (`oxphp_is_worker`, `oxphp_worker_id`, `oxphp_worker`)
remain available and route through the same internal state. New code should
prefer the class API.

The class also exposes runtime introspection useful for graceful self-recycling, observability, and health checks:

| Method | Returns |
|--------|---------|
| `Worker::isWorkerMode(): bool` | Whether the server is running in worker mode |
| `$worker->id(): int` | Stable per-thread worker ID |
| `$worker->startTime(): float` | Unix timestamp of when this worker started |
| `$worker->requestCount(): int` | Number of requests this worker has handled |
| `$worker->memoryUsage(): int` | Current `memory_get_usage(true)` for this worker |
| `$worker->rss(): int` | Current resident set size in bytes (Linux/macOS) |
| `$worker->maxMemoryBytes(): int` | Recycle threshold — `WORKER_MAX_MEMORY_MIB` × 1 MiB, or `0` when unlimited |
| `$worker->isExitScheduled(): bool` | Whether `scheduleExit()` has been called |
| `$worker->exitReason(): ?string` | `null` while running; `"scheduled"`, `"max_memory"`, or `"error"` once the worker is going down |

See [`OxPHP\Server\Worker`](../php/worker-class.md) for full signatures and worked examples.

## PHP Examples

### Detecting Worker Mode

Use `OxPHP\Server\Worker::isWorkerMode()` to check whether the current process is running in worker mode. This is useful for writing code that works in both traditional and worker mode.

```php
<?php
if (OxPHP\Server\Worker::isWorkerMode()) {
    // Reuse a persistent connection
    $redis = new Redis();
    $redis->pconnect('redis', 6379);
} else {
    // Traditional mode: connect per request
    $redis = new Redis();
    $redis->connect('redis', 6379);
}
```

### Symfony Worker Script

```php
<?php
use App\Kernel;

require __DIR__ . '/../vendor/autoload.php';

$kernel = new Kernel('prod', false);
$kernel->boot();

oxphp_worker(function () use ($kernel) {
    $request = Symfony\Component\HttpFoundation\Request::createFromGlobals();
    $response = $kernel->handle($request);
    $response->send();
    $kernel->terminate($request, $response);
});

$kernel->shutdown();
```

## Best Practices

- **Set `WORKER_MAX_MEMORY_MIB`** (e.g. `128`) so a leaking worker recycles automatically instead of consuming the host. Combine with `Worker::scheduleExit()` for application-driven recycling on top. A worker can grow even when the application leaks nothing: when a fatal error or a cancellation abandons a request inside an internal function, what that function had allocated for itself is not given back. A request cancelled in the middle of `usort()` keeps the whole array being sorted alive, and a worker that grew by more than 4 MiB that way is recycled, as described under [Recycling](#recycling); smaller leaks stay until `WORKER_MAX_MEMORY_MIB` is passed or, without it, until requests start failing against `memory_limit`. A fatal error or a cancellation that ends a request inside a `Fiber` it runs — one it started, or the one PHP runs garbage-collection destructors in — leaves what the fiber's own frames were holding allocated for the life of the worker in the same way.
- **Avoid storing per-request state in static properties or globals.** Since these persist across requests, leftover state from one request can leak into another.
- **Validate the soft reset early.** Add `Worker::current()->scheduleExit()` to your handler under a development flag and exercise the application end-to-end — this catches state-leak bugs before you commit to long-lived workers.
- **Handle database idle timeouts.** If your database driver disconnects after an idle period, catch the exception and reconnect, or use a connection pool that handles reconnection automatically.
- **Keep the outer scope minimal.** Only bootstrap what truly needs to persist — autoloaders, configuration, and shared services. Defer request-specific setup to the callback.

## See Also

- [Routing](routing.md) — how worker mode plugs into URL routing
- [Early Response](early-response.md) — send the response immediately and continue background processing
- [PHP Functions](../php/functions.md) — full reference for `oxphp_worker()`, `oxphp_is_worker()`, and other built-in functions
- [Configuration Reference](../operations/configuration.md) — complete list of environment variables
