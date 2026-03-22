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
 * Cooperative sleep: suspends the current fiber to let other requests
 * proceed on this worker thread.
 *
 * When called inside a fiber (worker mode with multiplexing), the fiber
 * is suspended and a timer is registered. The scheduler resumes it after
 * the specified duration. Other requests can be handled in the meantime.
 *
 * When called outside a fiber (traditional mode), falls back to blocking usleep().
 *
 * @param float $seconds Duration to sleep in seconds (e.g. 0.5 for 500ms)
 * @return void
 *
 * @example
 * oxphp_worker(function () {
 *     // Non-blocking: other requests proceed during sleep
 *     oxphp_sleep(0.1);  // 100ms cooperative sleep
 *     echo "done";
 * });
 */
function oxphp_sleep(float $seconds): void {}

/**
 * Cooperative microsecond sleep: suspends the current fiber to let other
 * requests proceed on this worker thread.
 *
 * Identical to oxphp_sleep() but accepts microseconds as an integer.
 * Falls back to blocking usleep() when not inside a fiber.
 *
 * @param int $microseconds Duration to sleep in microseconds
 * @return void
 *
 * @example
 * oxphp_worker(function () {
 *     oxphp_usleep(50000);  // 50ms cooperative sleep
 *     echo "done";
 * });
 */
function oxphp_usleep(int $microseconds): void {}

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

/**
 * Dispatch a closure for asynchronous execution on the dedicated async worker pool.
 *
 * The closure is transferred to a separate OS thread (PHP ZTS). Variables captured
 * via `use` and arguments passed via ...$args are serialized on the source thread
 * and deserialized on the async worker thread (independent copies).
 *
 * Supported argument types: null, bool, int, float, string, array.
 * Resources and objects are rejected with E_WARNING.
 *
 * Requires ASYNC_WORKERS > 0. Returns false if the async pool is disabled or
 * the queue is full.
 *
 * @param \Closure $closure The closure to execute asynchronously
 * @param mixed ...$args Arguments serialized to the async worker thread
 * @return int|false Promise ID (positive integer) on success, false on failure
 *
 * @example
 * $p = oxphp_async(function(int $x, int $y): int {
 *     return $x + $y;
 * }, 10, 20);
 * $result = oxphp_async_await($p); // 30
 */
function oxphp_async(\Closure $closure, mixed ...$args): int|false {}

/**
 * Block until the async task completes and return its result.
 *
 * The return value is deserialized from the async worker thread onto the
 * current thread's heap.
 *
 * Each promise ID can only be awaited once. Non-awaited promises are cleaned up
 * automatically at request end (RSHUTDOWN) with a 5-second timeout.
 *
 * @param int $promise_id Promise ID returned by oxphp_async()
 * @param float|null $timeout Maximum seconds to wait, null = wait indefinitely
 * @return mixed The return value of the closure
 *
 * @throws \OxPHP\AsyncException If the closure threw an exception or called die()/exit()
 * @throws \OxPHP\AsyncTimeoutException If the timeout expired before completion
 *
 * @example
 * $p = oxphp_async(function(): string { return 'hello'; });
 * $result = oxphp_async_await($p); // "hello"
 *
 * // With timeout:
 * try {
 *     $result = oxphp_async_await($p, 2.0);
 * } catch (\OxPHP\AsyncTimeoutException $e) {
 *     // task took longer than 2 seconds
 * }
 */
function oxphp_async_await(int $promise_id, ?float $timeout = null): mixed {}

/**
 * Await multiple promises and return all results.
 *
 * Blocks until every promise completes (or fails/times out). Returns an
 * associative array mapping each promise ID to its result value.
 *
 * @param int[] $promise_ids Array of promise IDs from oxphp_async()
 * @param float|null $timeout Per-promise timeout in seconds, null = no limit
 * @return array<int, mixed> Map of promise ID => result value
 *
 * @throws \OxPHP\AsyncException If any promise fails
 * @throws \OxPHP\AsyncTimeoutException If any promise times out
 *
 * @example
 * $p1 = oxphp_async(fn() => 1);
 * $p2 = oxphp_async(fn() => 2);
 * $p3 = oxphp_async(fn() => 3);
 * $results = oxphp_async_await_all([$p1, $p2, $p3]);
 * // [$p1 => 1, $p2 => 2, $p3 => 3]
 */
function oxphp_async_await_all(array $promise_ids, ?float $timeout = null): array {}

/**
 * Race multiple promises and return the first to complete.
 *
 * Uses true concurrent race semantics (futures::select_all) — the fastest
 * promise wins regardless of array order. Non-winning promises remain
 * individually awaitable via oxphp_async_await().
 *
 * On timeout, all specified promises are cancelled and cannot be awaited.
 *
 * @param int[] $promise_ids Array of promise IDs from oxphp_async()
 * @param float|null $timeout Overall timeout in seconds, null = no limit
 * @return array{id: int, value: mixed} The winning promise ID and its result
 *
 * @throws \OxPHP\AsyncException If the winning promise threw an exception
 * @throws \OxPHP\AsyncTimeoutException If no promise completes within timeout
 *
 * @example
 * $p1 = oxphp_async(fn() => slow_api_a());
 * $p2 = oxphp_async(fn() => slow_api_b());
 * $winner = oxphp_async_await_any([$p1, $p2]);
 * // ['id' => $p2, 'value' => ...] (whichever finished first)
 * $other = oxphp_async_await($p1); // non-winner still awaitable
 */
function oxphp_async_await_any(array $promise_ids, ?float $timeout = null): array {}

/**
 * Register a PHP class as an attribute-based decorator.
 *
 * The class must implement OxPHP\Decorator\AttributeInterface and be
 * marked with #[Attribute(...)]. Once registered, any function, method,
 * or class annotated with this attribute will have before()/after()
 * called around each invocation.
 *
 * Call once during application bootstrap (after autoloader setup).
 *
 * @param string $class Fully qualified class name
 * @return bool true on success, false with E_WARNING on validation failure
 *
 * @example
 * oxphp_register_decorator(Timer::class);
 *
 * #[Timer(label: 'api')]
 * function handle_request(): void { ... }
 */
function oxphp_register_decorator(string $class): bool {}

namespace OxPHP {
    /**
     * Thrown when an async task fails — the closure threw an exception,
     * or called die()/exit().
     *
     * The message contains the original exception class and message:
     * "Async task failed: [DomainException] invalid value"
     */
    class AsyncException extends \Exception {}

    /**
     * Thrown when oxphp_async_await() times out before the task completes.
     */
    class AsyncTimeoutException extends AsyncException {}

    /**
     * Reserved for future use. Previously planned for frozen variable
     * write protection; currently not thrown by the runtime.
     */
    class AsyncBorrowException extends \Exception {}
}

namespace OxPHP\Decorator {
    /**
     * Interface for attribute-based decorators.
     *
     * Implement this interface and register with oxphp_register_decorator()
     * to intercept function/method calls via PHP 8+ attributes.
     *
     * @example
     * #[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
     * class Timer implements AttributeInterface {
     *     public function before(Context $ctx): void {
     *         // called before the decorated function
     *     }
     *     public function after(Context $ctx): void {
     *         // called after the decorated function
     *     }
     * }
     */
    interface AttributeInterface {
        public function before(Context $ctx): void;
        public function after(Context $ctx): void;
    }

    /**
     * Context passed to decorator before()/after() methods.
     *
     * Properties are populated by the server before each call.
     * Lazy methods (getParams, getResult) avoid overhead when not used.
     */
    final class Context {
        /** Full target name: "App\Service::method" or "my_function" */
        public readonly string $target;

        /** Class name, or "" for standalone functions */
        public readonly string $class;

        /** Method name, or "" for standalone functions */
        public readonly string $method;

        /** Function name for TARGET_FUNCTION, or "" for methods */
        public readonly string $function;

        /** spl_object_id for method calls, 0 for functions */
        public readonly int $objectId;

        /** Current request ID from the server */
        public readonly string $requestId;

        /** W3C trace ID (if distributed tracing is enabled) */
        public readonly string $traceId;

        /**
         * Get the arguments passed to the decorated function.
         *
         * Lazy: the array is built from zvals on demand. Zero cost if not called.
         *
         * @return array Indexed array of argument values
         */
        public function getParams(): array {}

        /**
         * Get the return value of the decorated function.
         *
         * Only meaningful in after(). Returns null in before() or when
         * the function threw an exception.
         *
         * @return mixed Return value, or null
         */
        public function getResult(): mixed {}

        /**
         * Check whether the decorated function returned a value.
         *
         * Returns false in before(), or in after() when the function threw.
         *
         * @return bool true if getResult() has a meaningful value
         */
        public function hasResult(): bool {}
    }

    /**
     * Thrown when a Rust-native decorator rejects a function call
     * via DecoratorAction::Reject.
     */
    class RejectedException extends \Exception {}
}
