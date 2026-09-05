# Changelog

All notable changes to OxPHP are documented in this file.

## [Unreleased]

### Migration from 0.11.0

**`COMPRESSION_LEVEL` is deprecated in favour of `COMPRESSION_BROTLI_LEVEL`, and setting it now logs a `WARN` at startup.** It keeps both meanings it has always had — a level for Brotli, and `COMPRESSION_LEVEL=0` as the switch that turns off every coding — so nothing breaks by leaving it set. `COMPRESSION_BROTLI_LEVEL` overrides it where both are present. Note that a deployment which set it to a non-zero level was configuring *all* compression and now offers Zstandard and gzip as well, at their own defaults; `COMPRESSION_ENCODINGS` is where that is narrowed.

**`/config` reports `brotli_level`, `gzip_level` and `zstd_level` in place of `compression_level`.** There is no longer a single level to report: each coding has its own. Tooling that read `compression_level` from the internal `/config` JSON gets nothing under that name and should read the three new keys; the rest of the body is unchanged.

**`/health` answers 503 when the worker pool has lost its worker threads, or — in worker mode, the only place that state is detected — is wedged.** It previously answered 200 whatever the pool was doing. The JSON body gains a `pool_stalled` field and reports `"status": "degraded"` in those states, alongside the `executor_healthy` and failed-plugin conditions it already carried. A container health check pointed at `/health` — as the shipped compose file does — will report the container unhealthy there, which is the point of it. The wiring that needs checking is the reverse one: a Kubernetes *liveness* probe pointed at `/health` will now restart the pod when the pool wedges or a plugin fails, neither of which a restart is the right answer to on its own. Liveness belongs on `/health/liveness`, readiness on `/health/readiness`, startup on `/health/startup`; the documentation examples that pointed all three probes at the aggregate `/health` have been corrected.

### Added

- **`/metrics` reports when the pool has stopped taking work off its queue.** The supervisor already detected that state — requests waiting while workers sit idle and the count of requests the workers get through does not move — confirmed it over a second scan and left it only on real progress, but the only place it said so was a line in the log, which nothing alerts on by default. `oxphp_pool_stalled` publishes it: `1` once the state has held for a minute — a worker that exited on its memory ceiling and is re-running the application's bootstrap wears the same shape for a scan or two, and this waits it out — until the scan that sees the pool reaching workers again, `0` otherwise. Detecting the state needs both the live queue and that count, which only worker mode keeps, so the gauge is exported there alone; in the other modes the series is absent rather than a constant `0`, because a `0` from a supervisor that is not watching for the state reads exactly like a healthy pool. The same state appears as `pool_stalled` in the `/health` JSON.

- **`/metrics` reports how many request fibers each worker is carrying.** A worker multiplexes the requests it has accepted onto fibers, and admits at most 256 of them; past that its loop stops taking anything off the queue while continuing to run. How that state reads from outside depends on how the worker got there, and neither reading names it: where the fibers outlived the requests that made them — answered requests whose fibers were never reclaimed — the pool reports every worker idle and none busy, which is exactly what a quiet pool reports; where they hold requests still in flight, it reports itself fully busy, which is what honest saturation reports. The count that tells those apart from the states they imitate lived on the worker’s stack, where nothing outside could see it. `oxphp_worker_request_fibers_active{worker="N"}` exports it: `0` on an idle worker, `1` on one serving a request start to finish, higher where requests suspend and share the thread, and `256` on one that has stopped taking work off its queue. The worker publishes it from its own loop, and again on each of the two paths before the loop disappears into a handler — not when a request finishes, because a worker that has stopped finishing requests is precisely what the number is for, and a gauge written at completion would freeze at its last healthy reading exactly then. It also gives a plain reading of whether fibers are being reclaimed at all: on a healthy worker the count returns to zero once the traffic stops, and a count that does not is a leak long before it reaches the ceiling. The cost is one relaxed store per turn of the loop. Worker mode only, alongside the other per-worker gauges.

- **A pool that stops taking work off its queue now says so, and `/metrics` says what the queue is holding.** Neither the depth of the queue between admission and the workers nor the admission capacity left was exported, and without them a pool that has stopped consuming its queue reads as a healthy one: every worker idle, no busy worker, `200` from liveness throughout and from readiness for the first minute, static files served normally — while a growing set of requests will never be answered. It stays that way until the process is restarted, and it gets worse rather than better: once the queue fills, the admission permits those requests hold are gone too, and every new arrival waits out `QUEUE_WAIT_TIMEOUT_MS` and gets a `529`. An orchestrator keeps such a node in rotation throughout, and nothing in the log mentions any of it. Three gauges make the state legible: `oxphp_queue_depth` (requests admitted but not yet picked up), `oxphp_queue_capacity` (`QUEUE_CAPACITY`, the bound to read it against) and `oxphp_admission_slots_available` (permits nobody holds). Read together they separate the two ways admission can run out — a queue full of requests nothing is consuming, versus permits taken and never returned — which every other series renders identically. The server also watches for the combination itself: when two consecutive scans see work waiting — queued requests, or refusals climbing — with at least one worker idle and the pool getting nothing done, it logs a `WARN` carrying those same numbers, repeats it about once a minute for as long as it lasts, and logs an `INFO` once work starts moving again. Queued work counts and not only refusals, because the fault begins as a queue nobody is draining, where arrivals are still admitted and nothing is refused at all; waiting for a refusal counter to move would mean staying silent until the queue filled. Progress is measured as what the workers finished rather than what clients received, so a storm of client aborts — where the client is gone before any completion can be recorded — reads as the busy pool it is; that count exists only in worker mode, so the warning is emitted there and nowhere else, rather than falling back to completions and reporting a busy pool as a stopped one. Nothing is reported until the pool has finished its first request, so the application bootstrap at startup is not mistaken for a wedge — though only that first one: the counter is pool-wide and monotonic, so a worker that recycles later and boots again is not covered, and on a single-worker pool each such recycle under traffic prints one warning that clears itself when the new worker starts serving. And leaving the state takes work actually moving rather than merely a quiet moment, so a wedged pool with no traffic on it is not mistaken for a recovered one. Nothing about admission behaviour changes; the gauges appear only where there is a queue to describe, and are absent for the stub executor.

- **Dynamic responses are compressed with Zstandard for clients that accept it.** Compression cost is paid per request on everything PHP produces, and Brotli at a level a request can afford is the wrong tool for that: measured over real response bodies — rendered pages, list-endpoint JSON, the assets of a running site — Zstandard at level 6 produced fewer bytes than the Brotli quality earlier releases compressed everything with, on everything above a few kilobytes, in well under half the CPU, and its lead widened rather than narrowed on x86-64. Cached static files keep preferring Brotli, because their compressed copy is built once at maximum quality and served from memory afterwards, where nothing but size counts. So a client that accepts both now sees each coding where it wins: Zstandard on the document, Brotli on the assets — and on a static file the coding switches from one to the other once the stored copy lands, which is a change of representation, not of resource, and is covered by the `Vary: Accept-Encoding` those responses already carried. Clients that do not accept Zstandard are unaffected; it needs a browser from 2024 or later, so Brotli and gzip carry the rest exactly as before. Which codings the server offers is now `COMPRESSION_ENCODINGS` (default `br,zstd,gzip`, or `off` for none), and each has its own level — `COMPRESSION_BROTLI_LEVEL` (5), `COMPRESSION_ZSTD_LEVEL` (6), `COMPRESSION_GZIP_LEVEL` (6) — with a coding offered only when it is both listed and non-zero, and an unknown name in the list a startup error rather than a coding that quietly vanishes. `COMPRESSION_LEVEL` continues to work as a deprecated name for `COMPRESSION_BROTLI_LEVEL`. The encoder is Meta's libzstd, compiled from vendored C sources under BSD-3-Clause and recorded in `THIRD_PARTY_LICENSES.html`.

- **Clients that do not accept Brotli now get gzip instead of nothing.** Compression was Brotli-only, so every client without it — Chromium-based browsers over plain HTTP, most command-line tools, anything predating 2016 — was served uncompressed bytes; on a typical HTML document that is roughly five times the transfer. `Accept-Encoding` is now read as the ranking RFC 9110 §12.5.3 defines it to be: whichever coding the client weighted highest, of those the server offers, is what the response is encoded with, a `*` covers a coding the header does not name, and a zero weight is a refusal. Cached static files keep a separate compressed copy per coding, each built once in the background on the same terms as before, so a gzip client on a cached asset costs no more per request than a Brotli one, and a file only ever served to Brotli clients never costs a gzip compression. `COMPRESSION_GZIP_LEVEL` (default `6`) sets the per-request level — level 6 is zlib's own default and the point past which the ratio stops paying for the CPU. One consequence worth knowing before upgrading: `Range` requests were already declined for compressible representations served to a Brotli client, because byte offsets do not survive re-encoding — with gzip in the mix that now covers essentially every client, so a range request against a 300 KB CSS or JSON file gets the full 200 rather than a 206. Files above 3 MiB stream uncompressed and honour ranges exactly as before, as do all non-compressible types. Internally the gzip encoder is a Rust implementation of zlib rather than the system C library, which measured about twice as fast as the alternative pure-Rust backend on bodies from 16 KB up and drops `libz-sys` from the dependency tree.

- **A cached static file is compressed once instead of on every request.** The content cache held identity bytes and compression ran afterwards, per hit, at the per-request quality — a cached `jquery.min.js` spent CPU recompressing itself for every client that asked for it, forever, and shipped the same mediocre result each time. A file that stays in the cache now gets a maximum-quality Brotli artifact built once on a background thread and stored next to the identity bytes it was made from; later hits are answered from it. Measured over real assets the artifact is 8–12% smaller than what the per-request path produces, and on a page load of one document plus three cached assets the compression work drops by roughly five sixths while the bytes on the wire drop by about a tenth. Nothing waits for it: the request that triggers the build is served the way it always was, and so is every request until the artifact lands. The build is claimed per file, so a cold cache under load compresses each file once rather than once per concurrent request; only a few run at a time, so warming a site's whole asset tree cannot take the blocking pool away from the requests being served; and a file whose bytes do not compress is marked as such instead of being retried on every hit. The artifact shares the identity bytes' validator — when `STATIC_REVALIDATE` sees the file change on disk, both are dropped together — and it is charged against the same cache budget, so the ceiling on memory is unchanged. Response headers are unchanged: same `Vary`, same weak ETag, same absence of `Accept-Ranges` that a compressed static response already carried.

- **An exhausted `MAX_CONNECTIONS` budget is no longer silent.** Once running, queued and admission-parked requests hold every connection permit, the accept loop parks and arriving clients get no response at all — a state that, from the outside, was indistinguishable from a dead node: not a line in the log, no metric to alert on, and the health probe on `INTERNAL_ADDR` staying green because it does not go through the budget. An accept that has to wait now logs a `WARN` naming `MAX_CONNECTIONS`, the permit arriving logs an `INFO` with how long the loop was parked — written where the stall ends rather than on the next connection, which on an overload that subsided because the load went away may never come — and two metrics export the state: `oxphp_accept_stalled`, a gauge reading `1` while the loop is parked (scrapeable during the stall via the internal listener), and `oxphp_accept_stalls_total`, a counter of connections that had to wait, which catches stalls short enough to fall between two scrapes. The logging marks transitions rather than connections and is rate-limited, so a server flapping around the ceiling writes a bounded number of lines — and the limit never hides a stall that is still happening: one that starts inside a recent report's window is reported as soon as that window closes. The behaviour itself is unchanged: the loop still parks rather than refusing — clients see the same silence, but the node now reports it.

### Changed

- `/config` reports `brotli_level`, `gzip_level` and `zstd_level` instead of `compression_level` — each coding has its own level now, and one number can no longer describe the configuration.

- **Brotli's per-request quality is now 5 rather than 4.** Brotli's quality knee is a change of hasher between those two levels, and 4 is the cheap, weak half: measured over real bodies it produced *more* bytes than gzip does at its own default on JSON above 4 KB and on minified assets — jQuery 3.4% larger, a rendered HTML document 0.7% larger — while spending a fifth to a quarter more CPU than gzip on the same bodies. A server that offers both codings and prefers Brotli was therefore picking the larger and slower of the two for every client that accepts it. At quality 5 Brotli is the smaller everywhere measured, by 2–6% on real bodies and by considerably more on small documents where its static dictionary plays, for roughly twice gzip's CPU. This level now governs a smaller share of traffic than it used to, since clients that accept Zstandard are served that instead — what remains on Brotli is Safari, browsers predating 2024, and intermediaries. Two consequences worth knowing before upgrading: Brotli bodies over 4 KB are now compressed on the blocking pool rather than inline, because quality 5 is where the cost curve turns steep and that threshold was already drawn there; and the improvement a cached static file's maximum-quality copy shows over the per-request path narrows from 12–19% to 8–12%, because the per-request path itself got better. `COMPRESSION_BROTLI_LEVEL=4` restores the previous behaviour exactly.

### Fixed

- **The readiness probe reports whether the server can serve, instead of only whether it is shutting down.** `/health/readiness` checked three conditions — no shutdown in progress, the executor reporting itself healthy, and no plugin in a failed state — and only the first of them could ever come back false: the executor health check was a trait default that returned `true` and that no executor overrode, and none of the shipped plugins ever reports itself failed. A pool whose worker threads had all ended, and a pool that had stopped taking work off its queue, both answered 200 and stayed in rotation, while the documentation promised a 503 when "the PHP worker pool is unhealthy" — a promise no line of code kept. Both states now answer 503: the executor's check is a real one (the pool holds at least one worker thread that has not ended), and the supervisor's wedge detection is published where the probe can read it once the state has held for a minute — short of that it is still a warning in the log, because a worker re-loading the application after a recycle reads the same way and is seconds from serving. Detecting a wedge needs the queue and the count of requests the workers get through, which only worker mode keeps, so that half of the signal exists there alone; the rest applies in every mode. Load deliberately stays out of it — an instance shedding 529s is answering, quickly and on a working pool, and taking it out of rotation would hand its traffic to replicas in the same state until the service had no endpoints left. Overload is reported by `oxphp_admission_refused_total{reason}` and is answered with capacity, not with rotation.

- **Worker mode with the `streams` runtime hooks: a persistent PDO connection is no longer closed under the request that is using it.** Everything below belongs to that hook category — the one `RUNTIME_HOOKS=streams` enables, and `1`, `true` and `all` with it — because a fiber is suspended inside a socket read only where it is on, and that suspension is what lets a second request reach the constructor while the first is still inside it. With the category off a socket read pins the worker thread, two constructors on one worker cannot overlap, and none of this arises. `PDO::ATTR_PERSISTENT` keeps the driver's connection in a pool that outlives the request, and the constructor reaches that pool in two steps which are safe apart and not together once one worker thread carries several requests at a time: it looks a pooled connection up before it connects, and registers what it built afterwards. Two constructors overlapping therefore both miss the pool, both connect, and the second registration replaces the first entry — which closes that connection and frees the handle behind it, with no regard for how many objects still point at it. The request mid-exchange on that connection loses it and ends with a fatal error saying so, and the object still holding the freed handle can take the whole process down with a segmentation fault when it is released. Overlapping is the ordinary case rather than a rare one: a handler beginning `static $pdo ??= new PDO(...)` is reached by every request that arrives before the first connect completes, and a connect is exactly the kind of wait that lets those requests run — a benchmark against a four-line handler of that shape killed the server on the first burst of database traffic it saw. A second step loses the same sharing with no race at all, and quietly: before handing a pooled connection over PDO pings it, a ping cannot be sent on a connection another request is mid-exchange on, and PDO reads that as a connection that has died — so it drops that one from the pool and opens another. The request mid-exchange keeps its own connection and its reply, but the connection the application meant to share is no longer shared, and a worker under load ends up with one connection per request that constructed during someone else's query rather than the single one it asked for. Constructors asking for a persistent connection now run one at a time per worker thread, which is the whole width of a pool since each thread has its own, so the second one finds what the first registered instead of replacing it. And a pooled connection another request holds is reported alive without the ping, because sending one would itself land in the middle of that request's exchange — the connection being used is the plainest evidence there is that it lives. The same answer is given where the driver has no liveness check of its own — pdo_sqlite is one — since PDO asks nothing at all before handing such a connection over, which leaves the constructor's options landing on a handle in use with nothing in between. That answer carries a condition, because PDO does not stop at handing a pooled connection over: it writes the error mode and the autocommit flag onto the handle from the options the constructor was given, defaults included when they are absent, and then applies the rest of those options to it as well. On a connection in use those writes are the holder's — its error mode changing under it and, for an option the driver forwards such as `PDO::ATTR_STRINGIFY_FETCHES`, the PHP types its remaining rows come back as — with nothing raised on either side. So a connection in use is shared only with a constructor that would leave it exactly as it is: every option it passes must be one PDO stores on the handle itself — the error mode, the case folding, the null handling, the default fetch mode — and must pass the value that handle already holds. The `PDO::ATTR_PERSISTENT` entry every such constructor carries is not counted among them: it names the pool rather than setting anything, and the trip PDO makes with it through the driver — it has no case of its own there — is answered rather than sent to the connection. An option the driver has to be told about is refused whatever its value, `PDO::ATTR_AUTOCOMMIT` and `PDO::ATTR_STRINGIFY_FETCHES` included, because being told is a command sent inside the holder's exchange, and asking for the value that is already set does not make it less of one. Three things to know before upgrading: a persistent connection that really has died while another request is using it is no longer swapped out inside the constructor, so that request learns of it on its next query rather than silently getting a fresh one — where nothing is using the connection, including where its holder has dropped its last handle, the check runs exactly as before; a constructor that fails either of those conditions gets a connection of its own while the pooled one is in use, rather than adopting and rewriting it, which for a pool key whose options carry `PDO::ATTR_EMULATE_PREPARES` or `PDO::ATTR_STRINGIFY_FETCHES` means sharing between requests and between requests that construct before they query, but a connection of its own for one arriving while another request holds it — which lasts from that request's first query to its end, not for the length of a query; and a constructor that cannot wait, because its bounded wait was spent or because a userland fiber scheduler owns the context, goes ahead unserialised as it always did and says so in the log. Constructors that do not ask for a persistent connection wait for nothing and are unaffected.

- **Worker mode: a fatal error no longer leaves the worker able to spin forever.** With a function observer installed — `PROFILER_ENABLED` registers one for every user function, whether or not anything is ever profiled, and APM registers one for every function carrying `#[OxPHP\Apm\Trace]` — the engine keeps a chain of the calls whose end handlers are still open, and a fatal returns from none of them: every frame it abandons stays on that chain. A SAPI that shuts the request down closes them all as its first step. A worker does not shut the request down between requests, and the recovery that puts it back to serving skipped that step. Because the same recovery rewinds the VM stack to exactly where the request started, a later fatal abandons frames at the addresses the stale chain is still naming — and writes that chain into a frame the chain already leads to, closing it into a loop with no end. Two fatals on one worker are enough. Nothing reads the chain while it is merely stale; it is read in full when a request fiber dies, and that read walks the loop and never comes back — the worker spins at a full core, never finishes what it was doing, and is never replaced. On a single-worker pool that is the whole server; on a larger one it is each worker in turn, at whatever rate they take fatals. From outside, the pool reads as idle — no busy worker, `200` from liveness throughout and from readiness for the first minute, static files still served — while the queue grows and nothing in it is ever answered, until the process is restarted. The state is reached in ordinary operation: a storm of clients hanging up mid-request ends each of those requests in a fatal, and a pool serving that storm was found with all four of its workers spinning in exactly that walk. Recovery from a fatal now closes the open handlers first, the way a request shutdown does, so nothing outlives the request that raised it. A server where no function has an end handler — no profiler, and nothing traced — was never affected: what the chain holds is the calls whose end handlers are open, and a function that has none is never put on it.

- **Worker mode: a fatal error raised while the server was recovering from an earlier one no longer takes the worker with it.** Recovering from a fatal means giving back what the abandoned frames were holding, and that is not a leaf operation: one of the things a frame can hold is a stream handle, closing a stream disposes its filter chains, and disposing a userland filter calls that filter object's `onClose()`. So the recovery reaches ordinary application code, and a fatal raised in there — a memory limit, a time limit, an `E_USER_ERROR`, an uncaught exception — was raised at a point where the recovery's own error target had already been handed back to its caller. It jumped over everything that remained: the stack rewind, the flag that tells the engine the last request ended uncleanly, the flag that holds the cycle collector shut. In worker mode that is not one request. The jump left the loop that serves requests, so every request multiplexed onto that worker was dropped with it, the client seeing a connection closed with no response, and on a single-worker pool nothing answered until the pool noticed and booted a replacement. Each step of the recovery that can run application code is now contained, so the recovery finishes and the worker goes on serving. What is given up instead is bounded and much smaller: the frames the interrupted step had not reached yet stay held for the life of the worker — one request's worth of memory, not the worker and everything on it. The interrupted step is deliberately not retried, because the frame it stopped in has given up some of its variables and not others, and a second pass over it would give the same values up twice. A worker that takes no fatals is unaffected, and nothing about how a fatal is reported to the client changes: the request that raised it still gets a 500.

  Recovery also closes the calls it makes itself. With a function observer installed — `PROFILER_ENABLED` registers one for every user function, whether or not anything is ever profiled — the application code the recovery reaches goes onto the engine's chain of calls whose end handlers are still open, exactly like any other call, and a fatal in that code leaves it there. The stack rewind that follows hands that memory to the next request, and the chain outlives the request that built it — it belongs to the fiber, and the worker hands that fiber the next request — so the next fatal on that worker walked a chain naming a frame the next request had since written over. This was reachable only once the recovery became something a worker survives, and it is closed in the same change; a second fatal on one worker with an observer installed is covered by the test suite.

- **`oxphp_worker_memory_bytes` reports the memory a worker is holding instead of zero.** The gauge has been exported since worker mode shipped, and no code ever wrote a value into it: the field it was read from was declared, zeroed at worker start, and read back — nothing in between. Every scrape for the whole life of a process therefore read `0`, which is worse than the metric being absent, because a memory dashboard drew a flat line at zero and the leak worker mode is watched for read as the absence of one. The value is now sampled where the per-worker request counter next to it was already being written — at the end of each request, while that request's superglobals are still on the heap — and it is the worker thread's PHP heap: the same figure `memory_get_usage()` reports inside a handler, and the same one `WORKER_MAX_MEMORY_MIB` is measured against, so the graph and the recycle decision now describe one quantity rather than two. Worker recycling was never affected — it reads the heap directly and always has. Three things to know before alerting on it: the value is written when a request finishes, so a worker that has never served one reads zero rather than reporting its idle heap; a slot is reused when a worker is recycled, so it carries the retired worker's last value — typically the peak that tripped the ceiling — until its replacement finishes a request, with `oxphp_worker_uptime_seconds` for the same worker being what separates the two; and while the gauge and the ceiling read one quantity, they read it at different moments of the same loop — the gauge while the finished request's superglobals are still on the heap, the ceiling check after they are gone — so the exported number runs a few KB above what trips a recycle, and an alert set at exactly the ceiling can sit above it without one happening.

- **A response whose type is written in any case other than lowercase is compressed like the rest.** The compressible-type list was compared byte for byte, so a script setting `Content-Type: application/JSON` — or `Text/HTML`, or an SVG served as `IMAGE/SVG+XML` — was answered uncompressed while the identical lowercase type was compressed. RFC 9110 §8.3 makes media types case-insensitive; nothing about the response justified the difference, and there was no signal that compression had been skipped. The comparison now folds case, and a type still absent from the list is still never compressed.

- **A response the server would compress for someone now says so, even when it is not compressed for you.** `Vary: Accept-Encoding` was written only where a body was actually encoded, so the identity copy of a compressible dynamic response — sent to a client that asked for no coding, or one whose body did not shrink — went out with nothing to key a cache on. A shared cache in front of the server could store that copy under the bare URL and hand it to every client after it, including the ones that would have taken a quarter of the bytes. Static serving already declared the header for everything inside the compression window, encoded or not; the request path now matches it. Two things gain no header: a body the server never holds whole, which cannot be compressed under any header and so does not vary, and every response on a server configured to offer no coding at all, which never reads the header and would only be fragmenting a downstream cache by naming it.

- **A compression setting that is not valid UTF-8 fails at startup instead of falling back.** `COMPRESSION_LEVEL` read an unreadable value as an unset one and used the default, so a value mangled by whatever passed it in configured nothing and said nothing about it — and a silent default is the one outcome an operator has no way to notice. It is now rejected by name at startup, as are `COMPRESSION_ENCODINGS` and the per-coding level variables, which is what every other numeric variable already did.

- **A static file between 1 MiB and 3 MiB is no longer sent uncompressed.** Whether a response is buffered or streamed was decided by the content cache's 1 MiB limit, and the compression layer can only encode a body the server holds whole — so a memory decision was silently deciding what gets compressed. Everything between that limit and the 3 MiB ceiling of the compression window went out in full: a WASM module, a framework bundle, a source map, a large CSV export, all to clients that had asked for a coding and would have taken roughly a quarter of the bytes — while a PHP response of exactly the same size and type was compressed. Such a file is now read whole for that request and compressed, if and only if the client negotiated a coding and the type is compressible; everything else still streams from disk untouched — files above 3 MiB, types that carry their own compression, and clients that accept no coding. Three consequences worth knowing: the file is not cached, because the cache limit is unchanged and one bundle must not evict the small hot files that budget exists for, so it is compressed on each request rather than once — which is what nginx does for large files without `gzip_static`; a client that accepts a coding no longer gets `Range` on these files, since the representation it is served is now encoded, while identity clients keep ranges, which is where resumable downloads live; and because such a file stays in memory until its response has been written, only a bounded number are read at a time — a request arriving over that limit gets the file streamed and unencoded rather than waiting for room. Every response in that range now carries `Vary: Accept-Encoding`, streamed or compressed, so a shared cache cannot hand the full-size copy to a client that asked for a coding.

- **`Accept-Encoding: br;q=0` no longer selects Brotli.** The header was matched by coding name alone, so a client that had explicitly refused the coding got a Brotli body anyway — a zero weight means "not acceptable" (RFC 9110 §12.5.3), not "supported". Weights are now read: a zero weight is a refusal, a `*` covers Brotli when the header does not name it, an explicit entry outranks the wildcard in both directions, and coding names match case-insensitively. A weight that does not parse is not treated as a refusal — the coding stays listed at its default weight rather than being dropped over a malformed parameter.

- **A hot loop awaiting async results no longer leaks every delivered value under the tracing JIT, and reflecting on those functions' return types no longer crashes the worker.** Every function and method the server registers with a `mixed` return type — `oxphp_async_await()`, `Shared\Map::get()`, `Shared\Once::get()`, `Shared\Mutex::withLock()`, the pool and channel accessors — declared it with a malformed engine encoding. A declared internal return type is a contract the engine's optimizer trusts, and the malformed one read as "returns nothing at all": once a loop calling such a function crossed `opcache.jit_hot_loop` (64 back-edges) and was trace-compiled, the compiled code stopped freeing the returned value, so every further iteration leaked its full result — with 64 KiB strings, ~9.5 MB per 200-iteration request, never reclaimed within the request. The same encoding made reading any of these declared types back through Reflection — `ReflectionFunction` and `ReflectionMethod` `getReturnType()` alike — segfault the worker outright, JIT or no JIT. The declared types are now encoded the way the engine itself encodes `mixed`: compiled traces free delivered results, hot await loops hold steady memory, and Reflection prints the type. `opcache.jit=off` worked around only the leak — the Reflection crash had no workaround — and is no longer needed.

## [0.11.0] - 2026-08-20

### Migration from 0.10.0

**Worker mode: an event loop can no longer be run from inside a request.** A worker-mode request is now a fiber, and Revolt refuses to drive its loop from within one: `Revolt\EventLoop::run()` in a request handler now throws `Error: Can't call ...::run() within a fiber`. Move the call to the worker bootstrap, or drop it and let the server drive — everything else Revolt offers inside a request (`defer()`, `delay()`, `getSuspension()`, …) keeps working. Traditional, framework and SPA modes are untouched.

**Worker mode: read `$_POST` and `$_FILES` directly — the workarounds for their emptiness behave differently now.** With the superglobals populated, `request_parse_body()` on a POST returns an empty pair — the body has already been parsed, as under PHP-FPM (it remains the way to parse `PUT`/`PATCH`). And a `php://input` handle deliberately kept beyond its request — in a static, in `$GLOBALS`, in a cached PSR-7 object — is now a closed resource: reading it raises a catchable PHP error instead of returning another request's body.

**Worker mode: nothing set inside a request outlives it.** `ini_set()`, `set_time_limit()` and `error_reporting()` calls made during a request roll back at its end to the values the worker bootstrap established — settings meant to be permanent belong in the bootstrap, which is the baseline the rollback restores. Between requests the worker holds no superglobals either, `$_SERVER` included: code that runs outside any request (a session save handler, a destructor under GC) reads nothing rather than the previous request.

**Worker mode: input read through `ext/filter` does not survive a pause.** After a request resumes from a suspension during which another request ran on the same worker, `filter_input()`, `filter_input_array()` and `filter_has_var()` answer `null` for `INPUT_GET`/`INPUT_POST`/`INPUT_COOKIE` — previously they answered with the other request's values, so the fix for that leak is itself the behavior change. Read `$_GET`/`$_POST`/`$_COOKIE`, which travel with the request, or take the values before pausing. And `filter_input_array()` with a definitions array no longer multiplexes: a `FILTER_CALLBACK` that performs I/O holds the worker thread for the duration, and `Fiber::suspend()` inside one throws — run I/O-bound validation on the value after the call returns.

**`$request->query()` and `$request->cookie()` now decode their values.** Code that compensated with `rawurldecode()` / `urldecode()` now decodes twice and must drop the extra call — a value containing a literal `%` sequence is where that shows up first. `cookie()` reports a cookie sent empty (`Cookie: a=`) as `''` rather than `null`, and `query()` parses at most 1000 parameters.

**The default `FRAME_OPTIONS` is now `SAMEORIGIN` instead of `DENY`.** Cross-origin framing stays blocked; a deployment that relied on the built-in default to forbid *all* framing must set `FRAME_OPTIONS=DENY` explicitly.

**Malformed `MAX_CONNECTIONS` and `QUEUE_CAPACITY` values are now startup errors, and `QUEUE_CAPACITY=0` now means auto** (workers × 128) instead of a zero-capacity queue. Both previously fell back to their defaults silently; they now abort `oxphp serve` startup and are reported by `oxphp config --check`, naming the variable. An exactly-empty value still means unset. Audit your environment before upgrading.

**Under saturation, uploads are refused sooner.** The bodies held by requests waiting for admission are bounded in aggregate by `QUEUE_MAX_WAITING_BYTES` (default 64 MiB): a body that would push the parked total past the budget is answered `529` immediately instead of waiting. `oxphp_admission_refused_total{reason="waiting_bytes"}` names the knob; raising it is the remedy on upload-heavy deployments.

**The documentation moved from `docs/en/` up to `docs/`, and the translations left the repository** for [oxphp.dev](https://oxphp.dev/). Links into `docs/en/…`, `docs/ru/…` or `docs/zh/…` — from old release notes, issues, or search results — no longer resolve.

### Fixed

- **More than 32 sleep timers expiring at once no longer leaves the extra sleepers parked forever.** The scheduler poll that wakes fibers sleeping in `oxphp_sleep()`/`oxphp_usleep()` collects expired timers into a 32-slot buffer, but removed *every* expired timer from its registry while handing back only the first 32 — the rest were never woken: an awaited task ran out its caller's whole timeout, a fire-and-forget one held its pool slot for the life of the worker, a sleeping worker-mode request hung with nothing in the log. A CPU-bound stretch of a few hundred milliseconds is enough to expire 33 timers in one window. The poll now takes only what fits and leaves the rest registered for the next poll, which follows on the scheduler's very next turn, so a large burst wakes in batches of 32 instead of abandoning the tail.

- **Worker mode with the socket hooks: closing a connection another request is reading from no longer corrupts that request, and no longer leaves it waiting for a reply that cannot come.** Requests multiplexed on one worker share its connections, and any of them could close one — `$mysqli->close()`, `fclose()` on a shared handle, a reconnect helper — while another request was parked waiting for a reply on it: the parked request resumed into freed memory, silently handing whatever those bytes were to the application or crashing the worker, and even without corruption nothing would ever have woken it before its own read timeout (a day, for mysqlnd's default). The waiting request now fails immediately with a `500` naming what happened; the worker and everything else it serves carry on. Requests that own their connections, and every mode other than worker mode, are unaffected.

- **Worker mode: a fatal error no longer takes a piece of the worker's shared state with it.** Unwinding an aborted request released the variables an included script shared with the frame that included it twice, and the double release landed on the very array or object a worker-mode application keeps its cross-request state in: three fatals and the state was silently gone — caches, connection pools and boot configuration reset mid-run with nothing logged — and under the socket hooks a concurrent request holding the same value could take the worker down with a heap error. The variables are now handed back the way the engine hands them back when an include returns normally, so a fatal costs its own request and nothing else. Other serving modes share nothing across requests and were never affected.

- **Async tasks that hand work to each other are no longer paced by the pool's fixed 1 ms idle timer.** That interval was too long for handoffs — a `Shared\Channel` transfer cost ~0.7 ms per item almost entirely in waiting, so 50 000 items overran `max_execution_time` — and too short for real idleness: a pool parked in `oxphp_sleep(20)` woke 777 times a second to confirm nothing had happened. The interval now backs off from 50 µs to a 10 ms ceiling, resets whenever a turn does work, and is taken on the task queue itself so a newly dispatched task starts at once. Measured: 0.69 ms → 0.14 ms per channel item, 777 → 92 idle wakeups/s. The trade: a promise settled by another thread, or an await giving up on its timeout, can be noticed up to 10 ms later on a long-idle worker where it used to be up to 1 ms. Deadlines the worker set itself — sleeps, await timeouts, hooked socket deadlines — are exempt: the wait is shortened to land on them, not past them.

- **`Shared\Channel`: a fiber blocked in `send()` is no longer left waiting on a channel that has room in it.** The slot-freed signal went only to fibers already parked, and a fiber parks a moment after its send fails — so with several producers on one channel a sender could miss its wake permanently and stop for good on a channel that is empty and open: `send()` has no deadline and held its async-pool slot for the life of the process, `sendTimeout()` burned its whole budget and reported `Timeout`. Room is now handed over as state rather than a one-off signal: room that appears with nobody waiting is remembered and claimed by the next fiber to park. A `send()` racing a `close()` used to hang forever the same way and now reports `Closed`. `Shared\Channel` used from ordinary threads polls and was never affected.

- **Worker mode: closing the request body from PHP no longer destroys another stream and corrupts the heap.** The buffered request body appears in `get_resources('stream')` like any other stream, and after a script `fclose()`d it — the ordinary move when hunting a handle leak — the end of the request closed the same body again, by address: once the freed block had been reused, that second close destroyed a stream the script still held, and the next round on the same worker aborted the process with `zend_mm_heap corrupted`. The body is now held by its resource rather than by its address, so a body the application already closed is left alone and only one still belonging to the request is released. Reading `php://input`, `$_POST` and `$_FILES` is unchanged, as is what `get_resources('stream')` reports. Other modes release the body through the per-request resource list and were never affected.

- **`/__ox_shared/preview` no longer drops the connection when the value it previews is not ASCII.** Truncating a long string at the `SHARED_PREVIEW_STRING_LIMIT` byte budget (256 by default) could cut inside a multi-byte character, which ended the request with a panic — a closed connection with no status and no body. The cut is now rounded down to the nearest character boundary; the limit remains a byte budget and ASCII values are unchanged.

- **Worker mode: `filter_input()`, `filter_input_array()` and `filter_has_var()` no longer answer with another request's query values, session cookies and body fields.** The filter extension's own copy of the parsed input was filled on every request but emptied once per worker, so it accumulated for the life of the process: a request with no query string read the `?token=` of one served minutes earlier, `filter_input(INPUT_COOKIE, 'session')` returned another client's session id, and `filter_input_array()` merged everything the worker had ever served — while `$_GET`, `$_POST` and `$_COOKIE` stayed correct throughout. That copy is now given back at the start of every request. Two worker-mode boundaries: the storage cannot travel with a paused request, so after a suspension during which another request ran these functions answer `null` — read the superglobals, which do travel; and `filter_input_array()` with a definition array must not hand the worker away mid-read, so a `FILTER_CALLBACK` that does I/O holds the thread instead of multiplexing and `Fiber::suspend()` inside one throws — run such validation on the value after the call returns. Other modes empty the storage per request and were never affected.

- **`RUNTIME_HOOKS=sleep`: `sleep()` and `usleep()` no longer return instantly where the calling fiber cannot be suspended.** In the contexts where suspending is refused — a `declare(ticks)` handler, pcntl signal dispatch, while a request's input is being built, under a userland fiber scheduler — the hooked pair skipped the wait entirely instead of falling back to the native builtin: a throttle stopped throttling and a backoff between retries became a spin, silently. Both now hand the call to the native builtin on that path. `oxphp_sleep()` and `oxphp_usleep()` always had the fallback and are unchanged.

- **Worker mode: a request that pauses comes back knowing when it started.** `$request->startTime()` and `oxphp_server_info()['request_time']` read a per-thread slot that the next request overwrote and answering erased, so a resumed request read `0` — `microtime(true) - $request->startTime()` measured the time since 1970 — or, more quietly, a concurrent request's start time. `$_SERVER['REQUEST_TIME']` and `REQUEST_TIME_FLOAT` were correct throughout, so the two ways of asking disagreed from the pause onwards. The start time now travels with the request. Other modes were never affected.

- **Worker mode: a worker whose handler fails on every request is replaced now, and an application that merely answers `500` is no longer mistaken for one.** The three-consecutive-failures breaker was read only on the concurrent path, so a synchronous handler fataling on every request — the case the breaker exists for — never tripped it; it is now read on both dispatch paths. What counts is narrowed to a request that comes apart (a fatal, an out-of-memory, a stack overflow), wherever it comes apart: a fatal in a shutdown function or in a destructor run while the registry is released now counts too, where it previously *cleared* the run. An uncaught exception is neutral in both directions — counting it would rotate the pool during an ordinary dependency outage, and it handed the lever to clients (three oversized POSTs against a strict error handler rotated a worker from outside). A cancelled request is neutral too: `max_execution_time`, a client abort or a userland cancel is the server ending the request, not the handler failing at it — so a hung dependency no longer recycles workers through deadlines. A genuinely wedged worker is still the operator's call via `oxphp_worker_stuck_total`; `Worker::scheduleExit()` remains for applications that want out on their own terms. A worker that retires ends the other requests it was serving, as the memory ceiling already did, and the reason is exposed under `oxphp_worker_recycles_by_reason_total{reason="error"}`.

- **Worker mode: an ini directive a request changes is put back before the next request, instead of staying changed for every later request the worker serves.** `ini_set()`, `set_time_limit()` and `error_reporting()` outlived their request for the life of the worker: a debugging branch that turned `display_errors` on sent stack traces to every later client, `set_time_limit(7)` became everyone's deadline, a `memory_limit` raised around an import stayed raised. Directives now roll back to the values the worker bootstrap established — bootstrap is application configuration and survives; what a request sets ends with it, as under PHP-FPM. Boundaries: a worker with other work still in flight defers the rollback, since taking one request's directives back would strip another mid-run; `memory_limit` returns as a value at once and as an allocator ceiling when the worker's footprint leaves room; and `opcache.enable` is deliberately left where a request put it — nothing inside a worker can turn OPcache back on, so restoring the value would report an enabled cache on a worker compiling everything from source. Other modes always rolled back per request.

- **Worker mode: a warning raised while reading a request body now reaches the application as part of the request that sent the body, and a `set_error_handler` that throws over one answers `500` instead of nothing at all.** PHP reports body limits (`post_max_size`, `max_input_vars`, `max_file_uploads`, a malformed multipart boundary, each upload error) as warnings, and those used to fire on the worker's own stack before the request started: the bootstrap-installed handler ran outside any request with no fiber and no user frame, read the *previous* request's superglobals — a Sentry-style handler reported another client's URI and headers for a limit this client exceeded — and if it threw, the client's connection closed with no status, no body and nothing in the log. Each request now reads its body inside itself, after its own `$_SERVER` is complete, so a throwing handler is an ordinary uncaught exception: `500`, a log line, a root span, no effect on the worker's other requests. One boundary is the same one PHP-FPM has: `$_POST` and `$_FILES` do not exist yet while the body is being read, because reading it is what produces them. Two smaller changes travel with it: a request's superglobals are destroyed when it ends rather than when the next one overwrites them, so PHP that runs between requests — a `__destruct` under GC, a session save handler, worker-script code after `Worker::serve()` — sees them undefined instead of stale; `$_ENV` is deliberately excluded and keeps what a `.env` loader wrote at boot.

- **Worker mode: a `php://input` handle an application keeps past the end of its request is closed together with the body it reads through, instead of being left naming freed memory.** Such a handle — in a static, a global, a PSR-7 `ServerRequest` cached per worker — used to return a stale body on later requests: another request's or an empty one. The end of a request now closes those handles alongside the body, so a kept handle raises PHP's own catchable "supplied resource is not a valid stream resource" at the point of misuse, and an `is_resource()` guard sees `false` and takes its no-body branch. Reading `php://input` inside the request that received the body is unaffected, including across a pause. Two cases still read stale data rather than raising, so do not lean on the error firing: a handle wrapped in an object that forbids closing its stream — `SplFileObject('php://input')` — is deliberately left open, since closing underneath it would leave the object holding a dangling pointer; and a handle left open when `request_parse_body()` buffers the body a second time goes on reading the first copy. Other modes release handle and body together and were never affected.

- **Worker mode: `$_POST`, `$_FILES` and the POST half of `$_REQUEST` are populated again — they were empty for every request a worker ever served.** The body reader was picked once per worker, on a boot request that carries no body, and never again: an ordinary HTML form arrived complete and was thrown away in full, `$_POST` was `[]`, `$_FILES` was `[]`, and file uploads — including through `$request->file()` — did not work at all, silently. JSON APIs reading `php://input` were unaffected, which is why this stayed hidden. Both `application/x-www-form-urlencoded` and `multipart/form-data` are covered, under exactly the conditions PHP applies elsewhere (`enable_post_data_reading` included), and for multipart `php://input` is now empty as in every other SAPI, since PHP consumes the body to build `$_FILES`. Two notes for code written against the old behavior: `request_parse_body()` on a POST now returns an empty pair — read `$_POST`/`$_FILES` directly; it remains the way to parse `PUT`/`PATCH` — and the buffered body now belongs to its request and is released when it ends, so a `php://input` handle kept in a static no longer reads it later (see the entry on kept handles). Only worker mode was affected.

- **Worker mode: an upload's temporary file is deleted when its request ends, and a parsed request body no longer costs its worker memory.** Both cleanups ran only at end-of-request, which a worker never reached: every upload would have leaked its temp files into `upload_tmp_dir` and every POST a full copy of its body, for the life of the worker — and a request that read `php://input` already leaked its buffered body the same way before body parsing existed. Each request now releases its own. A paused request carries its uploads with it, so its files stay recognised by `is_uploaded_file()` and `move_uploaded_file()`.

- **Scraping metrics or introspection while a `Shared\*` object is being released no longer hangs the server thread that did it, permanently.** Every enumeration of the shared-state registry — the `Shared\*` gauges on `/metrics`, `/__ox_shared/summary` and `/__ox_shared/entries`, the pool-eviction scan, the reclaim a worker runs when it exits — walked it lazily under a read lock, and releasing the last handle to an object takes the write lock on the same shard: a release that happened during a walk made the walking thread wait on a lock it was itself holding, forever, with later `Shared\*` creations in that shard queueing behind it. Only a restart recovered. The registry is now read into a snapshot before anything is handed to the caller, so releases happen with no lock held; the same fix covers shutdown.

- **Worker mode with a dynamic pool now sizes itself from real demand, instead of climbing to its ceiling after the first request and staying there.** The scale manager and the workers measured idleness on different clocks, so every worker that had served even one request reported an idle age of zero from then on: nothing looked idle, the pool spawned to its ceiling, and the only retirable worker was the one just spawned — a silent spawn/retire cycle for the life of the process, with `PHP_WORKERS_IDLE_SECONDS` never applying to a worker actually serving. Both sides now read the same clock, which neither can choose. Static pools (`PHP_WORKERS=N`) and other modes were never affected.

- **Scale-down no longer offers up a worker that is still serving.** The idle stamp records when work last *arrived*, not whether it has finished, so a worker inside one long request was indistinguishable from an idle one and could be committed to a retirement it could not complete — its slot handed away while the request still ran. The manager now skips any worker with requests in flight.

- **A request that pauses no longer resumes holding another request's data.** Each request's copy of what it was asked for — path, headers, cookies, query string, body, request id — lived in a single slot per worker thread, and the next accepted request overwrote it: a resumed request read somebody else's through `$request->headers()`, `->cookie()`, `->query()`, `->body()`, `->path()`, `->ip()` and `oxphp_request_id()`, while its superglobals stayed its own, so `$_GET['id']` and `$request->query('id')` disagreed inside one request with no error anywhere. Each paused request now takes its data with it, and the SAPI's own view — method, query string, content type, cookie header — is re-pointed at it on resume. Requests that never pause, and traditional serving, were never affected.

- **Worker mode: `php://input` no longer hands a paused request another request's body.** The buffered body and the body-already-read mark were held once per worker thread, and the next request replaced both — so `file_get_contents('php://input')` after a pause returned another client's payload verbatim, and a request that read on both sides of a pause got two different bodies. Body state now travels with the paused request, so `php://input` reads the same thing before and after, and it is its own.

- **A PHP fatal raised while the server was handing request data to PHP no longer takes the whole process down.** Filling `$_SERVER` and the `headers()`/`cookies()`/`query()` accessors allocate per entry, so any of them can be where `memory_limit` lands — and that fatal jumped past the server's cleanup, after which the request's own close path used the stale state and the process aborted, taking every in-flight request on every worker with it. The trigger is ordinary — an application running close to its limit; first seen on a WordPress install with 256 MB. The server now finishes reading its request data before handing any of it to PHP, so a fatal in that window costs a `500` for the request that caused it and nothing for anyone else.

- **Worker mode with a dynamic pool: retiring a worker now retires it, and the process can still stop afterwards.** A worker-mode thread never read the retirement flag, so the pool shrank on paper — `oxphp_workers_current` dropped — while the thread kept serving, and the join scheduled for it meant `SIGTERM` never completed: orchestrators killed the container at their grace period. One retirement over the life of the process was enough, and the first lull in traffic produces one by itself. The thread now leaves the first time it finds itself with nothing in flight, and a completed retirement is logged (`Scale-down: retired worker thread stopped`). A worker holding a request that never ends does not reach that moment and still exits when the queue closes, as before. Static pools and other modes were unaffected.

- **`$request->query()` and `$request->cookie()` now decode their values, so they no longer disagree with `$_GET` and `$_COOKIE`.** Both handed back raw slices of the wire form, so any non-ASCII value differed between the two APIs, and an encoded parameter *name* was unfindable — `query('ключ')` returned `null` for a parameter that was plainly there. Both now follow the rules PHP applies when building the superglobals (query names and values decode with `+` as a space; cookie values keep `+` literal; cookie names are not decoded), byte-exact, so a binary cookie value — a signed token, `random_bytes()` — still verifies. `queryString()` remains the raw form, and `query()` still reports the name the client sent (`a.b`) where `$_GET` mangles it (`a_b`). Code that compensated with `rawurldecode()`/`urldecode()` now decodes twice and must drop the extra call. `query()` parses at most 1000 parameters; a deployment that raises `max_input_vars` past that will see `$_GET` hold parameters where `query()` stops.

- **`$request->cookie()` reports a present-but-empty cookie as `''` instead of `null`.** A zero-length value was treated as a missing cookie, so a caller could not tell "sent empty" from "not sent". Only a genuinely absent cookie now yields `null` (or `$default`); `query()` already drew that line and is unchanged.

- **Worker mode: an output buffer a request leaves open is no longer flushed into the next request's response.** A request ending with an unclosed `ob_start()` sent an empty body to its own client, and its buffered content was prepended to whatever the worker served next — one client's content delivered to another, with nothing in the log. The buffers a request opens are now closed where the request ends, as in every other SAPI; a request parked mid-buffer keeps its own.

- **Worker mode: an output buffer stays with the request that opened it across a suspension.** The buffer stack belonged to the worker thread, so a request served during another's pause wrote into the parked request's buffer — its own client received an empty body, its content went to the other client — and the parked request's buffered content was flushed to the wire on the way into the pause, past the handler it was buffered for. A request's buffers now travel with it and come back with their content on resume.

- **Worker mode: a response carries `Content-Type` exactly once, and a response with no body carries it at all.** The first response a worker sent duplicated the engine's default `Content-Type` — the wrong one first if the script had changed `default_charset` — and a response that wrote no body carried none where every other SAPI sends the default. Both are gone.

- **Worker mode: a response that sets no `Content-Type` of its own no longer costs its worker memory.** The engine's default content-type string was allocated per response and never returned in worker mode — about 3 MB per hundred thousand requests, growing for the life of the worker. The per-request reset now returns it, and a suspended request keeps its own content type across the pause (it is what output handlers such as `mb_output_handler` read to decide whether to convert).

- **Worker mode: a request that ends in a fatal error no longer costs its worker memory for the rest of the worker's life.** A fatal abandons the request where it stands, and everything the interrupted script was holding — its frames and their variables, the arguments of interrupted internal calls, its closures and their captures, the stack they stood on — stayed allocated in a worker that keeps serving: roughly 900 bytes for a trivial script, the full payload when the request fataled holding something large, growing without bound through a bad deploy or a hot path throwing on every call. All of it is now released where the engine would have released it.

- **Worker mode: a fatal raised while a generator is running no longer gives up the generator's variables twice.** The generator's frame was released both with the interrupted call chain and by the generator's own close, and the second release landed on values something else still held — freed while in use. The worker now takes the frame off the generator, so its values are given up exactly once.

- **Worker mode: a fatal error no longer switches off the cycle collector for the rest of the worker's life.** A fatal stops the engine's collector recording cycle candidates, and the flag is lowered only where a request starts up — once per worker. After its first fatal a worker never collected another cycle: every cyclic structure any later request built lived until process exit. The flag is now lowered where the worker goes back to serving. The one exception — a fatal raised from inside a collection, where the same flag stops the collector re-entering a half-marked run — instead retires the worker gracefully once its current request finishes, under the same recycle counter as `Worker::scheduleExit()`.

- **Worker mode: a fatal raised by a shutdown function is cleaned up like any other.** The engine swallows such a fatal behind a guard of its own, so the worker was handed what looked like a normally-ended request while the abandoned frames, the stack pointer inside them and both fatal flags stayed exactly where they were. Shutdown functions are where frameworks flush logs and report errors, which makes this the fatal most likely to follow another one. The worker now recognises it and runs the same cleanup it runs for a fatal in the handler.

- **Worker mode: an exception thrown by a shutdown function is reported instead of disappearing.** Under every other SAPI it becomes a fatal; a worker calls those callbacks inside a frame of its own, where that path does not run, so the exception vanished — no log, no report, a response indistinguishable from a healthy one, in exactly the code applications write to make failures visible. It is now reported in the response and log of the request that raised it. `set_exception_handler()` is deliberately not called for it: in worker mode that slot belongs to the worker, and calling it here would run one request's handler for another request's exception.

- **Worker mode: `error_get_last()` no longer answers with another request's error.** The last error lived on the worker thread and was cleared only between requests, so a request served right after a fatal read that fatal as its own — and shutdown-function code asking "did this request die on a fatal?", which is how frameworks catch fatals, reported failures for requests that had succeeded, with another request's message, file and line. A request now starts with no last error, and a suspended one takes its own with it.

- **Worker mode: the functions a request registers with `register_shutdown_function()` stay with it across a suspension.** The registry belonged to the worker thread, and the end of whichever request finished first ran and discarded everything in it: a parked request's callbacks ran inside another request — echoing into another client's response, writing the wrong session — and never ran for their own, silently on both counts. Shutdown functions now travel with their request and run at its own end, into its own response.

- **Background tasks: what a task registers with `register_shutdown_function()` no longer piles up on the thread that ran it.** Task threads close their request only when the process stops, so every callback a task registered — with its closure and everything it captured — lived until server shutdown and then ran, all together, for tasks long finished. A task's registrations are now discarded when the task ends; registering a shutdown function from a task consequently does nothing, which is the honest form of a callback with no response to write into. Requests are unaffected.

- **Worker mode: a worker that retires itself now ends the requests it was still serving, instead of dropping them.** On `Worker::scheduleExit()`, the memory ceiling or the error breaker, the requests still parked on the worker were unwound without their own state: their `finally` blocks, destructors and shutdown functions ran against another request's superglobals and output, and their responses were replaced by the generic `500 PHP Worker Error` page with nothing in the log. A retiring worker now ends each parked request the way a shutdown drain does — on its own state, uncatchably, running its own shutdown functions, answering its own client with its own output plus a `503` and `Retry-After`, next to a log line naming its script. Workers that retire with nothing else in flight behave as before.

- **Worker mode: concurrent requests are no longer indistinguishable to libraries that track the current fiber.** Requests ran on fibers the engine did not expose, so `Fiber::getCurrent()` returned `null` and every library that keys per-task state on it filed all concurrent requests under one key, silently sharing state: `open-telemetry/context` (the active span and everything that reads it), `revolt/event-loop` (`Suspension` identity, `FiberLocal`, and all of AMPHP above it), `monolog` (cycle-detection depth), `spiral/core` (container scope). Each request — and each `oxphp_async()` task — now runs as a real `Fiber` with a distinct identity, so those libraries isolate them the way they do under any other fiber-based runtime. Nothing about how a handler is written changes.

- **`oxphp_async_await_all()`, `oxphp_async_await_race()` and `oxphp_async_await_any()` no longer leak one reference per result.** Each copied a promise's result into its return array without releasing the temporary, so every string, array or object delivered stayed alive until the request's allocator was torn down — a bounded overshoot in traditional serving, unbounded growth in worker mode, where a fan-out loop awaiting sizeable payloads leaked the full payload every iteration. Scalar results and `oxphp_async_await()` were never affected, and nothing observable to a script changes.

- **Graceful shutdown: work that keeps running after `oxphp_finish_request()` is no longer discarded the moment shutdown begins.** A request that answers early and keeps working — mail, cache writes, webhooks — has already released its connection, and the drain counted live connections alone: it saw nothing to wait for, skipped its window, and tore the workers down mid-flight, contradicting the documented behavior. The drain now counts requests still executing on PHP workers alongside live connections: post-response work that fits the window completes, work that outlasts it is cancelled at the deadline like any other in-flight request, and the two shutdown log events carry an `in_flight_requests` field next to their connection counts. Tasks started with `oxphp_async()` and never awaited remain bounded by the async pool's own shutdown, not by the drain window.

- **Worker mode: a suspended request no longer resumes with another request's superglobals.** Suspending saved the engine's internal superglobal slots, but userland reads separate symbol-table entries that each incoming request rebinds — so a resumed request read the `$_GET`, `$_POST`, `$_COOKIE`, `$_REQUEST` and `$_SERVER` of whichever request ran while it was parked: one concurrent client's data read by another, with no error and no log line. Resuming now rebinds those entries as well.

- **Worker mode: `$_REQUEST` is now rebuilt for every request.** A worker built it once and never again, so a script reading `$_REQUEST` directly saw the merged query string, form fields and cookies of whichever request first loaded it — and a handler compiled at boot saw it empty on every request — while `$_GET`, `$_POST` and `$_COOKIE` stayed correct. The defect needed OPcache enabled; framework abstractions were unaffected, legacy code and plugins touching `$_REQUEST` directly were not.

- **Worker mode: values written into `$_ENV` at boot no longer vanish part-way through a worker's life.** A `.env` loader — phpdotenv, symfony/dotenv — writes into `$_ENV` without touching the process environment, and the next rebuild of that array (compiling a lazily-autoloaded file that mentions `$_ENV`, a `filter_input(INPUT_ENV)` call) silently dropped every one of those values on an arbitrary later request, while `getenv()` kept working. `$_ENV` is now pinned for the life of the worker and holds both the process values and what the application wrote. `filter_*(INPUT_ENV)` keeps working and reports the process environment, as in every other SAPI — from the snapshot taken when `$_ENV` was first built on that worker, so application writes to `$_ENV` are not in it and a later `putenv()` shows up in `getenv()` but not there. This assumes the default `auto_globals_jit=1`.

- **Worker mode: a worker no longer dies with a segmentation fault on its second request under a real application.** Every request after a worker's first was handed to a recycled fiber as though it were resuming, which installed a per-fiber state snapshot that had never been written: the header list became an empty structure, the superglobal slots undefined, the status `0`. Whether that crashed depended on the heap — a trivial echo handler ran millions of requests clean, which is why synthetic tests missed it, while WordPress crashed on each worker's second request at its `header()` calls. The snapshot is now installed only where it is written: when a suspended request actually resumes.

- **Overload is now shed on how long a request has waited, not on the queue depth at the instant it arrived — a burst no longer turns into 529s while the worker pool is idle.** Rejecting at a full queue meant the threshold was `QUEUE_CAPACITY`, a number unrelated to how long a request can afford to wait: measured on a 14-core host, 38–55 % of responses were 529 at high concurrency against endpoints the pool served with zero errors once allowed to queue. Requests now wait up to `QUEUE_WAIT_TIMEOUT_MS` (default 1000) for a slot, in arrival order, under a single deadline stamped on arrival that also covers the wait inside the queue — a request a worker reaches after its deadline is refused at pickup rather than executed. A slow deployment can now see 529s where it previously saw very late 200s: raise the budget if your clients genuinely wait longer, or set `0` to restore reject-immediately.

- **`oxphp_busy_workers` now counts busy workers, and `oxphp_workers_idle` idle ones.** Both were driven by a dispatch counter that included queued requests — so `busy` rose past the worker count and `idle` read zero on an unsaturated pool — and a client disconnect mid-request skipped the decrement, so the gauges only ever climbed. Both are now derived at scrape time from the workers themselves: `oxphp_busy_workers` never exceeds `oxphp_workers_current` and their ratio is a true utilization fraction; queued and waiting requests appear in `oxphp_pending_requests`; in worker mode a thread multiplexing many requests counts once.

- **`oxphp_queue_wait_us` no longer counts script execution time as queueing.** The histogram measured everything between dispatch and response, so a server with an empty queue reported its own PHP latency as queue wait. Execution time is now subtracted; values on an idle server drop to near zero.

- **The server no longer leaks memory on every request.** Each request permanently retained its small cancellation record — about 128 bytes, on every request in every routing mode, roughly 11 GB per day at a thousand requests per second, until the OOM killer. The record is now released on completion: a load run that previously climbed to 4.9 GiB over 40 million requests now holds below 50 MB.

- **Worker mode: completing a request no longer cancels async promises owned by other in-flight requests on the same worker thread.** Per-request cleanup drained the whole thread's promise table, so a request finishing while a sibling was suspended in `oxphp_async_await()` cancelled the sibling's still-running task — its await then failed with `TimeoutException` at the full deadline — and the drain could stall the shared scheduler for up to 5 seconds per orphaned promise. Ownership is now tracked per request fiber and each request cleans up only its own; the thread-wide drain remains where it is correct (traditional-mode shutdown and final worker teardown).

- **Worker mode: a request that finishes while its own fire-and-forget `oxphp_async()` task is still running no longer stalls the worker's scheduler.** Cleaning up the unsettled task's captured state blocked the worker thread for up to 5 seconds, freezing every other request multiplexed on it. The cleanup is now deferred and reclaimed off the hot path once the task settles; the blocking form remains only during worker shutdown, where the thread is exiting anyway.

- **Database auto-instrumentation now populates its span attributes — `OTEL_APM_SLOW_QUERY_MS` and `OTEL_APM_DB_CAPTURE_PARAMS_ENABLED` are no longer inert.** Both knobs were parsed and reported but never read, and PDO/mysqli spans carried only timing. Spans now carry the OpenTelemetry semantic-convention attributes `db.statement` (literal values obfuscated to `?` so PII stays out of traces), `db.operation`, `db.system`, `server.address`, `server.port` and `db.name`; a query at or over the slow threshold is flagged `oxphp.db.slow=true`, and bound parameters are recorded — raw, so opt in only where PII in traces is acceptable — in `db.params` when capture is enabled. `db.statement` is read from each call's own arguments or the statement object's own `queryString`, so it can never be another statement's SQL; a `mysqli_stmt::execute` span carries none (the SQL is on the `prepare` span). Cache, HTTP-client and file-I/O hooks are unchanged.

### Added

- **`RUNTIME_HOOKS=streams` makes blocking socket reads and `stream_select()` suspend the fiber instead of the worker thread.** A blocking read on a `tcp://` stream parks the current request or async-task fiber and lets the worker serve other requests until the answer arrives. It covers everything built on PHP streams — `fsockopen()`, `stream_socket_client()`, HTTP stream wrappers, and the database and cache clients riding on them (mysqlnd, so PDO_MySQL and mysqli; phpredis) — with no application changes; one configuration caveat: a MySQL DSN must name `127.0.0.1`, since the client reads `localhost` as a unix socket. `stream_select()` is covered too: the hook waits for readiness, then hands the call to PHP with a zero timeout, so PHP keeps its own return semantics. Like the `sleep` category it is off by default and inert outside a fiber, and the native contract is preserved: socket timeouts behave unchanged (a deadline is noticed on the next scheduler tick, so it fires at earliest ~100 µs late), a timed-out read still reports `timed_out`, and stream identity is untouched. Not covered: waiting for *write* readiness (deliberately — room in the send buffer is not stable the way read readiness is); ext/curl, so `curl_exec()` and the HTTP clients on it (Guzzle's default handler included); `unix://`/`udp://` streams; `socket_select()` and the wait inside `stream_socket_accept()`; `socket_export_stream()` streams; the connect and DNS phases; `ssl://`/`tls://` once crypto is active; and anything under a userland fiber scheduler (AMPHP, Revolt), where the hook steps aside. **One connection shared between concurrent fibers is safe and gains nothing** — and it is the normal shape of a worker-mode application, whose clients are opened at boot and handed to every request: a fiber parked on a read is parked mid-exchange, so a fiber claims a connection before using it — at the socket and at the PDO/mysqli/phpredis entry points — and keeps the claim to the end of its request; another fiber reaching the same connection waits, bounded by the smaller of `max_execution_time` and `default_socket_timeout` (30 s where neither is set), past which the call falls back on the client's own error with the reason in the server log. What a shared connection gains is the worker thread back while it waits; its exchanges still run one after another. Left uncovered deliberately: statement or result objects kept across requests, a second handle on a persistent connection mid-exchange, hand-written protocols on raw sockets that suspend between write and read, and connections reached outside any fiber (a destructor under GC). ⚠️ The boolean spellings `RUNTIME_HOOKS=1`, `true` and `all` mean *every* category, so a deployment carrying `1` for the sleep hooks picks up the socket hooks on upgrade with no edit of its own — name the categories explicitly to opt out. Cost, measured: 3–5 µs per socket round trip on a quiet worker, 7–11 µs with 200 fibers parked; a wide `stream_select()` pays ~0.65 µs per named descriptor, so a many-descriptor call that almost never waits — a request that is itself an event loop — is better left unhooked, while clients waiting on one connection pay around a microsecond.

- `QUEUE_WAIT_TIMEOUT_MS` (default `1000`): how long a request may spend waiting for a PHP worker before it is rejected with 529; waiting requests are admitted in arrival order. `0` rejects the moment the queue is full — the previous behavior — and applies no deadline inside the queue. The budget is one deadline stamped on arrival covering both waits, for a queue slot and inside the queue for a worker. A wait also ends early when the client goes away or the graceful-drain deadline arrives (those get 503 with the drain's retry window). Treat it as a latency budget, not a throughput knob — a waiting request holds its connection and its already-buffered body. Two cases deserve a shorter budget or `0`: applications that call back into the same server over HTTP, and deployments behind a balancer whose own timeout is shorter.

- `QUEUE_MAX_WAITING` (default: initial workers × 128, capped at half of `MAX_CONNECTIONS`; `0` = auto): how many requests may wait at once; past the cap a request is refused 529 immediately, counted under `oxphp_admission_refused_total{reason="waiting_full"}`. Without a cap, sustained overload accumulates waiters until every connection permit is taken and the server answers overload by not answering at all. How many requests can *usefully* wait is roughly workers × budget / handler latency — size it from your own latency; the configuration reference works the arithmetic through. Keep `PHP_WORKERS` + `QUEUE_CAPACITY` + `QUEUE_MAX_WAITING` under `MAX_CONNECTIONS`: the server warns at startup, and `oxphp config --check` reports, when the sum reaches the budget. On an HTTP/2-heavy deployment set it explicitly — one connection carries up to `H2_MAX_CONCURRENT_STREAMS` requests, so the connection-based default undercounts.

- `QUEUE_MAX_WAITING_BYTES` (default `67108864`, 64 MiB; `0` = auto): how much request-body memory the waiting set may hold between them. A body that would push the total past the budget is refused 529 at once, counted under `reason="waiting_bytes"`; a request carrying no body is never refused for it. The count cap above says nothing about size — the same waiters cost nothing on bodyless `GET`s and gigabytes on uploads. Raise it on upload-heavy applications, lower it on memory-capped containers; the two caps are counted separately, so the metric names which one to reach for. It does not cover bodies already handed to the queue (bounded by `QUEUE_CAPACITY`, in requests) or a body still being read off the connection.

- **`oxphp_admission_refused_total`**: counts requests answered without reaching a worker — shedding previously left no server-side trace at all, and refused requests inflated `oxphp_queue_wait_us` (they are now excluded from it). The `reason` label distinguishes the four overload reasons, all answered 529 — `queue_full`, `wait_timeout`, `waiting_full`, `waiting_bytes` — from `shutting_down` (503, with the drain's retry window) and `pool_unavailable` (500). Alert on the four overload reasons rather than the total, or an ordinary restart reads as a traffic spike. Requests waiting for admission occupy no worker: they appear in `oxphp_pending_requests`, deliberately not in `oxphp_busy_workers`.

- **Unhandled exceptions and fatal errors are now captured automatically on the request's root trace span.** A request that fails 5xx gets an OpenTelemetry `exception` event (`exception.type`, `exception.message`, `exception.stacktrace`, plus `exception.file` and `exception.line`) on the root SERVER span, with no `#[OxPHP\Apm\Trace]` attribute and no `oxphp_apm_error()` call, in all four serving modes, classless fatals included — a 500 becomes self-describing in the trace and lights up error inboxes that group by the exception event. Two boundaries: once a response has committed its status to the wire — streaming, or after `finish_request()` — a later fatal is logged only; and on the traditional request path an application whose `set_exception_handler()` renders its own error page (Laravel, Symfony, WordPress) consumes the exception before it becomes uncaught, so record it explicitly with `oxphp_apm_error($e)` from the framework's reporter — worker mode captures regardless, since the worker runtime catches the escaping exception itself. The existing `OTEL_APM_MESSAGE_MAX_BYTES` / `OTEL_APM_STACKTRACE_MAX_BYTES` caps apply.

### Changed

- **Breaking: in worker mode, an event loop can no longer be run from inside a request.** Revolt refuses to run its loop from within a fiber, and a worker-mode request is now a fiber, so `Revolt\EventLoop::run()` called from a request handler throws where it previously ran; everything else Revolt offers inside a request — `defer()`, `delay()`, `getSuspension()` — works as before. Move the call to the worker bootstrap, or drop it and let the server drive. Traditional, framework and SPA modes are untouched.

- **Worker mode: suspending or resuming a request's own fiber from userland is now refused rather than silently breaking the request.** `Fiber::getCurrent()` returns a real fiber inside a request now, but the server drives it: a userland suspend would park a request nothing will resume, a userland resume would run it a second time. Both throw `FiberError` at the point of the call, and the request continues undisturbed. Use `oxphp_sleep()` or await a promise to yield.

- **Worker mode: backtraces taken inside a request or a background task include one server frame**, `oxphp fiber loop`, below the application's own frames — the space in the name keeps it from colliding with any PHP function. Code that inspects `debug_backtrace()` by depth from the bottom should account for it.

- **Breaking: a malformed `MAX_CONNECTIONS` is now a startup error.** A value that is not a non-negative integer previously fell back to `10000` silently; it now fails at `oxphp serve` startup and at `oxphp config --check`, naming the variable — warranted because the default `QUEUE_MAX_WAITING` derives from `MAX_CONNECTIONS`, so a typo reshaped admission as well. An exactly-empty value still means unset.

- **Breaking: `QUEUE_CAPACITY=0` now means auto (workers × 128), and a malformed value is a startup error.** A literal `0` built a zero-capacity queue in which a request could only be handed over if a worker was already blocked waiting — never what an operator intends, and inconsistent with `ASYNC_QUEUE_CAPACITY`, where `0` has always meant auto. A malformed value previously fell back to the default silently. An exactly-empty value still means unset.

- **`oxphp_queue_wait_us` now has buckets reaching one second.** The boundaries stopped at 50 ms while a request may wait up to `QUEUE_WAIT_TIMEOUT_MS` — a second by default — so every wait worth acting on landed in `+Inf` together. Four boundaries are added (`100000`, `250000`, `500000`, `1000000`); existing `le` series keep their meaning, and the tail now matches `oxphp_request_duration_us` bucket for bucket.

- **The default `FRAME_OPTIONS` is now `SAMEORIGIN` instead of `DENY`,** matching the common default of nginx and Rails and unbreaking same-origin embedding (admin previews, dashboard widgets) that `DENY` blocked; cross-origin framing stays blocked. A deployment that relied on the built-in `DENY` to forbid *all* framing must now set it explicitly. An application's own `X-Frame-Options` or CSP `frame-ancestors` still overrides the server default entirely; an invalid value now falls back to `SAMEORIGIN`.

- **The documentation moved from `docs/en/` up to `docs/`.** With the translations gone the language directory named nothing, so every page lost a path segment (`docs/en/features/tls.md` → `docs/features/tls.md`). Links from the README, the changelog, the stub and `llms.txt` were rewritten; existing links into `docs/en/…` — from old release notes, issues, or search results — no longer resolve.

### Removed

- **The Russian and Chinese translations have left the repository for [oxphp.dev](https://oxphp.dev/).** `docs/ru/`, `docs/zh/`, `README.ru.md` and `README.zh.md` are gone — the site is where the translations are maintained, alongside languages the repository never carried, and keeping copies in git meant every documentation change either tripled or drifted. The English documentation stays under `docs/`. Links into `docs/ru/…` or `docs/zh/…` no longer resolve; the language switchers point at the site.

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
- `OxPHP\Http\Request::file()` and `Request::files()` now return the uploaded files instead of always `null` / `[]`. The object upload API documented in `docs/php/request-api.md` is wired to the request's parsed `$_FILES`: `file('avatar')` yields an `OxPHP\Http\UploadedFile` (or the first file of an array field `name="avatar[]"`, or `null` when the field is absent), `files('photos')` returns every file of one field, and `files()` returns a flat list of every upload. The scalar (`name="avatar"`), sequential-array (`name="avatar[]"`) and associative-array (`name="avatar[key]"`) `$_FILES` shapes are all handled. The `UploadedFile` accessors (`name()`, `clientType()`, content-detected `type()`, `size()`, `tmpPath()`, `error()`, `isValid()`, `moveTo()`) were already present; only the two `Request` entry points were stubbed, so code following the documented examples saw no files and had to fall back to the `$_FILES` superglobal.

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
[`docs/shared-state/shared-naming.md`](docs/shared-state/shared-naming.md).

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
- `OxPHP\Shared\*` method naming unified across types. The renames below are mechanical (semantics and signatures unchanged), and ship without alias shims — update call sites with sed before upgrading. The rules are documented at [`docs/shared-state/shared-naming.md`](docs/shared-state/shared-naming.md).
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
- Naming guide for `OxPHP\Shared\*` published at `docs/shared-state/shared-naming.md`. New `Shared\*` primitives must follow the rules listed there (`get`/`load` for reads, `set`/`store` for writes, `count()` via `\Countable`, `is*` for boolean getters, `try*` for non-blocking attempts, `fetch*` for atomic RMW returning prev value).
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

- New PHP class `OxPHP\Server\Worker` — unified runtime handle for worker introspection. Methods: `current`, `isWorkerMode`, `getId`, `getStartTime`, `getRequestCount` (1-based count of requests handled by this OS thread; grows in both modes since traditional reuses persistent threads), `getMemoryUsage`, `getRss`, `getMaxMemoryBytes`, `scheduleExit`, `isExitScheduled`, `getExitReason`, `serve`. Available in both traditional and worker modes. See `docs/php/worker-class.md`.
- New PHP exception `OxPHP\Server\Exception\InvalidServeContextException`, thrown by `Worker::serve()` when called outside worker mode.
- `Worker::scheduleExit()` — application-driven worker recycling. Marks the current worker for graceful exit after the active request completes; the supervisor respawns a fresh worker, re-running the outer scope. Companion methods `Worker::isExitScheduled()` and `Worker::getExitReason()` expose the pending exit state. No-op in traditional mode.
- Environment variables `ENTRY_FILE` and `WORKER_MODE_ENABLED` — single canonical entry script plus an explicit worker-mode toggle. `ENTRY_FILE` selects the routing mode by extension (unset = direct mapping, `*.php` = front controller, non-`.php` = SPA fallback). When `WORKER_MODE_ENABLED=true`, `ENTRY_FILE` must point at a `.php` script and the server runs persistent workers. Resolution accepts relative paths (against `DOCUMENT_ROOT`, including `..`) and absolute paths. The startup `mode_decided` log line records which combination was selected. See `docs/operations/configuration.md`.

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

See the [Shared State overview](docs/shared-state/shared-state.md) for the concept and mental model, and the per-type docs for API reference, runnable examples, and gotchas.

- [`Shared\Counter`](docs/shared-state/shared-counter.md) — atomic int64 with `inc` / `dec` / `add` / `compareAndSet` / `addBatch` / `reset`.
- [`Shared\Flag`](docs/shared-state/shared-flag.md) — atomic bool with `test` / `set` / `clear` / `exchange` / `compareAndSet`.
- [`Shared\Once`](docs/shared-state/shared-once.md) — run-once container with `init(factory)` / `trySet` / `get`. Reentrant `init` throws `DeadlockException`.
- [`Shared\Mutex`](docs/shared-state/shared-mutex.md) — poisoning mutex guarding a stored value. `with(callable, timeout)` and `tryWith(callable)` scope-guard the critical section; poisoning isolates failed-mid-update state.
- [`Shared\Channel`](docs/shared-state/shared-channel.md) — bounded MPMC queue with fiber-aware `send` / `recv`. `sendMany` / `recvMany` for batching.
- [`Shared\Map`](docs/shared-state/shared-map.md) — concurrent `string → mixed` store with `get` / `set` / `update` / `getOrSet` / `setIfAbsent` / batched `setMany` / `getMany` / `removeMany`. Per-instance cap via `maxEntries`.
- [`Shared\Pool`](docs/shared-state/shared-pool.md) — bounded object pool with lazy factory, optional destroy callback, strict `maxSize` budget, per-thread affinity, and idle-timeout eviction. `with($body)` scope-guards acquire/release.

#### Shared-registry observability

See [Shared Observability](docs/shared-state/shared-observability.md) for the operator's reference.

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

All Shared-state tunables are read at startup via the `SHARED_*` env-var prefix (fallbacks to `OX_SHARED_*` and bare keys). See [Shared State → Configuration](docs/shared-state/shared-state.md#configuration) for the full table. Highlights:

- `SHARED_MAX_ENTRIES` (default 100 000) / `SHARED_MAX_BYTES` (default 1 GiB) — global caps.
- `SHARED_CYCLE_DETECT_DEPTH` (16) / `SHARED_CYCLE_DETECT_EDGES` (10 000) — cycle-check walker bounds.
- `SHARED_INTROSPECTION_ENABLED` / `SHARED_METRICS_ENABLED` — per-feature kill switches.
- `SHARED_LOCK_DIAGNOSTICS` (`off` / `warn` / `strict`) — escalates reentry / deadlock signals.

#### Rust plugin-author API

- `MapInner::retain<F>` — exposes `DashMap::retain` with proper refcount release for nested `SharedValue::Shared` targets. Lets plugin authors prune a map in a single shard-walk instead of the N-lock `keys()`+`remove()` pattern.

#### Documentation

- [`docs/shared-state/shared-state.md`](docs/shared-state/shared-state.md) — overview, mental model, type-selection matrix, canonical hand-rolled-counter → `Shared\*` migration example.
- Per-type docs for all seven Shared\* v1 types (see list above).
- [`docs/shared-state/shared-observability.md`](docs/shared-state/shared-observability.md) — introspection endpoints, Prometheus catalogue, diagnostic playbooks.
- [`docs/shared-state/migrating-to-external-store.md`](docs/shared-state/migrating-to-external-store.md) — when and how to promote `Shared\*` state to Redis / NATS / Kafka.

#### Tooling

- `tests/soak/pool_soak.sh` + `tests/soak/workload.php` — manual (non-CI) 24h soak harness for pre-release Shared\Pool stability sign-off. Not wired into `tests/run_all.sh`; [invocation notes in the observability doc](docs/shared-state/shared-observability.md#long-running-soak-harness).

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

[0.11.0]: https://github.com/oxphp/oxphp/releases/tag/v0.11.0
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
