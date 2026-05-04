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
5. When the PHP script finishes, OxPHP closes the connection gracefully. If the client disconnects mid-stream, subsequent flush calls complete without error but the chunks are discarded.

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

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

$redis = new Redis();
$redis->pconnect('redis', 6379);

oxphp_worker(function () use ($redis) {
    if (($_SERVER['HTTP_ACCEPT'] ?? '') !== 'text/event-stream') {
        http_response_code(400);
        echo json_encode(['error' => 'SSE only']);
        return;
    }

    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');

    while (true) {
        $message = $redis->brPop('events', 25);
        if ($message) {
            echo "data: {$message[1]}\n\n";
        } else {
            // No message within timeout — send heartbeat to keep the connection alive
            echo ": heartbeat\n\n";
        }
        oxphp_stream_flush();
    }
});
```

## Troubleshooting

### The client receives no data until the script ends

The PHP output buffer is capturing output instead of streaming it. This happens when OB layers are active and `oxphp_stream_flush()` is not called.

**Fix:** Call `oxphp_stream_flush()` after each event. This function flushes all PHP output buffer layers and sends the accumulated output as a chunk.

### SSE connections are closed after a few minutes

The request timeout is terminating the connection. SSE streams must run longer than `REQUEST_TIMEOUT_SECONDS`.

**Fix:** Disable the request timeout for SSE endpoints:

```bash
REQUEST_TIMEOUT_SECONDS=0
```

If you have a mix of SSE and regular endpoints, set the timeout high enough for your longest stream, or run SSE on a separate OxPHP instance with the timeout disabled.

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

SSE endpoints require the request timeout to be disabled or set high. Each active SSE connection occupies one PHP worker for the full duration of the stream, so size the worker pool to accommodate your expected concurrent stream count.

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.5.0
    ports:
      - "8080:8080"
    volumes:
      - ./src:/var/www/html
    environment:
      DOCUMENT_ROOT: "/var/www/html/public"
      ENTRY_FILE: "index.php"
      REQUEST_TIMEOUT_SECONDS: "0"
      PHP_WORKERS: "32"
```

> **Note:** `REQUEST_TIMEOUT_SECONDS=0` disables the timeout for all requests on this instance, including non-SSE endpoints. If that is a concern, dedicate a separate OxPHP instance to SSE.

## Best Practices

- **Use `oxphp_stream_flush()` instead of native `flush()`** for automatic output buffer management and backpressure integration.
- **Send periodic comment heartbeats** (`: heartbeat\n\n`) every 20–30 seconds to keep intermediate proxies from closing idle connections and to detect client disconnections early.
- **Keep event payloads small.** Large payloads fill the 64-chunk buffer faster, causing PHP to stall on each flush. For large data, send an event ID and let the client fetch the full payload via a separate request.
- **Disable the request timeout** (`REQUEST_TIMEOUT_SECONDS=0`) for long-lived SSE endpoints, or set it high enough to cover your longest expected stream duration.
- **Size your worker pool for peak concurrent streams.** Each active SSE connection holds one PHP worker for its full duration. Budget at least one worker per expected concurrent client, plus additional workers for regular non-SSE requests.

## Notes

- Brotli compression is automatically skipped for streaming responses. Compression only applies to fully buffered responses.
- `oxphp_stream_flush()` returns `false` if `oxphp_finish_request()` was already called on the same request.
- In worker mode, the worker remains occupied for the full duration of the stream and handles the next request only after the PHP script exits.

## See Also

- [Worker Mode](worker-mode.md) -- persistent PHP processes for reduced bootstrap overhead
- [Timeouts](timeouts.md) -- configuring or disabling the request timeout for long-lived connections
- [PHP Functions](../php/functions.md) -- full reference for `oxphp_stream_flush()` and `oxphp_is_streaming()`
- [Compression](compression.md) -- Brotli compression behavior and which responses are compressed
