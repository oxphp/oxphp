---
title: Server-Sent Events (SSE)
description: Stream real-time data to browser clients using Server-Sent Events in OxPHP with built-in backpressure and connection management.
---

# Server-Sent Events (SSE)

OxPHP streams real-time data to clients using the Server-Sent Events protocol with built-in backpressure support. Set `Content-Type: text/event-stream` in your PHP script and call `oxphp_stream_flush()` — OxPHP handles the rest.

## How It Works

1. Your PHP script sets `Content-Type: text/event-stream` via `header()` and writes SSE-formatted lines using `echo`.
2. The first call to `oxphp_stream_flush()` sends the HTTP headers to the client and enters streaming mode. The client connection remains open.
3. Each subsequent call to `oxphp_stream_flush()` flushes buffered output as a new chunk, delivering it to the client immediately.
4. OxPHP maintains an internal buffer of up to 64 chunks between the PHP worker and the client. When the buffer is full — because a slow client has not consumed earlier chunks — `oxphp_stream_flush()` blocks until space becomes available. This prevents unbounded memory growth.
5. When the PHP script finishes, OxPHP closes the connection gracefully. If the client disconnects mid-stream, the script finds out at the first flush after OxPHP sees the connection close: OxPHP sets PHP's `connection_aborted()` flag and, unless the script has called `ignore_user_abort(true)`, stops the script inside that flush — the call does not return, which is how PHP stops a script under PHP-FPM when its output can no longer be written. A script that has called `ignore_user_abort(true)` gets the flush back and keeps running, and checking `connection_aborted()` is what ends its loop (see [Detecting client disconnects](#detecting-client-disconnects)).

> **Note:** Keep event payloads small to maintain smooth throughput. Large payloads can fill the 64-chunk buffer quickly, causing PHP to block on each flush.

## PHP Examples

### Basic SSE stream

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');
header('Connection: keep-alive');

for ($i = 0; $i < 100; $i++) {
    $data = json_encode(['counter' => $i, 'time' => microtime(true)]);
    echo "id: {$i}\n";
    echo "event: tick\n";
    echo "data: {$data}\n\n";
    oxphp_stream_flush();

    sleep(1);

    // Send a comment heartbeat every 15 seconds to keep proxies from closing idle connections
    if ($i % 15 === 0) {
        echo ": heartbeat\n\n";
        oxphp_stream_flush();
    }
}
```

### Checking streaming state

Use `oxphp_is_streaming()` to check whether the current request is already in streaming mode. This is useful in middleware or shared request handlers:

```php
<?php
if (!oxphp_is_streaming()) {
    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');
}

echo "data: {\"status\": \"connected\"}\n\n";
oxphp_stream_flush();
```

### Detecting client disconnects

A stream that has sent its headers learns that its client has gone only when it flushes. By default that flush is where the script stops: `oxphp_stream_flush()` (or `flush()`) does not return, so a `while (!connection_aborted())` condition is never evaluated again, and every `finally` block the flush was inside is skipped. PHP-FPM stops a script whose output can no longer be written the same way. What still runs is the end of the request: `register_shutdown_function()` callbacks, in which `connection_aborted()` returns `1`, and the destructors of the objects that go away with the request. Unlike PHP-FPM, which discards whatever those write, OxPHP stops a callback at the first output it sends and skips the callbacks registered after it. The destructors still run, and the first of them to send output is stopped the same way, with the destructors due after it skipped. Output held in an output buffer — one opened with `ob_start()`, or the one `output_buffering` in `php.ini` opens — is sent only when that buffer is flushed. Keep that cleanup free of output, or call `ignore_user_abort(true)` as below.

To clean up in the script itself — in a `finally` block, or after the loop — call `ignore_user_abort(true)` and let `connection_aborted()` end the loop. The flush that finds the client gone then returns, `connection_aborted()` returns `1` from that point on, and the loop leaves through its condition:

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

// A stream outlives max_execution_time, which is 30 s unless the PHP configuration sets another value
set_time_limit(0);

// Keep running when the client leaves, so that the loop below sees it and ends
ignore_user_abort(true);

$db = new PDO(/* ... */);

try {
    while (!connection_aborted()) {
        echo "data: " . json_encode(['ts' => time()]) . "\n\n";
        oxphp_stream_flush();
        sleep(1);
    }
} finally {
    $db = null; // also runs when the loop ends on a disconnect
}
```

With the call, that check is the only thing that ends the loop: the script keeps running after the client disconnects, what it writes is discarded, and `connection_aborted()` reports the disconnect. That is the same contract as under any other SAPI, and it means the loop needs a bound of its own — a stream that has asked to outlive its client and neither checks `connection_aborted()` nor ends by itself holds its worker until the server shuts down. The call applies to the request that makes it and to no other request on the same worker. The value a request *starts* with counts the same way, though: `ignore_user_abort = On` in `php.ini`, or an `ignore_user_abort(true)` in a worker script's bootstrap — the code before its request loop, which some worker examples begin with — is the baseline every request starts from, so every stream served there behaves as if it had made the call and needs its `connection_aborted()` check.

### Using native flush()

PHP's native `flush()` also works for streaming, but requires clearing all output buffer layers first. Prefer `oxphp_stream_flush()` — it manages output buffers automatically and integrates with OxPHP's backpressure system.

```php
<?php
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

while (ob_get_level()) {
    ob_end_clean();
}

for ($i = 0; $i < 100; $i++) {
    echo "data: " . json_encode(['counter' => $i]) . "\n\n";
    flush();
    sleep(1);
}
```

### SSE with Worker Mode

SSE works in both standard and worker mode. In worker mode, the streaming connection occupies the worker for the full duration of the stream. The worker handles the next request only after the script finishes.

In a broadcast every open stream has to receive every event, so the example reads them from a Redis Stream rather than popping them off a list. Reading a stream leaves its entries where they are and each reader keeps its own position, so every client reads every event the stream still holds. `BRPOP` on a list works the other way: it removes the element it returns, and that element goes to exactly one of the connections waiting on the list. That is the right shape for a work queue and the wrong one for a broadcast — with two streams open, each event would reach only one of them.

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

oxphp_worker(function () {
    if (($_SERVER['HTTP_ACCEPT'] ?? '') !== 'text/event-stream') {
        http_response_code(400);
        echo json_encode(['error' => 'SSE only']);
        return;
    }

    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');

    // A stream outlives max_execution_time, which is 30 s unless the PHP configuration sets another value
    set_time_limit(0);

    // One connection per stream: the blocking read below holds it for as long as the
    // stream lives. The last argument is the read timeout, which has to exceed the
    // 25 s the read blocks for.
    $redis = new Redis();
    $redis->connect('redis', 6379, 2.0, null, 0, 30.0);

    // The newest entry in the stream. It also finds out whether the key holds a stream at all,
    // while the answer can still be an error status: once the first frame is out the response is
    // a 200, and a browser reconnects after every close of one.
    $tail = $redis->xRevRange('events', '+', '-', 1);
    if ($tail === false) {
        error_log('SSE: ' . $redis->getLastError());
        http_response_code(500);
        return;
    }

    // Resume after the last event the browser saw (it sends the id back in Last-Event-ID).
    // A first connection, or a header that is not a stream id, starts after the newest entry.
    $last = $_SERVER['HTTP_LAST_EVENT_ID'] ?? '';
    if (!preg_match('/^\d{1,19}-\d{1,19}$/', $last)) {
        $last = $tail ? array_key_first($tail) : '0-0';
    }

    // Send the headers now; a quiet stream would otherwise send none until its first read returns.
    // This named event carries the starting id, which the browser takes as its Last-Event-ID before
    // the first event arrives; onmessage does not fire for it.
    echo "event: ready\nid: {$last}\ndata:\n\n";
    oxphp_stream_flush();

    while (true) {
        // Reading leaves the entries in the stream, so every open stream gets every one
        $read = $redis->xRead(['events' => $last], 100, 25000);
        if ($read === false) {
            // Redis refused the read, the key having been replaced by a non-stream, say. End the stream;
            // the browser reconnects and the check above answers it with an error status
            return;
        }
        foreach ($read['events'] ?? [] as $id => $fields) {
            echo "id: {$id}\ndata: {$fields['data']}\n\n";
            $last = $id;
        }
        if (!$read) {
            // No entry within 25 s — send heartbeat to keep the connection alive
            echo ": heartbeat\n\n";
        }
        oxphp_stream_flush();
    }
});
```

Publish from any PHP process — a request handler, a queue worker, a CLI script:

```php
$redis->xAdd('events', '*', ['data' => json_encode($payload)], 1000, true);
```

The last two arguments cap the stream at roughly 1000 entries (`MAXLEN ~ 1000`). Send JSON, or anything else without a line break, as `data`: the example writes it on a single `data:` line.

What the example relies on:

- **The entry id is the event id.** An id like `1790976033892-0` goes out as the event's `id:`, the browser returns the last one it saw in `Last-Event-ID` when it reconnects, and the stream continues after it (see [Behaviour on shutdown](#behaviour-on-shutdown)).
- **The first frame is an event that carries the starting id.** Safari and Firefox take an `id:` as the browser's `Last-Event-ID` only from a frame that has a `data:` line, so a bare `id:` would leave a browser cut off before its first event with no id to send: it would reconnect, start from the newest entry and miss whatever was published in between. The `ready` event, with an empty `data:` line, gives it one. `onmessage` does not fire for a named event and `addEventListener('ready', …)` does; a client library that hands every event to one callback has to skip it.
- **A stream that cannot start gets an error status, before its first frame.** Redis refuses to read a key that holds something other than a stream — a list left over from an earlier queue, say — and no id would make that read work, so the example finds it out with its first command, logs the message Redis gives and answers `500`. It has to be a status rather than a stream that just ends: once the first frame is out the response is a `200`, and a browser reconnects after every close of a `200` stream — every few seconds for as long as the page stays open, each time with a new Redis connection and nothing in the log. An `EventSource` does not retry after an error status; the page gets an `error` event with `readyState` at `EventSource.CLOSED` and has to open a new `EventSource` itself if it wants one. The same goes for a Redis that cannot be reached: `connect()` throws, the response is a `500`, and the `EventSource` is closed for good. A read that Redis refuses after the stream has started — the key replaced by a non-stream, say — ends it with a `return`, and the reconnect goes through the check above. Without that `return` the loop would take the `false` for a timeout and send nothing but heartbeats, back to back.
- **`Last-Event-ID` is checked for its format.** The header comes from the client and can hold anything, and Redis refuses a value that is not a stream id, `abc` for instance. The example treats such a value like a missing header and starts after the newest entry, the way a first connection does. The pattern admits up to 19 digits on each side of the dash, which keeps every value it accepts inside the range Redis reads.
- **A new stream reads the newest id once.** It does not pass `$` on every call: Redis resolves `$` to the newest id at the moment of each call, so entries added between two calls are skipped.
- **History is bounded.** A reader whose last event has since been trimmed away — a browser that was disconnected, or a stream held up by a slow client while more than the cap was published — continues from the oldest entry still there; the entries trimmed in between are not delivered, and nothing reports it. Size the cap to cover the longest outage you want a stream to resume from.
- **Each stream has its own connection, with a read timeout above the block time.** Without the timeout argument phpredis falls back to `default_socket_timeout`, and a read that blocks for longer than that fails with a `RedisException`. Do not open one connection per worker and hand it to every stream: with the `streams` [runtime hooks](../operations/configuration.md#runtime-hooks) on, a worker runs other requests while a stream waits on a read, and a request that has used a connection keeps it until it ends. A second stream that reaches the same connection waits for the first. If the first is still open when the wait runs out — the bound comes from `max_execution_time` and `default_socket_timeout`, see the hooks section — phpredis throws a `RedisException` and the second stream fails.
- **The script switches off PHP's execution timer.** `max_execution_time` is 30 seconds unless the PHP configuration sets another value, and the release image ships no `php.ini`; a stream is meant to outlive it. With the timer left on, the script ends with a fatal error: the timer fires after 30 seconds and takes effect when the read in progress returns, so with the 25 second reads here a quiet stream ends at the 50th second. In worker mode the call switches off the limit of the stream's own request and no other; see [Request Deadlines](worker-mode.md#request-deadlines).
- **A client that has closed its connection is noticed at the next flush.** Until then its request, and the Redis connection with it, stays open — at most 25 s here, the block time, which is also the heartbeat interval.

Redis Pub/Sub also delivers each message to every connected subscriber, but it keeps nothing: a message published while a client is reconnecting is gone, and there is no id to resume from. Use it only where a missed event does not matter.

### Several instances behind a load balancer

A stream is one long connection to one instance, chosen once by the balancer. With more than one instance:

- **Events have to cross instances.** `OxPHP\Shared\*` objects live inside one process (see [Shared State](../shared-state/shared-state.md)), so a stream on one instance cannot see an event published on another through them. Both have to read the same external store, which is what the Redis Stream above is.
- **Capacity is counted per instance.** Each instance serves the streams its own worker pool leaves room for (see [Best Practices](#best-practices)). Prefer a balancing method that counts open connections, such as `least_conn` in nginx, over round-robin: round-robin balances connections by arrival, not by how long they stay open, and a stream stays open far longer than an ordinary request.
- **No sticky sessions needed.** A reconnecting browser may land on a different instance, which is fine as long as the first frame and every event carry an `id:` and every instance reads the same stream.
- **The proxy must not buffer the stream.** A buffering proxy holds events back until its buffer fills. In nginx, turn it off for the stream's location with `proxy_buffering off`, or send `X-Accel-Buffering: no` from the script, which nginx honours unless `proxy_ignore_headers` says otherwise.
- **Heartbeats must be more frequent than the balancer's idle timeout** — see [Intermediate proxies close idle SSE connections](#intermediate-proxies-close-idle-sse-connections).
- **Use HTTP/2 between the browser and the balancer.** Over HTTP/1.1 a browser allows about six open connections per host across all its tabs, and each stream takes one; HTTP/2 carries many streams over a single connection.

## Troubleshooting

### The client receives no data until the script ends

The PHP output buffer is capturing output instead of streaming it. This happens when OB layers are active and `oxphp_stream_flush()` is not called.

**Fix:** Call `oxphp_stream_flush()` after each event. This function flushes all PHP output buffer layers and sends the accumulated output as a chunk.

### SSE connections are closed after a few minutes

PHP's `max_execution_time` is firing and terminating the script. SSE streams must run longer than the configured limit.

**Fix:** Disable the per-request execution timer at the top of the streaming script:

```php
set_time_limit(0);
```

This is preferred over setting `max_execution_time = 0` globally — it leaves the limit in place for non-SSE endpoints. Alternatively, if the entire instance is dedicated to long-lived streams:

```ini
; php.ini
max_execution_time = 0
```

### Intermediate proxies close idle SSE connections

Load balancers and proxies often close connections that carry no data for 30–60 seconds.

**Fix:** Send a comment heartbeat at regular intervals to keep the connection active:

```php
echo ": heartbeat\n\n";
oxphp_stream_flush();
```

### `oxphp_stream_flush()` returns `false`

`oxphp_finish_request()` was called earlier in the same request. Once the response is finished, streaming is not possible. Check your code for inadvertent calls to `oxphp_finish_request()` before streaming begins.

## Docker Example

SSE endpoints require PHP's execution timer to be disabled or set high. Each active SSE connection occupies one PHP worker for the full duration of the stream, so size the worker pool to accommodate your expected concurrent stream count.

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.12.0
    ports:
      - "8080:8080"
    volumes:
      - ./src:/var/www/html
    environment:
      DOCUMENT_ROOT: "/var/www/html/public"
      ENTRY_FILE: "index.php"
      PHP_WORKERS: "32"
```

Each streaming script should call `set_time_limit(0)` at the top so the per-request timer does not fire mid-stream. This keeps the global `max_execution_time` in effect for non-SSE requests, in worker mode as well: there each request has a limit of its own, so a stream switching its off leaves the requests served beside it bounded (see [Worker Mode](worker-mode.md#request-deadlines)).

## Best Practices

- **Use `oxphp_stream_flush()` instead of native `flush()`** for automatic output buffer management and backpressure integration.
- **Send periodic comment heartbeats** (`: heartbeat\n\n`) every 20–30 seconds to keep intermediate proxies from closing idle connections and to detect client disconnections early.
- **Keep event payloads small.** Large payloads fill the 64-chunk buffer faster, causing PHP to stall on each flush. For large data, send an event ID and let the client fetch the full payload via a separate request.
- **Disable PHP's execution timer per-script** with `set_time_limit(0)` for long-lived SSE endpoints, or set `max_execution_time` high enough to cover your longest expected stream duration.
- **Size your worker pool for peak concurrent streams.** Each active SSE connection holds one PHP worker for its full duration. Budget at least one worker per expected concurrent client, plus additional workers for regular non-SSE requests.

## Notes

- Event streams are never compressed, by either of two independent rules: `text/event-stream` is not a compressible media type, and a response whose length is unknown when the headers go out is passed through whatever the client accepts. That holds for every coding OxPHP offers — Brotli, Zstandard and gzip alike — so no configuration can turn compression on for a stream.
- `oxphp_stream_flush()` returns `false` if `oxphp_finish_request()` was already called on the same request.
- In worker mode, the worker remains occupied for the full duration of the stream and handles the next request only after the PHP script exits.

## Behaviour on shutdown

When the server receives SIGTERM (a rolling deploy, a `docker stop`, a Kubernetes pod eviction), it drains gracefully:

- Each open SSE stream is ended cleanly on its next `flush` — its handler bails as if the connection closed, so `register_shutdown_function()` callbacks still run and `error_get_last()['message']` reads `Request cancelled (shutdown)`.
- HTTP/2 clients receive a `GOAWAY` frame; HTTP/1.1 keep-alive connections are closed. The browser's `EventSource` reconnects automatically — to a healthy instance when a load balancer fronts the fleet.
- The stream does **not** receive a 503: its `200` headers were already sent, so the status cannot be rewritten. Design clients to resume from the last event id (send `id:` with each event; on reconnect the browser sends it back as the `Last-Event-ID` request header, available in PHP as `$_SERVER['HTTP_LAST_EVENT_ID']`); the [worker-mode example](#sse-with-worker-mode) does exactly that.

Ordinary (non-streaming) requests in flight at SIGTERM are not interrupted: they get the whole drain window to finish normally, and only requests still running when `DRAIN_TIMEOUT_SECONDS` (default 25) expires are cancelled, with ~2 more seconds to unwind. Set the orchestrator's termination grace period above `DRAIN_TIMEOUT_SECONDS` + 2, plus the longest blocking call a request can be in at the deadline — see [Graceful Shutdown](../operations/graceful-shutdown.md#shutdown-sequence).

"Streaming" here means any response that has already flushed chunked output — the server cannot tell a finite streaming download from an infinite event stream, so a large flushed download in flight at SIGTERM is ended early too, not just SSE. A request that called `oxphp_finish_request()` before SIGTERM counts as ordinary: its response is complete, and its remaining background work gets the drain window.

A handler blocked inside a single long-running **native** call (a native `sleep()`, a heavy `preg_match`, a blocking database query) cannot be interrupted until that call returns. In worker mode prefer the cooperative `oxphp_sleep()` in streaming loops — it yields to the fiber scheduler and is woken immediately on shutdown. Outside worker mode `oxphp_sleep()` falls back to a regular blocking sleep, so the stream reacts to shutdown at its next `flush` after the sleep returns.

## See Also

- [Worker Mode](worker-mode.md) -- persistent PHP workers for reduced bootstrap overhead
- [Timeouts](timeouts.md) -- configuring or disabling the request timeout for long-lived connections
- [PHP Functions](../php/functions.md) -- full reference for `oxphp_stream_flush()` and `oxphp_is_streaming()`
- [Compression](compression.md) -- which responses are compressed, and with which coding
