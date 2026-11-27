---
title: Overlapping Requests and Framework State
description: Why Laravel, Symfony and Yii3 leak request state or lose transactions when a worker overlaps requests, under RUNTIME_HOOKS above all, and how to run them.
---

# Overlapping Requests and Framework State

In worker mode, when the request a worker is running suspends, the worker can take a new request or resume one it has paused. Requests that overlap this way share everything the worker keeps between requests: the outer scope, static properties, the service container and its singletons, open connections (see [What Persists](worker-state.md#what-persists)). Code that keeps what belongs to one request in the request object and in local variables is not affected. Code that keeps it in a shared object hands it to whichever request runs next — and that is where most full-stack frameworks keep the current request, user, route and locale.

`RUNTIME_HOOKS` is where this usually starts to matter, because it turns waits inside the framework and its database and HTTP clients into suspension points. This page covers when requests overlap, what goes wrong in Laravel, Symfony and Yii3, what happens to a transaction on a shared connection, and which setups avoid both.

## When Requests Overlap

Overlap needs worker mode. In Traditional, Framework and SPA modes every request starts from a clean PHP state and no request suspends to let another one run on its thread; `RUNTIME_HOOKS` makes no request suspend there (see [Runtime Hooks](runtime-hooks.md#checking-that-hooks-are-active)).

In worker mode without `RUNTIME_HOOKS`, overlap starts only at OxPHP's own waits:

- `oxphp_sleep()` and `oxphp_usleep()`;
- `oxphp_async_await()` and the other `oxphp_async_await_*()` functions;
- a blocking send or receive on an [`OxPHP\Shared\Channel`](../shared-state/shared-channel.md) and an acquire that waits on a saturated [`OxPHP\Shared\Pool`](../shared-state/shared-pool.md), where those suspend the fiber rather than the thread;
- `oxphp_stream_flush()` or `flush()` in a streaming response, when the client reads more slowly than the script writes (see [Server-Sent Events](sse.md#how-it-works)).

A worker whose requests reach none of these runs them one after another. Each of them still finds what the previous one left in shared objects, so the integration has to reset that between requests, but no two requests use it at the same time.

With `RUNTIME_HOOKS`, the hooked waits are added, and they sit inside code you did not write:

- `sleep`: every `sleep()` and `usleep()`, in a retry loop or a rate limiter;
- `streams`: every reply a client waits for on a plain `tcp://` stream — mysqlnd (PDO_MySQL, mysqli), phpredis, the HTTP stream wrappers — and `stream_select()`, which for an application on MySQL or Redis over TCP means nearly every request;
- `net` (Linux only, listed together with `streams`): replies on TLS streams and on sockets that are not TCP — MySQL reached through `localhost`, which the client reads as its Unix socket, or a database or Redis behind TLS — and connects, TLS handshakes and writes that wait for room;
- `curl` (Linux only, with libcurl loaded as a library of its own, as in the official images): every request made through ext/curl — Guzzle with its default handler, Symfony HttpClient's `CurlHttpClient`, and the SDKs built on either.

Clients that wait inside their own library do not suspend: `pdo_pgsql`, which waits inside libpq, is one (see [The `net` category](runtime-hooks.md#the-net-category)). Under `streams`, a request that waits for a PDO, mysqli or phpredis connection another request holds does suspend, whatever the driver — see [Transactions on a Shared Connection](#transactions-on-a-shared-connection).

With any hook category on, a worker runs its first request alone for a bounded time, so that a container or router built during that request is finished before another request can see it (see [Hooked waits and shared application state](runtime-hooks-shared-state.md#hooked-waits-and-shared-application-state)). That covers a build, once per worker start. It does nothing for the state each request sets for itself.

## Request State in Shared Objects

The failure has the same shape in every framework that has it:

1. Request A sets the current user and locale on a shared service, then suspends.
2. Request B starts on the same worker and sets its own.
3. A resumes and reads B's user and B's locale.
4. When B ends, the reset that follows it clears or replaces the state A is still using.

OxPHP raises no error and logs nothing. The symptom is a response carrying another client's data, or an exception from a framework that refuses to set its state twice.

| Framework | Where the current request's state lives | When requests overlap |
|-----------|-----------------------------------------|-----------------------|
| Laravel | Static properties: the application in `Container::$instance`, the facade root and the resolved facades in `Facade::$app` and `Facade::$resolvedInstance`. An integration built on Laravel Octane sets a clone of the application there for each request, points shared singletons at that clone — the router, the authorization gate, the view factory, and the database and cache managers among them — and sets the translator's and Carbon's locale | A request that resumes finds whatever application the last request to start or end left in the statics — another request's clone, or the worker's own application — with that request's user and locale. Restoring the statics when a request resumes would not be enough: the shared singletons still point at the other clone |
| Symfony | Shared services: `RequestStack`, the security `TokenStorage`, the router's `RequestContext`, the translator's locale, and Doctrine's `EntityManager`, whose unit of work tracks every entity loaded through it, whichever request loaded it | A request can find another request's current request, user, route context and locale in them, or none, depending on which request last set or reset them — unless the integration keeps these services per request. A `flush()` in one request writes the changes another request has made to its entities and not flushed yet, even when neither request begins a transaction itself. That happened even with an integration that keeps the request stack, router context, security token and locale per request fiber, because the `EntityManager` stayed one per worker |
| Yii3, one container per worker | Shared services, among them the router's `CurrentRoute`, which accepts a route and a URI once and is reset after each request | A request that starts while another is suspended past routing fails with `LogicException` (`Can not set URI since it was already set.`), and the reset after it clears the route of the request still running |
| PSR-15 application that keeps route, user and locale in request attributes | The request object passed down the middleware stack | That state travels with its request. What the application's own services store is shared like anything else |

Checked against Laravel 13.43 with Octane 2.25, Symfony 8.1 with Doctrine ORM 3.8, and yiisoft/router 4.0.

What isolates such a framework is an integration that keeps these services per request. One way is per request fiber: in worker mode each request runs in a `Fiber` of its own, and `Fiber::getCurrent()` returns it anywhere in the request's code except inside fibers the application starts itself and in the destructors PHP's cycle collector runs, which get a fiber of their own. The other is a container built for each request, at the price of building it every time; that works only for a framework whose code gets its services from that container rather than through static properties — it does for Yii3, not for Laravel, whose application and facades are static, so a clone per request is shared as soon as the next request sets its own.

### Checking an application

The shape is easy to reproduce. This worker script keeps a user in a static property, as a framework's "current user" service would:

```php
<?php
final class CurrentUser
{
    public static ?string $name = null;
}

oxphp_worker(function () {
    $user = $_GET['user'] ?? 'anonymous';
    CurrentUser::$name = $user;

    if (isset($_GET['wait'])) {
        oxphp_sleep(1.0); // a hooked query, an await or a streaming flush does the same
    }

    echo "$user sees ", CurrentUser::$name, "\n";
});
```

Run it with `WORKER_MODE_ENABLED=true`, `ENTRY_FILE` pointing at it and `PHP_WORKERS=1`, so that both requests land on one worker, and send the second request while the first one waits:

```bash
curl 'http://localhost/?user=alice&wait=1' & sleep 0.2; curl 'http://localhost/?user=bob'; wait
```

```
bob sees bob
alice sees bob
```

To check a real application, add a route that sets the user or locale through the framework, calls `oxphp_sleep(1.0)` and reads it back, and send the same two requests.

## Transactions on a Shared Connection

A connection opened before `oxphp_worker()`, or kept by the framework's database layer for the life of the worker, is one connection for every request on that worker. A transaction on it belongs to the connection, not to the request that began it.

### Without the `streams` category

Nothing then claims the connection for a request. A request that suspends while its transaction is open lets the statements of any request that runs meanwhile on the same connection into that transaction, and they are committed or rolled back with it rather than with their own request. Request B can answer `201 Created` for a row that request A's rollback then removes.

That holds for every suspension point listed above, and for the hooked waits of the `sleep` and `curl` categories: a Guzzle call between `beginTransaction()` and `commit()` is enough.

### With the `streams` category

Under `streams` a request that has used a PDO, mysqli or phpredis connection holds it until the request ends, and another request's call on it waits (see [the first boundary](runtime-hooks-shared-state.md#first-boundary-a-fiber-that-queried-keeps-the-connection-until-its-request-ends)). B's plain write in the example above then waits for A to end and is kept. Two problems remain.

The requests that use the connection run one after another, each from its first query to its end, not only for the length of each query. A PDO or mysqli call that waits past [its bound](runtime-hooks-shared-state.md#second-boundary-every-wait-is-bounded) goes ahead without the claim, as it would without `streams`, and a phpredis call throws.

The framework's count of open transactions is a property of its shared connection object — `$transactions` on Laravel's `Illuminate\Database\Connection`, `$transactionNestingLevel` on Doctrine DBAL's `Connection` — and nothing claims that. When B's first call on the connection is `beginTransaction()` while A's transaction is open, it finds A's transaction counted and treats its own as nested in it, so instead of beginning a transaction it asks for a savepoint, and that statement waits for A. A request that has used the connection before either holds it or has already waited for A's request to end, and is not affected. What happens next depends on the library:

- **Laravel** raises its count only once the savepoint is created. A's commit or rollback therefore ends the real transaction, and B's savepoint then runs outside any transaction. PostgreSQL refuses it with `SAVEPOINT can only be used in transaction blocks`, so B's request fails; later requests are not affected.
- **Doctrine DBAL** raises its count before it sends anything, so A's commit or rollback goes to B's savepoint, which does not exist yet, and fails. A failed commit still lowers the count; if A's error path then rolls back, as `transactional()` does, the real transaction ends, A fails as well as B, and later requests are not affected. If A's request ends on the error without a rollback, or its rollback is the call that failed, the count never returns to zero and on PostgreSQL the real transaction stays aborted, so every later request on that worker that uses the connection fails with `SQLSTATE[25P02]` (current transaction is aborted). That lasts until the connection is closed — DoctrineBundle closes it on the first request at least `idle_connection_ttl` (600 seconds by default) after it connected — or the worker is replaced, and OxPHP does not replace it for this: an exception the application turns into a `500` is not one of the failures that [recycle a worker](worker-recycling.md#recycling).

Checked against Laravel 13.43 and Doctrine DBAL 4.6 on PostgreSQL. On MySQL the count goes wrong the same way, but how the server then answers the misplaced savepoint statements was not checked.

A transaction is the request's own only on a connection that request holds alone for as long as the transaction lasts — one it opens itself without `PDO::ATTR_PERSISTENT`, or one it acquires from a [`Shared\Pool`](../shared-state/shared-pool.md#database-connection-pool) — rather than on the connection the framework keeps for the worker.

## Running These Frameworks

- **Framework mode** (`ENTRY_FILE=index.php` with worker mode off) is how the [framework examples](../examples/index.md) run. Every request starts from a clean state, nothing overlaps on a thread, and `RUNTIME_HOOKS` makes no request suspend.
- **Worker mode without overlap.** Leave `RUNTIME_HOOKS` unset and keep every wait listed under [When Requests Overlap](#when-requests-overlap) out of the request path — `oxphp_sleep()`, awaits, streaming responses and the rest. The worker then runs its requests one after another, and the integration only has to reset state between them.
- **Worker mode with overlap** — `RUNTIME_HOOKS`, or any of those suspension points — needs two things. Request state has to be kept per request, as [above](#request-state-in-shared-objects). Any request that opens a transaction needs a connection of its own rather than the worker's shared one. Keeping request state per request does not give a request a connection of its own.

## See Also

- [Runtime Hooks](runtime-hooks.md) — the hook categories and what each one makes suspend
- [Shared Connections Under Runtime Hooks](runtime-hooks-shared-state.md) — how one connection behaves when several fibers share it
- [Fiber Multiplexing](fiber-multiplexing.md) — how a worker runs several requests at once and where it can suspend one
- [Worker State Between Requests](worker-state.md) — what a worker resets between requests and what it keeps
