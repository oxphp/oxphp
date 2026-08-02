---
title: Early Response
description: Send the HTTP response immediately with oxphp_finish_request() and continue running background tasks in OxPHP.
---

# Early Response

`oxphp_finish_request()` sends the complete HTTP response to the client immediately and lets your PHP script keep running for background work. This is the OxPHP equivalent of `fastcgi_finish_request()` in PHP-FPM.

## How It Works

1. Your script sets headers, status code, and echoes the response body as usual.
2. Call `oxphp_finish_request()` — OxPHP flushes all output buffers, marks the request as finished, and delivers the complete HTTP response to the client.
3. The script continues executing for background work such as sending emails, writing cache entries, or dispatching webhooks.
4. Any output produced after the call (`echo`, `print`, `var_dump`) is silently discarded.
5. `oxphp_finish_request()` returns `true` on the first call and `false` on any subsequent call within the same request.

## Use Cases

Early response is useful whenever you want to acknowledge a request immediately while deferring non-critical work:

- **Email sending** — return "accepted" immediately, send the email in the background
- **Cache warming** — respond with cached data, then regenerate the cache entry
- **Analytics and logging** — acknowledge the request, then write detailed analytics records
- **Webhook dispatch** — confirm receipt to the caller, then fan out webhook deliveries
- **Image processing** — return a URL immediately, then process the full-size image

## PHP Examples

### Basic usage

```php
<?php
header('Content-Type: application/json');
echo json_encode(['status' => 'accepted', 'id' => uniqid()]);

// Send response now — client receives the full response at this point
oxphp_finish_request();

// Background work runs here; client is no longer waiting
file_put_contents('/tmp/audit.log', date('c') . " request processed\n", FILE_APPEND);
send_notification_email($user);
```

### Guard against double invocation

`oxphp_finish_request()` returns `false` on the second and subsequent calls. Check the return value in middleware-heavy applications where multiple layers might call the function:

```php
<?php
function finish_and_cleanup(): void
{
    if (!oxphp_finish_request()) {
        // Already finished — background work was already scheduled
        return;
    }

    // First call — safe to run cleanup
    flush_metrics_buffer();
    close_external_connections();
}
```

### Conditional background work

```php
<?php
header('Content-Type: application/json');
$payload = json_decode(file_get_contents('php://input'), true);

$result = handle_request($payload);
echo json_encode($result);

if ($result['needs_sync']) {
    oxphp_finish_request();
    sync_to_external_service($result);
}
// No early finish if sync is not needed — script exits normally
```

### Worker mode

In worker mode, the PHP worker remains occupied until the entire script (including all background work) completes. The worker does not accept a new request until the callback returns.

```php
<?php
oxphp_worker(function () {
    $order = json_decode(file_get_contents('php://input'), true);

    $result = process_order($order);
    header('Content-Type: application/json');
    echo json_encode(['order_id' => $result['id'], 'status' => 'accepted']);
    oxphp_finish_request();

    // Worker is still occupied during this background work
    send_confirmation_email($result);
    update_inventory($result);
    notify_warehouse($result);
    // Worker becomes available after this point
});
```

> **Note:** Account for background processing time when sizing your worker pool. A worker that spends 3 seconds on post-response work after every request effectively handles fewer concurrent requests.

## Troubleshooting

### Background work is not completing

PHP's `max_execution_time` continues to apply after `oxphp_finish_request()` is called. If the total script execution time — including background work — exceeds the limit, the request is cancelled with a `Request cancelled (timeout)` fatal.

**Fix:** Raise `max_execution_time` (in `php.ini` or via `set_time_limit()` from the script), or move long background tasks to a message queue:

```php
set_time_limit(300);
oxphp_finish_request();
// ... long-running work ...
```

For work that regularly takes more than a few seconds, publish a message to Redis, RabbitMQ, or a similar queue and let a dedicated consumer handle it asynchronously.

### Session changes are lost

Session data must be written before calling `oxphp_finish_request()`. Changes made after the call are discarded.

**Fix:** Call `session_write_close()` before `oxphp_finish_request()`:

```php
<?php
$_SESSION['last_seen'] = time();
session_write_close();      // Persist session before finishing
oxphp_finish_request();     // Send response
```

### Response body is empty after calling `oxphp_finish_request()`

If you call `oxphp_finish_request()` before any `echo` output, the client receives an empty body. Build and output the response first, then call the function.

## Notes

- `oxphp_finish_request()` returns `true` on the first call and `false` on subsequent calls within the same request.
- All output (`echo`, `print`, `var_dump`) after the first call is silently discarded.
- In worker mode, the worker remains occupied until the entire callback completes, including all post-response code.
- The request timeout continues to apply to background code running after `oxphp_finish_request()`.
- `oxphp_finish_request()` and `oxphp_stream_flush()` are mutually exclusive: calling `oxphp_finish_request()` before starting a stream prevents streaming, and calling it after `oxphp_stream_flush()` closes the stream.

## See Also

- [Worker Mode](worker-mode.md) -- persistent PHP processes and how early response interacts with the request loop
- [Timeouts](timeouts.md) -- how the request timeout applies to background work
- [PHP Functions](../php/functions.md) -- full reference for `oxphp_finish_request()` and other built-in functions
