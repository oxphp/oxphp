---
title: Shared Connections Under Runtime Hooks
description: How one database or Redis connection behaves when several fibers share it under RUNTIME_HOOKS, and what a hooked wait lets other requests see.
---

# Shared Connections Under Runtime Hooks

This page covers what [runtime hooks](runtime-hooks.md) mean for a database or Redis connection that several fibers share, and for application state that outlives a request in worker mode.

## Sharing One Connection Between Fibers

**One connection shared between concurrent fibers is safe for the clients OxPHP guards, and gains nothing.** This is the normal shape of a worker-mode application rather than an edge case: WordPress, Laravel and Symfony open their database and cache clients once when the worker boots and hand the same ones to every request, and short of rewriting their data-access layer they cannot be asked for one connection per fiber.

### Why a shared connection needs a claim

A client protocol is a sequence of exchanges — write a command, read the answer — with nothing on the connection marking where one ends, so a fiber parked on a read is parked in the middle of one, and a second fiber's command landing there breaks the protocol.

The two clients fail at that differently, and neither well:

- mysqlnd tracks its connection state and refuses the command from it, before sending anything;
- phpredis has no such check, and the two fibers read each other's replies — one request's data returned to another with no error raised.

### Two levels of claim

A fiber therefore claims a connection before using it, at both of the levels this has to happen at:

- the socket ops, which keep the bytes in order;
- the client entry points of PDO and mysqli, since mysqlnd's refusal comes before any I/O and nothing at the socket can be reached in time to prevent it.

Another fiber reaching the same connection waits for it to be given up at the client level, where nothing but the connection's identity is held. At the socket level an `fwrite()` or `fputs()` from PHP code, on a stream with no write filter, waits as well, but only for the exchange the holder is parked in, not for the holder's request: what reaches the socket is the string the call was given, which nothing else touches while it waits. Anything else is refused, the way a socket timeout is refused.

A read cannot wait, because what PHP hands a read on a buffered stream, which is the default, is the stream's own buffer, which the holder's read fills and may reallocate while the second fiber would be suspended. Nor can a C client's write, because what it writes with is not its own: before every command mysqlnd resets the connection's packet counter, and it builds the command in a buffer that belongs to the connection whenever the command fits, so a second command issued while the first waited would break the first one's exchange. phpredis is guarded at the client level too, method by method, for exactly that reason.

### Writes under `streams` without `net`

Under `streams` without `net` the `fwrite()` is refused too wherever the OpenSSL extension is loaded, the official images included: OpenSSL then carries every `tcp://` stream and has chosen between its plain and TLS paths before the write reaches the hook, so a write that waited there could go out unencrypted on a connection the holder had meanwhile switched to TLS.

### What the claim covers

What the client level claims is the connection itself and not the PHP object holding it, so:

- a persistent connection reached through several PDO objects counts as one;
- a connection opened with `PDO::connect()` — which returns the driver's own subclass rather than a `PDO` — is covered like any other.

phpredis with connection pooling off (`redis.pconnect.pooling_enabled=0`) is a case of the same kind as a persistent connection reached through several PDO objects: it hands every object opened persistently with the same arguments the one connection — in a ZTS build through the constructor's `persistent` option, and in a build from its development branch at commit `b37b231` or later through `pconnect()` and `popen()` as well — and those objects count as one. That takes arguments of the types the method declares, or numeric strings in their place, as a port read from the environment is, written as a whole number for the port: an object opened with an argument PHP would have to convert any other way — a host passed as an object that converts to a string, or a port of `"6379.0"`, say — counts on its own. With pooling on, the default, phpredis gives each object a connection of its own.

The setting that counts is the one in force when `pconnect()` or `popen()` is called or, for an object the constructor opens, when its first command runs, since that is where phpredis connects it. A getter such as `getHost()` called before any command connects the object too, and OxPHP still goes by the setting of the first command, so changing the setting between the two leaves that object either waiting needlessly or unguarded.

phpredis 6.3.0 opens a plain socket for `pconnect()` in a ZTS build, as its development branch did before commit `b37b231`, and the version string cannot say which a build does, since the 6.3.0RC1 release and that branch on either side of the commit all report `6.3.0RC1`. So with pooling off, an object whose `pconnect()` is given no persistent id, or an `auth` entry in its context, and finds a constructor's connection registered under its arguments waits for whoever holds that connection, as if it were on it, until some `pconnect()` in the process given a persistent id and no `auth` has connected.

### Where the hook gains something

Read the hook's gain, then, as belonging to connections a fiber opens for itself: an async task making its own HTTP or database calls, a request opening its own client. What a shared connection gets is the worker thread back while it waits, so other requests can run the work that is not on that connection; its own exchanges run one after another, exactly as they do with the hook off.

Four boundaries are worth knowing.

### First boundary: a fiber that queried keeps the connection until its request ends

A fiber that queried once keeps the connection to the end of its request, because the end of the request is the first moment certainly past the end of an exchange. It keeps it while parked on anything else as well, so a request that has queried and then waits on work of its own that needs the same connection is waiting for itself, and the two come apart on [the bound below](#second-boundary-every-wait-is-bounded) instead of proceeding.

### Second boundary: every wait is bounded

Waiting is always bounded: by the smaller of `max_execution_time` and `default_socket_timeout`. Set neither and that bound is 30 seconds, since a server SAPI takes the engine's defaults — 30 for the first and 60 for the second; it is `default_socket_timeout`'s 60 only where `max_execution_time` is 0.

The two are read differently:

- `max_execution_time` is read as the request currently has it, since `set_time_limit()` is how a request states how long it may run;
- `default_socket_timeout` is read as the process started with, because it is the default deadline of a socket *operation* and a request narrowing it for a call of its own — a common thing for a library to do, and to leave behind — must not shorten this bound for the requests that follow it on the same worker.

#### Past the bound

Past the bound the call falls back on unguarded behaviour and says why in the server log:

- for PDO and mysqli it is handed to the client, whose own refusal of a command issued mid-exchange is the error the application already handles;
- for phpredis, which has no such refusal and would instead read someone else's reply, it raises a `RedisException` and sends nothing.

One PDO call is the exception, both here and for code running outside any fiber ([below](#code-outside-any-fiber)): `PDO::rollBack()` on a persistent connection another fiber holds, finding a transaction open on it, sends nothing and returns `false` — reporting, the way the error mode says, an error the connection already records, which can be the holder's — because that transaction is the holder's and a `ROLLBACK` sent between its statements would end it.

#### PDO handle teardown on a persistent connection

PDO's own object teardown is held to the holder as well. While a request or task holds a persistent connection, dropping a handle on it rolls back the transaction open there and runs the driver's end-of-request cleanup only if the holder was the last to use that handle — built it, or since called one of the PDO methods OxPHP guards through it. That holds wherever the handle is destroyed, PHP's cycle collector running in another request included. Any other handle skips both; on a connection nobody holds, no handle skips them for being another's.

From PHP 8.4.27 and 8.5.12 on, PDO itself sends that rollback only for the last handle on a persistent connection, taking any others to be the same script's, which carries on with the transaction through them. In a worker they are as often an earlier request's handles kept in a static property, so OxPHP sends the rollback PDO holds back, unless the request or task the dropped handle belongs to holds that connection itself and keeps another handle on it — one it built, or has called one of those methods through. Then the transaction stays open for that handle, as PDO intends — past the end of the request, too, if the handle is kept in a static property or a service container.

A handle an earlier request left in a static property is not the request's own until it has used it, so a transaction opened through a local handle and meant to be finished through such a property is rolled back when the local handle goes.

Code outside any request — a worker's boot code, say — never holds a connection, and its handles count only one another: on a connection nobody holds, dropping one rolls the transaction back unless another handle that code last used is left.

#### Refusal at the socket level

At the socket level a read is refused at once, and an `fwrite()` waits for the holder for at most the smaller of the stream's own timeout — the one `stream_set_timeout()` sets — and the bound the client level has, counted from the write. A refused operation of either kind fails the way a timeout does, so `stream_get_meta_data()` reports `timed_out`, and a refused write sends nothing.

That can cost more than the call, and on either side of an exchange: a client is free to read a refused operation as something worse than a failed command, and mysqlnd does so for both:

- a write that sends nothing marks the connection gone and closes it;
- a read that comes away empty marks it gone where it stands.

Every later command on it is answered from that state without touching the wire. So what is lost is not the call but the connection, for as long as anything holds the handle.

phpredis closes the connection on a refused read of a reply line. Any other refused read fails the call and leaves the rest of that reply unread on the connection, unless phpredis goes on to read another line of the same reply, which closes it — except in a build of its development branch at commit `7ad2f79` or later, which closes it on every refused read. A development-branch build at commit `48a46c7` or later closes it on a refused write too.

The client-level claim above is what keeps a guarded client's commands from reaching that refusal at all. A hand-written protocol on a raw socket and a userland client such as Predis have no equivalent, and for them closing the connection on a refused operation — which is what Predis does — ends the request parked on it, as described under [the fourth boundary](#fourth-boundary-closing-a-shared-connection-ends-the-parked-request). The socket level is their whole guard, and it guards one reply at a time (see [the third boundary](#third-boundary-uncovered-cases-and-persistent-connections)); give such a client a connection of its own per request, or use phpredis.

#### Two fibers waiting on each other

Because every wait is [bounded](#second-boundary-every-wait-is-bounded), two fibers each holding what the other waits for come apart on that bound instead of waiting on each other for good. The one such pair OxPHP can recognise comes apart at once instead: a request waiting in `session_start()` for the session another request on its worker has open (see [Worker State Between Requests](worker-state.md#a-worker-has-one-session-open-at-a-time)), while holding a connection that request is waiting for. The wait for the connection is then given up as soon as both are waiting, with the outcome a spent bound has, and the server log says so.

#### Code outside any fiber

Code running outside any fiber cannot wait on a claim at all, and for every client but phpredis and cURL only the socket level applies to it. In worker mode with `RUNTIME_HOOKS=streams` that is chiefly a destructor the worker loop runs between requests when it collects cycles, while other requests are parked. A socket operation it makes that would wait, on a connection whose holder is parked on it waiting for its reply, fails at once the way a timeout does, and sends nothing; at any other moment it goes ahead. The client-level claim does not refuse such code, because it lasts to the end of the holder's request: it would turn away a connection the holder is not using at that moment, which is most of the time.

What a refusal costs is the client's, as above. phpredis is refused a step earlier instead, while the fiber holding the connection is inside a phpredis call on it: the call returns `false` without sending anything, and the holder keeps its reply — a write refused at the socket would, in a development-branch build that closes the connection on one, close it under the holder, ending its request as described under [the fourth boundary](#fourth-boundary-closing-a-shared-connection-ends-the-parked-request). A holder that has gone from inside such a call into another on a different connection, from a `subscribe()` callback for one, counts as inside the second only, and the first is left to the socket level. mysqlnd either refuses the command from its own state or, when the holder's exchange left that state ready, has its write refused, closes the connection, and the holder's request ends as described under [the fourth boundary](#fourth-boundary-closing-a-shared-connection-ends-the-parked-request). A destructor is therefore a poor place to use a shared client at all. A cURL handle under `RUNTIME_HOOKS=curl` is refused at the handle instead, as libcurl's connections are not PHP streams and the socket level does not reach them: while another fiber is in the middle of a call on the handle or on the multi handle it is in, the call throws `OxPHP\Async\AsyncException` and changes nothing ([cURL Under Runtime Hooks](runtime-hooks-curl.md#curl-transfers)).

### Third boundary: uncovered cases and persistent connections

Some cases are deliberately left uncovered:

- A statement or result object kept across requests, which no claimed call precedes — `PDOStatement::execute()` on a statement prepared in an earlier request behaves as it does with no claim at all. What that costs is worth stating, because it is more than the call: such a call reaching the socket while any fiber is parked on the connection is refused there, and for mysqlnd a refusal is the connection rather than the call, as above — on the read that fetches the next row of an unbuffered result as much as on the write that sends the command.
- A protocol written by hand on a raw socket — write a command, suspend on something else, read the reply later — and a userland client such as Predis between the commands of one exchange, a `MULTI` and its `EXEC` with a wait in between: the socket level only stands in the way while the holder is parked on a reply itself, and outside that another fiber's command takes the connection over. The three clients above are covered here by their client-level claim, which neither has an equivalent of.

#### Persistent connections

A persistent connection is a case of its own and is covered rather than left out: `PDO::ATTR_PERSISTENT` keeps the driver's connection in a pool the whole worker shares, and PDO reaches that pool in two steps that do not survive being interleaved: it looks a pooled connection up before it connects, and registers what it built afterwards. So two constructors overlapping both miss the pool, and the second registration replaces the first entry, closing that connection under whichever request is reading its reply on it.

Overlapping is the ordinary case rather than a rare one, since `static $pdo ??= new PDO(...)` is reached by every request that arrives before the first connect finishes. So constructors asking for a persistent connection to one data source run one at a time per worker, which is the whole width of a pool, and the second one finds what the first registered.

The data source is the one PDO resolves — an alias from `pdo.dsn.*` in php.ini counts as the DSN it names. Constructors to different ones do not, as a rule, wait for each other, so one stuck connecting to a server that accepts the connection and never answers does not hold up constructors to a healthy one.

Two cases wait more rather than less:

- A DSN is compared only up to the first colon after its driver prefix — an IPv6 host is one way to have one — because PDO joins DSN, user and password into its pool key with colons and escapes nothing, so a DSN extended by `:…` can name the same pool entry as the one without. Sources that read the same up to that colon share one wait, a stuck one included: two IPv6 hosts in the same leading group of digits wait for each other as all constructors on a worker used to.
- A `uri:` DSN, or a DSN passed as anything other than a string, cannot be resolved without repeating what PDO does with it, so such a constructor waits until no persistent constructor to any data source is running on its worker, and they all wait while it runs. It holds nothing while it waits, so it does not hold the others up; but while a constructor to a server that never answers is running it cannot find that moment, and when its wait is spent it goes ahead unserialised — with `uri:` or object DSNs, the stuck-server case above still reaches it. And one that names the stuck server itself holds every persistent constructor on its worker up for as long as its connect lasts, exactly as before.

That wait is bounded like the ones above, and unlike them what it costs when it is spent is not an error the client raises. A constructor whose budget runs out goes ahead unserialised and says so in the server log, and two constructors overlapping that way replace each other's pool entry as before. That ends the request reading its reply on the connection replaced and, because a pooled handle is freed without regard to how many PDO objects still point at it, can end the worker process itself when one of those objects is later destroyed.

A constructor inside a userland fiber scheduler's own fiber is the one place where that budget is spent at once rather than occasionally: such a context cannot be suspended, so a constructor that finds another one already running gives up immediately instead of waiting. One that finds the way clear still takes its turn like any other, and the fibers OxPHP drives still wait for it.

A constructor where a fiber switch is refused cannot wait either — such as in a `FILTER_CALLBACK` of `filter_input_array()` given a definitions array, in a `__toString()` or an error handler `array_multisort()` runs as it compares, or in a `declare(ticks)` handler — and neither can one in the cleanup a worker runs around a request's end. There the constructor it would wait for belongs to a request or task parked on the same worker thread, which cannot run until this code returns, so the wait could only spend its budget, and going ahead unserialised is the race above. Such a constructor, finding another one running, is refused with a `PDOException` (`persistent connection not built: …`) instead. One that finds the way clear takes its turn as usual.

#### The liveness check and constructor options on a pooled connection

The other step is the liveness check PDO makes before handing a pooled connection over: it is a ping, sent on the connection itself. One sent while the fiber holding that connection is parked on it waiting for its reply is refused, so PDO reads it as dead, drops it from the pool and opens a second connection — the request mid-exchange keeps its own connection and its reply, but what the application meant to share is no longer shared.

A connection another fiber holds is therefore reported alive without the ping, on the grounds that being used is what a live connection looks like — but only to a constructor that would leave it exactly as it is. Where a driver supplies no liveness check of its own — `pdo_sqlite` is one — PDO asks nothing at all before handing the connection over, and the answer stands in that place instead, so what follows holds whichever driver opened the connection.

PDO does not stop at handing a pooled handle over: it writes the error mode and the autocommit flag onto it from the options it was given, defaults included when they are absent, and then applies the rest of those options to it as well. On a connection in use those writes are the holder's — its error mode changing under it, and for options the driver forwards, such as `PDO::ATTR_STRINGIFY_FETCHES`, the PHP types its remaining rows come back as.

So the connection is shared only with a constructor that would write nothing new onto it, and that is two conditions rather than one. Every option it passes must be one PDO stores on the handle itself — `PDO::ATTR_ERRMODE`, `PDO::ATTR_CASE`, `PDO::ATTR_ORACLE_NULLS`, `PDO::ATTR_DEFAULT_FETCH_MODE` — and must pass the value the connection already has. The `PDO::ATTR_PERSISTENT` entry that names the pool is not counted among them, although PDO passes it to the driver like any attribute it has no case for: it sets nothing, and that trip is answered rather than sent to the connection.

An option the driver has to be told about is refused whatever its value, `PDO::ATTR_AUTOCOMMIT` and `PDO::ATTR_STRINGIFY_FETCHES` included, because what a driver does with one is the driver's own — a field written on the connection, or a command put on the wire, which here would land inside the holder's exchange. The rule stands on whichever driver opened the connection, third-party ones included, so it is answered by where the option goes rather than by what any one driver does with it; asking for the value the connection already has does not change where it goes.

A constructor that fails either condition is told the connection is dead and gets one of its own, rather than changing one in use, and the server log says so once a second.

Note what that means for a pool key whose options carry `PDO::ATTR_EMULATE_PREPARES` or `PDO::ATTR_STRINGIFY_FETCHES`, as Laravel's do: it is shared between requests, and between fibers that construct before they query, but a constructor arriving while another fiber's request holds the connection opens its own. That is a longer window than the query it protects: a claim runs from the holder's first query to the end of its request, as above, not for the length of an exchange.

A connection nothing is using is checked as before — including one whose holder has dropped its last handle, since the claim outlives the object it was taken for and what it then names is a connection nobody can send anything on — so one that really has died is still replaced. That check is a command on the connection like any other, and it is claimed like one: a query arriving through one of the clients above while it is in flight waits for it, within the same bound as any other contended call, rather than landing inside it. The cases listed as uncovered stay uncovered here too — what reaches the connection without passing a claimed call can land inside the check as it can land inside a query.

### Fourth boundary: closing a shared connection ends the parked request

Closing a shared connection while another request is parked reading it ends that request, with a `500` and a log line naming what happened. Claiming keeps two exchanges apart; it cannot keep a connection alive, and the parked request's reply is on a connection that no longer exists — so the honest answer is to end it rather than to hand back whatever the freed memory now holds.

Which calls can do this depends on the client, and only one of them waits:

- `mysqli::close()` and `mysqli_close()` are claimed calls, so they wait for the holder and close once that wait is spent;
- `Redis::close()` is claimed too but raises a `RedisException` rather than closing;
- everything else closes at once — `fclose()` on a raw stream, and, importantly, PDO, which has no `close()` method at all: a connection is released by dropping the last reference to the handle (`unset($pdo)`, reassigning it, or letting it fall out of scope), and that path runs inside the engine's own object teardown where no claim is consulted before the connection is closed.

So the reconnect helper that reassigns a shared PDO handle ends the requests parked on the old connection, without the bounded wait its `mysqli` equivalent gets.

A request whose write is waiting at the socket level for the parked one is ended the same way, whoever closes the connection — the holder included, which Predis does when its own read times out. The request is told as soon as the close happens rather than at its own read deadline, which for mysqlnd would be `mysqlnd.net_read_timeout` — a day out of the box. Applications that close a shared connection to force a reconnect — WordPress's `$wpdb->check_connection()` is the usual one — should expect the requests parked on it at that moment to fail rather than to return the wrong data.

## Hooked waits and shared application state

In worker mode, every hooked wait that suspends the request is a point where the worker starts or resumes other requests. That is what the hooks are for, and it reaches into code you did not write: a cache read in a service constructor, a connect in a container factory, a `sleep()` in a retry loop. The per-request state listed under [fiber multiplexing](fiber-multiplexing.md) is saved and restored around the wait, but the objects the application keeps for the life of the worker are not. Requests that overlap see the container, singletons and `static` properties exactly as they are at that moment.

**A worker's first request runs alone.** In worker mode with any hook category enabled, a worker admits nothing beside the first request it takes, until that request ends or 250 ms after it first waits, whichever comes first. An application that builds its container or router in the first request therefore finishes the build before a second request can reach it, as long as it finishes within 250 ms of its first wait. This happens once each time a worker starts: at server start, after a recycle, and when a dynamic pool adds a worker. The requests held back stay in the shared queue, where any other free worker can take them. They spend their `QUEUE_WAIT_TIMEOUT_MS` budget there from the moment they arrive, so on a pool with no other worker free, a request whose budget ends before the hold does is answered with a `529`. That takes a non-zero budget shorter than the hold, configured that way or shortened by the server under overload, or a first request that stays busy for most of the budget before it first waits and is still running when the budget ends: at the default 1000 ms, about 750 ms of work before that wait, and less for requests that queued while the worker was starting. Without runtime hooks there is no such window: a request then suspends only at the [suspension points](fiber-multiplexing.md#suspension-points) the application calls.

Two cases stay with the application:

- **A build still running after that, or one that happens in a later request**, such as a service only some routes use, can still be seen half done by another request. Build such services in the worker script before `oxphp_worker()` is called. That code runs before the worker takes any request, so nothing can overlap it.
- **Per-request state kept in a shared service and cleared after the request**, such as a "current route" or "current request" singleton, breaks on any overlap, however warm the worker is: the second request finds the first one's value, and the first one's reset wipes the second's. Building it ahead does not help. Run such an application in worker mode without `RUNTIME_HOOKS`.

## See Also

- [Runtime Hooks](runtime-hooks.md) — the hook categories and where they do not suspend
- [cURL Under Runtime Hooks](runtime-hooks-curl.md) — the same claim applied to cURL handles
- [Worker State Between Requests](worker-state.md#what-gets-reset-between-requests) — what a worker resets and what it keeps
