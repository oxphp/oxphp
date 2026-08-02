# Changelog

All notable changes to OxPHP are documented in this file.

## [Unreleased]

### Fixed

- **Worker mode: an output buffer a request leaves open is no longer flushed into the next request's response.** A request that calls `ob_start()` without a matching `ob_end_flush()` or `ob_get_clean()` — deliberately, on an early return, or through a framework's error path — sent an empty body to its own client, and the content it had buffered was prepended to the body of whatever request the worker served next. That is one client's content delivered to another, with nothing in the log to show for it. The buffers a request opens are now closed where the request ends, which is where every other SAPI closes them: the content reaches the client that asked for it and no one else. Only the layers a request opened itself are closed, so a request parked mid-buffer keeps its own. Traditional, framework and SPA serving were never affected.

- **Worker mode: an output buffer stays with the request that opened it across a suspension.** A request that parks — in `oxphp_async_await()`, `oxphp_sleep()`, or a socket read under `RUNTIME_HOOKS` — left its open buffers standing on the worker, because the buffer stack belongs to the worker thread rather than to the request. Two silent consequences. Whatever request the worker served in that window wrote into the parked request's buffer instead of its own: its client received a response with no body, and its content was handed to the client the buffer belonged to. And what the parked request had buffered before suspending was flushed on the way into the suspension — sent to the client ahead of any header the rest of the request had yet to set, out of a buffer whose whole point was that it had not been sent, and through any handler the script had installed with half of what that handler was there to transform, so an `ob_get_clean()` after the suspension returned the tail alone. A request's buffers now travel with it: out of reach of anything else the worker serves, and back in place with their content when it resumes. Requests that never suspend were never affected, and neither was traditional, framework or SPA serving.

- **Worker mode: a response carries `Content-Type` exactly once, and a response with no body carries it at all.** The first response a worker sent carried the engine's default `Content-Type` twice — the default it emits while a request is starting up was kept in the server's header list and went out on top of the one the request itself produced, so a client saw two `Content-Type` lines, with the wrong one first if the script had changed `default_charset`. Separately, a response that wrote no body carried no `Content-Type` at all, where every other PHP SAPI sends the default. Both are gone: clearing a request's header state now clears it everywhere it is held, and a response is finished the way the engine finishes one, whether or not it wrote anything.

- **Worker mode: a response that sets no `Content-Type` of its own no longer costs its worker memory.** The engine allocates the default content type (`text/html; charset=UTF-8`) for every response that does not carry one and expects it back when the request ends — and in worker mode the end of a request is not where a worker gives memory back. Every such response, which is most of them, therefore kept one string for the life of the worker: 32 bytes each, about 3 MB per hundred thousand requests, growing for as long as the worker serves. Responses that set their own `Content-Type` were never affected, and neither was traditional, framework or SPA serving, where the engine's own end-of-request path returns it. The per-request reset now returns it too. A suspended request also keeps its own content type across the suspension: it is what output handlers such as `mb_output_handler` and `iconv` read to decide whether to convert what a request writes, and being thread-global it previously read whichever request touched it last.

- **Worker mode: a request that ends in a fatal error no longer costs its worker memory for the rest of the worker's life.** A fatal is not an exception: the engine abandons the request where it stands and jumps straight out, leaving behind everything the interrupted script was holding — the copy of the script the request ran on, the variables of every function it was inside, the arguments of the internal calls it was inside (a fatal from within `array_map()` held its array, one from within a database method held the object it was called on), the closures those calls ran — a closure declared inside a request is a new object every time, and it carries everything it closed over — and the stack those frames stood on. Serving one request per script, that memory goes back when the request ends. In worker mode it belongs to a worker that keeps serving, so each fatal kept its share until the process exited: measured at roughly 900 bytes for a small script, about 13 KB for one of a few hundred statements, and the full payload when the request fataled holding something large — a fatal 100 KB into a string cost the worker 100 KB, every time. A run of failing requests, whether a bad deploy, an outage downstream or a hot path throwing on every call, therefore grew the worker without bound. All of it is now released where the engine would have released it, and a fatal costs a worker no more than the same request without one.

- **Worker mode: a fatal raised while a generator is running no longer gives up the generator's variables twice.** The engine links a running generator's frame to whatever resumed it, so it is in the chain a fatal is reported from — but the frame belongs to the generator object, not to the interrupted call, and the generator hands it back itself when it closes. Released along with the rest of the chain, every value the generator was holding was given up twice: once by the worker and once by the generator. The second one landed on a value something else was still holding — a registry, a static, a property — which was then freed while still in use, so whatever read it next read memory that had already been handed to someone else. The worker now takes the frame off the generator instead: its values are given up exactly once and the generator's own close finds nothing left to do.

- **Worker mode: a fatal error no longer switches off the cycle collector for the rest of the worker's life.** A fatal raises two flags inside the engine, not one: besides marking the shutdown unclean, it stops the collector recording anything as a possible member of a cycle. The engine lowers that second flag where it starts a request — which in worker mode happens once, for the worker, rather than once per request. The first fatal a worker took was therefore the last moment it ever recorded one, and from then on nothing reached the collector at all: every cyclic structure any later request built lived until the process exited, whether that is a container holding services that hold the container, a parent holding children that hold the parent, or a closure bound to the object that stores it. The periodic collection a worker runs became a no-op with nothing left to collect, so a worker serving an application that builds one such graph per request grew for as long as it served — and none of it showed up in the measurements that already cover what a fatal costs, because those build no cycles. Both flags are now lowered where the worker goes back to serving. A fatal raised from inside a collection is the one case where the second flag stays raised: it is also what stops the collector re-entering itself, and an abandoned run leaves behind a half-marked set of objects that nothing will finish, so lowering it would hand the next collection that set to walk. A worker in that state cannot collect again, and is now retired once it has finished the request it is serving — the same graceful replacement, and the same recycle counter, as a worker whose handler calls `Worker::scheduleExit()`.

- **Worker mode: a fatal raised by a shutdown function is cleaned up like any other.** The engine runs `register_shutdown_function()` callbacks under a guard of its own that swallows the fatal, so the worker was handed what looked like a request that ended normally while everything the fatal left behind stayed exactly where it was: the abandoned frames and their variables, the stack pointer inside them, the cleared execution cursor, and both of the flags above. The frames were held for the life of the worker, a generator closed later lost its frame whole, the collector went off as described above, and the check that stops a worker releasing frames which are not its own was left with nothing to check against. Shutdown functions are where frameworks flush logs, close sessions and report errors, which makes this the fatal most likely to follow another one. A worker now recognises it and does the same cleanup it does for a fatal raised by the request itself.

- **Worker mode: an exception thrown by a shutdown function is reported instead of disappearing.** Under every other SAPI, an uncaught exception from a `register_shutdown_function()` callback is a fatal error: the engine calls those callbacks with no PHP frame left on the stack, and that is precisely what sends the exception down the path which prints `Uncaught ...` and aborts. A worker calls them from inside a frame of its own, where that path does not run, so the exception was neither reported nor re-thrown — no fatal, nothing in the log, no `set_exception_handler()` call, and a response indistinguishable from that of a request which had done nothing wrong. Shutdown functions are where applications flush logs, close sessions and report errors, so the failures hidden this way were the ones written specifically to be visible. Such an exception is now reported the way the engine reports it, in the response and the log of the request that raised it. The handler installed with `set_exception_handler()` is deliberately not called for it: in worker mode that slot belongs to the worker rather than to a request, so calling it here would run one request's handler for another request's exception.

- **Worker mode: `error_get_last()` no longer answers with another request's error.** The engine records the last error on the worker thread rather than on the request, and only the reset a worker runs when it is idle between requests cleared it. Three consequences, all silent. A request served immediately after one that ended in a fatal read that fatal as its own. A request that suspended came back reading whatever error was raised while it was parked. And a request that raised an error and then suspended left it where the request the worker served in that window could read it. What makes this more than a cosmetic mix-up is where `error_get_last()` is normally called: inside a `register_shutdown_function()` callback, to decide whether the request being closed died on a fatal — which is how frameworks and error handlers catch fatals at all. They therefore reported failures for requests that had succeeded, complete with another request's message, file and line. A request now keeps its own: a new one starts with no last error, and a suspended one takes its own with it and finds it again on resume.

- **Worker mode: the functions a request registers with `register_shutdown_function()` stay with it across a suspension.** The engine files them in a registry it keeps per worker thread, and the end of a request runs everything standing in that registry and then discards the lot. A request that parked — in `oxphp_async_await()`, `oxphp_sleep()`, or a socket read under `RUNTIME_HOOKS` — therefore left its callbacks where the next request to finish would take them, with both halves of that going wrong at once. They ran inside that request: anything they echoed went into another client's response body, and the session write, log flush, profiling sample or error report they were registered to perform ran against a request that was not the one being closed. And they were discarded there, so the request that registered them reached its own end with nothing left to run. The end-of-request work an application defers this way was performed for the wrong request and then never performed for the right one — silently, since a shutdown function that runs reports nothing and one that never runs reports nothing either. Shutdown functions now travel with their request: out of reach of anything else the worker serves while it is parked, and back in place when it resumes, where they run into their own response at their own request's end. Requests that never suspend were never affected, and neither was traditional, framework or SPA serving.

- **Background tasks: what a task registers with `register_shutdown_function()` no longer piles up on the thread that ran it.** The registry the engine files those callbacks in belongs to a thread, and it is emptied by the end of a request — but the threads that run `oxphp_async()` tasks open a single request when they start and close it only when the process stops. A callback registered from inside a task therefore stayed in that registry for the life of the server, holding its closure and everything the closure had captured, and every task's callbacks eventually ran together at shutdown, for tasks that had finished long before, with nothing left to close. A task that runs on every request made this a leak that grows for as long as the server is up. What a task registers is now discarded when the task ends, so nothing accumulates and nothing runs later on another task's behalf; registering a shutdown function from a task consequently does nothing at all, which is the honest form of a callback that has no response to write into and no request to be the end of. Shutdown functions registered by a request are unaffected and run exactly as before.

- **Worker mode: a worker that retires itself now ends the requests it was still serving, instead of dropping them.** A worker replaces itself for three reasons — a handler calling `Worker::scheduleExit()`, the memory ceiling, or the consecutive-error breaker — and it acted on all three the moment it saw them, with whatever else it was multiplexing still parked mid-request. Those requests were never ended: the teardown that followed unwound them without giving each one back the state it had parked with, so their `finally` blocks, destructors and shutdown functions ran against another request's superglobals, headers and output, and their own responses — including everything they had already produced — were dropped in favour of the server's generic `500 PHP Worker Error` page, with nothing in the log to say a request had been ended at all. Under a memory ceiling, which is exactly the setting where a worker is busy enough to be multiplexing, that turned routine recycling into 500s for concurrent clients. A retiring worker now ends each parked request the way a shutdown drain ends one: on its own state, unwinding uncatchably from the point it parked at, running its own shutdown functions, and answering its own client with its own output, a `503` and a `Retry-After`, next to an error-log line naming the request's own script and line. Requests that were not parked at that instant, and workers that retire with nothing else in flight, behave exactly as before.

- **Worker mode: concurrent requests are no longer indistinguishable to libraries that track the current fiber.** A request served in worker mode ran on a fiber the engine did not expose, so `Fiber::getCurrent()` returned `null` and every library that keys per-task state on the current fiber filed all concurrent requests on a worker under one key — reading each other's state with no error and no log line. The libraries affected are the ones most likely to be present: `open-telemetry/context` (so the active span, through the default context storage, and therefore anything that reads it — the current trace, baggage, a span a request believes it is inside), `revolt/event-loop` (`Suspension` identity and `FiberLocal`, and through it every package built on Revolt or AMPHP), `monolog/monolog` (cycle-detection depth), and `spiral/core` (container scope). Two requests multiplexed on one worker shared that state for as long as both were live, which no application-level reset could fix: resetting on entry to the second destroys what the first is parked on. Each request now runs as a real `Fiber`, so `Fiber::getCurrent()` returns a distinct object per in-flight request and those libraries isolate them the way they do under any other fiber-based runtime. Background tasks started with `oxphp_async()` get their own fiber identity for the same reason. Nothing about how a handler is written changes, and requests that never run concurrently were never affected.

- **`oxphp_async_await_all()`, `oxphp_async_await_race()` and `oxphp_async_await_any()` no longer leak one reference per result.** Each of the three materialized a promise's result in a temporary slot and then copied it into the array they return. The copy takes a reference of its own, so the temporary's reference had to be released afterwards — and none of the three released it. Every string, array or object delivered through them therefore stayed alive with one reference too many: unreachable from PHP, never collected, and reclaimed only when the request's allocator pool was torn down at the end of the request. In traditional request-per-script serving that is a bounded overshoot within a single request. In worker mode, where a request can live for the lifetime of the worker, it is unbounded growth proportional to how many results those functions delivered — a fan-out loop awaiting sizeable payloads leaks the full payload on every iteration. Results that are not reference-counted (`int`, `float`, `bool`, `null`) were never affected, and neither was `oxphp_async_await()` — the single-promise call writes its result straight into the return slot with no copy, which is why the defect never showed there. Nothing observable to a script changes: return values, keys and exception behaviour are identical, and the released reference is never the last one, so no destructor runs any earlier than before.
- **Graceful shutdown: work that keeps running after `oxphp_finish_request()` is no longer discarded the moment shutdown begins.** A request that sends its complete response early and then carries on working — the pattern behind sending mail, writing cache entries, or dispatching webhooks once the client has been answered — has already released its connection by the time `SIGTERM` arrives. The drain measured outstanding work by counting live connections alone, so on that path it saw nothing left to wait for, skipped its window entirely, and the workers were torn down with the background work still mid-flight. Measured with a ten-second drain window and six seconds of post-response work remaining, the process was gone one second after the signal and the work never finished — which contradicted the documented behaviour, that such a request "counts as ordinary — its background work gets the drain window". The drain now counts requests still executing on PHP workers alongside live connections: post-response work that fits inside the window runs to completion, and work that outlasts the window is interrupted at the drain deadline like any other in-flight request, so the `DRAIN_TIMEOUT_SECONDS` + ~2s bound applies to it for the first time. Streaming responses that call `oxphp_finish_request()` were never affected — they keep their connection until the stream ends, so the drain always covered them. Two shutdown log events, `Draining in-flight connections` and `Drain timeout reached, cancelling in-flight requests`, now carry an `in_flight_requests` field next to their connection counts; the two counts overlap rather than partition, since a request holding a live connection appears in both. Two boundaries worth knowing. Background tasks started with `oxphp_async()` and never awaited are not covered: they are bounded by the async pool's own shutdown after the drain, not by the drain deadline. And a worker thread that is still alive but has abandoned a request without running its terminal cleanup leaves that request counted, since the slot is only reclaimed once the thread itself has finished; the drain then runs its full window before exiting, where it previously exited at once — still bounded by `DRAIN_TIMEOUT_SECONDS` + ~2s, and still inside a default Kubernetes grace period.
- **Worker mode: a suspended request no longer resumes with another request's superglobals.** A request that parked — in `oxphp_async_await()`, `oxphp_sleep()`, or a socket read under `RUNTIME_HOOKS` — came back reading the `$_GET`, `$_POST`, `$_COOKIE`, `$_REQUEST` and `$_SERVER` (`REQUEST_URI`, `REMOTE_ADDR`, request headers) of whichever request the worker served while it was parked. That is not stale data from the same client: it is one concurrent client's request data read by another, with no error and no log line. Suspending saved and restored the engine's internal superglobal slots, but userland reads a separate set of symbol-table entries that each incoming request rebinds to its own arrays, and those were left pointing at the intruder. Resuming now rebinds them as well. Requests that never suspend were never affected.
- **Worker mode: `$_REQUEST` is now rebuilt for every request.** A worker built it once and then never again, so from its second request onward a script reading `$_REQUEST` directly saw the merged query string, form fields and cookies of whichever request first loaded that script — one client's parameters served to the next, with no error and no log line. `$_GET`, `$_POST` and `$_COOKIE` were correct throughout; only the merged view was stale. A worker's own handler body was the worst case: compiled at boot, before any request exists, it saw an empty `$_REQUEST` on every request it ever served. The defect needed OPcache enabled, and applications reading the request through a framework abstraction (Laravel, Symfony) were unaffected — legacy code and plugins touching `$_REQUEST` directly were not. `$_ENV` is deliberately not rebuilt alongside it: it describes the process rather than the request, and worker mode now pins it (see below).
- **Worker mode: values written into `$_ENV` at boot no longer vanish part-way through a worker's life.** A `.env` loader — vlucas/phpdotenv, symfony/dotenv, Laravel's `Env` — writes its values straight into `$_ENV` without touching the process environment, and in worker mode it runs once at boot and never again. From then on, the next thing that asked PHP for `$_ENV` rebuilt the array from the process environment and dropped every one of those values: compiling any not-yet-cached file that mentions `$_ENV`, which in practice is the first request reaching some lazily-autoloaded class, or a `filter_input(INPUT_ENV, …)` call. Nothing failed and nothing was logged — configuration read through `$_ENV` simply became empty on an arbitrary later request, on a worker that had served earlier requests correctly, while `getenv()` and `$_SERVER` kept working. `$_ENV` is now pinned for the life of the worker once it exists, and worker mode's treatment of it is documented for the first time (see the `$_ENV` section of the superglobals reference). One deliberate consequence: `filter_input(INPUT_ENV, …)`, `filter_input_array(INPUT_ENV)` and `filter_has_var(INPUT_ENV, …)` report no ENV variables in worker mode from the point `$_ENV` exists — in practice every request after the one that first materializes it — instead of reading the process environment. Read the environment with `getenv()`, which is unaffected, or `$_ENV` itself, which now holds both the process values and whatever the application wrote. This assumes the default `auto_globals_jit=1`; with it off, PHP repopulates `$_ENV` from the process environment on every request, before any extension gets a say.
- **Worker mode: a worker no longer dies with a segmentation fault on its second request under a real application.** Every request after a worker's first was handed to a recycled fiber as though it were resuming a suspended one, which re-installed a per-fiber state snapshot that had never been written — a fiber only ever reaches the reuse pool by *finishing* a request, while that snapshot is written only when a request *suspends* (in `oxphp_async_await()`, `oxphp_sleep()`, or a hooked socket read). Installing it overwrote the state the new request had just set up, with three visible consequences: the response header list became an empty structure with a zero element size, so the request's second `header()` call walked uninitialized memory and dereferenced a garbage pointer; the superglobal slots became undefined, so the first `$_REQUEST` access walked an invalid hash table; and the response status became `0`, so `http_response_code()` returned `false` where it owed `200`. Whether that faulted depended entirely on the heap: a trivial handler that echoes a string got zero-filled memory back and survived indefinitely — which is why synthetic worker-mode handlers ran millions of requests clean — while an application that allocates (WordPress issues several `header()` calls from `send_headers()`) got memory full of live data and crashed on request `N+1`, where `N` is `PHP_WORKERS`, i.e. on each worker's second request. Serving a new request now installs nothing from that snapshot; it is read only where it is written, when a suspended request resumes.
- **Overload is now shed on how long a request has waited, not on the queue depth at the instant it arrived — a burst no longer turns into 529s while the worker pool is idle.** A request that found the queue full was rejected immediately, so the rejection threshold was `QUEUE_CAPACITY`, which defaults to worker count × 128 — a number with no relationship to how long a request could actually afford to wait. On a 14-core host (7 workers, queue 896) that meant 38.6% of responses were 529 at 2000 concurrent connections against a sub-millisecond endpoint, and 54.6% at 1000 connections against a demo front controller, even though re-running the same load with a large queue served identical useful throughput with zero errors — the pool was never the bottleneck. The rejection was also self-concealing in benchmarks: raw requests/sec *rose* (316k vs 193k) precisely because emitting a 529 is cheap. Requests now wait up to `QUEUE_WAIT_TIMEOUT_MS` (default 1000) for a slot, in arrival order, and are rejected only if none frees up in time — so bursts within the pool's capacity are served, and a pool that genuinely cannot keep up still sheds. The budget is one deadline stamped on arrival and it covers the wait inside the queue as well: a request admitted with time to spare but reached by a worker after its deadline has passed is refused at pickup rather than executed. That matters because the queue is `PHP_WORKERS × 128` deep by default — on a pool serving 200 ms handlers, reaching the tail of a full queue takes some 25 seconds, so a budget that stopped at the admission half would have bounded the smaller wait and left the larger one open. It also means a slow deployment can now see 529s where it previously saw very late 200s; raise `QUEUE_WAIT_TIMEOUT_MS` if your clients genuinely wait longer than a second, or set `QUEUE_WAIT_TIMEOUT_MS=0` to restore the previous reject-immediately behavior (which applies no deadline at all).
- **`oxphp_busy_workers` now counts busy workers, and `oxphp_workers_idle` idle ones.** Both gauges were driven by a counter incremented when a request was handed to the queue and decremented on its completion paths, which made them wrong in two ways at once. They counted queued requests as well as executing ones, so `oxphp_busy_workers` rose past the total worker count under load and `oxphp_workers_idle` — computed by subtraction — read zero on a pool that was not saturated; and a client that disconnected mid-request took the whole dispatch with it, skipping the decrement, so the gauges only ever climbed and never recovered without a restart. Any dashboard, alert, or autoscaler reading them was eventually wrong. Both are now derived at scrape time from the workers themselves: `oxphp_busy_workers` is the number of worker threads with at least one request in flight, so it never exceeds `oxphp_workers_current`, and `oxphp_workers_idle` is the remainder. Requests waiting for admission or sitting in the queue are no longer counted as busy — they appear in `oxphp_pending_requests`, which covers every accepted request until it is answered. Two notes for existing dashboards: `oxphp_busy_workers / oxphp_workers_current` is now a true utilization fraction between 0 and 1 where it previously exceeded 1 whenever the queue backed up, and in worker mode the gauge counts threads rather than request fibers, so a thread multiplexing many requests counts once.
- **`oxphp_queue_wait_us` no longer counts script execution time as queueing.** The histogram measured everything between dispatch and the response, so a server with an empty queue reported its own PHP latency as queue wait — the metric could not answer the question it is named for, and a rise in it was as likely to mean "the application got slower" as "requests are queueing". Execution time is now subtracted, leaving the wait for admission plus the wait in the queue. One note for existing dashboards: values on an idle server drop to near zero where they previously tracked script latency.
- **The server no longer leaks memory on every request.** Each request permanently retained its small per-request cancellation record, so resident memory grew in direct proportion to the number of requests served and never came back — about 128 bytes per request, with no plateau. Nothing about the request mattered: a 404, a static file, and an executed PHP script accumulated it identically, in every routing mode, and disabling rate limiting, access logging, and compression made no difference. A server under sustained load would eventually be OOM-killed — at a thousand requests per second, growth was on the order of 11 GB per day. The record is now released when the request completes, and resident memory settles at a steady level once the allocator has warmed up: the load run that previously climbed to 4.9 GiB over 40 million requests now holds below 50 MB across 31 million, flat from the first few million on. Cancellation of in-flight PHP on client disconnect is unchanged.
- **Worker mode: completing a request no longer cancels async promises owned by other in-flight requests on the same worker thread.** The per-request cleanup of non-awaited `oxphp_async()` promises used to drain the whole worker thread's promise table, so when request fibers were multiplexed on one thread, any request finishing while a sibling was suspended in `oxphp_async_await()` cancelled the sibling's still-running task and left its await to fail with a `TimeoutException` at the full deadline — even though the task itself had completed successfully. The drain could also stall the shared scheduler for up to 5 seconds per orphaned promise, freezing every other request on that thread. Promise ownership is now tracked per request fiber and each completing request cleans up only its own promises; the thread-wide drain remains only where it is correct (traditional-mode request shutdown and final worker teardown).
- **Worker mode: a request that finishes while its own fire-and-forget `oxphp_async()` task is still running no longer stalls the worker's scheduler.** Cleaning up a not-yet-settled background task's captured state used to block the worker thread for up to 5 seconds while the task wound down, freezing every other request multiplexed on that thread — no new requests accepted, sleeps and awaits paused. That cleanup is now deferred and reclaimed off the hot path once the task settles, so sibling requests are served without interruption. The blocking form remains only during worker shutdown, where the thread is exiting anyway.
- **Database auto-instrumentation now populates its span attributes — `OTEL_APM_SLOW_QUERY_MS` and `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` are no longer inert.** Both knobs were parsed and reported on the internal config endpoint but never read, and the PDO/mysqli auto-instrumentation hooks emitted only a bare timing span — so an operator who set a slow-query threshold or enabled parameter capture saw nothing in their tracing backend. Database spans now carry the OpenTelemetry semantic-convention attributes `db.statement` (literal values obfuscated to `?` so PII stays out of traces), `db.operation`, `db.system`, `server.address`, `server.port`, and `db.name`, parsed from the PDO DSN / mysqli constructor and attached per connection. A query (not a connection) whose wall-time meets or exceeds `OTEL_APM_SLOW_QUERY_MS` is flagged `oxphp.db.slow=true`, and bound parameters are recorded — raw, not obfuscated, so opt in only where PII in traces is acceptable — in `db.params` when `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED=true`. `db.statement` is read from each `query` / `prepare` call's own arguments (so it appears on that span) and, on a `PDOStatement::execute` span, from the statement object's own `queryString` property — read per object, so it can never be another statement's SQL. A `mysqli_stmt::execute` span carries no `db.statement` (mysqli exposes no such property; the SQL is on the `mysqli::prepare` span). Cache (Redis, Memcached), HTTP-client (cURL), and file-I/O hooks are unchanged (bare timing spans).

### Added

- **`RUNTIME_HOOKS=streams` makes blocking socket reads and `stream_select()` suspend the fiber instead of the worker thread.** With the new `streams` hook category enabled, a blocking read on a `tcp://` stream parks the current request or async-task fiber and lets the worker serve other requests until the answer arrives, then resumes it where it left off. It applies to everything built on PHP streams — `fsockopen()`, `stream_socket_client()`, HTTP stream wrappers, and database and cache clients that ride on them (mysqlnd, so PDO_MySQL and mysqli; phpredis) — without touching application code, so a worker no longer sits idle for the duration of every query or backend call. One configuration caveat comes with that: a MySQL DSN has to name `127.0.0.1`, because the client reads `localhost` as a request for a unix socket, which is not a `tcp://` stream. Like the existing `sleep` category it is off by default and inert outside a fiber, and it preserves the native contract: socket timeouts (`stream_set_timeout()`, `default_socket_timeout`) behave unchanged, a timed-out read still reports `timed_out` via `stream_get_meta_data()`, and stream identity is untouched so `socket_import_stream()` keeps working. `stream_select()` is covered too, which matters because it is how plain PHP waits on several sockets at once and no stream read op is involved: the hook waits for the descriptors the three arrays name and then hands the call to PHP with the timeout set to zero, so PHP still decides the return count, the rewriting of the arrays down to the ready streams, the warnings and the argument errors. It steps aside — running exactly as it would unhooked — for a read stream that already holds buffered data (which `stream_select()` answers from the buffer, never consulting a descriptor), for a stream with no descriptor at all, for a descriptor the kernel will not watch for readiness (a regular file, say), and for a descriptor at or past `FD_SETSIZE`, which PHP's own `select()` refuses outright; a busy worker really can hold more than 1024 descriptors, and such a call fails the same way with or without the hook. What the category does not cover: waiting for write readiness; ext/curl, which drives sockets itself below PHP streams, so `curl_exec()`, `curl_multi_*` and the HTTP clients built on them (Guzzle's default handler among them) are untouched; `unix://`, `udp://` and `udg://` streams; `socket_select()`, which is ext/sockets working on raw descriptors, and the wait inside `stream_socket_accept()`; streams produced by `socket_export_stream()`, which carry a different ops table; the connect and DNS-resolution phases; and `ssl://` / `tls://` once crypto is active (before `stream_socket_enable_crypto()` succeeds, an SSL stream's reads pass through the patched slot and do suspend). Write *waiting* is left alone deliberately — read readiness is stable, while room in the send buffer is granted and withdrawn by the peer, so a fiber woken on writability can find the window closed by the time the write runs and PHP blocks for its whole timeout anyway; a write is still looked at for one thing, which connection's exchange it belongs to, and an uncontended one runs the native path untouched. Under a userland fiber scheduler (AMPHP, Revolt) the hook also steps aside and uses the native path, since a fiber started by such a scheduler runs on a context OxPHP's scheduler cannot resume. **One connection shared between concurrent fibers is safe for the clients OxPHP guards, and gains nothing** — which matters because it is the normal shape of a worker-mode application rather than an edge case: WordPress, Laravel and Symfony open their database and cache clients once when the worker boots and hand the same ones to every request. A client protocol is a sequence of exchanges (write a command, read the answer) with nothing on the connection marking where one ends, so a fiber parked on a read is parked in the middle of one; a second fiber's command landing there breaks the protocol, and the two clients fail at it differently — mysqlnd refuses the command from its own connection state, before sending anything, while phpredis has no such check and the two fibers read each other's replies, which is one request's data returned to another with no error raised. A fiber therefore claims a connection before using it, at both levels this needs to happen at (the socket, and the client entry points of PDO and mysqli, since mysqlnd's refusal happens before any I/O and no socket-level guard can reach it), and another fiber reaching the same connection waits for it to be given up. What the client level claims is the connection itself rather than the PHP object holding it, so a persistent connection reached through several PDO objects counts as one, and a connection opened with `PDO::connect()` — which returns the driver's own subclass rather than a `PDO` — is covered like any other. Read the hook's gain, then, as belonging to connections a fiber opens for itself — an async task making its own calls, a request opening its own client: what a shared connection gets is the worker thread back while it waits, so other requests can run the work that is not on that connection, while its own exchanges run one after another exactly as they do with the hook off. Three boundaries worth knowing. A fiber that queried once keeps the connection to the end of its request, because the end of the request is the first moment certainly past the end of an exchange. Waiting is always bounded — by the smaller of `max_execution_time` and `default_socket_timeout`, which is 30 seconds where neither is set, since a server SAPI takes the engine's defaults of 30 and 60, and `default_socket_timeout`'s 60 only where `max_execution_time` is 0; the first is read as the request currently has it (that is what `set_time_limit()` is for) and the second as the process started with, so that a request narrowing the socket default for a call of its own cannot shorten the bound for every request after it on the same worker — and past the bound the call falls back on unguarded behaviour with the reason in the server log: for PDO and mysqli it is handed to the client, whose own refusal of a command issued mid-exchange is the error the application already handles, while for phpredis, which has no such refusal and would instead read someone else's reply, it raises a `RedisException` and sends nothing. A socket-level conflict has no bound of its own because it never waits: the operation fails at once the way a timeout does, so `stream_get_meta_data()` reports `timed_out`. Two fibers each holding what the other waits for therefore come apart on that bound rather than waiting on each other for good. Waiting happens at the client entry points, where nothing but the connection's identity is held; at the socket level a conflict is refused the way a socket timeout is refused, because a fiber suspended inside an operation on another fiber's stream would hold a pointer its owner may free — and phpredis is guarded at the client level too, method by method, for that reason. Some cases are deliberately left uncovered: a statement or result object kept across requests, which no claimed call precedes, so `PDOStatement::execute()` on a statement prepared in an earlier request behaves as it does with no claim at all; constructing a second handle on a persistent connection while another fiber is mid-exchange on it, since PDO checks that a pooled connection is alive before handing it over, that check fails mid-exchange, and PDO answers by dropping the connection; a protocol written by hand on a raw socket that writes a command, suspends on something else and reads the reply later, since the socket level only refuses while the holder is parked on the reply itself and the three clients above are covered there by their client-level claim, which hand-written code has no equivalent of; and anything reaching a connection outside a fiber altogether, such as a destructor run by the engine's cycle collector between requests. Socket timeouts keep their meaning, with one caveat: the deadline is examined once per scheduler tick, so a timeout fires no sooner than the next tick — at best 100 µs in worker mode and 1 ms in the async pool, and later under load. Enabling it: `RUNTIME_HOOKS=streams`, or a list such as `RUNTIME_HOOKS=sleep,streams`. Note that the boolean spellings `1`, `true` and `all` mean *every* category, so a deployment already carrying `RUNTIME_HOOKS=1` for the `sleep` hooks picks up the socket hooks — and the memory-protection change they perform on PHP's stream ops table — on upgrade, with no edit of its own. Name the categories explicitly to opt out of that. Cost, measured rather than asserted: about 3–5 µs per socket round trip on a worker with nothing else parked, about 5–6 µs with 64 fibers parked on descriptors, and about 7–11 µs with 200 — readiness is resolved through a set the kernel keeps between waits, so the figure tracks how many descriptors became ready rather than how many are waiting. An idle worker waits on those descriptors rather than sleeping a fixed interval and noticing readiness on its next tick — waiting blind cost about 2 ms per round trip instead. A wide `stream_select()` carries an overhead the narrow case does not, since every descriptor a call names is registered before the wait and removed after. With the waiting taken out of the measurement — every descriptor already readable, so only the overhead is left — that is about 56 µs per call against 9 µs unhooked at 64 descriptors and 130–150 µs against 16–21 µs at 200, roughly 0.65 µs per descriptor. It is a fixed cost per call rather than a slowdown of the same work: a `stream_select()` that actually waits dwarfs it, and what it buys is the worker thread, which the unhooked call holds for the whole wait. Two shapes are the exception and are better left unhooked — a many-descriptor call that almost never waits, and a request that is itself the event loop — while clients waiting on one connection (mysqlnd, phpredis, the HTTP stream wrappers) pay around a microsecond.
- `QUEUE_WAIT_TIMEOUT_MS` (default `1000`): how long a request may spend waiting for a PHP worker before it is rejected with 529. Waiting requests are admitted in arrival order as workers pick up the requests ahead of them. `0` rejects the moment the queue is full, which is how earlier versions behaved, and applies no deadline to requests already in the queue. The budget is a single deadline stamped on arrival and covers both waits a request can face — for a slot in the queue, and inside the queue for a worker — so it bounds the whole time a request spends not executing, though not the time its handler then takes. How many requests may wait at once is capped by `QUEUE_MAX_WAITING` (below), so a sustained overload can never consume every connection permit and leave the server unable to accept anything at all. Treat the budget as a latency budget, not a throughput knob: a request's body is already buffered by the time it reaches the queue, so raising the budget leaves a genuinely overloaded server holding proportionally more connections, and more request bodies, before it answers. Two cases deserve a shorter budget or `0`: applications that call back into the same server over HTTP (the inner call cannot be admitted until the outer one releases its worker, so the wait is spent before it starts), and deployments fronted by a load balancer whose own timeout is below the budget. Besides a free slot, two things end a wait early: the client going away, wherever the protocol reports a disconnect at all, so a balancer that times out and retries does not leave its abandoned attempts holding places in the waiting set; and the graceful-drain deadline, at which anything still waiting is answered 503 with the same retry window as the rest of the drain rather than losing its connection when the process exits.
- `QUEUE_MAX_WAITING` (default: initial workers × 128, capped at half of `MAX_CONNECTIONS` and never below 1; `0` = auto): how many requests may be parked waiting for a free PHP queue slot at the same time. Past the cap a request is rejected with 529 immediately instead of waiting, and the refusal is counted under `oxphp_admission_refused_total{reason="waiting_full"}`. Waiting is not free — a parked request holds its connection, its connection permit and its already-buffered request body for up to `QUEUE_WAIT_TIMEOUT_MS`, and OxPHP buffers the body before dispatch, so there is no server-side size limit applied at that point. An uncapped waiting set would let a sustained overload accumulate waiters until every connection permit was taken, at which point the server stops accepting and answers overload by not answering at all: a connect timeout instead of an error. The cap bounds the resources held, not the waits that will pay off: how many requests can *usefully* wait is roughly `workers × budget / handler latency`, which the server cannot know at startup, and the default is deliberately generous so that fast handlers — where the pool drains a deep backlog well inside the budget — are not refused by default. On a slow handler most of that default is waiters that will be refused a second later, so size it from your own latency; the configuration reference works the arithmetic through. It is deliberately not derived from `QUEUE_CAPACITY` — tying the two would refuse burst absorption exactly where the queue is shallow — and the `MAX_CONNECTIONS` / 2 ceiling reserves the other half of the connection budget for accepting and refusing. One caveat: both terms are sized in connections while the budget is spent by requests, and over HTTP/2 a single connection carries up to `H2_MAX_CONCURRENT_STREAMS` requests — set the value explicitly on an h2-heavy deployment.
- **`oxphp_admission_refused_total`**: counts requests answered without reaching a worker. Overload shedding previously left no server-side trace at all — it was visible only as a client-side status code, so a deployment could be turning away a third of its traffic with nothing in `/metrics` to say so. Refused requests are also excluded from `oxphp_queue_wait_us`, which otherwise reported the expired wait budget as if it were queueing latency and made shedding look like a slow pool. The counter carries a `reason` label — `queue_full`, `wait_timeout`, `waiting_full`, `shutting_down`, `pool_unavailable` — because the conditions call for different responses: a budget that expired says the pool is too slow for the deadline, a full waiting set says the server is holding all the connections it will hold, and a queue full with no budget says waiting is switched off. Only those three are overload and answer 529. `shutting_down` answers 503 with the same retry window as the rest of graceful drain, because a client told "overloaded" learns the wrong thing about an instance that is going away, and `pool_unavailable` — no worker thread left to hand the request to — answers 500. The metric is named for the mechanism rather than for overload precisely because of those two: a counter called "overloaded" would make an ordinary restart read as a traffic spike, so alert on the three overload reasons specifically. Note also that `oxphp_busy_workers` deliberately does **not** count requests still waiting for a queue slot — they occupy no worker, and counting them would run the gauge to thousands against a pool of a handful of threads. Those show up in `oxphp_pending_requests`, which covers every accepted request until it is answered.
- **Unhandled exceptions and fatal errors are now captured automatically on the request's root trace span.** When a request fails with an uncaught exception or a fatal error and returns a 5xx, OxPHP attaches an OpenTelemetry `exception` event (`exception.type`, `exception.message`, `exception.stacktrace`, plus the extensions `exception.file` and `exception.line`) to the root SERVER span — with no `#[OxPHP\Apm\Trace]` attribute and no `oxphp_apm_error()` call. This makes a 500 self-describing in the trace and lights up error backends that group by the exception event (e.g. New Relic Errors Inbox). It works across traditional, framework, SPA, and worker modes, including for classless fatals (a synthetic type, no stacktrace). There is a streaming boundary: once a response has committed its status to the wire — a streaming (`oxphp_stream_flush()`) response, or one that called `finish_request()` — a fatal thrown afterwards is logged only and is not added to the root span, since the request has already completed. This holds for both a committed 5xx and a committed 2xx. The message and stacktrace obey the existing `OTEL_APM_MESSAGE_MAX_BYTES` / `OTEL_APM_STACKTRACE_MAX_BYTES` caps. Note the boundary on the traditional request path (Traditional / Framework / SPA modes): applications that install `set_exception_handler()` and render their own error page (Laravel, Symfony, WordPress, …) consume the exception before it becomes uncaught, so automatic capture does not fire for them — record it explicitly with `oxphp_apm_error($e)` from the framework's error reporter. Worker mode is different: the worker runtime catches an exception escaping the `oxphp_worker()` closure directly (it does not route through `set_exception_handler()`), so capture fires there regardless.

### Changed

- **Breaking: in worker mode, an event loop can no longer be run from inside a request.** Revolt refuses to run its loop from within a fiber, and a worker-mode request is now a fiber, so `Revolt\EventLoop::run()` called from a request handler throws `Error: Can't call ...::run() within a fiber` where it previously ran. Everything else Revolt offers from inside a request is unaffected — `defer()`, `delay()`, `getSuspension()` and the rest register and return as before; only driving the loop is refused. The refusal is Revolt's own guard behaving as it does under any fiber-based server: the loop belongs outside the request, and inside one the OxPHP scheduler is already driving. Traditional, framework and SPA modes are untouched — their requests are not fibers, `Fiber::getCurrent()` is `null` there, and `run()` works exactly as before. An application that called `run()` per request should move that call to its worker bootstrap, or drop it and let the server drive.

- **Worker mode: suspending or resuming a request's own fiber from userland is now refused rather than silently breaking the request.** `Fiber::getCurrent()` returns a real fiber inside a request, which makes `Fiber::suspend()` reachable where it previously threw "Cannot suspend outside of a fiber", and makes `$fiber->resume()` / `$fiber->throw()` reachable to anything holding that object. The server drives that fiber: a userland suspend would park a request nothing will resume, and a userland resume would run it a second time while the scheduler still intends to. Both now throw `FiberError` at the point of the call — the suspend at the caller's `Fiber::suspend()`, the resume at the resumer — and the request itself continues undisturbed. Use `oxphp_sleep()` or await a promise to yield.

- **Worker mode: backtraces taken inside a request or a background task include one server frame.** Both now run inside a fiber the server starts, and the callable it starts appears in `debug_backtrace()` and in exception stack traces as `oxphp fiber loop`, below the application's own frames. The name contains a space so it cannot collide with, or be mistaken for, a PHP function; it is not callable and does not appear in `get_defined_functions()`. Code that renders a trace shows one more line; code that inspects `debug_backtrace()` by depth from the bottom should account for it.

- **Breaking: a malformed `MAX_CONNECTIONS` is now a startup error.** A value that is not a non-negative integer (e.g. `MAX_CONNECTIONS=500x`) previously fell back to `10000` silently. It now fails at `oxphp serve` startup and at `oxphp config --check`, naming the variable — the same treatment the `QUEUE_*` and `ASYNC_*` knobs already get, and warranted here because the default `QUEUE_MAX_WAITING` is capped at half of `MAX_CONNECTIONS`, so a typo reshaped admission as well as the connection limit. An exactly-empty value still means unset.
- **Breaking: `QUEUE_CAPACITY=0` now means auto (workers × 128), and a malformed value is a startup error.** `0` was taken literally and built a zero-capacity queue in which a request could only be handed over if a worker happened to be blocked waiting for one at that exact moment — never what an operator writing `0` intends, and inconsistent with `ASYNC_QUEUE_CAPACITY`, where `0` has always meant auto. A value that is not a non-negative integer (e.g. `QUEUE_CAPACITY=1k`) previously fell back to the default silently; it now fails at `oxphp serve` startup and at `oxphp config --check`, naming the variable. An exactly-empty value still means unset.
- **`oxphp_queue_wait_us` now has buckets reaching one second.** The boundaries stopped at 50 ms, while a request may wait up to `QUEUE_WAIT_TIMEOUT_MS` — a second by default — before it is refused, so every wait long enough to be worth acting on landed in `+Inf` together and the histogram could report that waits were long but not how long. The range and the budget now line up: a request that is served waited less than the budget by construction, so the top boundary is the point past which there is nothing left to resolve. Four boundaries are added (`100000`, `250000`, `500000`, `1000000`); the existing `le` series keep their meaning and existing queries keep working. The new tail matches `oxphp_request_duration_us`, so queue wait and total duration can now be read against each other bucket for bucket.
- **The default `FRAME_OPTIONS` is now `SAMEORIGIN` instead of `DENY`.** Out of the box OxPHP now allows a page to be framed by other pages on the same origin, while still blocking the cross-origin framing that clickjacking relies on — matching the common default of nginx, Rails, and other servers, and unbreaking legitimate same-origin embedding (admin previews, dashboard widgets, same-origin payment components) that `DENY` blocked. This is a slight relaxation of the default policy: a deployment that relied on the built-in `DENY` to forbid *all* framing, including same-origin, must now set `FRAME_OPTIONS=DENY` explicitly to keep that behavior. The emitted headers are unchanged per value (`SAMEORIGIN` → `X-Frame-Options: SAMEORIGIN` + `Content-Security-Policy: frame-ancestors 'self'`), and an application that sets its own `X-Frame-Options` or a CSP `frame-ancestors` directive still overrides the server default entirely. An invalid `FRAME_OPTIONS` value now falls back to `SAMEORIGIN` (the new default) rather than `DENY`.

## [0.10.0] - 2026-07-12

### Migration from 0.9.0

**`oxphp serve` and `oxphp run` now drop OS privileges to `www-data` by default.** Started as root on a host where the `www-data` account exists — as in the official image — the process binds its listeners as root and then permanently drops to `www-data` before serving any traffic or running any PHP, so the official image no longer serves as root out of the box. The default is best-effort and never aborts startup: started non-root it keeps the current user; started as root without a `www-data` account it logs a warning and stays root. To restore the previous always-root behavior pass `--user=root`; to drop to another account pass `--user=<name|uid[:gid]>`. Orchestrator-level drops (`docker run --user`, Compose `user:`, Kubernetes `securityContext`) are unaffected and take precedence — when the container already starts non-root, the self-drop is a no-op.

**Configuration is now validated fail-closed — several previously-silent misconfigurations abort startup.** Each of the following was silently tolerated before and is now a hard error at `oxphp serve` / `oxphp run` startup and at `oxphp config --check`, so audit your environment before upgrading: a half-configured TLS pair (`TLS_CERT` set without `TLS_KEY`, or vice versa, including when the other is empty); a non-UTF-8 `TLS_CERT` / `TLS_KEY` value; a `TLS_MIN_VERSION` in a foreign syntax (e.g. `TLSv1.2`) — it is now validated even when TLS is disabled, so a plain-HTTP deployment carrying a stale value must switch it to `1.2` / `1.3` or unset it; a malformed `ASYNC_WORKERS` / `ASYNC_QUEUE_CAPACITY` / `ASYNC_MAX_FIBERS` (e.g. `ASYNC_WORKERS=8x`, which previously collapsed to `0` and disabled the async pool); and an invalid `SUPERGLOBALS_ENABLED` passed to `oxphp run` (which previously fell back to defaults wholesale, silently re-enabling superglobals an operator had disabled). An exactly-empty value still means unset in every case.

**A `QUERY` request without a `Content-Type` header now returns `400`, not `415`.** This was the only server-generated `415`, so a custom error page keyed on it (`ERROR_PAGES_DIR/415.html`) must be renamed to `400.html`; a `415.html` now fires only for a `415` your PHP application returns.

**Profiler HTTP export resolves its wire envelope differently for pre-existing configs.** `PROFILER_EXPORT_XHGUI=false` now disables the xhgui envelope even when the export URL path ends in `/run/import` (previously that path forced the `{profile, meta}` wrapper on regardless of the flag), and envelope auto-detection now keys only on each tool's canonical endpoint **path** — xhgui on `/run/import`, Buggregator on `/api/profiler/store` — no longer on a fuzzy `xhgui` substring anywhere in the URL. If you point `PROFILER_EXPORT_URL` at a non-standard path, set `PROFILER_EXPORT_XHGUI` / `PROFILER_EXPORT_BUGGREGATOR` explicitly.

### Added

- **Buggregator profiler export.** The profiler's HTTP push can now wrap xhprof data in the envelope Buggregator's `POST /api/profiler/store` expects (`profile` + `app_name`, `tags`, `hostname`, `date`), so profiles land in a Buggregator instance with correct project grouping and tag filtering. `PROFILER_EXPORT_BUGGREGATOR` toggles it (tri-state, like `PROFILER_EXPORT_XHGUI`); when unset it auto-detects an export URL whose **path** ends in `/api/profiler/store` (a narrow suffix match, not an anywhere-substring, so an unrelated collector at a nested path is not wrapped — set `PROFILER_EXPORT_BUGGREGATOR=false` to push raw to such a URL). The Buggregator envelope always emits xhprof, so `PROFILER_EXPORT_FORMAT` is ignored for it — a non-xhprof value is warned at startup rather than fatal (an optional export's misconfiguration never crashes the server). `PROFILER_EXPORT_APP_NAME` sets the `app_name`, and `PROFILER_EXPORT_TAGS` sets `tags` from a `key=value,key2=value2` list (a malformed token, an empty key, or a duplicate key is a startup error). The Buggregator and xhgui envelopes are mutually exclusive — enabling both is a startup error. `hostname` is taken from `$HOSTNAME`, falling back to the `gethostname(2)` syscall when that shell variable is not exported to the process. Previously, pointing the export URL at Buggregator sent the bare xhprof map, which Buggregator accepted with a success status but stored as an empty profile.
- **Graceful drain of long-lived connections on shutdown.** On SIGTERM the server stops accepting, sends `GOAWAY` to HTTP/2 clients and closes idle HTTP/1.1 keep-alives, and ends every open long-lived stream (SSE and other flush loops) promptly and uncatchably — `register_shutdown_function()` callbacks still run and `error_get_last()['message']` reads `Request cancelled (shutdown)`; a `try/catch (\Throwable)` around the streaming loop cannot swallow it. Ordinary in-flight requests are left alone: they get the whole `DRAIN_TIMEOUT_SECONDS` window to finish normally, and only requests still running at the deadline — including CPU-bound ones that never flush or sleep — are cancelled, after which the process exits within ~2s. The `DRAIN_TIMEOUT_SECONDS` default is lowered from 30 to 25 so the full shutdown sequence — drain, ~2s unwind, telemetry flush — fits inside Kubernetes' default 30-second termination grace period. Previously idle keep-alive connections and open streams were held for the full drain timeout and then dropped mid-stream with no signal to the peer. Set the orchestrator's termination grace period above `DRAIN_TIMEOUT_SECONDS` + 2s.
- OTLP trace export now works over TLS. An `OTEL_EXPORTER_OTLP_ENDPOINT` with an `https://` scheme is exported encrypted on both the gRPC (`grpc`) and HTTP (`http/protobuf`) transports, with the collector certificate verified against the system trust store. Previously the default gRPC transport had no TLS support at all — an `https://` gRPC endpoint failed to connect — and the HTTP transport's TLS backend was non-deterministic. Both transports now use rustls with system (native) roots, keeping the build OpenSSL-free. Verification uses the OS trust store, so the runtime image must ship a CA bundle (e.g. `ca-certificates`) — the official image installs it. Custom CA bundles and mutual-TLS client certificates are not yet supported.
- **Full exception data on APM span events.** The `exception` span event now carries `exception.message` and `exception.stacktrace` alongside `exception.type`, matching the OpenTelemetry exception semantic conventions, so error inboxes (New Relic Errors Inbox, Jaeger, Grafana Tempo) render the message and stack without an in-application subscriber. This applies to both the automatic `#[OxPHP\Apm\Trace]` decorator and the manual `oxphp_apm_error($e)` SDK call — the latter previously accepted the exception argument but only flipped the span to error status without recording any of it; a bare string argument (`oxphp_apm_error('gateway timeout')`) is now recorded as `exception.message`. The message and stacktrace are captured length-delimited and decoded lossily, so a non-UTF-8 (e.g. latin1-from-a-database) or embedded-NUL message is preserved rather than dropped. The stacktrace is taken from the exception's `getTraceAsString()`. Both string attributes are size-capped so an exception wrapping a large payload cannot bloat the export unbounded: the message by `OTEL_APM_MESSAGE_MAX_BYTES` (default 4096, matching New Relic's per-attribute value limit) and the stacktrace by `OTEL_APM_STACKTRACE_MAX_BYTES` (default 8192); `0` disables either cap. Over the cap each is truncated from the tail (for the stacktrace the `#0` throw-site frame is preserved) with a `…(truncated)` marker. Argument values inside frames follow PHP's own `zend.exception_ignore_args` setting.
- `TLS_MIN_VERSION`: minimum accepted TLS protocol version, `1.2` (default — TLS 1.2 and 1.3 accepted, the previous behavior) or `1.3` (a TLS 1.2 ClientHello is rejected at the handshake). Any other value — including `1.0` and `1.1`, which the built-in TLS implementation does not support at all, and non-UTF-8 bytes from a corrupted env file — is a hard startup error rather than a silent fallback, so a mistyped security floor cannot quietly weaken the configuration; an empty value is treated as unset. **Breaking:** the value is validated at startup and by `oxphp config --check` even when TLS is not enabled, so a plain-HTTP deployment whose environment already carries a `TLS_MIN_VERSION` in a foreign syntax (e.g. `TLSv1.2`) — previously ignored — must switch it to `1.2`/`1.3` or unset it. The effective floor is reported as `tls_min_version` in the internal `/config` endpoint. Cipher suites remain non-configurable by design: the TLS stack ships only modern AEAD suites (AES-GCM, ChaCha20-Poly1305 with ECDHE), so there are no weak ciphers to disable.

### Changed

- The profiler HTTP export now resolves its wire envelope from `PROFILER_EXPORT_XHGUI` / `PROFILER_EXPORT_BUGGREGATOR` and the URL in one place, with two behavior changes from 0.9.0 for pre-existing export configs: (1) `PROFILER_EXPORT_XHGUI=false` now disables the xhgui envelope even when the `PROFILER_EXPORT_URL` path ends in `/run/import` — previously such a URL forced the `{profile, meta}` wrapper on regardless of the flag; (2) an export URL whose **path** ends in `/api/profiler/store` now auto-selects the Buggregator envelope, which always emits xhprof — a non-xhprof `PROFILER_EXPORT_FORMAT` is ignored for it (warned at startup, not fatal; the xhgui envelope is treated the same, so a mismatched format no longer silently ships an un-enveloped body a receiver can't parse). Envelope auto-detection now keys only on each tool's canonical endpoint **path** — xhgui on `/run/import`, Buggregator on `/api/profiler/store` — and no longer on a fuzzy `xhgui` substring anywhere in the URL (host, path, or query). This removes the ambiguity where the same URL could plausibly mean either envelope; point `PROFILER_EXPORT_URL` at a non-standard path and set `PROFILER_EXPORT_XHGUI` / `PROFILER_EXPORT_BUGGREGATOR` explicitly. Set `PROFILER_EXPORT_XHGUI` / `PROFILER_EXPORT_BUGGREGATOR` explicitly (`true`/`false`) to override auto-detection either way.
- **Breaking: `oxphp run` no longer silently ignores an invalid `SUPERGLOBALS_ENABLED`.** The one-shot CLI now parses only the configuration it actually consumes — `SUPERGLOBALS_ENABLED` and the `ASYNC_*` pool settings. Previously a parse error anywhere in the config made `run` fall back to built-in defaults wholesale, silently re-enabling superglobals an operator had disabled (folding the whole process environment, secrets included, into `$_SERVER`) and turning off the async pool; garbage in `SUPERGLOBALS_ENABLED` now prints the error and exits non-zero. Server-only variables (`ENTRY_FILE`, `TLS_*`, `TRUSTED_PROXIES`, …) are not read by `oxphp run` at all, so a job container inheriting a web deployment's env template cannot fail on configuration it never uses.
- **Breaking: a half-configured TLS pair now aborts startup.** `TLS_CERT` set without `TLS_KEY` (or vice versa — including when the other is empty) previously started the server in plain HTTP with no hint; a half-pair is almost always a typo'd variable name, and silently serving unencrypted traffic on a port meant for HTTPS fails open. `oxphp serve` now refuses to start, naming the missing variable, and `oxphp config --check` reports the same error before deployment. A startup warning (not an error) is logged when `TLS_MIN_VERSION=1.3` is set while TLS is not enabled — the floor is validated but has no effect.
- A non-UTF-8 `TLS_CERT` or `TLS_KEY` value is now a hard startup error instead of silently starting the server without TLS, and an exactly-empty value (`${TLS_CERT:-}`-style substitution) is treated as unset — previously two empty values crashed startup with a bare filesystem error from reading an empty path. A whitespace-only value is deliberately *not* treated as unset: it is never a valid path, and collapsing it would silently downgrade an intended-HTTPS port to plain HTTP — it fails loudly as an unreadable path instead. When both variables resolve to unset but are *present* in the environment (an empty-rendered substitution — e.g. a broken secret mount), the server logs a startup warning before serving plain HTTP, so the downgrade leaves a trace; genuinely absent variables stay silent.
- **Breaking: malformed `ASYNC_WORKERS`, `ASYNC_QUEUE_CAPACITY`, or `ASYNC_MAX_FIBERS` values are now startup errors** (in `oxphp serve`, `oxphp run`, and `oxphp config --check`) instead of silently falling back to defaults — `ASYNC_WORKERS=8x` previously collapsed to `0` and quietly disabled the async pool. An exactly-empty value still means unset; `ASYNC_MAX_FIBERS=0` keeps its meaning of "default cap".
- TLS startup errors now name the variable and the file: a typo'd `TLS_CERT` path fails with `TLS_CERT: cannot read /etc/ssl/cert.pem: No such file or directory` instead of a bare `No such file or directory (os error 2)`; PEM-parse and certificate/key-mismatch errors are prefixed the same way.
- **Breaking: `oxphp serve` and `oxphp run` now drop OS privileges to `www-data` by default.** When started as root on a host where the `www-data` account exists (as in the official image), the process binds its listeners as root and then permanently drops to `www-data` before serving any traffic or running any PHP — so the official image no longer serves as root out of the box. The default is best-effort and never aborts startup: started as non-root it keeps the current user, and started as root without a `www-data` account it logs a warning and continues as root. Previously the process kept running as whoever started it (root in the official image) unless `--user` was passed. To restore the old behavior pass `--user=root`; to drop to a different account pass `--user=<name|uid[:gid]>`. An explicit `--user` remains fail-fast (it must start as root). Orchestrator-level drops (`docker run --user`, Compose `user:`, Kubernetes `securityContext`) are unaffected and take precedence — when the container already starts non-root, the default self-drop is a no-op.
- `STATIC_REVALIDATE=on` now amortizes its filesystem check: a cached file's modification time is re-checked via `stat()` at most once every 3 seconds per file, instead of on every request. Within that window cached content is served straight from memory under a shared read lock with no syscall; the single combined cache lookup also resolves conditional (304) requests, so a served static hit performs at most one `stat()` (previously up to two). On-disk changes become visible within 3 seconds rather than immediately, in exchange for making the mode cheap enough to run without measurable per-request overhead. The default remains `off`.
- A `QUERY` request that arrives without a `Content-Type` header now returns `400 Bad Request` instead of `415 Unsupported Media Type`, matching RFC 10008 (the HTTP QUERY method): a request carrying no media-type information is malformed, and `415` is reserved for a `Content-Type` that is present but unsupported. This was the only server-generated `415`, so a custom error page keyed on `415` (`ERROR_PAGES_DIR/415.html`) for this case must be renamed to `400.html`; a `415.html` now fires only for a `415` your PHP application returns.

### Fixed

- Worker-mode per-request cancellation (client abort or request timeout) could target whichever request was most recently accepted on the worker instead of the one actually being cancelled, so a timeout or disconnect on one multiplexed request could abort an unrelated in-flight request sharing the same worker.
- Once any request on a worker called `oxphp_finish_request()`, every other stream multiplexed onto that worker silently dropped the output of its subsequent `oxphp_stream_flush()` calls.

## [0.9.0] - 2026-06-25

### Migration from 0.8.0

**A timed-out `await` now cancels the abandoned async task instead of letting it finish.** Previously a task whose `await` (or `await_all` / `await_any` / `await_race`) timed out kept running to the end of the request; its side effects — database writes, cache fills, outbound calls — would still complete in the background. The task is now force-cancelled at the timeout: one parked in cooperative sleep or suspended awaiting a child is resumed into cancellation, and a CPU-bound task is interrupted at the next opcode boundary. Code that relied on a timed-out task quietly running to completion must instead treat the timeout as a hard abort — move work that must always finish out of the awaited task, or give it a budget under the `await` timeout. Tasks still in flight when the request ends are likewise drained.

**`/config` no longer reports `internal_addr` or `error_pages_dir`.** These two keys were removed from the internal `/config` JSON because they leak deployment topology and a filesystem path without serving any metrics-scraping need. Tooling that read `internal_addr` or `error_pages_dir` from `/config` must source them another way (the process environment / your deployment manifest); the remaining keys are unchanged.

**A port-only `INTERNAL_ADDR` now binds loopback, not all interfaces.** `INTERNAL_ADDR=:9090` previously failed to resolve; it now binds `127.0.0.1:9090`. To expose the internal server off-host, set an explicit `INTERNAL_ADDR=0.0.0.0:9090` — and pair it with `INTERNAL_ALLOW_IPS` (new in this release), since the server now warns at startup when the internal listener is reachable off-host without an allow-list.

### Added

- **Async task composition (nested `oxphp_async`).** An async task may now itself call `oxphp_async()` and suspend on `await_all` / `await_any` / `await_race`, so a task can fan out child tasks and await them without blocking its worker thread — the previous "no nested async" restriction is removed. The async executor runs each task on a cooperative fiber (a C scheduler driven from Rust) instead of blocking a worker for the task's whole duration; JIT trace state (`jit_trace_num`, `vm_stack_page_size`) is saved and restored across fiber switches so JIT-compiled tasks resume correctly.
- `ASYNC_MAX_FIBERS` (default `256`): bounds concurrent async tasks. The process-global in-flight cap is `ASYNC_MAX_FIBERS × ASYNC_WORKERS` and limits queued + running tasks via a CAS-bounded counter; a dispatch past the cap is rejected immediately with `AsyncException` rather than blocking, so fan-out composition cannot deadlock waiting on a slot it will never get. Exposed in `/config` as `async_max_fibers` and `async_in_flight_cap`.
- Async task observability: the `oxphp_async_tasks_in_flight` and `oxphp_async_tasks_in_flight_limit` gauges, and the `oxphp_async_output_discarded_bytes_total` counter (output written by an async task — which has no client to receive it — is discarded at worker idle). The gauges render only when the async pool has wired its in-flight counter.
- `PROFILER_EXCLUDE_PATHS`: comma-separated glob patterns (same syntax as `PHP_DENY_PATHS`) whose matching request paths are kept out of `PROFILER_SAMPLE_RATE` sampling — so framework self-traffic such as Symfony's `/_profiler` and `/_wdt` toolbar requests no longer pollute the captured profiles. Exclusion applies to automatic sampling only: a request carrying an explicit trigger (`x-oxphp-profile` header, `OXPROF` cookie, or `__oxprof` query parameter) is still profiled, even on an excluded path. Unset or empty excludes nothing.
- `INTERNAL_ALLOW_IPS`: a CIDR allow-list for the internal server. A peer outside the list receives `403` on `/metrics`, `/config`, and other internal paths — before any handler runs, so it cannot probe which paths exist. Health endpoints (`/health`, `/healthz`, `/readyz`, `/startupz` and their long forms) are always reachable so orchestrator and load-balancer health checks never break. Unset/empty allows all peers (the prior behavior). Loopback is not implicit — list `127.0.0.1/32` to keep localhost access. A malformed list aborts startup. There is deliberately no bearer-token option: a token invites exposing the port "because it's protected"; the controls are network isolation plus this allow-list.

### Changed

- A timed-out `await` now cancels the abandoned async task instead of letting it run to request end. A task parked in cooperative sleep or suspended awaiting a child is force-resumed into cancellation, and a CPU-bound task is interrupted at an opcode boundary (best-effort, bounded by opcode-boundary latency); any tasks still in flight when the request ends are drained. The previous behavior — the abandoned task kept running until request end — is replaced.
- A port-only `INTERNAL_ADDR` (e.g. `:9090`) now binds `127.0.0.1` instead of failing to resolve; bind an explicit `0.0.0.0:9090` to expose the internal server off-host.
- `/config` no longer reports `internal_addr` or `error_pages_dir` — deployment topology and filesystem paths that aided an attacker and were not needed by metrics scrapers.
- The server warns at startup when the internal listener is reachable off-host (`0.0.0.0`, `::`, or a public address) and no `INTERNAL_ALLOW_IPS` is set; a private-interface bind logs an informational note instead, and the warning is suppressed once an allow-list is configured.

### Fixed

- A fiber send-waiter parked on a full `Shared\Channel` is now woken when `recv_blocking` frees a buffer slot. The slow path freed the slot but only signaled the non-fiber send notifier, which a parked fiber waiter does not observe — so a blocked sender stranded until some consumer happened to take the `tryRecv` fast path, making `sendTimeout` trip spuriously under saturation. Both open-channel branches now drain a send-waiter on slot free, mirroring `tryRecv` (the send-side counterpart of the already-fixed receive-side handoff gap).
- Fixed a graceful-shutdown crash by tearing down PHP on the main thread.
- A fiber `await` on a closed async promise now rejects instead of stalling.
- `await_all` cancels and strands the remaining promises when it bails out early, rather than leaving them running.

## [0.8.0] - 2026-06-13

### Migration from 0.7.0

**Worker mode no longer executes `.php` files from the document root per-request.** Previously the worker-mode router ran the full Traditional resolution chain with the worker as the last fallback: an existing `/about.php` was executed per-request instead of reaching the worker, `/blog/` resolved to `blog/index.php`, and — most surprisingly — a root `index.php` in the document root absorbed every unmatched request so the worker never saw them. The router now matches the documented contract (and the FrankenPHP / RoadRunner worker model): static assets are served from disk, and *every* other request — `.php` URIs, extensionless paths, directory indexes, `/` — is dispatched to the worker `ENTRY_FILE`. Deployments that relied on mixing per-request `.php` scripts with a worker in one document root should either move those scripts behind the worker's router or serve them from a separate non-worker instance.

**`PATH_INFO` in Framework mode no longer mirrors the full request URI.** In a `ENTRY_FILE=*.php` (front-controller) setup, `$_SERVER['PATH_INFO']` previously carried the entire original path for every request (`/users/42` → `PATH_INFO=/users/42`). It now follows CGI semantics: it is set only when the request explicitly names the entry file with a trailing segment (`/index.php/news` → `/news`); a bare application route carries no `PATH_INFO`. Front controllers that routed on `PATH_INFO` should read `$_SERVER['REQUEST_URI']` instead. For the same reason, `$_SERVER['PHP_SELF']` on an application route now reports the front controller (`/index.php`) rather than the full request path (`/users/42`) — apps that built form actions or routed on `PHP_SELF` should also switch to `REQUEST_URI`. This matches nginx + PHP-FPM. Conversely, classic `/index.php/route` applications (which expect the tail *after* `index.php`) now receive the correct value.

### Added

- **HTTP Range requests for static files (RFC 9110 §14).** Static responses now advertise `Accept-Ranges: bytes`; a GET with a single-range `Range` header (`bytes=N-M`, `bytes=N-`, `bytes=-N`) receives `206 Partial Content` with `Content-Range`, enabling `<video>`/`<audio>` seeking, resumable downloads (`curl -C -`, `wget -c`), and partial PDF loading. Unsatisfiable ranges return `416` with `Content-Range: bytes */<size>`. `If-Range` is honored with strong ETag comparison and exact-date `Last-Modified` comparison; the date form is only accepted once the file's modification second has elapsed, since a fresher `Last-Modified` is not yet a strong validator per RFC 9110. Multi-range requests fall back to the full `200` response (no `multipart/byteranges`), and ranges apply only to static files — never to PHP responses, matching nginx + FastCGI behavior. Ranges and compression are mutually exclusive, also matching nginx: `206` responses are never compressed, and for clients that accept brotli, range handling is disabled on representations that would be served compressed — a resumed download could otherwise splice unencoded bytes onto a compressed prefix. Only in-memory-cached files (up to 1 MiB) are ever compressed, so ranges always apply to non-compressible content (video, archives, images) and to every file streamed from disk. Because the response form for compression-eligible files (encoding, range handling, even 200 vs 416) depends on `Accept-Encoding`, those responses always carry `Vary: Accept-Encoding`, including when served uncompressed — otherwise a shared cache could store an identity response unkeyed and serve it to brotli clients.
- **Configurable HTTP/2 connection limits.** Five environment variables bound per-connection resource use; they are read once at startup and the resolved values are logged (`HTTP/2 limits`). `H2_MAX_CONCURRENT_STREAMS` caps simultaneously-open streams (default `max_workers × 4`, floored at 32 — sized to the blocking worker pool, since each open stream maps to a queued PHP request, so the cap also bounds single-connection queue amplification). `H2_MAX_PENDING_RESET` bounds pending reset streams (default 20). `H2_MAX_HEADER_LIST_BYTES` bounds total decoded header bytes per request (default 64 KiB). `H2_KEEPALIVE_INTERVAL_SECS` sets the PING keepalive interval (default 20; `0` disables keepalive). `H2_KEEPALIVE_TIMEOUT_SECS` sets the PONG deadline (default 10, clamped to ≥1 s so a `0` value can't make every PING fail immediately). Unparsable values fall back to the default rather than aborting startup.

### Changed

- Static-file ETags are now strong (`"<size>-<mtime_hex>"`) instead of weak (`W/"…"`). `If-Range` requires strong comparison per RFC 9110, and size + mtime fully identify a static file's bytes — the same reasoning behind nginx's strong static ETags. Responses the brotli layer actually compresses carry a weakened `W/"…"` tag instead, because the encoded body is a different representation — this prevents clients from resuming a compressed download with unencoded 206 fragments (nginx's gzip filter does the same). `If-None-Match` accordingly uses weak comparison, so both tag forms revalidate to 304.
- Static file serving takes size, mtime, and ETag from the opened file handle instead of a separate `stat()` on the path. Previously a deploy replacing a file between the two syscalls could pair the new file's bytes with the old file's validator and `Content-Length` — with Range support that race would have become silent data corruption in resumed downloads.
- In Framework mode, `$_SERVER['PATH_INFO']` is set only for an explicit `/index.php/extra` request (honest CGI path-info), not for every request. Application routes are exposed through `REQUEST_URI`; `SCRIPT_NAME` always identifies the executed front controller.
- Worker mode routing follows the documented "static or worker" contract: static assets are served from disk, everything else is dispatched to the worker `ENTRY_FILE`. Arbitrary `.php` files, directory indexes, and the root `index.php` fallback are no longer resolved per-request in worker mode (see Migration above).
- `PHP_DENY_PATHS` now applies in SPA mode. SPA executes existing `.php` files directly — the same uploaded-shell exposure as Traditional mode — but the deny-list was previously force-disabled whenever `ENTRY_FILE` was set. It remains ignored (with a startup warning) in Framework and Worker modes, where arbitrary `.php` files are never executed directly.
- In Framework mode a missing static asset now falls back to the front controller (`ENTRY_FILE`) with the original URI as `PATH_INFO`, instead of returning a hard 404. This matches the canonical `try_files $uri /index.php` nginx front-controller config and aligns Framework with Traditional mode, which already falls back on a static miss; the trade-off is that a request for a non-existent asset now costs a PHP dispatch. SPA mode keeps the hard 404 — a static shell has no PHP router to handle the missing path. A hard 404 is still returned when the front controller itself is absent.

### Fixed

- Server security headers no longer overwrite headers set by the application. `X-Content-Type-Options`, `X-Frame-Options`, and `Content-Security-Policy` sent from PHP (via `header()`) previously were silently replaced by the server defaults — a full application CSP collapsed to `frame-ancestors 'none'`, and an application `X-Frame-Options: SAMEORIGIN` was ignored. The server values are now fallbacks applied only when the response carries no such header (insert-if-absent, like Apache's `Header setifempty`). The two framing headers are treated as one policy: an application-set `X-Frame-Options` suppresses the server `Content-Security-Policy: frame-ancestors` fallback (CSP overrides `X-Frame-Options` in modern browsers, so adding it would override the application's framing policy), and symmetrically an application CSP containing a `frame-ancestors` directive suppresses the server `X-Frame-Options` fallback (which would over-block in legacy browsers that ignore CSP). The `FRAME_OPTIONS` default (`DENY`) is unchanged and still applies to responses that set nothing themselves (static files, error pages).
- Custom error pages (`ERROR_PAGES_DIR`) no longer drop semantically required response headers. The handler used to rebuild the error response from scratch, so a configured `416.html` erased `Content-Range: bytes */<size>` (which resuming clients need to correct their range), a `529.html` erased `Retry-After`, and a `405.html` would erase `Allow`. The custom page now replaces only the body, its Content-Type/Content-Length, and the headers coupled to the replaced body — Content-Encoding, ETag, Last-Modified — so an application error response compressed by `ob_gzhandler` can't label the plain HTML page as gzip, and the application's validators can't revalidate it in caches.
- Conditional requests (`If-None-Match` / `If-Modified-Since`) on static files produce 304 only for GET and HEAD, per RFC 9110 — other methods receive the full response instead of a bodyless 304.
- `$_SERVER['SCRIPT_NAME']` (and `DOCUMENT_URI` / `PHP_SELF`) are now correct in all routing modes. They are derived from the resolved script path relative to the document root instead of by subtracting the decoded `PATH_INFO` length from the raw percent-encoded URI. The old computation produced an **empty** `SCRIPT_NAME` in Framework mode and a mis-sliced one when the path contained percent-encoded characters (`/a.php/u%20ser` yielded `/a.php/u` rather than `/a.php`). `PATH_INFO` is likewise percent-decoded, consistent with PHP-FPM.
- `PHP_DENY_PATHS` covers scripts reached through directory-index resolution. A request to `/uploads/` that resolved to `uploads/index.php` previously bypassed the deny-list entirely, because only the request URI was matched — the resolved script path is now re-checked after routing.
- Streamed responses are no longer buffered by the compression layer. A `flush()`-streamed PHP response (`oxphp_stream_flush()`, Server-Sent Events) with a compressible content type and a brotli-capable client was silently collected in full before compression — destroying time-to-first-byte and holding the entire stream in memory until the script ended (the size cap was checked only after the collect). Responses whose length is unknown when headers are sent now pass through uncompressed, matching the documented invariant; buffered responses, which always report an exact size, are unaffected.

### Security

- **HTTP/2 hardened against denial-of-service amplification.** Several single-connection attack classes are now bounded by the limits above. HPACK indexed-reference bombs are capped by `H2_MAX_HEADER_LIST_BYTES`, which bounds total decoded header bytes per request. Rapid Reset (CVE-2023-44487) is bounded by `H2_MAX_PENDING_RESET`: a connection that floods `HEADERS`+`RST_STREAM` to churn server-side work without ever hitting the stream concurrency cap is closed with `GOAWAY ENHANCE_YOUR_CALM` while other connections keep being served. Window Stall — holding many streams open with a zero receive window to pin server memory — is bounded by `H2_MAX_CONCURRENT_STREAMS`, whose worker-aware default ties the cap to the backend's actual capacity. Dead and half-open connections are detected and closed via PING/PONG keepalive (`H2_KEEPALIVE_INTERVAL_SECS` / `H2_KEEPALIVE_TIMEOUT_SECS`), reclaiming stream slots an attacker could otherwise hold by going silent. The Rapid Reset defence was previously implicit in the `h2` crate default; it is now an explicit, operator-visible, and tunable value.

## [0.7.0] - 2026-06-05

### Migration from 0.6.0

One breaking change, in the JSON access log only — every PHP-facing API is unchanged.

**Access-log field `remote_addr` → `remote_ip`.** The client-IP field is renamed and now carries the IP without a port. Behind a configured `TRUSTED_PROXIES` it previously emitted a synthetic `IP:0` (the real source port belongs to the proxy, not the client), which broke parsers that validate the port and diverged from nginx/Apache/Caddy. Update any log pipeline that keys on the old name:

| Before | After |
| --- | --- |
| `{"remote_addr": "10.0.0.1:0", …}` | `{"remote_ip": "10.0.0.1", …}` |

The client/proxy source port stays available to PHP via `$_SERVER['REMOTE_PORT']`.

### Added

- **PHP-style CLI: `oxphp serve`, `oxphp run`, and implicit run.** `oxphp serve` is the explicit HTTP-server role; bare `oxphp` (and the published `CMD ["oxphp"]`) still starts the server unchanged, so nothing breaks. `oxphp run <script.php> [args…]` executes a single PHP file to completion under CLI semantics (`PHP_SAPI === 'cli'`, `phpinfo()` prints text) on the main thread with no listener or worker pool, exiting with the script's own exit code — the full OxPHP engine (fibers, `OxPHP\Shared\*`, engine plugins) is available underneath. `oxphp <script.php>` is shorthand for `oxphp run`, and a leading `#!` line is skipped, so an extensionless `#!/usr/bin/env oxphp` script runs directly. The role is chosen by keyword: the exact tokens `serve`/`run`/`config` select a subcommand, any other first positional is treated as a script path (resolved by file content, not extension). `-d key[=value]` ini overrides are accepted on the run path (repeatable; folded into the SAPI before module startup, so they beat `php.ini` for every directive type), and flags after the script path pass through to PHP as `$argv`. An unopenable script is rejected with `Could not open input file: <path>` (exit 1) before engine startup, matching `php`. `oxphp run` also honors `SUPERGLOBALS_ENABLED` instead of forcing superglobals on.
- **In-binary privilege drop via `--user`.** `oxphp serve --user=<name|name:group|uid|uid:gid>` (and the same flag on the `run` path) drops OS privileges before any request-handling thread is spawned — while the process is still single-threaded — in the fixed order `initgroups` → `setgid` → `setuid` → a `setuid(0)` re-acquisition probe → best-effort `PR_SET_NO_NEW_PRIVS`. `setuid` (not `seteuid`) drops the real, effective, and saved IDs irreversibly, and the probe aborts startup if root can be regained. Names resolve via `getpwnam_r`/`getgrnam_r` at CLI-parse time so an unknown user/group fails before startup; numeric uids are taken verbatim (Docker `--user` convention). Starting as non-root with `--user` is a hard error, never a silent no-op. This lets a container start as root to bind a privileged port and then serve traffic as an unprivileged user without an orchestrator-level drop.
- Behind a configured `TRUSTED_PROXIES`, `$_SERVER['SERVER_PORT']` (and the object-API `Request::port()`) now honors the `X-Forwarded-Port` header. Previously the public port was derived only from the port suffix of `X-Forwarded-Host` / `Forwarded: host=`, defaulting to 443/80 by scheme — so a proxy listening on a non-standard public port (e.g. AWS ALB on 8443, or nginx with `proxy_set_header X-Forwarded-Port $server_port;`) that sent a portless `X-Forwarded-Host` could not convey that port to PHP, and absolute-URL builders produced links to 443/80. Port resolution priority is now `X-Forwarded-Port` → port suffix of the forwarded/`Host` header → 443/80 by scheme. The header is read only from trusted peers, must be a single value in `1..=65535`, and is ignored when an RFC 7239 `Forwarded` header is present (consistent with `X-Forwarded-*` being ignored in that case).
- `$_SERVER['REMOTE_PORT']` is now populated from an RFC 7239 `Forwarded: for=ip:port` node when the trusted proxy supplies one. It remains `"0"` behind a proxy otherwise — `X-Forwarded-For` carries no source port, so the value is zeroed rather than guessed.

### Changed

- **Breaking (access log):** the JSON access-log field `remote_addr` is renamed to `remote_ip` and now carries the client IP **without** a port. Previously, behind a configured `TRUSTED_PROXIES`, the field showed a synthetic `IP:0` (e.g. `10.0.0.1:0`) — the real source port belongs to the proxy, not the client, so it was zeroed — which broke log parsers that validate the port as `> 0` and diverged from nginx/Apache/Caddy, whose `$remote_addr` is IP-only. Update any log pipeline that keys on `remote_addr` to read `remote_ip`. The client/proxy source port is unchanged in PHP and still available via `$_SERVER['REMOTE_PORT']`.
- The `Shared\Channel` operations metric now counts every `recv` attempt — an empty/closed `tryRecv` and a timed-out `recvTimeout` included, not only successful pops — matching the existing send-side accounting. Workloads that poll empty channels will see a higher recv op count; the counter is otherwise unchanged.

### Fixed

- Fixed two worker crashes (`SIGSEGV` in `zend_hash_index_find`) in the function-decorator path, both observed on amd64 and latent on arm64 (different allocator reuse). The per-thread decorator instance cache kept its bucket storage in the request memory arena but a persistent "initialized" flag, so once request shutdown freed the arena the next request dereferenced freed memory — the cache is now re-created every request. Separately, `before()`/`after()` held raw `zval*` pointers into that cache across PHP calls that could resize and reallocate it; each cached instance is now refcount-copied for the duration of its dispatch, so a resize can no longer dangle the pointer.
- The built-in profiler decorators `#[OxPHP\Profile\SlowThreshold(ms: N)]`, `#[OxPHP\Profile\MemoryThreshold(kb: N)]`, and `#[OxPHP\Profile\Mark(label: …)]` now honor their constructor arguments. Each was previously a single shared instance carrying a hardcoded default (`ms: 100` / `kb: 64` / no label), so the value written in the source was ignored and every occurrence behaved identically. Each attribute occurrence now builds its own configured instance, named arguments map by name even when written out of declaration order, and a repeated or dual (method + class) attribute resolves each occurrence independently instead of reading the first one twice.
- Span events recorded by `#[OxPHP\Apm\Trace]` (`Slow`, `MemorySpike`, `Mark`, and uncaught-exception events) are now exported through the APM OpenTelemetry exporter instead of being silently dropped, so they render natively in Jaeger / Tempo / Grafana. The event kind is preserved as the `oxphp.event.kind` span-event attribute.
- A `Shared\*` value returned from an `oxphp_async()` closure could be freed before the awaiting fiber materialized it, so `await` resolved a dead reference to `NULL`. The async return path now pins nested shared references into a keepalive before deep-freeing the return `zval` and releases it only after the fiber deserializes — mirroring the same in-transit fix already present on the channel path.
- Decorator nesting overflow now fails loudly. Past the nesting limit (raised 32 → 256) the per-thread context stack silently reused its top slot, corrupting outer frames' context during unwind; `begin()` now throws the new `OxPHP\Decorator\StackOverflowException` and `begin`/`end` stay balanced. The depth counter is reset per request so it cannot accumulate on a long-lived worker.
- `OxPHP\Shared\Map\KeyCursor::key()` is declared as `mixed` rather than the union `int|string`. The bridge's return-type wire format cannot encode a union, so the union degraded to "no return type" and tripped PHP's tentative-return-type deprecation against the `Iterator::key(): mixed` contract; `mixed` matches the interface (the stub keeps the precise `int|string` hint for IDEs and static analysis).
- `OxPHP\Http\Request::file()` and `Request::files()` now return the uploaded files instead of always `null` / `[]`. The object upload API documented in `docs/en/php/request-api.md` is wired to the request's parsed `$_FILES`: `file('avatar')` yields an `OxPHP\Http\UploadedFile` (or the first file of an array field `name="avatar[]"`, or `null` when the field is absent), `files('photos')` returns every file of one field, and `files()` returns a flat list of every upload. The scalar (`name="avatar"`), sequential-array (`name="avatar[]"`) and associative-array (`name="avatar[key]"`) `$_FILES` shapes are all handled. The `UploadedFile` accessors (`name()`, `clientType()`, content-detected `type()`, `size()`, `tmpPath()`, `error()`, `isValid()`, `moveTo()`) were already present; only the two `Request` entry points were stubbed, so code following the documented examples saw no files and had to fall back to the `$_FILES` superglobal.

## [0.6.0] - 2026-05-28

### Migration from 0.5.0

Mechanical replacements unless noted otherwise. Apply with grep/sed before upgrading.

**1. `oxphp_async_await_any()` — name kept, semantics replaced**

The function under that name now follows JS `Promise.any`: returns the first FULFILLED promise, throws `AggregateAsyncException` only when every promise rejects. The previous "first settled, success or failure" behavior moved to `oxphp_async_await_race()`.

```php
// If you wanted "first response, regardless of success":
$winner = oxphp_async_await_race([$p1, $p2], 5.0);

// If you wanted "first SUCCESS, ignore failures": same call, new failure shape:
try {
    $winner = oxphp_async_await_any([$p1, $p2], 5.0);
} catch (\OxPHP\Async\AggregateAsyncException $e) {
    // every promise rejected
} catch (\OxPHP\Async\TimeoutException $e) {
    // deadline elapsed before any fulfilled
}
```

**2. Cancelled-request HTTP status no longer collapses to `500`**

The wire status now reflects the cancel reason: `max_execution_time` exhaustion → `504`, graceful-drain shutdown → `503` with `Retry-After: 5`, mid-request client disconnect → `499` (log-only, not on the wire). Supervisor kills (`Stuck`) and userland cancels (`UserCancel`) keep `500`.

- Replace any monitoring/log pattern that maps `500` to "timeout" with `504` (or, more robustly, the `oxphp_request_cancelled_total{reason}` metric).
- If you ship a custom `ERROR_PAGES_DIR`, add `504.html`, `503.html`, and optionally `499.html` next to `500.html`.
- `5xx` rate SLOs will drop after rollout because `499` is no longer 5xx — this is honest improvement, not a regression.

**3. `Shared\Counter` — `inc`/`dec`/`addBatch`/`reset` removed**

`Counter::inc()`, `Counter::dec()`, `Counter::addBatch()`, and `Counter::reset()` were removed. `inc()`/`dec()` collapse into `add(int $delta = 1)` (`add()` adds 1, `add(-1)` decrements); `addBatch($deltas)` becomes `add(array_sum($deltas))`; `reset()` becomes `set(0)`, which — like the old `reset()` — returns the previous value. `get()`, `set()` (atomic exchange, returns the previous value), `compareAndSet()`, and `id()` keep their 0.5.0 signatures. The behavioural changes: `add()` gains a default delta of `1`, and every operation is now `Relaxed` rather than `SeqCst` — a Counter is a statistics accumulator, not a synchronisation point — use `Shared\Atomic` (with an explicit `Ordering`) when a counter must synchronise other memory or run a CAS that publishes other state.

```php
// Before (0.5.0)
$c->inc();
$c->inc(5);
$c->dec();
$c->addBatch([1, 2, 3]);
$prev = $c->reset();

// After
$c->add();
$c->add(5);
$c->add(-1);
$c->add(array_sum([1, 2, 3]));
$prev = $c->set(0);
```

**4. `OxPHP\Shared\*` — unified method naming**

| Before                          | After                       |
| ---                             | ---                         |
| `$ch->pending()`                | `$ch->count()`              |
| `$pool->size()`                 | `$pool->count()`            |
| `$flag->test()`                 | `$flag->isSet()`            |
| `$map->setIfAbsent($k, $v)`     | `$map->trySet($k, $v)`      |

`Map`, `Channel`, and `Pool` now also implement `\Countable`, so
`count($map)`, `count($ch)`, `count($pool)` work without calling the
method directly. The rationale and the rules for new methods live in
[`docs/en/shared-state/shared-naming.md`](docs/en/shared-state/shared-naming.md).

**5. `OxPHP\Server\Worker` — drop the `get` prefix**

| Before                       | After                     |
| ---                          | ---                       |
| `$w->getId()`                | `$w->id()`                |
| `$w->getStartTime()`         | `$w->startTime()`         |
| `$w->getRequestCount()`      | `$w->requestCount()`      |
| `$w->getMemoryUsage()`       | `$w->memoryUsage()`       |
| `$w->getRss()`               | `$w->rss()`               |
| `$w->getMaxMemoryBytes()`    | `$w->maxMemoryBytes()`    |
| `$w->getExitReason()`        | `$w->exitReason()`        |

`Worker::current()`, `Worker::isWorkerMode()`, `scheduleExit()`, `isExitScheduled()`, and `serve()` are unchanged.

**6. Base exception class renames**

```php
// Before
catch (\OxPHP\Async\Exception $e) { ... }
catch (\OxPHP\Shared\Exception $e) { ... }

// After
catch (\OxPHP\Async\AsyncException $e) { ... }
catch (\OxPHP\Shared\SharedException $e) { ... }
```

Subclasses (`TimeoutException`, `BorrowException`, `ClosedException`, …) keep their names — only the parent FQN changes.

**7. `oxphp_request_heartbeat()` → `set_time_limit()`**

```php
// Before
oxphp_request_heartbeat(30);

// After
set_time_limit(30);
```

Both reset the per-request timer to N seconds from now.

**8. `REQUEST_TIMEOUT_SECONDS` → `max_execution_time`**

```ini
; php.ini (or custom.ini)
max_execution_time = 30
```

Or per-script: `set_time_limit(30);`. Drop `REQUEST_TIMEOUT_SECONDS` from your deployment manifest.

**9. `sapi` key removed from `oxphp_server_info()`**

```php
// Before
$sapi = oxphp_server_info()['sapi'];  // hardcoded "oxphp", lied about the real SAPI

// After
$sapi = php_sapi_name();              // "cli-server"
```

**10. `Shared\Once` — `getOrInit()`, `status()`, and a failure policy**

`init()` is renamed to `getOrInit()`. `isInitialized(): bool` is removed in favour of `status(): Once\Status` (`Uninitialized`/`Pending`/`Ready`/`Poisoned`). `get()` now throws `UninitializedException` on an unset or in-flight cell instead of returning `null`, so a stored `null` is a real value distinguishable via `status()`. A failed `getOrInit()` factory is retryable by default; opt into permanent failure with `new Once(onFactoryError: Once\FailureMode::Poison)`, after which value methods throw `PoisonedException`. `trySet()` now accepts arrays and nested `Shareable` values, not only scalars.

```php
// Before
$o = new OxPHP\Shared\Once();
$v = $o->init(fn () => compute());
if ($o->isInitialized()) { $cached = $o->get(); }

// After
$o = new OxPHP\Shared\Once();
$v = $o->getOrInit(fn () => compute());
if ($o->status() === OxPHP\Shared\Once\Status::Ready) { $cached = $o->get(); }
```

**11. `Shared\Flag` — redesigned as the bool twin of `Shared\Atomic`**

`isSet()` / `set()` / `clear()` / `exchange()` are removed. Flag now mirrors `Shared\Atomic`: `load` / `store` / `swap` / `compareAndSet`, each taking an optional `Ordering` (default `SeqCst`). `swap` returns the previous value; `store` returns `void`.

```php
$f->isSet();               // → $f->load()
$f->set();                 // → $f->store(true)   (or $f->swap(true) for the prior value)
$f->clear();               // → $f->store(false)  (or $f->swap(false) for the prior value)
$f->exchange($new);        // → $f->swap($new)
$f->compareAndSet($e, $n); // unchanged (now also accepts optional $success / $failure Ordering)
```

### Breaking changes

- **`Shared\Channel` and `Shared\Mutex` adopt a trichotomous wait-policy API.** The single overloaded `?float $timeout` argument was replaced with three explicit methods per direction — `try*` for non-blocking, the bare verb for forever, and `*Timeout(int $ms)` for bounded — and the return shape moved from mixed/null/bool to value-typed Result classes (Channel) or exception-style (Mutex). No alias shims. Mechanical migration:

  | Was | Is |
  |---|---|
  | `$ch->trySend($v): bool` | `$ch->trySend($v): SendResult` (`isOk` / `isFull` / `isClosed`) |
  | `$ch->send($v, ?float $timeout = null)` (throws `TimeoutException` / `ClosedException`) | `$ch->send($v): SendResult` (forever) / `$ch->sendTimeout($v, int $ms): SendResult` |
  | `$ch->tryRecv(): mixed` (`null` on empty, throws on closed) | `$ch->tryRecv(): RecvResult` (`isOk` / `isEmpty` / `isClosed`; `value()` / `valueOr($d)`) |
  | `$ch->recv(?float $timeout = null): mixed` (`null` on timeout or closed) | `$ch->recv(): RecvResult` (forever) / `$ch->recvTimeout(int $ms): RecvResult` |
  | `$ch->sendMany($vs, ?float $timeout = null): int` (throws `TimeoutException` on partial) | `$ch->sendMany($vs, int $ms): int` (partial count, no throw on timeout/close) |
  | `$ch->recvMany($max, ?float $timeout = null): array` | `$ch->recvMany($max, int $ms): array` |
  | `$m->with($fn, ?float $timeout = null): mixed` | `$m->withLock($fn): mixed` / `$m->withLockTimeout($fn, int $ms): mixed` |
  | `$m->tryWith($fn): mixed` (`null` on contention) | `$m->tryWithLock($fn): mixed` (throws `ContentionException`) |
  | `$m->isPoisoned()`, `$m->clearPoison()` | removed; PHP throws no longer corrupt the mutex |

  Timeout parameters on the `*Timeout` methods are `int $ms (> 0)` in milliseconds, not `?float $seconds` — zero, negative, non-int, and absent values raise `OxPHP\Shared\TypeException` (use `try*` or the bare verb for those policies). The Channel `RecvResult` `value()` accessor throws `OxPHP\Shared\SharedException` if called on a non-Ok variant; use `isOk()` / `valueOr()` / `status()` to dispatch. The Mutex closure signature changed from `function ($value): mixed` (return-to-commit) to `function (&$value): mixed` (by-ref mutation; the return value becomes the caller's return value). `Shared\TimeoutException` is removed — `OperationTimeoutException` (now under `Async\AsyncException`) replaces it for `withLockTimeout` and the Pool-saturated path; `Shared\ClosedException` remains registered but is deprecated and only thrown by the still-unmigrated `Shared\Pool`; `Shared\PoisonedException` is now a first-class part of the redesigned `Shared\Once` (its `Poison` failure mode) and is no longer deprecated. `Shared\DeadlockException` is reparented from `Shared\TimeoutException` to `Async\AsyncException`, so a single `catch (Async\AsyncException)` now sweeps every concurrency outcome across Shared\* and Async\*.
- `Shared\Counter` reshaped to a minimal accumulator: `inc()`, `dec()`, `addBatch()`, and `reset()` were removed in favour of `add(int $delta = 1)` (covers increment and decrement) and `set(0)` (windowed reset, returns the previous value). `get()`, `set()`, `compareAndSet()`, and `id()` are retained with their 0.5.0 signatures; `add()` gains a default delta of `1`. All operations switched from `SeqCst` to `Relaxed` — a Counter is statistics, not a synchronisation point; use `Shared\Atomic` (with an explicit `Ordering`) to synchronise other memory, run an ordered CAS, or store arbitrary atomic int state.
- `oxphp_async_await_any(array, ?float): array` was renamed to `oxphp_async_await_race(array, ?float): array`. The implementation is unchanged — first settled (success or failure) wins, as before. If your code relied on this behavior, replace the function name in-place.
- `OxPHP\Shared\*` method naming unified across types. The renames below are mechanical (semantics and signatures unchanged), and ship without alias shims — update call sites with sed before upgrading. The rules are documented at [`docs/en/shared-state/shared-naming.md`](docs/en/shared-state/shared-naming.md).
  - `Channel::pending()` → `Channel::count()`
  - `Pool::size()` → `Pool::count()`
  - `Flag::test()` → `Flag::isSet()`
  - `Map::setIfAbsent($key, $value)` → `Map::trySet($key, $value)`

### Added

- `OxPHP\Shared\Channel\RecvResult` and `OxPHP\Shared\Channel\SendResult` — value-typed returns for the new Channel API. `RecvResult` accessors: `isOk`, `isEmpty`, `isTimeout`, `isClosed`, `value` (throws `SharedException` on non-Ok), `valueOr($default)`, `status(): RecvStatus`. `SendResult` is payload-free: `isOk`, `isFull`, `isTimeout`, `isClosed`, `status(): SendStatus`. Closed / full / timeout are normal outcomes for fan-out dispatchers, so they appear as result variants instead of exceptions on the hot path.
- `OxPHP\Shared\Channel\RecvStatus` and `OxPHP\Shared\Channel\SendStatus` — unbacked enums for exhaustive `match` dispatch on the Result discriminant.
- `OxPHP\Shared\OperationTimeoutException` (extends `OxPHP\Async\AsyncException`) — thrown by `Mutex::withLockTimeout` and `Pool::acquire` on deadline expiry. Cross-plugin parent makes a single `catch (Async\AsyncException)` sweep both Shared\* timeouts and Async\* await timeouts.
- `OxPHP\Shared\ContentionException` (extends `OxPHP\Async\AsyncException`) — thrown by `Mutex::tryWithLock` when the lock is held.
- `OxPHP\Shared\CorruptedMutexException` (extends `OxPHP\Shared\SharedException`) — thrown on every subsequent `Mutex::withLock*` call after a prior Rust panic crossed the FFI boundary inside the closure. Sticky, non-recoverable — discard the instance and create a new one.
- `Shared\Atomic` — generic int64 atomic primitive. Methods: `load`, `store`, `swap`, `compareAndSet`, `fetchAdd`, `fetchSub`, `fetchAnd`, `fetchOr`, `fetchXor`. Each accepts an optional `Shared\Ordering` parameter (default `Ordering::SeqCst`). `fetch*` returns the previous value (Rust convention), in deliberate contrast to `Counter::add` which returns the new value.
- `Shared\Ordering` — backed-int enum with `Relaxed`, `Acquire`, `Release`, `AcqRel`, `SeqCst`. Maps one-to-one to `std::sync::atomic::Ordering`.
- `Shared\InvalidOrderingException` (extends `Shared\SharedException`) — thrown when an `Atomic` operation receives a memory ordering invalid for that operation (e.g. `store(Ordering::Acquire)`, `compareAndSet(_, _, _, Ordering::Release)`).
- `oxphp_async_await_any(array, ?float): array` now exists with proper JavaScript `Promise.any`-style semantics: the first FULFILLED promise wins. Rejections are accumulated. If every promise rejects, throws the new `OxPHP\Async\AggregateAsyncException` carrying all errors (`getErrors()`, `getErrorMap()`, `getPromiseIds()`). On timeout, throws `OxPHP\Async\TimeoutException` with `getPartialErrors()` and `getCancelledPromiseIds()` populated.
- `OxPHP\Async\AggregateAsyncException` (extends `AsyncException`) — new exception class. Methods: `getErrors(): list<\Throwable>` (positional, keyed 0..N-1 by input position), `getErrorMap(): array<int, \Throwable>` (keyed by promise id), `getPromiseIds(): list<int>`.
- `OxPHP\Async\TimeoutException::getPartialErrors(): array<int, \Throwable>` and `getCancelledPromiseIds(): list<int>` — new methods. Existing throw sites (`oxphp_async_await()`, `oxphp_async_await_all()`, `oxphp_async_await_race()`) populate them with empty arrays; only `oxphp_async_await_any()` timeouts fill them. The cancelled-id list is an audit trail — those promises have already been signalled to cancel and their receivers stranded, so they cannot be re-awaited.
- `OxPHP\Shared\Map`, `OxPHP\Shared\Channel`, and `OxPHP\Shared\Pool` now `implements \Countable`. `count($map)`, `count($channel)`, and `count($pool)` work directly without calling the `->count()` method. For `Pool` the count covers total live slots (in-use + idle).
- Naming guide for `OxPHP\Shared\*` published at `docs/en/shared-state/shared-naming.md`. New `Shared\*` primitives must follow the rules listed there (`get`/`load` for reads, `set`/`store` for writes, `count()` via `\Countable`, `is*` for boolean getters, `try*` for non-blocking attempts, `fetch*` for atomic RMW returning prev value).
- `OxPHP\Shared\Once\Status` (unbacked enum: `Uninitialized`, `Pending`, `Ready`, `Poisoned`) and `OxPHP\Shared\Once\FailureMode` (backed-int enum: `Reset = 0`, `Poison = 1`). `Once::status(): Once\Status` reports the cell's state and never throws; `Once::getOrInit(callable): mixed` is the canonical race-free get-or-init (it replaces `init()`). `Once::__construct` takes `Once\FailureMode $onFactoryError = Reset` to choose retryable-vs-terminal factory-failure behaviour.
- `OxPHP\Shared\Registry` — name-keyed process-global handles for every `Shared\*` type. `Registry::map($key, $factory)`, `Registry::counter($key, $factory)`, etc. (one method per type, plus an untyped `Registry::global($key, $factory)` escape hatch) bind a `Shared\*` entry under a string key so every worker thread and every request that reaches the same key converges on the same entry. The factory runs at most once per successful bind (block-losers across worker threads; reentrancy from inside its own factory throws `Shared\DeadlockException`). Named entries are pinned for process lifetime; `Registry::remove($key): bool` drops the binding (the underlying object survives while any handle holds it, and the next typed call under the same key creates a NEW entry — documented namespace-management semantics). `Registry::keys(): array` lists currently-bound keys. `Registry::memoryUsage(): int` and `Registry::count(): int` report the whole Shared\* layer (estimate, not RSS; transient — `count() != count(keys())`). Closes the gap where the `new Shared\*()` bootstrap pattern produced per-worker instances rather than one shared entry; in traditional mode it also gives same-host APCu-style cross-request persistence.

### Removed

- `OxPHP\Shared\TimeoutException` class. The exception was a `SharedException` sibling thrown by `Channel::send`/`sendMany`, `Mutex::with`/`withLock` timed variants, and `Pool::acquire`. The new replacement is `OxPHP\Shared\OperationTimeoutException` (now under `OxPHP\Async\AsyncException`); Channel's `*Timeout` methods return `RecvResult::Timeout` / `SendResult::Timeout` instead of throwing. `catch (OxPHP\Shared\TimeoutException)` clauses must be updated — there is no `class_alias` shim.
- `Shared\Mutex::isPoisoned()`, `Shared\Mutex::clearPoison()`. Public poison observability and recovery were removed: the underlying behaviour they exposed never gave the caller useful work to do (corruption is now sticky and always a server bug; PHP throws no longer corrupt the lock at all). Catch `OxPHP\Shared\CorruptedMutexException` and discard the instance instead.
- `Shared\Mutex::with($fn, ?float $timeout)` and `Shared\Mutex::tryWith($fn)`. Use `withLock` / `withLockTimeout($fn, int $ms)` / `tryWithLock` instead.
- `Shared\Channel::send($v, ?float $timeout)`, `Channel::recv(?float $timeout)`, `Channel::sendMany(..., ?float $timeout)`, `Channel::recvMany(..., ?float $timeout)`. The float-seconds timeout parameter is gone everywhere on Channel. Use `sendTimeout($v, int $ms)` / `recvTimeout(int $ms)` for bounded waits, and pass the bare verbs (`send($v)` / `recv()`) for forever. The batch methods now take a mandatory `int $ms (> 0)` and return partial results without throwing on timeout or mid-batch close.
- `REQUEST_TIMEOUT_SECONDS` env var. Use `max_execution_time` in `php.ini` (or `set_time_limit($seconds)` per script) instead.
- `oxphp_request_heartbeat($time)` PHP function. Use `set_time_limit($seconds)` instead — both reset the per-request timer to N seconds from now.
- `oxphp_bridge_set_deadline` / `_get_deadline` / `_is_deadline_expired` C exports from the bridge.
- `tokio::time::timeout` wrapping of the dispatch future. SIGALRM-driven `max_execution_time` is now the single execution-timeout source.
- `Shared\Once::init()` (renamed to `getOrInit()`) and `Shared\Once::isInitialized()` (replaced by `status(): Once\Status`). No alias shims — update call sites before upgrading.
- **BREAKING:** `sapi` key from the array returned by `oxphp_server_info()`. The key used to hardcode `"oxphp"`, contradicting `php_sapi_name()` which reports `"cli-server"` (the real SAPI module name, kept that way for OPcache compatibility). Callers reading `$info['sapi']` will now get `null`. Use `php_sapi_name()` to get the SAPI identifier directly.

### Changed

- Execution-timeout cancellation now bails through the unified `Request cancelled (timeout)` error instead of `Maximum execution time of N second(s) exceeded`. Userland-visible state (`connection_status() & PHP_CONNECTION_TIMEOUT`, registered shutdown handlers) is preserved.
- **BREAKING:** Cancelled requests no longer collapse to a single `500`. The wire status now reflects the cause: `max_execution_time` / `set_time_limit()` exhaustion → **`504 Gateway Timeout`**; graceful-drain cancellation → **`503 Service Unavailable`** with a `Retry-After: 5` header (userland-set `Retry-After` wins); client closed the connection mid-request → **`499`** (nginx-style "Client Closed Request", visible only in access logs and metrics — never written to the wire); supervisor-initiated kills (`Stuck`) and userland-initiated cancels (`UserCancel`) keep returning `500`. Anything that pattern-matched `500` to detect timeouts must switch to `504` (or, more robustly, the `oxphp_request_cancelled_total{reason}` metric). Operators with a custom `ERROR_PAGES_DIR` should add `504.html`, `503.html`, and optionally `499.html` next to their existing `500.html`. `ClientAbort` moving out of `5xx` will improve generic `5xx`-rate SLOs after rollout — this is honest improvement (these were never server errors), but called out so the SLO drop isn't mistaken for a regression.
- **BREAKING:** `OxPHP\Server\Worker` instance methods dropped the `get` prefix to match the rest of the public PHP API (which uses noun-style accessors like `Request::method()`, `Request::headers()`). Renames: `getId()` → `id()`, `getStartTime()` → `startTime()`, `getRequestCount()` → `requestCount()`, `getMemoryUsage()` → `memoryUsage()`, `getRss()` → `rss()`, `getMaxMemoryBytes()` → `maxMemoryBytes()`, `getExitReason()` → `exitReason()`. No `__call` shim — call sites must be updated. `Worker::current()`, `Worker::isWorkerMode()`, `scheduleExit()`, `isExitScheduled()`, and `serve()` are unchanged.
- **BREAKING:** Renamed base exception classes to remove shadowing of PHP's global `\Exception` inside the `OxPHP\Async\` and `OxPHP\Shared\` namespaces: `OxPHP\Async\Exception` → `OxPHP\Async\AsyncException`, `OxPHP\Shared\Exception` → `OxPHP\Shared\SharedException`. Subclasses (`TimeoutException`, `BorrowException`, `ClosedException`, etc.) keep their names; only their parent FQN changes. No `class_alias` shim — any `catch (\OxPHP\Async\Exception $e)` or `catch (\OxPHP\Shared\Exception $e)` clauses must be updated to the new names.
- `OxPHP\Shared\*` timeout convention unified. Every wait method now takes `?float $timeout = null`: `null` waits forever, `0.0` is a non-blocking try, positive values are seconds, `INF` is forever, `NaN` and negative values raise `OxPHP\Shared\TypeException`. `Mutex::__construct` no longer accepts `$defaultTimeout`. `Pool::__construct` no longer accepts `$defaultAcquireTimeout`; pass the timeout at the call site (`acquire()` / `with()`). `Channel::tryRecv()` no longer accepts an argument and is non-blocking, matching `trySend()` (this only fixes the stub — the implementation never accepted the argument). Blocking methods on Mutex, Pool, and `Channel::send` / `sendMany` raise `TimeoutException` on deadline expiry; `Channel::recv` and `recvMany` instead return `null` / a partial array on timeout, intentionally asymmetric with send. Pool's `idleTimeout` lifecycle parameter is unchanged.
- Repository Dockerfile layout reorganized to separate "how the official image is built" from "how to use the image in your project":
  - `Dockerfile` → `docker/dev/Dockerfile` (used by `compose.yml`).
  - `Dockerfile.alpine-release` → `docker/release/alpine/Dockerfile` (used by CI to publish `ghcr.io/oxphp/oxphp`). The `alpine/` subdirectory leaves room for future `docker/release/debian/`, `docker/release/distroless/` variants.
  - `Dockerfile.best.example` → `examples/dockerfile/Dockerfile` (copy-and-adapt template for downstream users; also adds a sibling `README.md`).
  No `Dockerfile*` remains in the repo root, so a stray `docker build .` no longer accidentally kicks off the dev build. Update any tooling that referenced the old paths.
- `ox_shared.metrics_enabled` is now an actual runtime opt-out for per-entry operation counters on `Shared\*` primitives. Previously the flag was inert on the `record_op` path — per-entry counters incremented regardless of the setting, and only the registry's coarse aggregate metrics responded to it. Now, when `metrics_enabled = false`, `Entry::ops` stays at `0` and introspection snapshots (`OxPHP\Shared\introspect()`, `oxphp_shared_*` debug exports) report `0` for per-entry op counts. Operators who were reading per-entry `ops` values while running with `metrics_enabled = false` will see `0` instead of the previously-incrementing approximate count; switch the flag back to `true` (the default) to restore the prior behaviour.
- `Shared\*` memory accounting now books per-entry storage-chain overhead (`Arc<Entry>`, DashMap shard bucket, allocator prologues — ~200 B per entry) and propagates container growth (`Map::set`, `Channel::send`, `Pool::try_reserve_budget`) into the registry's `total_bytes` gauge. Previously `mem_bytes()` for scalar types (`Atomic`, `Counter`, `Flag`) reported only the inner content (~8–16 B) and container types froze the value at insert time — operators who relied on `OX_SHARED_MAX_BYTES` as a hard cap saw real RSS exceed the configured limit by ~12× for scalar-heavy workloads and arbitrarily for Map/Channel growth. **Operator action**: a worker that previously sustained ~6M `Shared\Atomic` entries under `OX_SHARED_MAX_BYTES=128MiB` will now top out around ~600K. Either raise the cap to match the previous structural budget (e.g. `≈1.6GiB`) or rely on the orchestrator-level memory limit (cgroups / k8s `resources.limits.memory`) and treat `OX_SHARED_MAX_BYTES` as a grace cap. The accounted bytes still drift ±10–30% vs `mallinfo` — the constant is a structural estimate of the storage chain, not a heap-profiler measurement.
- `OxPHP\Shared\*::id()` is now seeded from `getrandom` at registry start, so the value returned by `$shared->id()` is a large opaque number instead of the previous `1, 2, 3, …` monotonic sequence. The id remains stable for the lifetime of the entry within the process, the documented `$a->id() === $b->id()` identity test is unchanged, and the value continues to address the `/__ox_shared/preview?id=…` and `/__ox_shared/entries/:id` observability endpoints. **Operator action**: code that *parses* an id (regex-matched it, range-checked `< N`, treated it as an insertion-order proxy, or persisted it outside this process expecting it to resolve elsewhere) will need to stop — the id is a per-process opaque token, not a stable handle. The wire format on tag-7 cross-thread transfer is unchanged (`u64`). On the rare path where `getrandom` is blocked (seccomp/sandbox), the registry falls back to the legacy monotonic counter and logs a `WARN`.
- **BREAKING:** `Shared\Once::get()` now throws on a cell that is not `Ready` — `UninitializedException` when empty or while a factory is in flight (`Pending`), `PoisonedException` when a `Poison`-mode factory previously failed — instead of returning `null`. A stored `null` is therefore a real value, distinguishable from "not set" via `status()`. `trySet()` now accepts the full value range (arrays and nested `Shareable`), not just scalars, and throws `PoisonedException` on a poisoned cell. Factory-failure behaviour is selected at construction: `Reset` (default) returns the cell to `Uninitialized` so a later `getOrInit()` retries, `Poison` makes it terminally `Poisoned`; in both modes the factory's exception is re-thrown to the current caller. The per-instance observability JSON at `/__ox_shared/entry?id=N` now reports `status` (`uninitialized`/`pending`/`ready`/`poisoned`) instead of inferring a boolean `initialized` from a non-null snapshot, fixing a mislabel of cells storing `null`.

### Deprecated

- `PHP_DENY_DIRS` env var renamed to `PHP_DENY_PATHS` to reflect that values are glob patterns and may match individual `.php` files, not only directories. The legacy name remains accepted as an alias and emits a startup `WARN`; when both are set, `PHP_DENY_PATHS` wins and `PHP_DENY_DIRS` is reported as ignored. The alias will be removed in a future release — switch to `PHP_DENY_PATHS` in your environment and orchestration configs.
- `SHARED_SHUTDOWN_TIMEOUT_SECONDS` env var (and its `OX_SHARED_SHUTDOWN_TIMEOUT_SECONDS` alias) is deprecated and ignored. The setting never gated anything: `SharedRegistry::drain()` is synchronous — `Shared\Channel` and `Shared\Pool` wake blocked waiters via `close()` and return immediately; `Map`, `Mutex`, `Counter`, `Flag`, `Atomic`, and `Once` never block. The overall graceful-shutdown deadline is owned at server level by `DRAIN_TIMEOUT_SECONDS` (default `30s`), which waits on the connection-drain loop in `main.rs` long enough for woken PHP requests to unwind and flush. The `SharedConfig::shutdown_timeout_seconds` field is removed in this release; the env-var aliases are still accepted (with a startup `WARN`) for one release cycle and will be removed afterwards. Tune `DRAIN_TIMEOUT_SECONDS` instead.
- `OxPHP\Shared\*` observability names trailing the renamed PHP API are emitted as deprecated aliases alongside the new names: Prometheus `oxphp_shared_channel_pending` (use `oxphp_shared_channel_count`) and `oxphp_shared_pool_size` (use `oxphp_shared_pool_count`); JSON keys `Channel.pending` and `Pool.size` at `/__ox_shared/entries/:id` (use `.count`). The deprecated `# HELP` lines are tagged so dashboards picking the metric up via help-text discovery surface the migration hint. A startup `WARN` from the `ox_shared` plugin announces the dual emission whenever introspection or metrics are enabled. The deprecated aliases will be removed in a future release — update Grafana panels, Prometheus alert rules, and any JSON consumers before upgrading. **Scrape sizing note:** during the deprecation window each `Shared\Channel` and `Shared\Pool` emits one extra gauge line (`_pending` plus `_count`, `_size` plus `_count`) carrying the same value as its canonical counterpart, so the contribution of these series to `/metrics` doubles for the duration. The extra cardinality is `1 × N_channels + 1 × N_pools` and disappears when the aliases are removed.

### Performance

- Reduced per-call overhead of `Shared\*` primitive operations: the PHP wrapper now holds the registry entry directly, so the global shared-map lookup is gone from every call. Earlier in this cycle the per-call lookup count was halved (two → one) when the per-entry op counter stopped re-resolving the entry; this change removes the remaining one. The optimisation is unconditional and applies whether `ox_shared.metrics_enabled` is on or off. On a 14-core development host the per-op hot path is now within criterion noise of a raw atomic load — at 8 threads the geomean ratio between the previous and the new shape is approximately 4.7×, with the largest wins on contended read-only ops (`Atomic::load`, `Flag::isSet` — renamed from `test` later in this cycle, `Once::status` — replacing `isInitialized` later in this cycle); the improvement is expected to be larger on 32–64-core hosts where the DashMap shard lock dominated.

### Fixed

- `Request::startTime(true)` and `oxphp_server_info()['request_time']` now agree across all SAPI modes and lifecycle phases. Both return `0.0` when no HTTP request is being processed — during worker boot (top-level code in the entry script before `oxphp_worker()` enters its receive loop) and between requests in worker mode — and the request start timestamp during request handling. Previously the worker-mode field leaked the worker thread's spawn time during boot and the previous request's timestamp between requests, while traditional mode left it set to the last finished request after `php_request_shutdown`. Code that reads either API outside an active request (boot-phase initialization, async callbacks running between requests) will now observe `0.0` instead of a misleading non-zero value. OPcache and other RSHUTDOWN consumers of `sapi_get_request_time()` still see a valid timestamp because the field is reseated to the current wall-clock time immediately before the worker's final `php_request_shutdown`.
- SSE / streaming: `connection_aborted()` now correctly returns `true` after the client disconnects mid-stream, matching standard PHP / php-fpm semantics. Previously the flag stayed `false` for streaming responses, so portable loops like `while (!connection_aborted()) { echo ...; flush(); }` could only terminate via implicit bailout instead of breaking out cleanly through their `finally` blocks. Mid-stream disconnects are now also detected on the next flush via the streaming channel — previously only the early-response oneshot was probed, which had already been consumed when streaming started, so disconnect detection was effectively disabled for the lifetime of the stream.
- `OTEL_TRACES_SAMPLER_ARG` invalid or out-of-range values are now clamped to `[0.0, 1.0]` and logged at warn level, per the OpenTelemetry specification. Previously, parse errors silently fell back to `1.0` and out-of-range values (e.g. `2.5`, `-1`) were passed through to the SDK unchecked. A typo such as `OTEL_TRACES_SAMPLER_ARG=o.1` (letter `o` for `0`) now surfaces a warning instead of silently turning 10 % sampling into 100 %.
- Unknown `OTEL_TRACES_SAMPLER` values now emit a warn log identifying the offending value, instead of silently defaulting to `parentbased_traceidratio`. The fallback sampler is unchanged.

## [0.5.0] - 2026-05-05

Headline work since `v0.4.0`: a **canonical entry-script + worker-mode model** (`ENTRY_FILE` / `WORKER_MODE_ENABLED` retiring `INDEX_FILE` / `WORKER_FILE`), a new `OxPHP\Server\Worker` class for runtime introspection and application-driven recycling via `Worker::scheduleExit()`, strict parsing of boolean and `STATIC_MAX_AGE` env vars, and a clearer `STATIC_MAX_AGE` / `STATIC_REVALIDATE` rename for the static-file cache.

### Added

- New PHP class `OxPHP\Server\Worker` — unified runtime handle for worker introspection. Methods: `current`, `isWorkerMode`, `getId`, `getStartTime`, `getRequestCount` (1-based count of requests handled by this OS thread; grows in both modes since traditional reuses persistent threads), `getMemoryUsage`, `getRss`, `getMaxMemoryBytes`, `scheduleExit`, `isExitScheduled`, `getExitReason`, `serve`. Available in both traditional and worker modes. See `docs/en/php/worker-class.md`.
- New PHP exception `OxPHP\Server\Exception\InvalidServeContextException`, thrown by `Worker::serve()` when called outside worker mode.
- `Worker::scheduleExit()` — application-driven worker recycling. Marks the current worker for graceful exit after the active request completes; the supervisor respawns a fresh worker, re-running the outer scope. Companion methods `Worker::isExitScheduled()` and `Worker::getExitReason()` expose the pending exit state. No-op in traditional mode.
- Environment variables `ENTRY_FILE` and `WORKER_MODE_ENABLED` — single canonical entry script plus an explicit worker-mode toggle. `ENTRY_FILE` selects the routing mode by extension (unset = direct mapping, `*.php` = front controller, non-`.php` = SPA fallback). When `WORKER_MODE_ENABLED=true`, `ENTRY_FILE` must point at a `.php` script and the server runs persistent workers. Resolution accepts relative paths (against `DOCUMENT_ROOT`, including `..`) and absolute paths. The startup `mode_decided` log line records which combination was selected. See `docs/en/operations/configuration.md`.

### Changed

- `oxphp_worker_recycles_by_reason_total{reason="max_requests"}` Prometheus label is renamed to `reason="scheduled"` to reflect that the recycle reason is now driven by `Worker::scheduleExit()` instead of an automatic request counter.
- `/config` endpoint now reports `entry_file` and `worker_mode_enabled` in place of `index_file`, `worker_file`, and the synthetic `worker_mode` boolean.
- Static file cache environment variables renamed for clarity: `STATIC_CACHE_TTL` → `STATIC_MAX_AGE` (the value is the `Cache-Control: max-age` it sets), and `STATIC_CACHE` → `STATIC_REVALIDATE` with the polarity flipped (`STATIC_REVALIDATE=on` enables mtime revalidation; previously `STATIC_CACHE=off` did the same thing). Defaults are unchanged: 30 days `max-age`, no revalidation. `/config` reports `static_max_age` and `static_revalidate` in place of `static_cache_ttl` and `static_cache_enabled`.
- **BREAKING:** `STATIC_MAX_AGE` (and the deprecated `STATIC_CACHE_TTL`) are now strictly parsed: garbage values like `STATIC_MAX_AGE=garbage` fail at startup with an error naming the variable, where they previously silently fell back to a missing `Cache-Control` header. Empty assignments (`STATIC_MAX_AGE=`) and unset variables still fall back to the default (30 days), matching the bool-parser policy.
- **BREAKING:** boolean environment variables are now strictly parsed against a fixed canonical set (`on`/`true`/`1`/`yes` for truthy, `off`/`false`/`0`/`no` for falsy, case-insensitive and trimmed). Any non-empty value outside that set — including typos like `ture` — fails fast at startup with an error naming the variable, rather than silently defaulting. An unset variable or empty assignment (`FOO=`) falls back to the default; this matches Docker Compose / Kubernetes substitution like `FOO=${FOO}` when the host variable is missing. Affected variables: `WORKER_MODE_ENABLED`, `STATIC_REVALIDATE`, `TRACE_CONTEXT`, `SUPERGLOBALS_ENABLED`, `SHARED_ENABLED`, `SHARED_METRICS_ENABLED`, `SHARED_INTROSPECTION_ENABLED`, `SHARED_INTROSPECTION_PREVIEW_ENABLED`, `SHARED_POISON_STRICT`, `PROFILER_ENABLED`, `PROFILER_INTERNAL`, `PROFILER_EXPORT_XHGUI`. The legacy `STATIC_CACHE` compatibility shim remains intentionally lenient (only `off` enables revalidation, anything else disables). Audit any deployment that relied on non-canonical bool values like `enabled` — these now refuse to start.

### Deprecated

- Environment variable `WORKER_MAX_REQUESTS` — parsed and ignored; emits a `WARN` log line at startup if set. Migrate to `WORKER_MAX_MEMORY_MIB` for safety-net recycling, or to `Worker::scheduleExit()` for application-driven recycling. Will be removed entirely in a subsequent release.
- Environment variables `INDEX_FILE` and `WORKER_FILE` — still parsed for backwards compatibility; emit a `WARN` log line at startup and map onto the new model: `INDEX_FILE=...` ≡ `ENTRY_FILE=...`, and `WORKER_FILE=...` ≡ `WORKER_MODE_ENABLED=true ENTRY_FILE=...`. When both legacy and new variables are set, the new ones win and the warning still fires. The legacy forms will be removed in a subsequent release.
- Environment variables `STATIC_CACHE_TTL` and `STATIC_CACHE` — still parsed for backwards compatibility; emit a `WARN` log line at startup and map onto the new model: `STATIC_CACHE_TTL=...` ≡ `STATIC_MAX_AGE=...`, and `STATIC_CACHE=off` ≡ `STATIC_REVALIDATE=on`. When both legacy and new variables are set, the new ones win and the warning still fires. The legacy forms will be removed in a subsequent release.

### Internal

- New benchmark tooling under `scripts/`: `bench-wrk.sh` (one-shot wrk runner against a configurable target) and `sweep-config.sh` (matrix sweep over `TOKIO_WORKERS` × `PHP_WORKERS` for tuning). Not wired into CI; local-only.
- Bump dependencies for the post-`0.4.0` cycle: `opentelemetry` / `opentelemetry_sdk` / `opentelemetry-otlp` / `opentelemetry-semantic-conventions` `0.27 → 0.31`, `tonic` `0.12 → 0.14` (now requires the explicit `grpc-tonic` feature on `opentelemetry-otlp`), `rand` `0.8 → 0.10`, `getrandom` `0.2 → 0.4`, `reqwest` `0.12 → 0.13`, `brotli` `7 → 8`, `lru` `0.12 → 0.18`. No user-visible behavior change; OTel migration switches to `SdkTracerProvider` with `with_batch_exporter`, `Resource::builder()`, and the new `force_flush()` `Result` shape, and `rand`/`getrandom` call sites move to the `Rng::random` / `getrandom::fill` APIs.

## [0.4.0] - 2026-05-02

Headline work since `v0.3.0`: **PHP 8.5 support** (opt-in via `:php8.5*` tags, `latest` still resolves to 8.4 pending soak), and a chain of fiber / `Shared\*` / streaming bugfixes that surfaced once worker-mode `Channel`/`Map`/`Pool` traffic and `oxphp_async()` workers got real exercise.

### Added

- PHP 8.5 support. Build pipeline now produces `:php8.5`, `:php8.5-alpine{X.Y}`, and patch-pinned `:0.X.Y-php8.5.Z-alpine{X.Y}` tags alongside the existing 8.4 ones. `latest` and unsuffixed `{ver}` continue to resolve to PHP 8.4 in this release; the default flip to 8.5 lands in a follow-up release after a soak window. To opt in early, pull `:php8.5` (or any `*-php8.5*` variant).
- `SAPI_HEADER_DELETE_PREFIX` support (PHP 8.5.6+) — `header_remove('X-Foo-')` now strips every previously-set header whose `"{name}: {value}"` line case-insensitively starts with the given prefix, matching upstream SAPI behavior. PHP < 8.5.6 keeps the existing exact-match semantics.

### Fixed

- Streaming responses on the traditional executor losing chunked `Transfer-Encoding` after the first `oxphp_stream_flush` on a worker. The bridge per-request context (`stream_mode` / `headers_sent` / `finished`) is `__thread`-local and was leaking across requests; the traditional path now resets it before each request to match worker mode.
- `oxphp_bridge_in_fiber()` misreporting fiber context — both on the main thread (where PHP seeds `EG(current_fiber_context)` to `EG(main_fiber_context)` at request startup) and inside a user-level `Fiber::start()` body (which installs its own `zend_fiber_context` distinct from main). The latter caused `Shared\Channel::recv()` / `send()` inside a user Fiber to take the fiber-suspend path, hit rc=1 from `oxphp_bridge_fiber_await` ("not in oxphp fiber"), and surface as `RuntimeException: recv: fiber_await rc=1`. The predicate now keys off the SAPI's private `oxphp_current_fiber` __thread pointer via a registered callback (`oxphp_bridge_set_in_fiber_check`), which is the only authoritative source for "is this thread inside an oxphp scheduler fiber". User fibers correctly fall through to the thread-blocking branch.
- `DELETE /__profiler/runs/{id}` panicking the worker with "cannot block from within a runtime". The internal HTTP server dispatches sync handlers from inside hyper's async service, so the index-lock acquire switched from `blocking_lock()` to a `try_lock` retry loop with a 5 s deadline that degrades to `503` on real contention.
- Plugin-defined classes, interfaces, enums, and free functions advertised no parameter names, so PHP named-argument syntax (e.g. `$ch->send('x', timeout: 0.1)`) failed with "Unknown named parameter" and `ReflectionParameter::getName()` returned an empty list. The bridge method/function registration now carries per-parameter name/type/optional arrays, and the SAPI extension synthesizes a full `zend_internal_arg_info` array with real names instead of an unnamed return-only stub.
- `OxPHP\Shared\Channel`, `Shared\Map`, and `Shared\Pool` handles arriving as `null` on the receiving worker thread when captured by an `oxphp_async()` closure (via `use ($var)` or as a variadic arg). The cross-thread wrapper rebuild in the C bridge listed only `Counter`/`Flag`/`Once`/`Mutex` in its tag→class switch, so the other three `SharedType` variants fell through to `default` and produced `IS_NULL`. The mapping now lives only in Rust (`SharedType::php_class_cstr`) and the C bridge calls a weak-linked `oxphp_shared_class_name(type_tag)` export, so adding a `SharedType` variant cannot drift again. Re-enables the `shared/test_channel_fiber_*` worker-mode suites.

### Internal

- `src/php/bindings.rs` split into `src/php/bindings/{common.rs, v8_4.rs, v8_5.rs}` with cfg-selected per-version modules. `build.rs` detects the linked PHP via `php-config --vernum` (or `PHP_VERSION_ID` env override).
- `release.yml` gains a `php-suite` pre-publish gate running a focused subset of `tests/run_all.sh` (`headers, cookies, get_post_request, input, pathinfo, errors`) against both 8.4 and 8.5 images before manifest creation. Failure aborts the publish entirely.
- `weekly-rebuild.yml` gains skip-if-unchanged via upstream digest annotation comparison plus the same focused subset suite job, gated on whether any matrix cell actually rebuilt.

## [0.3.0] - 2026-04-22

Headline work since `v0.2.0`: **shared state for PHP workers without Redis** (seven `OxPHP\Shared\*` primitives), a per-request **PHP profiler**, **APM auto-instrumentation**, trusted-proxy and Kubernetes integrations for production deployments, security-header hardening, and a cosign-signed parametrized Docker image matrix.

### Added

#### `OxPHP\Shared\*` primitives

See the [Shared State overview](docs/en/shared-state/shared-state.md) for the concept and mental model, and the per-type docs for API reference, runnable examples, and gotchas.

- [`Shared\Counter`](docs/en/shared-state/shared-counter.md) — atomic int64 with `inc` / `dec` / `add` / `compareAndSet` / `addBatch` / `reset`.
- [`Shared\Flag`](docs/en/shared-state/shared-flag.md) — atomic bool with `test` / `set` / `clear` / `exchange` / `compareAndSet`.
- [`Shared\Once`](docs/en/shared-state/shared-once.md) — run-once container with `init(factory)` / `trySet` / `get`. Reentrant `init` throws `DeadlockException`.
- [`Shared\Mutex`](docs/en/shared-state/shared-mutex.md) — poisoning mutex guarding a stored value. `with(callable, timeout)` and `tryWith(callable)` scope-guard the critical section; poisoning isolates failed-mid-update state.
- [`Shared\Channel`](docs/en/shared-state/shared-channel.md) — bounded MPMC queue with fiber-aware `send` / `recv`. `sendMany` / `recvMany` for batching.
- [`Shared\Map`](docs/en/shared-state/shared-map.md) — concurrent `string → mixed` store with `get` / `set` / `update` / `getOrSet` / `setIfAbsent` / batched `setMany` / `getMany` / `removeMany`. Per-instance cap via `maxEntries`.
- [`Shared\Pool`](docs/en/shared-state/shared-pool.md) — bounded object pool with lazy factory, optional destroy callback, strict `maxSize` budget, per-thread affinity, and idle-timeout eviction. `with($body)` scope-guards acquire/release.

#### Shared-registry observability

See [Shared Observability](docs/en/shared-state/shared-observability.md) for the operator's reference.

- Internal-server endpoints: `/__ox_shared/summary`, `/entries`, `/entry?id=…`, `/preview?id=…`, `/types`, `/graph?id=…` for live registry introspection.
- Prometheus metrics under `oxphp_shared_*` — aggregate-per-type (`objects_total`, `operations_total`, `bytes`, `capacity_saturation`) plus per-instance for Channel / Map / Pool.
- Cross-thread deadlock detector — `oxphp_shared_deadlock_detected_total` ticks when the wait-for scanner finds a mutex cycle.
- Shared `preview` previews are gated behind `SHARED_INTROSPECTION_PREVIEW_ENABLED` so production deployments can disable value exposure without losing shape counts.

#### Profiling

- Per-request PHP profiler (`plugin-profiler` feature — now part of the default Cargo feature set; `-DOXPHP_WITH_PROFILER=1` propagated to both C build stages) with four output formats: xhprof, speedscope, pprof, collapsed.
- PHP SDK: `OxPHP\Profile\{start, stop, pause, resume, mark, metric, is_active}` functions.
- Seven PHP attributes: `#[Profile]`, `#[Exclude]`, `#[Sample]`, `#[Tag]`, `#[Mark]`, `#[SlowThreshold]`, `#[MemoryThreshold]`.
- Trigger modes: cookie (`OXPROF=<token>`), header (`X-OxPHP-Profile: <token>`), query (`?__oxprof=<token>`), and statistical (`PROFILER_SAMPLE_RATE`).
- In-memory LRU cache (`PROFILER_RETENTION_COUNT`) + disk retention with background trimmer (5-second cadence, atomic rename).
- Token-bucket disk write rate limiting (`PROFILER_DISK_MAX_PER_SEC`).
- HTTP push (`PROFILER_EXPORT_URL`) with 3× exponential backoff retry, 5 s wallclock cap, bearer-token auth, xhgui envelope auto-detect.
- Internal HTTP routes at `/__profiler/` — list, metadata, raw format download, speedscope redirect, DELETE, config, stats — with optional bearer-token auth and path-traversal revalidation.
- Prometheus metrics: `oxphp_profiler_runs_total{source}`, `spans_collected_total`, `bytes_written_total{format}`, `disk_drops_total`, `http_push_failures_total`, `truncated_total`, `in_memory_runs`.
- `xhgui` Docker test profile demonstrating the full push → mongo → xhgui UI flow.
- Per-locale documentation at `docs/{en,ru,zh}/features/profiling.md`.

#### APM & tracing

- APM plugin (`plugin-apm`) with auto-instrumentation, PHP tracing SDK, and error capture.
- Plugin PHP builder API for registering Rust-backed PHP functions and classes from plugins. Async and APM subsystems migrated from the C extension into Rust plugins.
- Return-type support in the C builder API for plugin methods.

#### HTTP & routing

- **Trusted proxy support** via `TRUSTED_PROXIES` — accepts trusted-proxy CIDR list (or `private`), processes RFC 7239 `Forwarded` and `X-Forwarded-*` headers, and overrides `REMOTE_ADDR`, `HTTPS`, `REQUEST_SCHEME`, `SERVER_NAME`, `SERVER_PORT` for PHP using the rightmost-non-trusted algorithm.
- **Kubernetes health probes** at `/readyz` and `/livez` with graceful-shutdown awareness.
- `PATH_INFO` splitting via `SPLIT_PATH_INFO_ENABLED` — nginx/PHP-FPM-style front-controller routing.
- `PHP_DENY_DIRS` env var to block `.php` execution in specified paths.
- Dot-path access blocked by default, with an RFC 8615 `.well-known` exception.

#### Security headers

- `X-Content-Type-Options: nosniff` on all responses.
- Configurable `X-Frame-Options` (`FRAME_OPTIONS`, default `SAMEORIGIN`) for clickjacking protection.

#### Operations

- CLI argument parsing: `--help`, `--version`, `--config --check`.
- Startup errors emitted as structured JSON logs (previously plain text).
- Docker `HEALTHCHECK` wired into `compose.yml`.

#### Supply chain & packaging

- Parametrized Docker image matrix: two Dockerfiles (dev + `Dockerfile.alpine-release`) sharing `ARG PHP_VERSION` / `ARG ALPINE_VERSION` / `ARG BASE_IMAGE`.
- Canonical minor-floating (`{ver}-php{minor}-alpine{alpine}`) and patch-pinned tags published to `ghcr.io/oxphp/oxphp`, plus aliases (`php{minor}`, `latest`, etc.).
- cosign-signed release images via GitHub OIDC.
- Weekly rebuild workflow re-publishes canonical tags with fresh upstream PHP patches and re-signs.
- Prod image now ships `php` CLI, `docker-php-ext-install`, `phpize`, and `www-data` out of the box (was bare alpine in 0.2.0).

#### Testing

- PHP integration test suite — 186 tests across 21 groups and 12 Docker profiles, covering apm, async, errors, framework, pathinfo, ratelimit, TLS, timeout, worker and more.

#### Configuration

All Shared-state tunables are read at startup via the `SHARED_*` env-var prefix (fallbacks to `OX_SHARED_*` and bare keys). See [Shared State → Configuration](docs/en/shared-state/shared-state.md#configuration) for the full table. Highlights:

- `SHARED_MAX_ENTRIES` (default 100 000) / `SHARED_MAX_BYTES` (default 1 GiB) — global caps.
- `SHARED_CYCLE_DETECT_DEPTH` (16) / `SHARED_CYCLE_DETECT_EDGES` (10 000) — cycle-check walker bounds.
- `SHARED_INTROSPECTION_ENABLED` / `SHARED_METRICS_ENABLED` — per-feature kill switches.
- `SHARED_LOCK_DIAGNOSTICS` (`off` / `warn` / `strict`) — escalates reentry / deadlock signals.

#### Rust plugin-author API

- `MapInner::retain<F>` — exposes `DashMap::retain` with proper refcount release for nested `SharedValue::Shared` targets. Lets plugin authors prune a map in a single shard-walk instead of the N-lock `keys()`+`remove()` pattern.

#### Documentation

- [`docs/en/shared-state/shared-state.md`](docs/en/shared-state/shared-state.md) — overview, mental model, type-selection matrix, canonical hand-rolled-counter → `Shared\*` migration example.
- Per-type docs for all seven Shared\* v1 types (see list above).
- [`docs/en/shared-state/shared-observability.md`](docs/en/shared-state/shared-observability.md) — introspection endpoints, Prometheus catalogue, diagnostic playbooks.
- [`docs/en/shared-state/migrating-to-external-store.md`](docs/en/shared-state/migrating-to-external-store.md) — when and how to promote `Shared\*` state to Redis / NATS / Kafka.

#### Tooling

- `tests/soak/pool_soak.sh` + `tests/soak/workload.php` — manual (non-CI) 24h soak harness for pre-release Shared\Pool stability sign-off. Not wired into `tests/run_all.sh`; [invocation notes in the observability doc](docs/en/shared-state/shared-observability.md#long-running-soak-harness).

### Changed

- **Routing refactored** into per-mode modules with a performance and behavior overhaul.
- **Request latency reduced across all stack layers** — hot-path allocations, routing, and response assembly.
- `oxphp_request_heartbeat($time)` now also resets PHP's own `max_execution_time` timer to `$time` seconds alongside the server-side deadline. Previously only the server deadline was extended, so long-running scripts could still be killed by Zend's "Maximum execution time exceeded" fatal even after a heartbeat. Scripts that opted out of the PHP timer via `set_time_limit(0)` or `max_execution_time=0` are left alone — the heartbeat does not re-enable a disabled timer.
- Welcome page redesigned as a minimal "is running" status page.
- **Prod image `USER` policy**: `Dockerfile.alpine-release` no longer sets a final `USER` — matches `nginx:alpine` / `php-fpm:alpine` / `frankenphp:alpine` conventions. Deployments drop privileges at the orchestrator level (`docker run --user www-data`, Compose `user:`, Kubernetes `runAsUser`). `chown www-data:www-data /var/www/html` still runs at build time.
- SAPI executor split into per-file modules; worker pool hot path tightened.
- Decorator registry migrated from `unsafe static mut` to `OnceLock`.
- Legacy plugin modules removed in favor of `ox_*` rewrites.
- PHP worker config parsing centralized in `Config`.
- Hyper updated to 1.9; unused `serde` dependency dropped.

### Breaking Changes

- **Async namespace migration**: all async-related PHP classes moved under `OxPHP\Async\`:
  - `OxPHP\AsyncException` → `OxPHP\Async\Exception`
  - `OxPHP\AsyncTimeoutException` → `OxPHP\Async\TimeoutException`
  - `OxPHP\AsyncBorrowException` → `OxPHP\Async\BorrowException`
  - `OxPHP\BorrowedProxy` → `OxPHP\Async\BorrowedProxy`
- Async functions (`oxphp_async`, `oxphp_async_await`, etc.) are now provided by the `plugin-async` feature flag. Without it, the functions are not available. Function names are unchanged.
- **Plugin API**: `Plugin::shutdown` now takes `&mut self` (was `&self`). Plugin authors must update implementations.
- **Plugin config**: `env::set_var` side-effects from plugin init no longer propagate to the core server — plugins must publish core-relevant flags through the explicit core-flags API.
- **`RequestComplete` event**: string-serialized metadata replaced with typed fields.

### Performance

- `Shared\Pool` acquire/release uncontested hot path: **≤ 5 µs gate, ~0.9 µs observed in Docker**. Per-thread affinity keeps slots hot in the acquiring thread without cross-thread handoff.
- Map `set` / `get` path avoids serialisation for nested `Shareable` refs — the refcount-bump retain path is cycle-checked before any mutation, so rejected inserts leak nothing.
- Request path: fewer allocations and clones across routing, response assembly and hot-path dispatch.

### Fixed

- Pool chaos reclaim: in-flight slot counts are refunded when a SAPI worker thread panics mid-acquire, so a crashing worker no longer silently burns budget in the surviving workers' view.
- Cross-thread `Shared\*` access no longer depends on the `worker_liveness` hook for Map / Counter / Flag / Once / Mutex — only Pool uses thread-registration (for its affinity + reclaim paths).
- Async worker SIGBUS from cross-thread `MAP_PTR` access.
- `headers_list()` returning empty — the header handler now returns `SAPI_HEADER_ADD`.
- `payload()` returning null for JSON body after a PDO query reused the request buffer.
- `SecurityHeadersHandler` env-variable race: `FRAME_OPTIONS` is now resolved at startup rather than read per request.
- Decorator `RejectedException` dispatch and instance-cache collisions across requests.
- `-Wint-to-pointer-cast` warning in bridge `server_context` assignment.
- Default-feature (`php`) compile/clippy errors that were previously masked by the `--no-default-features` CI profile.
- TLS test profile now generates v3 certificates dynamically and the runner supports HTTPS.
- E2E runner `curl_args` parsing no longer strips shell quotes incorrectly.
- Cancelled-task exception class corrected to `OxPHP\Async\Exception`.

## [0.2.0] - 2026-03-27

### Added

#### Async & Concurrency

- Fiber-based request multiplexing in worker mode — concurrent I/O within a single worker thread
- Async promises (`oxphp_async()`) — parallel PHP execution via dedicated thread pool
- Distributed tracing with W3C Trace Context propagation and OpenTelemetry export

#### PHP API

- HTTP Object API (`OxPHP\Http\Request`) with lazy bridge accessors
- HTTP interfaces (`OxPHP\Http\RequestInterface`, `OxPHP\Http\SessionInterface`, `OxPHP\Http\AttributesInterface`) with clone/serialize blocking on request-scoped classes
- Attribute-based decorator system (`oxphp_register_decorator()`, `OxPHP\Decorator\AttributeInterface`) with PHP observer integration

#### Server Variables

- `HTTPS` — set to `"on"` when TLS is active
- `REQUEST_SCHEME` — `"https"` or `"http"` per PHP-FPM/nginx convention
- `DOCUMENT_URI` — alias for `SCRIPT_NAME` for nginx/PHP-FPM compatibility
- `REQUEST_TIME_FLOAT` — request start time with microsecond precision

#### HTTP Compliance

- `Date` header on all HTTP responses per RFC 9110 §6.6.1
- `Content-Type` header on all error responses per RFC 9110

#### Observability

- Request duration histograms, byte counters, and subsystem metrics
- `trace_context` field exposed in `/config` endpoint

#### Static Files

- `STATIC_CACHE=off` mode with mtime-based content cache revalidation via `stat()` checks

### Changed

- Default listen port changed from 8080 to 80 (TLS-aware: defaults to 443 when `TLS_CERT` is set)
- Backpressure response changed from 503 to 529 (Site is overloaded)
- `workers_idle` metric now calculated dynamically during scrape (was always 0 in static pool mode)
- `workers_spawned_total` counter now includes initial worker spawn
- `/config` endpoint now exposes `log_level` and other missing runtime settings

### Fixed

- `SERVER_PROTOCOL` now reflects actual HTTP version (was hardcoded to `HTTP/1.1`) per RFC 3875
- `REQUEST_TIME` now returns request start time (was returning current time)
- IPv6 Host header parsing for `SERVER_NAME` and `SERVER_PORT`
- Request timeout now returns 408 instead of 504 per RFC 9110
- Duplicate `oxphp_response_time_us_total` Prometheus metric removed
- Session state cleanup added to worker soft reset to prevent state leaks between requests
- Missing fiber source files added to alpine-release Dockerfile

## [0.1.0] - 2026-03-08

First public release. OxPHP replaces nginx + PHP-FPM with a single async binary
written in Rust, providing HTTP serving, native PHP execution via custom SAPI,
and built-in observability.

### Core

- Async HTTP/1.1 server built on Hyper + Tokio with graceful shutdown
- Custom PHP SAPI (`oxphp`) with full superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`)
- C bridge library (`liboxphp_bridge.so`) for zero-copy Rust↔PHP communication via direct zval access
- PHP ZTS (Zend Thread Safety) multi-threaded worker pool with bounded queue and 529 backpressure
- Three routing modes: Traditional (direct file mapping), Framework (front controller), SPA (fallback to `index.html`)
- Static file serving with in-memory cache, MIME detection, and HTTP caching (ETag, Last-Modified, 304 responses)
- Brotli compression with configurable quality level (0–11) and minimum size threshold
- TLS support via PEM certificate/key
- PHP 8.4 compatibility

### Worker Mode

- Persistent PHP worker processes (`oxphp_worker()`) that handle multiple requests with soft reset between them
- Early response completion (`oxphp_finish_request()`) for background processing after response is sent
- SSE streaming with real-time chunked delivery (`oxphp_is_streaming()`, `oxphp_stream_flush()`)
- Cooperative timeout and cancellation via `oxphp_request_heartbeat()`
- Worker mode detection (`oxphp_is_worker()`) and introspection (`oxphp_worker_id()`, `oxphp_server_info()`)
- Resilient to `exit`/`die` in PHP 8.4 worker mode

### Worker Pool

- Static pool mode: fixed number of workers (`PHP_WORKERS=N`)
- Dynamic pool mode: auto-scaling between min and max workers (`PHP_WORKERS=MIN:MAX`)
- Auto-detect mode: defaults to CPU/2 workers (`PHP_WORKERS=0`)
- Per-worker memory limits (`WORKER_MAX_MEMORY_MIB`) and request limits (`WORKER_MAX_REQUESTS`)
- Dead worker respawning via health monitor (static) / scale manager (dynamic)
- `catch_unwind` prevents panics from poisoning the channel

### Observability

- Prometheus metrics endpoint (`/metrics`) with request counts, durations, status codes, active connections, queue depth, and worker mode stats
- Health check endpoint (`/health`)
- Runtime configuration endpoint (`/config`)
- Structured JSON access logging with configurable levels (`ACCESS_LOG`: off/error/all)
- Request ID generation (`oxphp_request_id()`) in `{timestamp:08x}{counter:08x}` format
- Structured PHP error logging via `zend_error_cb`

### Security & Limits

- Per-IP rate limiting (`RATE_LIMIT`, `RATE_WINDOW_SECONDS`)
- Header read timeout (`HEADER_TIMEOUT_SECONDS`) and request timeout (`REQUEST_TIMEOUT_SECONDS`)
- Graceful shutdown with drain timeout (`DRAIN_TIMEOUT_SECONDS`)
- Configurable request body limits
- Path traversal protection with canonicalization

### Plugin System

- Plugin trait with lifecycle hooks (init, startup, shutdown)
- Typed event dispatcher with priority ordering at every lifecycle point
- Events: ConnectionAccepted, RequestReceived, RouteResolved, ScriptExecutionComplete, ResponseBuilding, RequestComplete
- Native PHP function registration from plugins (zero-copy zval access, no JSON serialization)
- Plugin context API for handler registration and configuration

### Performance

- mimalloc global allocator for reduced per-alloc latency
- Configurable multi-threaded Tokio runtime (`TOKIO_WORKERS`)
- Route LRU cache for fast path resolution
- Thread-local buffer reuse for server variables
- Single Arc clone per request (reduced from 10)
- OPcache support with correct `sapi_get_request_time()` initialization

### Infrastructure

- Multi-stage Alpine Docker build (`php:8.4-zts-alpine`)
- Multi-platform Docker images (amd64/arm64) published to GHCR
- CI workflows: nightly build, PR checks (fmt, clippy, tests), release tagging
- Best-practice Dockerfile example with separate dev/prod targets
- HTTP QUERY method support (RFC 10008)
- Documentation in English, Russian, Belarusian, and Chinese

### PHP Functions

| Function | Description |
|---|---|
| `oxphp_request_id()` | Current request identifier |
| `oxphp_worker_id()` | Current worker thread ID |
| `oxphp_server_info()` | Server runtime information |
| `oxphp_request_heartbeat()` | Signal liveness for cooperative timeout |
| `oxphp_finish_request()` | Flush response early, continue background work |
| `oxphp_is_worker()` | Whether running in worker mode |
| `oxphp_is_streaming()` | Whether SSE streaming is active |
| `oxphp_stream_flush()` | Flush SSE chunk to client |
| `oxphp_worker(callable)` | Enter persistent worker loop |

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Server listen address |
| `DOCUMENT_ROOT` | `www/public` | Document root path |
| `INDEX_FILE` | `index.php` | Front controller file |
| `PHP_WORKERS` | CPU/2 | Worker pool size (`N`, `MIN:MAX`, or `0`) |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Dynamic pool idle timeout |
| `TOKIO_WORKERS` | CPU/2 | Tokio runtime threads (0 = auto) |
| `QUEUE_CAPACITY` | workers×128 | Bounded queue size |
| `RATE_LIMIT` | `0` (off) | Max requests per window per IP |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window |
| `HEADER_TIMEOUT_SECONDS` | `10` | Header read timeout |
| `REQUEST_TIMEOUT_SECONDS` | `30` | Request execution timeout |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Graceful shutdown drain |
| `COMPRESSION_LEVEL` | `4` | Brotli quality 0–11 (0 = off) |
| `STATIC_CACHE_TTL` | `3600` | Static file Cache-Control max-age |
| `ACCESS_LOG` | `all` | Log level: off/error/all |
| `ERROR_PAGES_DIR` | — | Directory with `{status}.html` pages |
| `TLS_CERT` / `TLS_KEY` | — | TLS certificate/key PEM paths |
| `INTERNAL_ADDR` | `0.0.0.0:9090` | Health/metrics/config endpoint |
| `WORKER_MAX_REQUESTS` | `0` (unlimited) | Max requests per worker before restart |
| `WORKER_MAX_MEMORY_MIB` | `0` (unlimited) | Max worker memory before restart |
| `EXECUTOR` | `sapi` | Executor type: sapi/stub |

[0.10.0]: https://github.com/oxphp/oxphp/releases/tag/v0.10.0
[0.9.0]: https://github.com/oxphp/oxphp/releases/tag/v0.9.0
[0.8.0]: https://github.com/oxphp/oxphp/releases/tag/v0.8.0
[0.7.0]: https://github.com/oxphp/oxphp/releases/tag/v0.7.0
[0.6.0]: https://github.com/oxphp/oxphp/releases/tag/v0.6.0
[0.5.0]: https://github.com/oxphp/oxphp/releases/tag/v0.5.0
[0.4.0]: https://github.com/oxphp/oxphp/releases/tag/v0.4.0
[0.3.0]: https://github.com/oxphp/oxphp/releases/tag/v0.3.0
[0.2.0]: https://github.com/oxphp/oxphp/releases/tag/v0.2.0
[0.1.0]: https://github.com/oxphp/oxphp/releases/tag/v0.1.0
