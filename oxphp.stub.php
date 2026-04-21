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
 * @version 0.3.0
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
 * @throws \OxPHP\Async\Exception If the closure threw an exception or called die()/exit()
 * @throws \OxPHP\Async\TimeoutException If the timeout expired before completion
 *
 * @example
 * $p = oxphp_async(function(): string { return 'hello'; });
 * $result = oxphp_async_await($p); // "hello"
 *
 * // With timeout:
 * try {
 *     $result = oxphp_async_await($p, 2.0);
 * } catch (\OxPHP\Async\TimeoutException $e) {
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
 * @throws \OxPHP\Async\Exception If any promise fails
 * @throws \OxPHP\Async\TimeoutException If any promise times out
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
 * @throws \OxPHP\Async\Exception If the winning promise threw an exception
 * @throws \OxPHP\Async\TimeoutException If no promise completes within timeout
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

namespace OxPHP\Async {
    /**
     * Thrown when an async task fails — the closure threw an exception,
     * or called die()/exit().
     *
     * The message contains the original exception class and message:
     * "Async task failed: [DomainException] invalid value"
     */
    class Exception extends \Exception {}

    /**
     * Thrown when oxphp_async_await() times out before the task completes.
     */
    class TimeoutException extends Exception {}

    /**
     * Thrown by every access to a BorrowedProxy — proxies substituted for
     * `use`-captured variables in oxphp_async() callbacks that would not
     * be safe to touch on the promise thread.
     */
    class BorrowException extends \Exception {}

    /**
     * Opaque stand-in for a `use`-captured value in an oxphp_async() closure
     * when the original value is still borrowed by the source thread.
     *
     * Every access — property read/write, method call, isset/unset, casts,
     * JSON serialisation — throws {@see BorrowException}. The only safe
     * thing a handler can do with a BorrowedProxy is check its class with
     * `instanceof` and fall back to data captured by value.
     *
     * @internal Produced by the runtime; never constructed from PHP.
     */
    final class BorrowedProxy implements \JsonSerializable
    {
        /** @throws BorrowException Always. */
        public function __get(string $name): mixed {}

        /** @throws BorrowException Always. */
        public function __set(string $name, mixed $value): void {}

        /** @throws BorrowException Always. */
        public function __call(string $name, array $arguments): mixed {}

        /** @throws BorrowException Always. */
        public function __isset(string $name): bool {}

        /** @throws BorrowException Always. */
        public function __unset(string $name): void {}

        /** @throws BorrowException Always. */
        public function __toString(): string {}

        /** @throws BorrowException Always. */
        public function __debugInfo(): ?array {}

        /** @throws BorrowException Always. */
        public function jsonSerialize(): mixed {}
    }
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

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Shared — Process-wide concurrent primitives
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Shared {

    /**
     * Marker interface implemented by every Shared\* type.
     *
     * Values that implement Shareable may be stored inside container
     * Shared types (Map, Channel) as nested refcount-managed references
     * without being serialised. Plain PHP objects cannot — the runtime
     * rejects them with {@see TypeException}.
     *
     * Implemented internally by Counter, Flag, Once, Mutex, Channel, Map,
     * Pool. User code cannot implement this interface directly.
     */
    interface Shareable {}

    /**
     * Atomic signed 64-bit counter, visible from every PHP worker thread.
     *
     * All operations are lock-free (`SeqCst`). Use Counter for rate
     * counters, progress trackers, sequence generators, or any shared
     * integer state that would otherwise require Redis INCR.
     *
     * @link docs/en/features/shared-counter.md
     */
    final class Counter implements Shareable
    {
        public function __construct(int $initial = 0) {}

        /** Current value. */
        public function get(): int {}

        /** Set to `$value`, returning the previous value. */
        public function set(int $value): int {}

        /** Atomically add `$by` (default 1), returning the new value. */
        public function inc(int $by = 1): int {}

        /** Atomically subtract `$by` (default 1), returning the new value. */
        public function dec(int $by = 1): int {}

        /** Atomically add `$delta` (may be negative), returning the new value. */
        public function add(int $delta): int {}

        /**
         * Compare-and-set. Returns true if the swap succeeded (current
         * value was `$expect`), false otherwise.
         */
        public function compareAndSet(int $expect, int $new): bool {}

        /** Atomic sum of `$deltas`, returning the new value. */
        public function addBatch(array $deltas): int {}

        /** Reset to `$newValue` (default 0), returning the previous value. */
        public function reset(int $newValue = 0): int {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Atomic boolean flag, visible from every PHP worker thread.
     *
     * Use Flag for feature toggles, circuit-breaker state, shutdown
     * signals — anything that fits a single yes/no switch.
     *
     * @link docs/en/features/shared-flag.md
     */
    final class Flag implements Shareable
    {
        public function __construct(bool $initial = false) {}

        /** Current value. */
        public function test(): bool {}

        /** Atomically set to true, returning the previous value. */
        public function set(): bool {}

        /** Atomically set to false, returning the previous value. */
        public function clear(): bool {}

        /**
         * Compare-and-set. Returns true if the swap succeeded.
         */
        public function compareAndSet(bool $expect, bool $new): bool {}

        /** Atomically set to `$new`, returning the previous value. */
        public function exchange(bool $new): bool {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Run-once container. The factory callback is invoked exactly once
     * across all workers; subsequent readers see the stored value.
     *
     * Use Once for lazy cross-worker initialisation (config loading,
     * expensive singletons) where every worker may race to request
     * the value on first touch.
     *
     * @link docs/en/features/shared-once.md
     */
    final class Once implements Shareable
    {
        public function __construct() {}

        /**
         * Returns the stored value.
         *
         * @throws UninitializedException If `init()`/`trySet()` has not succeeded yet.
         */
        public function get(): mixed {}

        /** Whether `init()`/`trySet()` has completed. */
        public function isInitialized(): bool {}

        /**
         * Set to `$value` iff not yet initialised. Returns true on success,
         * false if already set.
         */
        public function trySet(mixed $value): bool {}

        /**
         * Invoke `$factory` once and store its return value. If another
         * caller is already running the factory, blocks until it finishes
         * and returns the stored value.
         *
         * @throws DeadlockException If the same thread reenters `init()`.
         */
        public function init(callable $factory): mixed {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Poisoning mutex guarding a stored value.
     *
     * If the callable passed to `with()` / `tryWith()` throws, the mutex
     * becomes poisoned and further acquisitions throw
     * {@see PoisonedException} until `clearPoison()` is called — this
     * prevents callers from reading a value left in an inconsistent
     * half-updated state.
     *
     * @link docs/en/features/shared-mutex.md
     */
    final class Mutex implements Shareable
    {
        /**
         * @param mixed      $initial        Starting value.
         * @param float|null $defaultTimeout Default `with()` timeout in seconds (null = wait forever).
         */
        public function __construct(mixed $initial = null, ?float $defaultTimeout = null) {}

        /** Whether the mutex is in a poisoned state. */
        public function isPoisoned(): bool {}

        /** Clear the poisoned state so `with()` can acquire again. */
        public function clearPoison(): bool {}

        /**
         * Acquire the lock, invoke `$fn` with the stored value (mutable by
         * reference), and release. Returns `$fn`'s return value.
         *
         * @param float $timeout Seconds to wait; 0.0 = use the constructor default.
         *
         * @throws TimeoutException  On timeout.
         * @throws PoisonedException If the mutex was poisoned.
         */
        public function with(callable $fn, float $timeout = 0.0): mixed {}

        /**
         * Non-blocking variant of `with()`. Returns null without invoking
         * `$fn` if the lock is contended.
         *
         * @throws PoisonedException If the mutex was poisoned.
         */
        public function tryWith(callable $fn): mixed {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Bounded multi-producer / multi-consumer queue with fiber-aware
     * blocking send / recv.
     *
     * When the channel is full, `send()` suspends the calling fiber; when
     * empty, `recv()` suspends. Outside a fiber the calls block the worker
     * thread. Use Channel for fan-out/fan-in work pipelines across workers.
     *
     * @link docs/en/features/shared-channel.md
     */
    final class Channel implements Shareable
    {
        /** @param int $capacity Maximum queued values (>= 1). */
        public function __construct(int $capacity) {}

        /**
         * Non-blocking send. Returns false if the channel is full or closed.
         */
        public function trySend(mixed $value): bool {}

        /**
         * Blocking send. Waits up to `$timeout` seconds (0.0 = forever).
         *
         * @throws TimeoutException On timeout.
         * @throws ClosedException  If the channel was closed.
         */
        public function send(mixed $value, float $timeout = 0.0): bool {}

        /**
         * Non-blocking receive. Returns null if the channel is empty.
         * Use `pending()` to distinguish an empty channel from a `null` value.
         *
         * @throws ClosedException If the channel was closed and drained.
         */
        public function tryRecv(float $timeout = 0.0): mixed {}

        /**
         * Blocking receive. Waits up to `$timeout` seconds (0.0 = forever).
         *
         * @throws TimeoutException On timeout.
         * @throws ClosedException  If the channel was closed and drained.
         */
        public function recv(float $timeout = 0.0): mixed {}

        /** Close the channel. Subsequent sends fail; pending recvs drain remaining values then throw. */
        public function close(): bool {}

        /** Whether the channel is closed. */
        public function isClosed(): bool {}

        /** Current number of queued values. */
        public function pending(): int {}

        /**
         * Batched send. Returns the number of values actually accepted
         * before the channel became full / closed or the timeout expired.
         */
        public function sendMany(array $values, float $timeout = 0.0): int {}

        /**
         * Batched receive. Drains up to `$max` values, blocking only for
         * the first one up to `$timeout` seconds.
         *
         * @return array<int, mixed>
         */
        public function recvMany(int $max, float $timeout = 0.0): array {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Concurrent `string → mixed` key-value store, visible from every
     * worker thread.
     *
     * Nested {@see Shareable} values are stored as refcount-managed
     * references without serialisation. Cycle insertion is rejected
     * with {@see CycleException}. Per-instance cap via `maxEntries`.
     *
     * @link docs/en/features/shared-map.md
     */
    final class Map implements Shareable
    {
        /**
         * @param int|null $maxEntries Per-instance size cap, or null for unlimited
         *                             (still bounded by the process-global
         *                             `SHARED_MAX_ENTRIES` budget).
         */
        public function __construct(?int $maxEntries = null) {}

        /** Read a value; returns `$default` when absent. */
        public function get(string $key, mixed $default = null): mixed {}

        /**
         * Write a value.
         *
         * @throws TypeException     Value is not a scalar, array of scalars, or Shareable.
         * @throws CycleException    Writing this Shareable would form a reference cycle.
         * @throws CapacityException Map reached `maxEntries` or the process cap.
         */
        public function set(string $key, mixed $value): void {}

        /** Whether a key exists. */
        public function has(string $key): bool {}

        /** Remove a key, returning the previous value or null. */
        public function remove(string $key): mixed {}

        /** Remove all entries. */
        public function clear(): int {}

        /** Number of entries currently stored. */
        public function count(): int {}

        /**
         * Snapshot of all keys at the call time.
         *
         * @return string[]
         */
        public function keys(): array {}

        /** Configured per-instance cap, or null if unlimited. */
        public function maxEntries(): mixed {}

        /**
         * Set `$value` iff the key was absent. Returns true on insert,
         * false if the key already existed.
         */
        public function setIfAbsent(string $key, mixed $value): bool {}

        /**
         * Atomically update a value with `$fn(mixed $old): mixed`. Returns
         * the new value. Throws {@see TypeException} for unsupported values.
         */
        public function update(string $key, callable $fn): mixed {}

        /**
         * Return the current value, or call `$factory()` to compute and
         * store a new value if the key is absent.
         */
        public function getOrSet(string $key, callable $factory): mixed {}

        /**
         * Bulk set. Returns the number of entries written.
         *
         * @param array<string, mixed> $kv
         */
        public function setMany(array $kv): int {}

        /**
         * Bulk get.
         *
         * @param string[] $keys
         * @return array<string, mixed>
         */
        public function getMany(array $keys): array {}

        /**
         * Bulk atomic update. Applies `$fn` to each of `$keys`; missing
         * keys are skipped.
         *
         * @param string[] $keys
         * @return array<string, mixed>
         */
        public function updateMany(array $keys, callable $fn): array {}

        /**
         * Bulk remove. Returns the number of keys actually removed.
         *
         * @param string[] $keys
         */
        public function removeMany(array $keys): int {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Bounded object pool with per-thread slot affinity and idle-timeout
     * eviction.
     *
     * Factory runs lazily on first acquire; destroy (if provided) runs
     * on slot eviction or pool shutdown. `maxSize` is a hard budget —
     * acquire blocks (or fails with `TimeoutException`) once reached.
     *
     * @link docs/en/features/shared-pool.md
     */
    final class Pool implements Shareable
    {
        /**
         * @param callable      $factory               Called to create a pooled resource. Receives no arguments.
         * @param callable|null $destroy               Called with the resource on eviction/shutdown.
         * @param int           $maxSize               Hard budget of live slots (default 32).
         * @param float         $idleTimeout           Seconds of inactivity before a slot is evicted (default 300).
         * @param float|null    $defaultAcquireTimeout Default acquire timeout; null uses 5.0s.
         */
        public function __construct(
            callable $factory,
            ?callable $destroy = null,
            int $maxSize = 32,
            float $idleTimeout = 300.0,
            ?float $defaultAcquireTimeout = 5.0,
        ) {}

        /**
         * Acquire a slot. Returns a Handle scoped to the current thread.
         *
         * @param float $timeout Seconds to wait; 0.0 = use the constructor default.
         *
         * @throws TimeoutException If the pool is saturated.
         */
        public function acquire(float $timeout = 0.0): Pool\Handle {}

        /** Release a handle back to the pool. Idempotent once per handle. */
        public function release(Pool\Handle $handle): void {}

        /**
         * Scope-guarded acquire + release. Invokes `$body($resource)` with
         * the pooled value and returns whatever it returns, releasing even
         * on exception.
         *
         * @throws TimeoutException If the pool is saturated.
         */
        public function with(callable $body, float $timeout = 0.0): mixed {}

        /** Force-evict idle slots now. Returns the number of slots evicted. */
        public function evict(): int {}

        /** Total live slots (in-use + idle). */
        public function size(): int {}

        /** Slots currently checked out by callers. */
        public function inUse(): int {}

        /** Idle slots ready to acquire. */
        public function idle(): int {}

        /** Callers currently blocked in `acquire()`. */
        public function waiting(): int {}

        /** Configured hard budget. */
        public function maxSize(): int {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    // ── Exception hierarchy ─────────────────────────────────────
    //
    //   \Exception
    //     └── OxPHP\Shared\Exception
    //          ├── StaleHandleException
    //          ├── TypeException
    //          │    └── CycleException
    //          ├── CapacityException
    //          ├── ClosedException
    //          ├── PoisonedException
    //          ├── TimeoutException
    //          │    └── DeadlockException
    //          └── UninitializedException

    /** Base class for every Shared\* exception. */
    class Exception extends \Exception {}

    /** Operation used a handle whose underlying entry was dropped. */
    class StaleHandleException extends Exception {}

    /** Value is not storable in the target Shared\* container. */
    class TypeException extends Exception {}

    /** Map::set would form a cycle via nested Shareable references. */
    class CycleException extends TypeException {}

    /** Container is full and cannot accept a new entry. */
    class CapacityException extends Exception {}

    /** Channel was closed and cannot send / drained on recv. */
    class ClosedException extends Exception {}

    /** Mutex was poisoned by a previous callback throwing mid-update. */
    class PoisonedException extends Exception {}

    /** Operation exceeded its wait budget. */
    class TimeoutException extends Exception {}

    /** Reentrant / cross-thread wait would deadlock — extends TimeoutException. */
    class DeadlockException extends TimeoutException {}

    /** Access to a Once before it was initialised. */
    class UninitializedException extends Exception {}
}

namespace OxPHP\Shared\Pool {

    /**
     * Scope-bound reference to an acquired Pool slot.
     *
     * Clone is forbidden. `get()` returns a copy of the underlying resource
     * that is safe to use within the acquiring thread; on destruction or
     * explicit release via {@see \OxPHP\Shared\Pool::release()} the slot
     * returns to the pool.
     *
     * @internal Produced by {@see \OxPHP\Shared\Pool::acquire()}; never constructed directly.
     */
    final class Handle
    {
        /**
         * The pooled resource. Always call inside the acquiring thread.
         *
         * @throws \OxPHP\Shared\Exception If the handle has already been released.
         */
        public function get(): mixed {}
    }
}

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Profile — Per-request PHP profiler SDK
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Profile {

    /**
     * Whether profiling is actively capturing spans for this request.
     *
     * Returns true only when the profiler is enabled *and* a profile is
     * active (triggered by cookie / header / query / sample rate) and not
     * paused. Cheap: two thread-local reads, no FFI hop.
     *
     * Use to gate expensive profile-only instrumentation (custom metrics,
     * debug-only attributes).
     *
     * @example
     * if (\OxPHP\Profile\is_active()) {
     *     \OxPHP\Profile\metric('cache.hit_ratio', $hits / $total);
     * }
     */
    function is_active(): bool {}

    /**
     * Enable profiling for the rest of the current request.
     *
     * Sets the mode to "profile all" and clears any paused state. If a
     * profile was already running in a lower-detail mode, the span buffer
     * is reset (mid-request upgrade invariant). Call at most once per
     * request — the trigger at RINIT is the preferred entry point.
     */
    function start(): void {}

    /**
     * Stop further span capture. Already-open spans close naturally as
     * PHP returns through them so `open_stack` stays balanced.
     */
    function stop(): void {}

    /**
     * Soft variant of {@see stop()} — identical semantics but documentary
     * intent is "temporarily pause, will resume()". Pair with
     * {@see resume()} around hot sections you don't care to profile.
     */
    function pause(): void {}

    /** Clear the paused flag. Inverse of {@see pause()}. */
    function resume(): void {}

    /**
     * Attach a named marker event to the topmost open span.
     *
     * No-op when no span is open. Visible in tracing UIs as a point event
     * on the span timeline — use for significant moments (cache miss,
     * retry attempt, validation step).
     *
     * @param string              $label Marker name ("cache.miss", "retry.attempt")
     * @param array<string,mixed> $attrs Optional key/value attributes (string-coerced)
     */
    function mark(string $label, array $attrs = []): void {}

    /**
     * Append `metric.<name> = <value>` to the current span's attributes.
     *
     * No-op when no span is open. Use for span-scoped measurements:
     * payload size, cache hit ratio, queue depth at entry.
     */
    function metric(string $name, float $value): void {}
}

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Profile — Profiler attributes
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Profile {

    /**
     * Explicitly include a function / method / class in profile capture.
     *
     * Interacts with {@see Exclude} and the profile filter: when both are
     * reachable, the most-specific attribute wins (method > class > file).
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD | \Attribute::TARGET_CLASS)]
    final class Profile
    {
        public function __construct() {}
    }

    /**
     * Exclude a function / method / class from profile capture.
     *
     * Useful for hot-path helpers that would otherwise dominate the flame
     * graph without adding signal.
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD | \Attribute::TARGET_CLASS)]
    final class Exclude
    {
        public function __construct() {}
    }

    /**
     * Capture this function / method only at the given sampling rate.
     *
     * @param float $rate Value in [0.0, 1.0]; e.g. 0.1 = 10 % of calls.
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class Sample
    {
        public function __construct(public readonly float $rate) {}
    }

    /**
     * Attach a static key/value tag to every span produced by the
     * decorated target. Repeatable — multiple `#[Tag]` accumulate.
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD | \Attribute::TARGET_CLASS | \Attribute::IS_REPEATABLE)]
    final class Tag
    {
        public function __construct(
            public readonly string $key,
            public readonly string $value,
        ) {}
    }

    /**
     * Add a marker event at the entry of the decorated target.
     *
     * @param string|null $label Mark label (null ⇒ derive from target name).
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class Mark
    {
        public function __construct(public readonly ?string $label = null) {}
    }

    /**
     * Emit a `slow.call` marker if the decorated call exceeds `$ms`
     * wall-clock milliseconds.
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class SlowThreshold
    {
        public function __construct(public readonly int $ms) {}
    }

    /**
     * Emit a `memory.spike` marker if the decorated call grows PHP's
     * memory usage by more than `$kb` kilobytes.
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class MemoryThreshold
    {
        public function __construct(public readonly int $kb) {}
    }
}

// ═══════════════════════════════════════════════════════════════
//  OxPHP APM — Functions & Attribute
// ═══════════════════════════════════════════════════════════════

/**
 * Execute a callback inside a child span. The span is automatically
 * closed when the callback returns or throws. On exception, the span
 * is marked as error with an exception event, then the exception
 * is re-thrown.
 *
 * When APM is disabled (OTEL_APM_ENABLED != true): calls $callback
 * directly with no overhead. $span_id passed to callback is 0.
 *
 * @param string   $name       Span name ("db.query", "payment.charge")
 * @param callable $callback   Receives (int $span_id) as argument
 * @param array    $attributes Initial span attributes ['key' => 'value']
 * @return mixed   Return value of $callback
 *
 * @example
 * $user = oxphp_apm_trace('user.fetch', function(int $span) use ($id) {
 *     oxphp_apm_attribute('user.id', $id);
 *     return User::find($id);
 * });
 */
function oxphp_apm_trace(string $name, callable $callback, array $attributes = []): mixed {}

/**
 * Start a new child span manually. Returns a span ID that MUST be
 * passed to oxphp_apm_end(). For most cases, prefer oxphp_apm_trace()
 * which handles closing automatically.
 *
 * When APM is disabled: returns 0. All functions accept 0 as no-op.
 *
 * @param string $name       Span name
 * @param array  $attributes Initial span attributes
 * @return int   Span ID (local to this request, not the OTel span ID)
 *
 * @example
 * $span = oxphp_apm_start('payment.charge', ['provider' => 'stripe']);
 * try {
 *     $result = $stripe->charge($amount);
 * } finally {
 *     oxphp_apm_end($span);
 * }
 */
function oxphp_apm_start(string $name, array $attributes = []): int {}

/**
 * Close a span opened by oxphp_apm_start().
 *
 * Repeat calls with the same ID are no-op. Unclosed spans are
 * force-closed at request end with an E_NOTICE and oxphp.span.leaked
 * attribute.
 *
 * @param int $span_id Span ID from oxphp_apm_start()
 */
function oxphp_apm_end(int $span_id): void {}

/**
 * Add an attribute to a span.
 *
 * Without $span_id: applies to the current (innermost) active span.
 * With $span_id: applies to that specific span.
 *
 * Supported value types: string, int, float, bool, string[].
 * No active span = silent no-op.
 *
 * @param string   $key      Attribute name (e.g. "db.system", "payment.amount")
 * @param mixed    $value    Attribute value
 * @param int|null $span_id  Target span, or null for current span
 *
 * @example
 * oxphp_apm_attribute('order.total', 99.99);
 * oxphp_apm_attribute('cache.hit', true, $outer_span);
 */
function oxphp_apm_attribute(string $key, mixed $value, ?int $span_id = null): void {}

/**
 * Add a named event (timestamp marker) to a span.
 *
 * Events are visible in tracing UIs as markers on the span timeline.
 * Use for noteworthy moments: cache misses, retries, validations.
 *
 * @param string   $name       Event name ("cache.miss", "retry.attempt")
 * @param array    $attributes Event attributes ['attempt' => 3]
 * @param int|null $span_id    Target span, or null for current span
 *
 * @example
 * oxphp_apm_event('cache.miss', ['key' => 'user:session:abc']);
 */
function oxphp_apm_event(string $name, array $attributes = [], ?int $span_id = null): void {}

/**
 * Record an exception on a span. Adds an "exception" event with
 * exception.type, exception.message, exception.stacktrace attributes
 * and sets span status to Error.
 *
 * The span is NOT closed -- use in catch blocks to record the error
 * while continuing execution. Inside oxphp_apm_trace() callbacks,
 * exceptions are recorded automatically.
 *
 * @param \Throwable $exception The caught exception
 * @param int|null   $span_id   Target span, or null for current span
 *
 * @example
 * try {
 *     riskyOperation();
 * } catch (\Throwable $e) {
 *     oxphp_apm_error($e);
 * }
 */
function oxphp_apm_error(\Throwable $exception, ?int $span_id = null): void {}

/**
 * Set span status explicitly.
 *
 * OXPHP_APM_OK (0): operation succeeded.
 * OXPHP_APM_ERROR (1): operation failed.
 *
 * Default status is "Unset" -- OTel infers from context.
 * Use Error without oxphp_apm_error() for business logic failures
 * that aren't exceptions.
 *
 * @param int         $code        OXPHP_APM_OK or OXPHP_APM_ERROR
 * @param string|null $description Optional status description
 * @param int|null    $span_id     Target span, or null for current span
 */
function oxphp_apm_status(int $code, ?string $description = null, ?int $span_id = null): void {}

/**
 * Get the current trace ID (32 hex chars).
 *
 * Same value as $_SERVER['OXPHP_TRACE_ID'], but convenient in
 * tracing context without accessing superglobals.
 *
 * @return string Trace ID, or "" if tracing is disabled
 */
function oxphp_apm_trace_id(): string {}

/**
 * Get the span ID of the current active span (16 hex chars).
 *
 * Use for building traceparent headers in manual HTTP calls.
 *
 * @return string Span ID, or "" if no active span
 */
function oxphp_apm_span_id(): string {}

/**
 * Build a W3C traceparent header value for the current trace context.
 *
 * Format: "00-{trace_id}-{span_id}-01"
 * Uses the current active span's ID for propagation.
 *
 * @return string Formatted traceparent, or "" if tracing is disabled
 *
 * @example
 * $response = file_get_contents('https://api.example.com', false,
 *     stream_context_create(['http' => [
 *         'header' => 'traceparent: ' . oxphp_apm_header(),
 *     ]])
 * );
 */
function oxphp_apm_header(): string {}

/** Span status: operation succeeded. */
define('OXPHP_APM_OK', 0);

/** Span status: operation failed. */
define('OXPHP_APM_ERROR', 1);

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Apm — Trace Attribute
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Apm {

    /**
     * Attribute-based decorator for automatic span creation.
     *
     * Wraps the decorated function/method in a span. On exception,
     * the span is marked as error with an exception event, then the
     * exception is re-thrown.
     *
     * When APM is disabled: attribute is ignored (no consumer = no-op
     * per PHP specification).
     *
     * Auto-registered when OTEL_APM_ENABLED=true. No manual
     * oxphp_register_decorator() call needed.
     *
     * @example
     * class PaymentService {
     *     #[Trace('payment.charge')]
     *     public function charge(int $amount): Receipt {
     *         return $this->gateway->process($amount);
     *     }
     *
     *     #[Trace]  // span name: "PaymentService::validate"
     *     public function validate(Order $order): bool {
     *         return $order->total > 0;
     *     }
     * }
     */
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class Trace implements \OxPHP\Decorator\AttributeInterface
    {
        /**
         * @param string|null $name Span name. null = "{Class}::{method}" or "{function}"
         */
        public function __construct(
            public readonly ?string $name = null,
        ) {}

        public function before(\OxPHP\Decorator\Context $ctx): void {}
        public function after(\OxPHP\Decorator\Context $ctx): void {}
    }
}
