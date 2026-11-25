---
title: Runtime Hooks
description: Opt-in RUNTIME_HOOKS categories that make sleep(), socket reads, stream_select(), connects, TLS and cURL waits suspend the fiber instead of the worker thread.
---

# Runtime Hooks

`RUNTIME_HOOKS` replaces blocking PHP builtins with implementations that suspend the current fiber instead of pinning the worker thread, so code you cannot edit — a framework, a database client, an HTTP library — lets the worker run other requests while it waits. It is off by default and enabled by category.

## How It Works

| Category | What it hooks |
|----------|---------------|
| `sleep` | Native `sleep()` and `usleep()` suspend the current fiber exactly like `oxphp_sleep()`/`oxphp_usleep()` — including the contexts where those two cannot suspend either and wait on the worker thread instead (see below) |
| `streams` | Two waits suspend the current fiber instead of pinning the worker thread: a blocking **read** on a `tcp://` socket stream, and **`stream_select()`**. The read covers clients that block on one socket — `fsockopen()`, `stream_socket_client()`, HTTP stream wrappers, mysqlnd (PDO_MySQL, mysqli), phpredis; `stream_select()` covers loops that wait on several at once. No code changes to the client either way; what the other requests on the worker can see while one waits is [covered separately](runtime-hooks-shared-state.md#hooked-waits-and-shared-application-state). Clients that wait some other way are unaffected (see below) |
| `net` | The waits that happen below PHP streams, where the `streams` read hook cannot see them, suspend the current fiber instead of pinning the worker thread: connecting, the TLS handshake, TLS reads, writes, and reads on sockets that are not TCP. Needs `streams` listed alongside it, and works on Linux only; [The `net` category](#the-net-category) lists what it covers and what it does not |
| `curl` | The waits of a cURL transfer suspend the current fiber instead of pinning the worker thread: everything `curl_exec()` waits for, from resolving the host name to the last byte of the answer, and the wait inside `curl_multi_select()`. That covers the HTTP clients built on ext/curl — Guzzle with its default handler, Symfony HttpClient's `CurlHttpClient`, and the SDKs built on either. A cURL call made on a handle another fiber is in the middle of a call on waits for it, or is refused with an exception (`curl_multi_exec()` returns at once instead where the fiber can be suspended), so requests sharing a client that keeps one multi handle for all of them, as Symfony HttpClient does and Guzzle does for its asynchronous requests, take turns calling on it while their transfers run side by side ([cURL Under Runtime Hooks](runtime-hooks-curl.md#curl-transfers)). Needs neither `streams` nor `net`, and works on Linux only, with libcurl loaded as a library of its own, as it is in the official images |

### The `net` category

The waits that happen below PHP streams, where the `streams` read hook cannot see them, suspend the current fiber instead of pinning the worker thread:

- **connecting** (`stream_socket_client()`, `fsockopen()` and every client built on PHP streams — mysqlnd, so PDO_MySQL and mysqli, and phpredis among them);
- the TLS **handshake**;
- **reads on `ssl://`/`tls://` streams** once crypto is active;
- **writes** that have to wait for room in the send buffer;
- **stream reads on `unix://`, `udp://` and `udg://` sockets, on `stream_socket_pair()` streams and on the streams `socket_export_stream()` makes from sockets that are not TCP**.

`net` needs `streams` listed alongside it (see [Checking That Hooks Are Active](#checking-that-hooks-are-active)), and works on Linux only.

These waits are not covered:

- waits inside libpq (`pgsql`, `pdo_pgsql`) and libmemcached, which call `poll()` from their own libraries;
- DNS resolution;
- `stream_socket_recvfrom()`;
- writes on a plain (non-TLS) stream with an infinite timeout (`default_socket_timeout=-1`);
- persistent connects (`pfsockopen()`, `STREAM_CLIENT_PERSISTENT`, and the persistent modes of the clients built on them).

libcurl's waits are the `curl` category's.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `RUNTIME_HOOKS` | _(off)_ | Opt-in replacement of blocking PHP builtins with fiber-suspending implementations. `1`/`true`/`all` enables every hook category; a comma-separated list enables specific categories (e.g. `RUNTIME_HOOKS=sleep,streams`) |

`1`, `true` and `all` enable **every** category, `streams` included. A deployment already setting `RUNTIME_HOOKS=1` for the sleep hooks therefore starts hooking sockets on upgrade without any edit of its own; list the categories explicitly (`RUNTIME_HOOKS=sleep`) if that is not what you want. Enabling `streams` also patches one entry of PHP's stream ops table in memory at startup; the page is put back the way it was found, but on a platform where its original protection cannot be determined it stays writable — a small loss of hardening, reported in the server log. Enabling `net` patches the same kind of entries in the ops tables of the other socket transports and of OpenSSL, on the same terms, and makes the bridge library answer `poll()` for the calls PHP's own code makes — every other caller, the server's own included, is passed straight through to libc. Enabling `curl` makes it answer the calls libcurl makes as well, and replaces the handlers of the cURL functions that run a transfer or change a handle, so that they can claim it.

## Checking That Hooks Are Active

**Checking that the setting was picked up.** The server reports the categories it installed, as a parsed set, in two places. On the internal server:

```console
$ curl -s localhost:9090/config | jq .runtime_hooks
[
  "sleep",
  "streams",
  "net",
  "curl"
]
```

and from PHP, for a deployment whose internal server is not reachable or not enabled:

```php
<?php
echo json_encode(oxphp_server_info()['runtime_hooks']);   // ["sleep","streams","net","curl"]
```

Both read the same value, and both report the set rather than the string: `RUNTIME_HOOKS=all`, `RUNTIME_HOOKS=1` and `RUNTIME_HOOKS=sleep, streams, net, curl` are one state and all three come back as `["sleep","streams","net","curl"]`. An empty list means no hook is installed — which is the answer for the variable unset, for `RUNTIME_HOOKS=0`, for a value naming *only* categories this build does not know, and for a misspelled variable name, since all four enable nothing. A value mixing the two is not that case: `RUNTIME_HOOKS=sleep,bogus` reads as `["sleep"]`, because the unrecognised name enables nothing while the recognised one still does. So a non-empty list is not by itself evidence that every name in the value was understood: each unrecognised name gets its own line in the server log at startup, and that line is the only place they are named. This is worth checking after any change to the setting: an unrecognised environment variable is not an error anywhere, so the difference between "enabled" and "misspelled" is otherwise only visible as latency.

**Installed is not the same as in effect.** What that list reports is which builtins had their handlers replaced, which happens at startup and does not depend on the routing mode. Where there are no fibers at all the replacements delegate to the handlers they replaced, so a traditional-mode server reports `["sleep","streams","net","curl"]` while behaving natively, with one exception: `curl_multi_getcontent()` on a transfer in a multi handle that may still write to it returns a copy, as it does in worker mode ([cURL Under Runtime Hooks](runtime-hooks-curl.md#curl-transfers)). A `php` CLI process reports `["sleep","streams"]`: the engine is part of that executable, so `net` cannot tell PHP's waits from the program's own and stays off, and `curl` stays off with it. Read the list as "this process is set up for the hooks", and the routing mode as whether anything can use them. Within `streams` it is narrower still: the socket-read part can fail to install on its own, on a platform where the stream ops table cannot be made writable, while `stream_select()` and the database entry points stay hooked. The category is reported either way, and the only signal for that partial failure is a line in the server log saying socket reads stay blocking. That line follows PHP's error logging: with `error_log` unset, which is the default, it reaches the server's own log as a `warn`, and `LOG_LEVEL=error` hides it. The field says `streams` either way. The same is true of the unrecognised-category line above, and of the lines `net` and `curl` write, below.

`net` is reported only when it is actually in place: listed without `streams` it stays off with a line in the server log saying so, and on a platform that is not Linux, or where the server's own code cannot be told apart from PHP's at startup, or where PHP's `poll()` is not bound to the one the server intercepts, it stays off with a line saying connects and TLS waits stay blocking — so a `net` missing from the list after asking for it is the answer, not a gap in the report. The one thing the list does not tell you is where OpenSSL lives: the waits OpenSSL makes — the TLS handshake, including the one at the end of an `ssl://` connect, and reads and writes on a TLS stream once crypto is active — are parked only when OpenSSL is built into PHP, as it is in the official images. With OpenSSL loaded as a separate shared extension (`extension=openssl`), those waits are not PHP's own as far as the server can tell, so they keep the thread, while connects, `unix://` and the other reads still suspend and `net` is still reported.

`curl` is reported on the same terms. Without ext/curl there is nothing for it to do, and it is left out without a word. On a platform that is not Linux, with libcurl built into PHP rather than loaded as a library of its own, or where libcurl's `poll()` is not bound to the one the server intercepts, it stays off with a line in the server log saying cURL transfers stay blocking. The last is the case with PHP 8.4 on glibc when ext/curl is a shared extension (`extension=curl`): PHP loads it with `RTLD_DEEPBIND`, which binds the libcurl it brings in to the C library's `poll()`.

Hooks take effect inside worker-mode request fibers and async task fibers. Outside a fiber (traditional/framework/SPA request context, CLI) the original native behavior is preserved, including argument validation errors, `curl_multi_getcontent()` aside where the `curl` category is installed, as above. With the `sleep` hooks enabled, third-party code calling `sleep()` stops pinning the worker thread wherever the fiber can be suspended — which is not everywhere, as the next paragraph sets out — and needs no code changes either way. Cancelling an async task during a hooked sleep unwinds it with `OxPHP\Async\AsyncException`, and a hooked `sleep()` that suspends returns `0`, the signal-interruption return value of the native builtin arising only on the blocking path below.

## Where Hooks Do Not Suspend

### Where the `sleep` hooks do not suspend

Where a fiber switch is refused, the hooked `sleep()`/`usleep()` hand the call to the builtin they replaced, which waits on the worker thread and returns what it returns — so a `sleep()` cut short by a signal there gives back the seconds it did not sleep, as it does outside a fiber and as it would with the hooks off. (The argument check is the hook's own either way: it rejects anything above `4294967295`, which the unhooked builtin accepts and truncates.)

Switching is blocked wherever leaving the current frame would be unsafe, which covers more than a list can close. It is blocked:

- inside a `declare(ticks)` handler and inside pcntl's signal dispatch, both upstream PHP's own doing;
- while a request's input is being built, in Worker mode by `request_parse_body()` as well as before the script runs;
- inside a `FILTER_CALLBACK` reached from `filter_input_array()` given a per-field definitions array, described in the note under `$_GET` in [Superglobals](../php/superglobals.md);
- inside `array_multisort()`, in the code it runs as it compares — a `__toString()`, an error handler or a destructor, say — since the sort keeps its comparison functions on the worker thread for the length of the call.

The hooks also wait on the worker thread in the cleanup a worker runs after a request has come apart — after a fatal error, a timeout, or a write to a stream whose client has already left. They wait in the destructors it runs there, a decorator's `after()` and a stream filter's `onClose()`, so a `__destruct()` that sleeps to back off does it holding the thread. So do they in an output handler that a fatal error's message passes through as it is displayed, just before that cleanup. Switching itself is blocked there only where the message is displayed inside a `Fiber` the application started, or inside the one PHP runs destructors in when it collects garbage inside a `Fiber` — in every routing mode, wherever OxPHP's extension is loaded: suspending that `Fiber` would leave the message half written while the code that resumed it ran on, so starting, suspending or resuming a `Fiber` in that handler throws `FiberError`. Anywhere else, the `Fiber` a Worker-mode request or an `oxphp_async()` task runs as included, a `new Fiber` started in that code runs as usual.

The ordinary end of a request is not one of these: the hooks suspend in its shutdown functions, and in the destructors its variables run as they go — except those of what it left in a superglobal or in the closure of an `oxphp_async()` task it never awaited, which run after its session is written and wait on the thread.

A fiber a userland scheduler started — AMPHP, Revolt, or a bare `new Fiber` — is a different case rather than another entry in that list: switching is not blocked there, and the scheduler suspends and resumes its own fibers normally, but OxPHP cannot resume a context it does not own, so the hook stands aside for the same reason the `streams` hook does. This case is about a fiber started *inside* a request — by `new Fiber` directly, or by a scheduler the application drives there — and not about the shape of a whole deployment. A worker-mode request is itself a fiber OxPHP drives — which is why Revolt refuses to run its loop from inside one, as the note below on the `streams` hook says — so the hooks apply to the request's own fiber normally. What does sleep on the worker thread is the code inside a fiber the application started itself, and that share of the load has to be sized as if the hook were off.

`oxphp_sleep()` and `oxphp_usleep()` take their blocking path in every one of these places, so reaching for them instead is not a way around it.

### Where the `streams` hook does not suspend

What the `streams` hook makes cooperative is a blocking read on a PHP socket stream and a wait inside `stream_select()`. Before counting on it, check that your client waits one of those two ways, or one of the ways the `net` and `curl` categories add (below) — several common clients do not, and for them nothing changes:

- **ext/curl.** `curl_exec()` and `curl_multi_*` talk to sockets themselves, below PHP streams, so neither `streams` nor `net` sees them, and under those two every HTTP client built on curl — Guzzle with its default handler among them — waits on the thread. The `curl` category is what covers them; see [cURL transfers](runtime-hooks-curl.md#curl-transfers).
- **`socket_select()`.** That is ext/sockets, a different API on raw descriptors, and it is not hooked. `stream_select()` is. The same applies to the wait inside `stream_socket_accept()`, which is not hooked either.
- **`unix://`, `udp://` and `udg://` streams, socket pairs, streams from `socket_export_stream()` on sockets that are not TCP, and `ssl://`/`tls://` once crypto is active.** Their ops tables are private to PHP or to OpenSSL, so the `streams` hook does not reach them; the `net` category does, and with it listed a stream read (`fread()`, `fgets()` and the like) on any of them suspends the fiber like a `tcp://` one. `stream_socket_recvfrom()` is not a stream read: it calls `recvfrom()` directly, with no wait of PHP's own to suspend, so on a `udp://` or `udg://` socket it still holds the thread until a datagram arrives. Without `net` they hold the thread as before (before `stream_socket_enable_crypto()` succeeds an SSL stream's reads are delegated to the plain socket read and suspend either way). `socket_export_stream()` makes a stream of the transport the socket uses: a UDP, unix or datagram unix socket becomes a `udp://`, `unix://` or `udg://` stream, a TCP socket an ordinary `tcp://` stream — which `streams` alone already covers — and a socket of any other kind the generic stream that a socket pair has.
- **Connecting and DNS resolution.** With `net` a connect made through PHP's stream transports suspends the fiber: `stream_socket_client()`, `fsockopen()` and the clients built on them. `ftp_connect()` and `socket_connect()` connect on their own, outside the stream layer, and are not covered. A persistent connect is the exception, and it is more than `pfsockopen()` and `STREAM_CLIENT_PERSISTENT`: PDO_MySQL with `PDO::ATTR_PERSISTENT`, mysqli with the `p:` host prefix and phpredis with `pconnect()` open their streams as persistent too, and their connects keep the thread for the wait, as they did before the category existed. The reason is the persistent list: a persistent stream is entered in it under its key before it is connected, so a second fiber asking for the same key while the first waits would find a stream with no descriptor yet, take it for a dead connection and close it under the fiber still connecting. Some clients give every connection a key of its own — mysqlnd does — and nothing can find those streams that way, but the server decides by whether the stream is persistent, not by who could find it, so they hold the thread as well. It costs the connect, which a persistent connection pays when it is first opened, and not the reads and writes on it. DNS resolution does not suspend the fiber: a host name is resolved on the worker thread before the connect starts, so a slow resolver holds it. A client with a network library of its own — `pdo_pgsql` and `pgsql` (libpq), the `memcached` extension (libmemcached) — connects and waits inside that library, which neither category sees. A client that holds its connection open benefits on every query regardless.
- **MySQL reached through `localhost`.** The MySQL client reads `localhost` as "use the unix socket", which is not a `tcp://` stream, so the `streams` hook does not see it — write `127.0.0.1` in the DSN instead, or list `net`, which covers `unix://` streams. This applies to PDO_MySQL and mysqli alike, and is easy to miss because everything still works, just without the benefit.
- **Writes.** Under `streams` alone only reads wait. Read readiness holds still as long as one fiber owns the descriptor and nothing else drains it, whereas room in the send buffer is granted and withdrawn by the peer, so a fiber woken on writability can find the window closed again by the time the write runs, after which PHP blocks the thread for its full timeout regardless. A write that fills the socket buffer therefore behaves exactly as it does without the hook, which in practice costs little: waiting on a reply is what takes time, not handing a query to the kernel. Writes are still looked at, for one thing only: which fiber's exchange the connection is currently carrying, so that an `fwrite()` from PHP code can wait for another fiber's exchange to end rather than land in the middle of it — the [note on shared connections](runtime-hooks-shared-state.md) says when it can. On a connection no other fiber is using, which is every connection in the ordinary case, the write runs the native path untouched. With `net` listed, a write that has to wait for room suspends the fiber as well, and the descriptor is checked again after every wake, so a window that closed in the meantime costs another suspension rather than the thread. A plain stream whose timeout is infinite (`default_socket_timeout=-1`) is the exception: PHP sends on it with a blocking `send()` and never waits on its own, so there is nothing to suspend and the write holds the thread as it does without the hook. A TLS stream with an infinite timeout is not: OpenSSL's own loop waits in `poll()` with no limit, and that wait is suspended.

Within that scope the hook keeps the native contract: socket timeouts (`stream_set_timeout()`, `default_socket_timeout`) apply unchanged, a timed-out read still reports `timed_out` through `stream_get_meta_data()`, and stream identity is untouched, so `socket_import_stream()` and similar keep working. One limit on the timeout: the deadline is only examined once per scheduler tick, so it fires no sooner than the next tick — at best 100 µs in worker mode and 50 µs in the async pool, and longer under load, since a tick lasts as long as whatever the worker is running. The async pool pauses for longer than that when nothing is happening, but not past a deadline it is holding: a read or write deadline shortens the pause to land on itself, so waiting longer does not make the timeout later. With `default_socket_timeout` at 60 seconds this is not something most deployments can observe.

`stream_select()` is hooked by waiting on the descriptors its three arrays name and then handing the call to PHP with the timeout set to zero, so PHP still decides everything you can observe: the return count, the rewriting of the arrays down to the ready streams, the warnings, and the argument errors. The wait is skipped — and the call runs exactly as it would without the hook — whenever the arrays hold something the hook will not stand in for: a read stream with data already buffered (which `stream_select()` answers from the buffer, without looking at a descriptor), a stream that has no descriptor at all, an element that is not a live stream (a closed one left in the array, say — PHP answers that with an error, not a wait), a descriptor the kernel will not watch for readiness at all — a regular file is the usual case, and one counts as ready the moment you ask — or a descriptor at or past `FD_SETSIZE`, which PHP's own `select()` refuses outright. That last one is a real ceiling rather than a formality: a busy worker can hold more than 1024 open descriptors, and a `stream_select()` naming one of them fails the same way with or without the hook.

Sharing one connection between concurrent fibers is covered in [Shared Connections Under Runtime Hooks](runtime-hooks-shared-state.md). One more limit worth knowing:

- Running the `streams` hook underneath a userland fiber scheduler (AMPHP, Revolt) falls back to blocking I/O. A fiber started by such a scheduler runs on its own context, which OxPHP's scheduler cannot resume, so the hook detects this and takes the native path rather than corrupting either scheduler. This is about a fiber a userland scheduler starts *inside* a request. The request's own fiber is one OxPHP drives, and in worker mode it is a real `Fiber` — `Fiber::getCurrent()` inside a request returns it — so libraries that only need to tell concurrent requests apart work without any fallback. The same fact is why Revolt refuses to run its event loop from inside a worker-mode request; see the note on that in the changelog.

## Cost

Cost, measured: about 3–5 µs per socket round trip on a worker with nothing else parked, about 5–6 µs with 64 fibers parked on descriptors, and about 7–11 µs with 200. Readiness is resolved through a set the kernel keeps between waits, so what the figure tracks is how many descriptors became ready, not how many are waiting. An idle worker waits on those descriptors rather than sleeping for a fixed interval and noticing readiness on its next tick, which matters a great deal: sleeping blind cost roughly 2 ms per round trip instead.

A wide `stream_select()` carries an overhead the narrow case does not, because every descriptor a call names is registered before the wait and removed after it. Measured with the waiting itself taken out — every descriptor already readable, so the call returns at once and only the overhead remains — it is about 6 µs against 5 µs unhooked at one descriptor, 56 µs against 9 µs at 64, and 130–150 µs against 16–21 µs at 200: roughly 0.65 µs per descriptor.

Read that as a fixed cost per call, not as a slowdown of the same work. A `stream_select()` that actually waits — which is what a select loop is written for — dwarfs it: a millisecond of waiting makes even the 200-descriptor figure about a tenth of the call, and what it buys is the worker thread, which the unhooked call holds for the whole wait. Two shapes are the exception, and for them the category is better left off: a call over many descriptors that almost never waits, because there is no thread time to recover; and a request that *is* the event loop, where the thread has nothing else to run anyway. Clients that wait on one connection — mysqlnd, phpredis, the HTTP stream wrappers — sit at the top of the table, where the overhead is around a microsecond.

## Best Practices

**When to use `RUNTIME_HOOKS` vs. `oxphp_sleep()`.** The hook does not replace the first-party primitive — they cover different cases:

- Reach for `oxphp_sleep()` / `oxphp_usleep()` in code you write. They suspend the fiber in worker mode with no environment flag to set, and `oxphp_sleep()` accepts fractional seconds (`oxphp_sleep(0.25)`), a precision native `sleep()` (integer seconds) cannot express. What they do not buy is an exemption from the cases above: there they hold the worker thread exactly as the hooked builtins do.
- Enable `RUNTIME_HOOKS=sleep` for code you **cannot** edit — a framework or vendor library that calls native `sleep()`/`usleep()` directly. It is off by default, does nothing outside a fiber, and keeps the native return contract: a hooked `sleep()` still takes an `int`, and it returns `0` when it suspends and whatever the builtin returns when it falls back. The argument check is its own, as above.

Relying on `RUNTIME_HOOKS` for your own handlers ties their cooperativeness to a deployment setting rather than the code; prefer the explicit `oxphp_sleep()` there.

## See Also

- [Shared Connections Under Runtime Hooks](runtime-hooks-shared-state.md) — one database or Redis connection used by several fibers, and application state a hooked wait exposes to other requests
- [cURL Under Runtime Hooks](runtime-hooks-curl.md) — cURL transfers, shared handles and multi handles under the `curl` category
- [Fiber Multiplexing](fiber-multiplexing.md) — how a worker runs several requests at once and where it can suspend one
- [Configuration Reference](../operations/configuration.md#runtime-hooks) — the `RUNTIME_HOOKS` variable
