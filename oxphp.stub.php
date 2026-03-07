<?php
/**
 * OxPHP Extension Stub File
 *
 * Provides IDE autocompletion and static analysis support for
 * functions defined by the oxphp_sapi PHP extension.
 *
 * This file is NOT loaded at runtime — it is only used by IDEs
 * (PhpStorm, VS Code + Intelephense) and static analyzers (PHPStan, Psalm).
 *
 * @package OxPHP
 * @version 0.1.0
 * @link https://github.com/oxphp/oxphp
 */

/**
 * Returns the unique request ID for the current request.
 *
 * The same value is sent in the X-Request-ID response header.
 * If the client sends an X-Request-ID header, the server passes
 * it through instead of generating a new one.
 *
 * @return string 16-character hex request ID (e.g. "67b9a3c100000042")
 *
 * @example
 * $id = oxphp_request_id();
 * error_log("[$id] Processing order");
 */
function oxphp_request_id(): string {}

/**
 * Returns the index of the PHP ZTS worker thread handling this request.
 *
 * Worker indices range from 0 to PHP_WORKERS - 1. Useful for
 * per-worker caching, debugging, and log correlation.
 *
 * @return int Zero-based worker thread index
 *
 * @example
 * $tmp = "/tmp/worker_" . oxphp_worker_id() . "_buffer.dat";
 */
function oxphp_worker_id(): int {}

/**
 * Returns server metadata for the current request.
 *
 * The request_time is a Unix timestamp with microsecond precision,
 * set before php_request_startup() for accurate timing.
 *
 * @return array{sapi: string, version: string, worker_id: int, request_time: float}
 *
 * @example
 * $info = oxphp_server_info();
 * // ["sapi" => "oxphp", "version" => "0.1.0", "worker_id" => 3, "request_time" => 1740000000.123]
 */
function oxphp_server_info(): array {}

/**
 * Flushes the response to the client and marks the request as finished.
 *
 * Any code after this call continues executing without blocking
 * the HTTP response. Similar to fastcgi_finish_request() in PHP-FPM.
 *
 * Returns false if already called on this request.
 *
 * @return bool true on success, false if already finished
 *
 * @example
 * echo json_encode(["status" => "accepted"]);
 * oxphp_finish_request();
 * // background work — client already got 200 OK
 * send_notification_email($user);
 */
function oxphp_finish_request(): bool {}

/**
 * Checks whether the server is running in worker mode.
 *
 * In worker mode, PHP boots once and handles multiple requests via
 * oxphp_worker(). In traditional mode, each request spawns a fresh
 * PHP process. Use this to conditionally enable worker-specific logic.
 *
 * @return bool true if running in worker mode
 *
 * @example
 * if (oxphp_is_worker()) {
 *     // persistent connections, shared state, etc.
 * }
 */
function oxphp_is_worker(): bool {}

/**
 * Checks whether the current request is in streaming mode.
 *
 * In streaming mode (SSE, chunked transfer), output is flushed
 * to the client immediately rather than buffered.
 *
 * @return bool true if streaming mode is active
 *
 * @example
 * if (oxphp_is_streaming()) {
 *     echo "data: " . json_encode($event) . "\n\n";
 *     flush();
 * }
 */
function oxphp_is_streaming(): bool {}

/**
 * Extends the request timeout to prevent the server from killing
 * long-running scripts.
 *
 * Call periodically in long-running loops. The timeout is extended
 * by the given number of seconds from the time of the call.
 *
 * @param int $time Seconds to extend the timeout by (default: 10)
 * @return bool Always true
 *
 * @example
 * foreach ($large_dataset as $row) {
 *     oxphp_request_heartbeat(30);
 *     process($row);
 * }
 */
function oxphp_request_heartbeat(int $time = 10): bool {}

/**
 * Activate streaming mode and flush buffered output as a chunk to the client.
 *
 * On the first call, HTTP headers are sent immediately. Each subsequent call
 * flushes any output written since the last flush as a new chunk.
 *
 * Use this for Server-Sent Events (SSE), chunked transfer, or any real-time
 * streaming pattern. Streaming mode is also auto-activated when PHP sets
 * Content-Type: text/event-stream.
 *
 * Returns false if oxphp_finish_request() was already called.
 *
 * @return bool true on success, false if request is already finished
 *
 * @example
 * header('Content-Type: text/event-stream');
 * header('Cache-Control: no-cache');
 * for ($i = 0; $i < 10; $i++) {
 *     echo "data: " . json_encode(["counter" => $i]) . "\n\n";
 *     oxphp_stream_flush();
 *     sleep(1);
 * }
 */
function oxphp_stream_flush(): bool {}

/**
 * Enter worker mode loop. The handler is called for each HTTP request.
 *
 * Between requests, a soft reset cleans per-request state (output buffers,
 * headers, superglobals) without destroying the PHP heap. Bootstrap state
 * (autoloader, DI container, routes, variables in the outer scope) persists.
 *
 * Only available when WORKER_FILE env var is set. Returns true on graceful
 * shutdown (channel closed), or exits the loop on max_requests/max_memory
 * limits. Code after oxphp_worker() runs during shutdown.
 *
 * @param callable $handler Called for each request with fresh superglobals
 * @return bool true on graceful exit, false if not in worker mode
 *
 * @example
 * $app = new App();  // boot once
 * oxphp_worker(function () use ($app) {
 *     $app->handle();  // called per request
 * });
 * $app->terminate();  // graceful shutdown
 */
function oxphp_worker(callable $handler): bool {}
