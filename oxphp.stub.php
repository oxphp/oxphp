<?php
/**
 * OxPHP Extension Stub File
 *
 * Provides IDE autocompletion and static analysis support for
 * functions and classes defined by the oxphp_sapi PHP extension.
 *
 * This file is NOT loaded at runtime — it is only used by IDEs
 * (PhpStorm, VS Code + Intelephense) and static analyzers (PHPStan, Psalm).
 *
 * @package OxPHP
 * @version 0.1.0
 * @link https://github.com/oxphp/oxphp
 */

// ═══════════════════════════════════════════════════════════════
//  Global Functions
// ═══════════════════════════════════════════════════════════════

/**
 * Returns the HTTP Request object for the current request context.
 *
 * The object is a lightweight proxy — data is fetched lazily from Rust
 * thread-local storage via FFI only when a method is called. No allocation
 * overhead for data you never access.
 *
 * @return \OxPHP\Http\RequestInterface Typed, read-only request object
 *
 * @throws \OxPHP\Http\Exception\NoActiveRequestException If no active request context
 * @throws \OxPHP\Http\Exception\AsyncContextException If called from an oxphp_async() callback
 * @throws \OxPHP\Http\Exception\WorkerIdleException If worker is between requests
 *
 * @example
 * $request = oxphp_http_request();
 * $page = $request->query('page', 1);
 * $token = $request->header('Authorization');
 */
function oxphp_http_request(): \OxPHP\Http\RequestInterface {}

/**
 * Check if PHP superglobals ($_GET, $_POST, etc.) are populated.
 *
 * When SUPERGLOBALS_ENABLED=false, the object API via oxphp_http_request()
 * is the only way to access request data.
 *
 * @return bool true if superglobals are enabled (default)
 *
 * @example
 * if (!oxphp_superglobals_enabled()) {
 *     // must use oxphp_http_request() for all request data
 *     $request = oxphp_http_request();
 * }
 */
function oxphp_superglobals_enabled(): bool {}

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

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Http — Request Object API
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Http {

    /**
     * Read-only HTTP request interface.
     *
     * All HTTP data is fixed at the moment of receipt. The only mutable
     * component is attributes() for middleware enrichment. Data is fetched
     * lazily from Rust via FFI — only what you access crosses the bridge.
     */
    interface RequestInterface
    {
        // ── URI & Method ──

        /** HTTP method (e.g. "GET", "POST"). */
        public function method(): string;

        /** URI path without query string (e.g. "/users/42"). */
        public function path(): string;

        /**
         * Full URI including scheme, host, port (if non-default), path, and query.
         * Port is omitted when it matches the scheme default (80/443).
         *
         * @example "https://example.com:8080/users/42?page=2"
         */
        public function fullUri(): string;

        /** URI scheme: "http" or "https". */
        public function scheme(): string;

        /** Hostname from Host header, or empty string if absent. */
        public function host(): string;

        /** Port from Host header, or scheme default (80 for http, 443 for https). */
        public function port(): int;

        /** Raw query string without leading "?", or null if absent. */
        public function queryString(): ?string;

        /** Whether the request arrived over TLS. */
        public function isSecure(): bool;

        /** Case-insensitive HTTP method comparison. */
        public function isMethod(string $method): bool;

        // ── Protocol ──

        /** Full protocol string (e.g. "HTTP/1.1"). */
        public function httpProtocol(): string;

        /** Protocol version only (e.g. "1.1", "2"). */
        public function httpProtocolVersion(): string;

        // ── Query Parameters ($_GET replacement) ──

        /**
         * Access query string parameters.
         *
         * Supports bracket notation: ?a[]=1&a[]=2 → ['a' => ['1', '2']].
         * Bridge returns flat pairs; bracket parsing happens on PHP side.
         *
         * @param string|null $key Specific key, or null for all params
         * @param mixed $default Returned when key is absent
         * @return mixed Array of all params, or single value, or $default
         */
        public function query(?string $key = null, mixed $default = null): mixed;

        // ── Parsed Body ($_POST + JSON replacement) ──

        /**
         * Access parsed request body based on Content-Type.
         *
         * - application/x-www-form-urlencoded → array
         * - multipart/form-data → array
         * - application/json → decoded array/object (null on invalid JSON)
         * - other Content-Type → null
         *
         * Not tied to HTTP method — works with POST, PUT, PATCH, etc.
         * Parsed result is cached per request. Parsing happens in Rust.
         *
         * @param string|null $key Specific key, or null for full body
         * @param mixed $default Returned when key is absent
         * @return mixed Parsed body or single value or $default
         */
        public function payload(?string $key = null, mixed $default = null): mixed;

        // ── Headers ──

        /**
         * Get a header value by name (case-insensitive).
         *
         * Returns the raw value as-is. For multi-value headers (Accept,
         * X-Forwarded-For), the full string is returned:
         * "text/html,application/xhtml+xml,application/xml;q=0.9"
         *
         * @param string $name Header name
         * @param string|null $default Returned when header is absent
         * @return string|null Header value or $default
         */
        public function header(string $name, ?string $default = null): ?string;

        /** All headers as name => value array. */
        public function headers(): array;

        /** Check if a header exists (case-insensitive). */
        public function hasHeader(string $name): bool;

        // ── Cookies ($_COOKIE replacement) ──

        /**
         * Get a cookie value by name.
         *
         * @param string $name Cookie name
         * @param string|null $default Returned when cookie is absent
         * @return string|null Cookie value or $default
         */
        public function cookie(string $name, ?string $default = null): ?string;

        /** All cookies as name => value array. */
        public function cookies(): array;

        // ── Raw Body (php://input replacement) ──

        /** Raw request body bytes. Not cached — FFI call each time. */
        public function body(): string;

        /** Content-Type header value, or null. */
        public function contentType(): ?string;

        // ── File Uploads ($_FILES replacement) ──

        /**
         * Get a single uploaded file by field name.
         * For array fields (name="photos[]"), returns the first file.
         *
         * @return UploadedFileInterface|null The file or null if not found
         */
        public function file(string $name): ?UploadedFileInterface;

        /**
         * Get uploaded files.
         *
         * Without argument: all files as a flat UploadedFileInterface[] array.
         * With name: all files for that field (supports name="photos[]").
         *
         * @param string|null $name Field name filter, or null for all
         * @return UploadedFileInterface[]
         */
        public function files(?string $name = null): array;

        // ── Client ──

        /** Client IP address (REMOTE_ADDR). */
        public function ip(): string;

        // ── Timing ──

        /**
         * Request start timestamp.
         *
         * @param bool $asFloat true for float with sub-second precision
         * @return int|float Unix timestamp
         */
        public function startTime(bool $asFloat = false): int|float;

        // ── Attributes (mutable middleware container) ──

        /**
         * Mutable key-value container for middleware enrichment.
         *
         * Per-request, shared between Fibers on the same thread.
         * The Attributes object is created on first call and cached.
         */
        public function attributes(): AttributesInterface;

        // ── Session ──

        /**
         * Live read-only view on $_SESSION.
         *
         * Returns null if session_start() has not been called.
         * Values reflect $_SESSION state at the time of each method call.
         * Session management (start, save, destroy, set) via native session_*().
         */
        public function session(): ?SessionInterface;
    }

    /**
     * Read-only view on $_SESSION.
     *
     * Session lifecycle (start, save, destroy, write) is managed through
     * native PHP session_*() functions. This interface only reads.
     */
    interface SessionInterface
    {
        /** Current session ID (session_id()). */
        public function id(): string;

        /** Current session name (session_name()). */
        public function name(): string;

        /**
         * Get a session value by key.
         *
         * @param string $key Session key
         * @param mixed $default Returned when key is absent
         * @return mixed Session value or $default
         */
        public function get(string $key, mixed $default = null): mixed;

        /** Check if a key exists in the session. */
        public function has(string $key): bool;

        /** All session data as an array. */
        public function all(): array;
    }

    /**
     * Represents an uploaded file with server-side MIME type detection.
     *
     * type() detects MIME from file contents (magic bytes), not from the
     * client-provided Content-Type which can be spoofed. The detected type
     * is cached on first call. moveTo() automatically calls type() before
     * moving to ensure the cache is populated.
     */
    interface UploadedFileInterface
    {
        /** Original filename from the client. */
        public function name(): string;

        /** MIME type reported by the client (unreliable, can be spoofed). */
        public function clientType(): string;

        /**
         * MIME type detected from file contents (magic bytes).
         *
         * Cached on first call. Returns "application/octet-stream" if
         * detection fails. moveTo() auto-calls type() before moving.
         */
        public function type(): string;

        /** File size in bytes. */
        public function size(): int;

        /** Path to the temporary uploaded file. */
        public function tmpPath(): string;

        /** Upload error code (UPLOAD_ERR_* constant). */
        public function error(): int;

        /** Whether the upload succeeded (error === UPLOAD_ERR_OK). */
        public function isValid(): bool;

        /**
         * Move the uploaded file to a destination path.
         *
         * Automatically calls type() before moving to cache MIME detection.
         * Returns false if the file is not valid or the move fails.
         *
         * @param string $path Destination file path
         * @return bool true on success
         */
        public function moveTo(string $path): bool;
    }

    /**
     * Mutable key-value container for request attributes.
     *
     * Used by middleware to attach data to the request (auth user, locale,
     * route parameters, etc.). Per-request, shared between Fibers.
     */
    interface AttributesInterface
    {
        /**
         * @param string $key Attribute key
         * @param mixed $default Returned when key is absent
         * @return mixed Attribute value or $default
         */
        public function get(string $key, mixed $default = null): mixed;

        /** Set an attribute value. */
        public function set(string $key, mixed $value): void;

        /** Check if an attribute exists. */
        public function has(string $key): bool;

        /** Remove an attribute. */
        public function remove(string $key): void;

        /** All attributes as an array. */
        public function all(): array;
    }

    /** @internal Native implementation — use RequestInterface for type hints. */
    final class Request implements RequestInterface
    {
        public function method(): string {}
        public function path(): string {}
        public function fullUri(): string {}
        public function scheme(): string {}
        public function host(): string {}
        public function port(): int {}
        public function queryString(): ?string {}
        public function isSecure(): bool {}
        public function isMethod(string $method): bool {}
        public function httpProtocol(): string {}
        public function httpProtocolVersion(): string {}
        public function query(?string $key = null, mixed $default = null): mixed {}
        public function payload(?string $key = null, mixed $default = null): mixed {}
        public function header(string $name, ?string $default = null): ?string {}
        public function headers(): array {}
        public function hasHeader(string $name): bool {}
        public function cookie(string $name, ?string $default = null): ?string {}
        public function cookies(): array {}
        public function body(): string {}
        public function contentType(): ?string {}
        public function file(string $name): ?UploadedFileInterface {}
        public function files(?string $name = null): array {}
        public function ip(): string {}
        public function startTime(bool $asFloat = false): int|float {}
        public function attributes(): AttributesInterface {}
        public function session(): ?SessionInterface {}
    }

    /** @internal Native implementation — use SessionInterface for type hints. */
    final class Session implements SessionInterface
    {
        public function id(): string {}
        public function name(): string {}
        public function get(string $key, mixed $default = null): mixed {}
        public function has(string $key): bool {}
        public function all(): array {}
    }

    /** @internal Native implementation — use UploadedFileInterface for type hints. */
    final class UploadedFile implements UploadedFileInterface
    {
        public function name(): string {}
        public function clientType(): string {}
        public function type(): string {}
        public function size(): int {}
        public function tmpPath(): string {}
        public function error(): int {}
        public function isValid(): bool {}
        public function moveTo(string $path): bool {}
    }

    /** @internal Native implementation — use AttributesInterface for type hints. */
    final class Attributes implements AttributesInterface
    {
        public function get(string $key, mixed $default = null): mixed {}
        public function set(string $key, mixed $value): void {}
        public function has(string $key): bool {}
        public function remove(string $key): void {}
        public function all(): array {}
    }
}

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Http\Exception — Context-aware request exceptions
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Http\Exception {

    /**
     * No active HTTP request in this context.
     *
     * Base class for all request-context exceptions.
     * Thrown by oxphp_http_request() when called outside a request lifecycle.
     */
    class NoActiveRequestException extends \RuntimeException {}

    /**
     * Cannot access request from an oxphp_async() worker thread.
     *
     * Async workers run on separate OS threads without request context.
     */
    class AsyncContextException extends NoActiveRequestException {}

    /**
     * Worker is idle — waiting for the next request.
     *
     * Thrown when oxphp_http_request() is called in worker mode
     * but outside the request handler callback.
     */
    class WorkerIdleException extends NoActiveRequestException {}
}

// ═══════════════════════════════════════════════════════════════
//  OxPHP — Async & Decorator classes
// ═══════════════════════════════════════════════════════════════

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
