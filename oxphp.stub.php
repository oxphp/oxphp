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
 * Migration: oxphp_request_heartbeat($time) was removed.
 * Replace each call with set_time_limit($seconds). Both reset the
 * per-request execution timer to N seconds from now.
 *
 * @package OxPHP
 * @version 0.7.0
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
 * @return array{version: string, worker_id: int, request_time: float, worker_mode: bool}
 *
 * @example
 * $info = oxphp_server_info();
 * // ["version" => "0.1.0", "worker_id" => 3, "request_time" => 1740000000.123, "worker_mode" => false]
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
 *
 * @see \OxPHP\Server\Worker::isWorkerMode() Object-oriented equivalent; both
 *      return the same value (delegate to the same underlying check).
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
 *
 * @see \OxPHP\Server\Worker::serve() Object-oriented equivalent. Both share
 *      the same dispatch loop and per-thread re-entry guard, but differ at
 *      the boundary: oxphp_worker() emits E_WARNING and returns false when
 *      called outside worker mode, whereas Worker::serve() throws
 *      InvalidServeContextException and returns void.
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
 * @throws \OxPHP\Async\AsyncException If the closure threw an exception or called die()/exit()
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
 * @throws \OxPHP\Async\AsyncException If any promise fails
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
 * @throws \OxPHP\Async\AsyncException If the winning promise threw an exception
 * @throws \OxPHP\Async\TimeoutException If no promise completes within timeout
 *
 * @example
 * $p1 = oxphp_async(fn() => slow_api_a());
 * $p2 = oxphp_async(fn() => slow_api_b());
 * $winner = oxphp_async_await_race([$p1, $p2]);
 * // ['id' => $p2, 'value' => ...] (whichever finished first)
 * $other = oxphp_async_await($p1); // non-winner still awaitable
 */
function oxphp_async_await_race(array $promise_ids, ?float $timeout = null): array {}

/**
 * Wait for the first FULFILLED promise (analog of JavaScript Promise.any).
 *
 * Unlike oxphp_async_await_race(), rejections do NOT win — they accumulate.
 * If at least one promise fulfills before the deadline, returns its id+value
 * and leaves remaining pending promises individually awaitable. If every
 * promise rejects, throws AggregateAsyncException carrying all errors.
 *
 * On timeout, throws TimeoutException populated with partial errors collected
 * up to the deadline and the ids of promises still pending. Pending promises
 * are cancelled AND their receivers are dropped — those ids cannot be re-awaited
 * with oxphp_async_await*() afterwards (each such call throws
 * "unknown or already-awaited promise id"). The pending list is an audit
 * trail, not a queue of resumable work.
 *
 * @param int[]      $promise_ids Array of promise IDs from oxphp_async()
 * @param float|null $timeout     Overall timeout in seconds; null = no limit
 * @return array{id: int, value: mixed} The first-fulfilled promise's id and result
 *
 * @throws \OxPHP\Async\AggregateAsyncException If every promise rejects
 * @throws \OxPHP\Async\TimeoutException        If the deadline elapses before any fulfills
 *
 * @example
 * $a = oxphp_async(fn() => fetch_mirror_a());
 * $b = oxphp_async(fn() => fetch_mirror_b());
 * try {
 *     $winner = oxphp_async_await_any([$a, $b], 2.0);
 *     // ['id' => $a or $b, 'value' => ...]
 * } catch (\OxPHP\Async\AggregateAsyncException $e) {
 *     foreach ($e->getErrors() as $i => $err) {
 *         // by input position
 *     }
 * } catch (\OxPHP\Async\TimeoutException $e) {
 *     foreach ($e->getPartialErrors() as $promise_id => $err) {
 *         // who failed
 *     }
 *     $cancelled = $e->getCancelledPromiseIds();
 *     // Audit-only: these ids are no longer awaitable.
 * }
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
    class AsyncException extends \Exception {}

    /**
     * Thrown by async-await-style functions when a deadline expires.
     *
     * For oxphp_async_await_any(), additional context is exposed via
     * getPartialErrors() and getCancelledPromiseIds(). For other call sites
     * (oxphp_async_await, oxphp_async_await_all, oxphp_async_await_race)
     * both methods return empty arrays.
     */
    class TimeoutException extends AsyncException
    {
        /**
         * Errors collected from promises that rejected before the deadline,
         * keyed by promise_id. Only populated by oxphp_async_await_any().
         *
         * @return array<int, \Throwable>
         */
        public function getPartialErrors(): array {}

        /**
         * Promise IDs that did not settle before the deadline. The cancel
         * flag has been set on each, AND their receivers were dropped —
         * passing any of these ids to oxphp_async_await() /
         * oxphp_async_await_race() / oxphp_async_await_any() /
         * oxphp_async_await_all() afterwards throws OxPHP\Async\AsyncException
         * ("unknown or already-awaited promise id"). Treat the list as a
         * fire-and-forget audit trail, not a queue of resumable work. Only
         * populated by oxphp_async_await_any().
         *
         * @return list<int>
         */
        public function getCancelledPromiseIds(): array {}
    }

    /**
     * Thrown by oxphp_async_await_any() when every provided promise rejected
     * before any could fulfill (analog of JavaScript AggregateError).
     */
    class AggregateAsyncException extends AsyncException
    {
        /**
         * Errors in the order of the input promise_ids array.
         *
         * @return list<\Throwable>
         */
        public function getErrors(): array {}

        /**
         * Errors keyed by promise_id (input order preserved as values).
         *
         * @return array<int, \Throwable>
         */
        public function getErrorMap(): array {}

        /**
         * Promise IDs in the order of the input array.
         *
         * @return list<int>
         */
        public function getPromiseIds(): array {}
    }

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

    /**
     * Thrown when nested decorated calls exceed the decorator context
     * stack depth (256 levels). Fails loud instead of silently corrupting
     * decorator context for the overflowing frame.
     */
    class StackOverflowException extends \Exception {}
}

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Shared — Process-wide concurrent primitives
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Shared {

    /**
     * # Timeout convention (Shared\*)
     *
     * Every bounded-wait method in `OxPHP\Shared\` follows the same
     * trichotomy — there is no `?float $timeout`:
     *
     *  * a bare method ({@see Pool::acquire}, {@see Mutex::withLock},
     *    {@see Channel::recv}) waits **forever**;
     *  * a `try*` method ({@see Pool::tryAcquire}, {@see Channel::tryRecv})
     *    is **non-blocking**;
     *  * a `*Timeout(int $ms)` method ({@see Pool::acquireTimeout},
     *    {@see Mutex::withLockTimeout}, {@see Channel::recvTimeout}) waits a
     *    **bounded** number of milliseconds. `$ms` must be `> 0`, otherwise
     *    {@see TypeException}.
     *
     * Where an outcome is *expected*, it is returned as an object rather than
     * thrown: Channel uses {@see Channel\RecvResult} / {@see Channel\SendResult};
     * {@see Pool::tryAcquire} returns `null` on a saturated pool. A bounded
     * wait that expires raises {@see OperationTimeoutException}, which extends
     * {@see \OxPHP\Async\AsyncException} — NOT {@see SharedException}.
     */

    /**
     * Marker interface implemented by every Shared\* type.
     *
     * Values that implement Shareable may be stored inside container
     * Shared types (Map, Channel) as nested refcount-managed references
     * without being serialised. Plain PHP objects cannot — the runtime
     * rejects them with {@see TypeException}.
     *
     * Implemented internally by Atomic, Counter, Flag, Once, Mutex, Channel,
     * Map, Pool. User code cannot implement this interface directly.
     */
    interface Shareable {}

    /**
     * Memory-ordering constraint for {@see Atomic} operations, mirroring the
     * C++/Rust `memory_order` model.
     *
     * Stronger orderings cost more on weakly-ordered CPUs (ARM, POWER) but
     * are nearly free on x86. The default for every Atomic method is
     * `SeqCst` — correct everywhere, only sometimes slower than necessary.
     *
     * Not every method accepts every case:
     *
     *  * {@see Atomic::load} — `Relaxed`, `Acquire`, `SeqCst`
     *  * {@see Atomic::store} — `Relaxed`, `Release`, `SeqCst`
     *  * {@see Atomic::compareAndSet} `$failure` — `Relaxed`, `Acquire`, `SeqCst`
     *  * read-modify-write ({@see Atomic::swap}, the `fetch*` family, and
     *    {@see Atomic::compareAndSet} `$success`) — any case
     *
     * Passing a disallowed case throws {@see InvalidOrderingException}.
     */
    enum Ordering: int
    {
        /** No ordering constraint beyond the operation's own atomicity. */
        case Relaxed = 0;
        /** Later reads/writes cannot be reordered before this load. */
        case Acquire = 1;
        /** Earlier reads/writes cannot be reordered after this store. */
        case Release = 2;
        /** Acquire (on the load half) + Release (on the store half) for RMW ops. */
        case AcqRel = 3;
        /** Sequential consistency: a single total order seen by all threads. */
        case SeqCst = 4;
    }

    /**
     * Atomic signed 64-bit counter, visible from every PHP worker thread.
     *
     * All operations are lock-free and `Relaxed`: each is atomic (no lost
     * ticks, no torn reads) but establishes no happens-before with other
     * memory. Use Counter for rate counters, progress trackers, sequence
     * generators, or any shared integer state that would otherwise require
     * Redis INCR. To synchronise other memory through the integer, use
     * {@see Atomic} (with an explicit {@see Ordering}).
     *
     * @link docs/en/shared-state/shared-counter.md
     */
    final class Counter implements Shareable
    {
        public function __construct(int $initial = 0) {}

        /** Current value (atomic load). */
        public function get(): int {}

        /** Set to `$value`, returning the previous value (atomic exchange; set(0) resets a window). */
        public function set(int $value): int {}

        /** Atomically add `$delta` (default 1; may be negative), returning the new value. */
        public function add(int $delta = 1): int {}

        /**
         * Compare-and-set. Returns true if the current value was `$expect`
         * and was replaced with `$new`, false otherwise. Relaxed ordering;
         * for ordered CAS use {@see Atomic}.
         */
        public function compareAndSet(int $expect, int $new): bool {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Generic atomic signed 64-bit integer with explicit memory-ordering
     * control, visible from every PHP worker thread.
     *
     * Where {@see Counter} is a fixed-`Relaxed` accumulator surface, Atomic
     * exposes the full lock-free toolkit — load/store/swap, compare-and-set,
     * and fetch arithmetic/bitwise — each taking an {@see Ordering} so
     * latency-sensitive code can relax barriers it does not need. Use it for
     * lock-free state machines, sequence numbers, bitmask flags, and
     * seqlock-style protocols.
     *
     * Every method defaults to `Ordering::SeqCst`; pass a weaker ordering
     * only when you can prove it correct. A disallowed ordering for a given
     * method throws {@see InvalidOrderingException}.
     *
     * @link docs/en/shared-state/shared-atomic.md
     */
    final class Atomic implements Shareable
    {
        public function __construct(int $initial = 0) {}

        /**
         * Atomically read the current value.
         *
         * @param Ordering $order One of Relaxed, Acquire, SeqCst.
         */
        public function load(Ordering $order = Ordering::SeqCst): int {}

        /**
         * Atomically store `$value`.
         *
         * @param Ordering $order One of Relaxed, Release, SeqCst.
         */
        public function store(int $value, Ordering $order = Ordering::SeqCst): void {}

        /** Atomically store `$value`, returning the previous value. */
        public function swap(int $value, Ordering $order = Ordering::SeqCst): int {}

        /**
         * Compare-and-set. If the current value equals `$expect`, replace it
         * with `$new` and return true; otherwise leave it unchanged and
         * return false.
         *
         * @param Ordering $success Ordering applied on a successful swap (any case).
         * @param Ordering $failure Ordering applied on failure — one of
         *                          Relaxed, Acquire, SeqCst.
         */
        public function compareAndSet(
            int $expect,
            int $new,
            Ordering $success = Ordering::SeqCst,
            Ordering $failure = Ordering::SeqCst,
        ): bool {}

        /** Atomically add `$delta` (may be negative), returning the previous value. */
        public function fetchAdd(int $delta, Ordering $order = Ordering::SeqCst): int {}

        /** Atomically subtract `$delta`, returning the previous value. */
        public function fetchSub(int $delta, Ordering $order = Ordering::SeqCst): int {}

        /** Atomically bitwise-AND with `$mask`, returning the previous value. */
        public function fetchAnd(int $mask, Ordering $order = Ordering::SeqCst): int {}

        /** Atomically bitwise-OR with `$mask`, returning the previous value. */
        public function fetchOr(int $mask, Ordering $order = Ordering::SeqCst): int {}

        /** Atomically bitwise-XOR with `$mask`, returning the previous value. */
        public function fetchXor(int $mask, Ordering $order = Ordering::SeqCst): int {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Atomic boolean flag, visible from every PHP worker thread.
     *
     * The bool twin of {@see Atomic}: lock-free load/store/swap/compareAndSet
     * with explicit memory ordering. Use Flag for feature toggles,
     * circuit-breaker state, shutdown signals, one-shot init markers — any
     * single cross-worker yes/no switch.
     *
     * @link docs/en/shared-state/shared-flag.md
     */
    final class Flag implements Shareable
    {
        public function __construct(bool $initial = false) {}

        /**
         * Atomically read the current value.
         *
         * @param Ordering $order One of Relaxed, Acquire, SeqCst.
         */
        public function load(Ordering $order = Ordering::SeqCst): bool {}

        /**
         * Atomically store `$value`.
         *
         * @param Ordering $order One of Relaxed, Release, SeqCst.
         */
        public function store(bool $value, Ordering $order = Ordering::SeqCst): void {}

        /** Atomically store `$value`, returning the previous value. */
        public function swap(bool $value, Ordering $order = Ordering::SeqCst): bool {}

        /**
         * Compare-and-set. If the current value equals `$expect`, replace it
         * with `$new` and return true; otherwise leave it unchanged and
         * return false.
         *
         * @param Ordering $success Ordering applied on a successful swap (any case).
         * @param Ordering $failure Ordering applied on failure — one of
         *                          Relaxed, Acquire, SeqCst.
         */
        public function compareAndSet(
            bool $expect,
            bool $new,
            Ordering $success = Ordering::SeqCst,
            Ordering $failure = Ordering::SeqCst,
        ): bool {}

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
     * @link docs/en/shared-state/shared-once.md
     */
    /**
     * @template T
     */
    final class Once implements Shareable
    {
        /**
         * @param Once\FailureMode $onFactoryError What happens to OTHER callers
         *        if a `getOrInit()` factory throws. Default `Reset` (retryable);
         *        `Poison` makes the cell terminally unusable.
         */
        public function __construct(
            Once\FailureMode $onFactoryError = Once\FailureMode::Reset
        ) {}

        /**
         * Returns the stored value.
         *
         * @return T
         * @throws UninitializedException If the cell is empty or a factory is
         *         currently running (Pending) with no value published yet.
         * @throws PoisonedException If a factory previously failed in `Poison` mode.
         */
        public function get(): mixed {}

        /**
         * State of the cell, for introspection / diagnostics. Never throws —
         * including on a poisoned cell (this is the one safe observer of poison).
         */
        public function status(): Once\Status {}

        /**
         * Push model: atomically store `$value` iff the cell is empty.
         *
         * For values WITHOUT side-effecting acquisition only. Initialise
         * resources (handles, sockets) through `getOrInit()` — a `trySet()`
         * that loses the race merely drops a value to GC, but a resource
         * acquired before a lost race would orphan.
         *
         * @param T $value
         * @return bool true if this call stored the value; false if the cell
         *         was already Ready or Pending. A false on Pending does NOT
         *         guarantee a later get() succeeds — a Reset-mode factory can
         *         still fail and clear the cell. If you need the value back,
         *         use getOrInit().
         * @throws PoisonedException If the cell is poisoned.
         */
        public function trySet(mixed $value): bool {}

        /**
         * Pull model: return the value, or compute it exactly once. Parallel
         * callers block on the winner and receive its value on success. If the
         * winner's factory throws in Reset mode the next blocked caller becomes
         * the initialiser and retries; in Poison mode the cell goes terminal.
         * The factory's own exception is always re-thrown to the current caller.
         *
         * @param callable(): T $factory
         * @return T
         * @throws DeadlockException If the same thread reenters `getOrInit()`.
         * @throws PoisonedException If the cell is already poisoned.
         */
        public function getOrInit(callable $factory): mixed {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Cross-thread mutual exclusion guarding a stored value.
     *
     * Three method variants encode the wait policy explicitly:
     *
     *   - {@see withLock()}        — block until acquired (forever, or
     *     fiber cancellation).
     *   - {@see tryWithLock()}     — non-blocking; throws
     *     {@see ContentionException} if the lock is held.
     *   - {@see withLockTimeout()} — bounded wait; throws
     *     {@see OperationTimeoutException} on deadline.
     *
     * If the callable throws an ordinary PHP exception, the lock is
     * released and the exception propagates — the mutex is NOT corrupted
     * (partial mutation is acceptable; caller is responsible for invariant
     * restoration).
     *
     * If a Rust panic crosses the FFI boundary (a server bug), the mutex
     * enters a sticky corrupted state and every subsequent acquisition
     * throws {@see CorruptedMutexException}. There is no API to clear
     * corruption — discard the instance and create a new one.
     *
     * @link docs/en/shared-state/shared-mutex.md
     */
    final class Mutex implements Shareable
    {
        /** @param mixed $initial Starting value. */
        public function __construct(mixed $initial = null) {}

        /**
         * Acquire the lock (waiting forever, or until the request fiber
         * is cancelled), invoke `$fn` with the stored value (mutable by
         * reference), and release. Returns `$fn`'s return value.
         *
         * @throws CorruptedMutexException If a prior closure invocation
         *         crashed via Rust panic and left the mutex unusable.
         * @throws DeadlockException If a wait-for cycle is detected.
         */
        public function withLock(callable $fn): mixed {}

        /**
         * Non-blocking variant of {@see withLock()}.
         *
         * @throws ContentionException If the lock is currently held.
         * @throws CorruptedMutexException If the mutex is unusable.
         */
        public function tryWithLock(callable $fn): mixed {}

        /**
         * Bounded-wait variant of {@see withLock()}.
         *
         * @param int $ms Wait budget in milliseconds. Must be `> 0` — use
         *                {@see withLock()} for forever or
         *                {@see tryWithLock()} for non-blocking.
         *
         * @throws OperationTimeoutException If the deadline expires.
         * @throws CorruptedMutexException If the mutex is unusable.
         * @throws DeadlockException If a wait-for cycle is detected.
         * @throws TypeException If `$ms` is not a positive int.
         */
        public function withLockTimeout(callable $fn, int $ms): mixed {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Bounded multi-producer / multi-consumer queue with fiber-aware
     * blocking send / recv.
     *
     * Two return-style conventions live side-by-side:
     *
     *   - **Channel produces results** via {@see Channel\RecvResult} /
     *     {@see Channel\SendResult}. Closed / full / timeout are NORMAL
     *     outcomes for channels (fan-out dispatchers see them daily),
     *     so they appear as result variants — no exceptions in the hot path.
     *
     *   - **Mutex throws** on contention / timeout. The asymmetry is
     *     deliberate: long-held contention is a design smell for mutexes,
     *     but routine for channels.
     *
     * Three wait policies per direction:
     *
     *   - `try*`        non-blocking
     *   - (bare)        block forever (or until the request fiber is cancelled)
     *   - `*Timeout`    bounded wait, `int $ms > 0`
     *
     * @link docs/en/shared-state/shared-channel.md
     */
    final class Channel implements Shareable, \Countable
    {
        /**
         * @param int $capacity Maximum queued values (>= 1).
         * @throws TypeException If `$capacity` is not a positive int.
         * @throws CapacityException If the requested capacity's slot array
         *         would exceed `SHARED_MAX_CHANNEL_BYTES` (default 64 MiB).
         */
        public function __construct(int $capacity) {}

        // ── Receive ────────────────────────────────────────────────

        /** Non-blocking receive. Returns Ok / Empty / Closed. */
        public function tryRecv(): Channel\RecvResult {}

        /** Block until a value arrives or the channel closes. */
        public function recv(): Channel\RecvResult {}

        /**
         * Bounded receive.
         *
         * @param int $ms Wait budget in milliseconds. Must be `> 0`.
         * @throws TypeException If `$ms` is not a positive int.
         */
        public function recvTimeout(int $ms): Channel\RecvResult {}

        // ── Send ───────────────────────────────────────────────────

        /**
         * Non-blocking send. Returns Ok / Full / Closed.
         *
         * @throws ValueTooLargeException If `$value`'s serialised size exceeds
         *         the per-value cap (`SHARED_MAX_VALUE_SIZE`, default 1 MiB).
         */
        public function trySend(mixed $value): Channel\SendResult {}

        /**
         * Block until a slot frees or the channel closes.
         *
         * @throws ValueTooLargeException If `$value`'s serialised size exceeds
         *         the per-value cap (`SHARED_MAX_VALUE_SIZE`, default 1 MiB).
         */
        public function send(mixed $value): Channel\SendResult {}

        /**
         * Bounded send.
         *
         * @param int $ms Wait budget in milliseconds. Must be `> 0`.
         * @throws TypeException If `$ms` is not a positive int.
         * @throws ValueTooLargeException If `$value` exceeds the per-value cap.
         */
        public function sendTimeout(mixed $value, int $ms): Channel\SendResult {}

        // ── Batch ──────────────────────────────────────────────────

        /**
         * Send up to N values within a deadline. Returns the count
         * actually accepted. Never throws on full / closed / timeout —
         * use the return value to detect partial progress.
         *
         * @param int $ms Wait budget in milliseconds. Must be `> 0`.
         * @throws TypeException If `$ms` is not a positive int.
         * @throws ValueTooLargeException If any element exceeds the per-value
         *         cap; the whole batch is rejected (0 sent).
         */
        public function sendMany(array $values, int $ms): int {}

        /**
         * Receive up to `$max` values within a deadline. Returns a
         * partial array (possibly empty) on timeout or close.
         *
         * @param int $ms Wait budget in milliseconds. Must be `> 0`.
         * @return array<int, mixed>
         * @throws TypeException If `$ms` is not a positive int.
         */
        public function recvMany(int $max, int $ms): array {}

        // ── Lifecycle ──────────────────────────────────────────────

        /** Close the channel. Subsequent sends produce SendResult::Closed. */
        public function close(): bool {}

        /** Whether the channel is closed. */
        public function isClosed(): bool {}

        /** Current number of queued values. Implements `Countable`. */
        public function count(): int {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Concurrent `int|string → mixed` key-value store, visible from every
     * worker thread.
     *
     * `null` is never a stored value — it is the absence sentinel across
     * the whole API: a `null` return means "no such key", and writing a
     * `null` value throws {@see TypeException}. Keys are `int` or `string`,
     * kept distinct (`123` and `"123"` are different keys; no PHP key
     * coercion); string keys are binary-safe (stored as opaque bytes, so
     * non-UTF-8 keys round-trip faithfully). Nested {@see Shareable} values
     * are stored as
     * refcount-managed references without serialisation; cycle insertion is
     * rejected with {@see CycleException}. `maxEntries` is an approximate
     * per-instance ceiling.
     *
     * @link docs/en/shared-state/shared-map.md
     */
    final class Map implements Shareable
    {
        /**
         * @param int|null $maxEntries Approximate per-instance ceiling for
         *        OOM-safety, or null for unbounded (still bounded by the
         *        process-global `SHARED_MAX_ENTRIES` budget). Must be > 0;
         *        0 or negative throws {@see TypeException}.
         */
        public function __construct(?int $maxEntries = null) {}

        /** Value for `$key`, or `null` if absent. */
        public function get(int|string $key): mixed {}

        /**
         * Lazily stream `key => value` for `$keys`; absent keys are skipped.
         * Returns a native lazy iterator — one value is materialised at a
         * time and `break` stops further work.
         *
         * @param iterable<int|string> $keys
         * @return \Iterator<int|string, mixed>
         */
        public function getMany(iterable $keys): \Iterator {}

        /** Approximate entry count (striped, weakly consistent). */
        public function count(): int {}

        /** Configured per-instance ceiling, or null if unbounded. */
        public function maxEntries(): ?int {}

        /**
         * Insert or overwrite.
         *
         * @throws TypeException          `$value` is null, or not a scalar /
         *                                array of supported / Shareable.
         * @throws ValueTooLargeException Serialised value exceeds the per-value
         *                                cap (`SHARED_MAX_VALUE_SIZE`).
         * @throws CycleException         Storing this Shareable would form a cycle.
         * @throws CapacityException      New key while at `maxEntries`.
         */
        public function set(int|string $key, mixed $value): void {}

        /**
         * Insert iff absent. Returns the existing value, or `null` if the
         * insert happened (`null` ⟺ inserted). Atomic.
         *
         * @throws TypeException|ValueTooLargeException|CycleException|CapacityException
         */
        public function setIfAbsent(int|string $key, mixed $value): mixed {}

        /**
         * Bulk insert/overwrite. Per-key atomic, NOT batch-atomic: keys are
         * applied one at a time, so a mid-batch failure leaves earlier keys
         * stored (partial apply). The successfully-written count is not
         * recoverable once an exception is thrown — re-read with `getMany`
         * if you need to know what landed.
         *
         * @param iterable<int|string, mixed> $entries
         * @return int Number of entries written (only on full success).
         * @throws TypeException          A null/non-storable value or non-int|string key.
         * @throws ValueTooLargeException A value over the per-value cap.
         * @throws CycleException         A value that would form a reference cycle.
         * @throws CapacityException      A new key past `maxEntries`.
         */
        public function setMany(iterable $entries): int {}

        /** Remove `$key`. Returns whether it existed. No value materialised. */
        public function remove(int|string $key): bool {}

        /**
         * Bulk remove. Per-key atomic. Returns the number actually removed.
         *
         * @param iterable<int|string> $keys
         */
        public function removeMany(iterable $keys): int {}

        /** Remove all entries; returns the number removed. */
        public function clear(): int {}

        /** Overwrite and return the previous value (`null` ⟺ was absent). */
        public function swap(int|string $key, mixed $value): mixed {}

        /** Remove and return the previous value (`null` ⟺ was absent). */
        public function pop(int|string $key): mixed {}

        /**
         * Atomically store `$new` iff the current value equals `$expected`.
         * `null` is the absence sentinel on both sides:
         *   ($expected = null, $new = V)    → insert iff absent
         *   ($expected = A,    $new = B)    → replace iff current === A
         *   ($expected = A,    $new = null) → remove  iff current === A
         * Equality is by content (arrays order-sensitive, like `===`).
         * Returns true iff the swap was applied.
         *
         * @throws TypeException|ValueTooLargeException|CycleException|CapacityException
         */
        public function compareAndSet(int|string $key, mixed $expected, mixed $new): bool {}

        /**
         * Weakly-consistent traversal: `$fn(int|string $key, mixed $value)`
         * runs with no lock held. Keys deleted mid-traversal are skipped;
         * late inserts may be missed. Return `false` from `$fn` to stop early.
         *
         * @param callable(int|string, mixed): (bool|null) $fn
         */
        public function forEach(callable $fn): void {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Bounded object pool with per-thread slot affinity and idle-timeout
     * eviction.
     *
     * Factory runs lazily on first acquire; destroy (if provided) runs on
     * slot eviction or pool shutdown. `maxSize` is a hard budget — once it
     * is reached, acquire waits for a free slot (see the namespace-level
     * timeout convention).
     *
     * @link docs/en/shared-state/shared-pool.md
     */
    final class Pool implements Shareable
    {
        /**
         * @param callable      $factory       Creates a pooled resource. Receives no arguments; must return an object.
         * @param callable|null $destroy       Called with the resource on eviction/shutdown.
         * @param int           $maxSize       Hard budget of live slots; must be > 0 (default 32).
         * @param int           $idleTimeoutMs Idle ms before a slot is eligible for eviction; 0 disables eviction (default 300_000).
         */
        public function __construct(
            callable $factory,
            ?callable $destroy = null,
            int $maxSize = 32,
            int $idleTimeoutMs = 300_000,
        ) {}

        /** Acquire a slot, waiting forever. Always returns a Handle. */
        public function acquire(): Pool\Handle {}

        /** Non-blocking acquire. Returns a Handle, or null if the pool is saturated. */
        public function tryAcquire(): ?Pool\Handle {}

        /**
         * Acquire a slot within a bounded budget.
         *
         * @throws OperationTimeoutException On deadline expiry (extends Async\AsyncException, NOT SharedException).
         * @throws TypeException If `$ms <= 0`.
         */
        public function acquireTimeout(int $ms): Pool\Handle {}

        /**
         * Scope-guard: acquire (forever), invoke `$body($resource)` with the
         * raw pooled resource, and release the slot afterward — even if
         * `$body` throws. Returns whatever `$body` returns.
         */
        public function with(callable $body): mixed {}

        /**
         * Scope-guard with a bounded acquire budget.
         *
         * @throws OperationTimeoutException On deadline expiry.
         * @throws TypeException If `$ms <= 0`.
         */
        public function withTimeout(callable $body, int $ms): mixed {}

        /** Point-in-time snapshot of pool counters. */
        public function stats(): Pool\Stats {}

        /** Force-evict all idle slots reachable from this worker now. Returns the count. */
        public function evict(): int {}

        /** Registry ID for this instance. */
        public function id(): int {}
    }

    /**
     * Name-keyed process-global handles for `Shared\*`. Identity-by-name
     * complements identity-by-handle (`new Shared\*()`): every worker /
     * request that calls a method with the same `$key` converges on the
     * one entry the registry binds under that key.
     *
     * Get-or-create semantics: the factory runs **exactly once** per
     * successful bind (block-losers across worker threads). On hit, the
     * factory is ignored. The bound entry is pinned for the process
     * lifetime — invalidate by mutating in place (`$map->clear()`,
     * `$counter->set(0)`) or unbind the namespace key with `remove()`.
     *
     * Works in both routing modes. In worker mode the named entry
     * collapses the per-worker bootstrap pattern into one shared
     * instance; in traditional mode it survives across requests, giving
     * APCu-style same-host coordination without an external store.
     *
     * @see Shareable
     */
    final class Registry
    {
        /**
         * Typed get-or-create. Returns the existing `Map` bound under
         * `$key`, or runs `$factory()` (must return a fresh `Map`),
         * binds and returns it.
         *
         * The exception set is identical for every typed method
         * (`counter`, `atomic`, `flag`, `once`, `mutex`, `channel`,
         * `pool`) and for `global` (which never raises `TypeException`).
         * The list is documented once here and elided on the siblings
         * to keep the stub readable.
         *
         * @throws TypeException If `$key` is bound to a different
         *                       Shared type, the factory returned the
         *                       wrong type, or the factory returned a
         *                       non-Shareable / non-serialisable value.
         * @throws DeadlockException If called for `$key` from inside
         *                           its own factory on the same thread
         *                           (reentrancy), or if a cross-key
         *                           cycle held the call past the
         *                           30-second gate timeout.
         * @throws CapacityException If creating the entry would exceed
         *                           the `SHARED_MAX_ENTRIES` /
         *                           `SHARED_MAX_BYTES` caps.
         * @throws SharedException If the registry is draining
         *                         (graceful shutdown in progress), or
         *                         a peer creator won the bind race
         *                         after this thread's factory finished.
         * @throws \InvalidArgumentException If `$key` is empty.
         */
        public static function map(string $key, callable $factory): Map {}

        /** Typed get-or-create for `Shared\Counter`. See {@see map()} for the full exception list. */
        public static function counter(string $key, callable $factory): Counter {}

        /** Typed get-or-create for `Shared\Atomic`. See {@see map()} for the full exception list. */
        public static function atomic(string $key, callable $factory): Atomic {}

        /** Typed get-or-create for `Shared\Flag`. See {@see map()} for the full exception list. */
        public static function flag(string $key, callable $factory): Flag {}

        /** Typed get-or-create for `Shared\Once`. See {@see map()} for the full exception list. */
        public static function once(string $key, callable $factory): Once {}

        /** Typed get-or-create for `Shared\Mutex`. See {@see map()} for the full exception list. */
        public static function mutex(string $key, callable $factory): Mutex {}

        /** Typed get-or-create for `Shared\Channel`. See {@see map()} for the full exception list. */
        public static function channel(string $key, callable $factory): Channel {}

        /** Typed get-or-create for `Shared\Pool`. See {@see map()} for the full exception list. */
        public static function pool(string $key, callable $factory): Pool {}

        /**
         * Untyped escape hatch. Returns whatever is bound under `$key`
         * (no type guard). On miss, runs `$factory()` which must return
         * any `Shareable`. Does NOT raise `TypeException` on hit; every
         * other exception from {@see map()} still applies.
         */
        public static function global(string $key, callable $factory): Shareable {}

        /**
         * Unbind a key and drop its pin. Existing handles continue to
         * work on the now-anonymous entry; the entry self-deregisters
         * when its last handle drops. The next typed call under the
         * same key creates a NEW entry — captured handles do not
         * automatically converge on it. This is namespace management,
         * not object teardown.
         *
         * @return bool True if a bound key was removed, false if absent.
         * @throws \InvalidArgumentException If `$key` is empty.
         */
        public static function remove(string $key): bool {}

        /**
         * Currently-bound keys. `Creating` (in-flight first-touch)
         * slots are NOT included.
         *
         * @return list<string>
         */
        public static function keys(): array {}

        /**
         * Estimated bytes across **all** Shared\* entries — named AND
         * anonymous. Same accounting `SHARED_MAX_BYTES` caps. This is a
         * static estimate, not RSS; use a heap profiler / cgroups for
         * the true number.
         */
        public static function memoryUsage(): int {}

        /**
         * Live Shared\* entries (named + anonymous). Transient —
         * **not** equal to `count(Registry::keys())`, which is the
         * named namespace only.
         */
        public static function count(): int {}

        /**
         * Registry is a static facade. The constructor is registered on
         * the class only so the runtime can throw a clear
         * `SharedException` on `new Registry()` — calling it always
         * fails. Marked `public` here so static analysers and IDEs see
         * the same surface the runtime exposes; the `@throws` documents
         * the unconditional failure.
         *
         * @throws SharedException always — Registry cannot be instantiated.
         */
        public function __construct() {}
    }

    // ── Exception hierarchy ─────────────────────────────────────
    //
    //   \Exception
    //     ├── OxPHP\Async\AsyncException
    //     │    ├── OxPHP\Shared\OperationTimeoutException
    //     │    ├── OxPHP\Shared\ContentionException
    //     │    └── OxPHP\Shared\DeadlockException
    //     └── OxPHP\Shared\SharedException
    //          ├── StaleHandleException
    //          ├── TypeException
    //          │    └── CycleException
    //          ├── CapacityException
    //          ├── ClosedException             (Channel / Once close paths)
    //          ├── PoisonedException           (deprecated; Once only)
    //          ├── UninitializedException
    //          ├── InvalidOrderingException
    //          └── CorruptedMutexException

    /** Base class for every Shared\* exception. */
    class SharedException extends \Exception {}

    /** Operation used a handle whose underlying entry was dropped. */
    class StaleHandleException extends SharedException {}

    /** Value is not storable in the target Shared\* container. */
    class TypeException extends SharedException {}

    /** Map::set would form a cycle via nested Shareable references. */
    class CycleException extends TypeException {}

    /** Container is full and cannot accept a new entry. */
    class CapacityException extends SharedException {}

    /**
     * A single value's serialised size exceeds the per-value cap
     * (`SHARED_MAX_VALUE_SIZE`, default 1 MiB). Thrown by Map writes.
     */
    class ValueTooLargeException extends SharedException {}

    /**
     * A receive/send was attempted on a closed {@see Channel}, or a closed
     * {@see Once} was accessed. (Pool surfaces this only on the internal
     * shutdown-drain path; it is not part of Pool's user-facing surface,
     * since Pool has no `close()`.)
     */
    class ClosedException extends SharedException {}

    /**
     * @deprecated Once initializer panicked. Removed once Once migrates
     *             to a Result-style API.
     */
    class PoisonedException extends SharedException {}

    /** Access to a Once before it was initialised. */
    class UninitializedException extends SharedException {}

    /**
     * An {@see Atomic} operation was given a memory ordering it does not
     * permit (e.g. `Acquire` on a store, `Release` on a load).
     */
    class InvalidOrderingException extends SharedException {}

    /**
     * Mutex acquisition failed because a prior closure invocation
     * crashed via Rust panic and left the mutex unusable. There is
     * no recovery API — discard the instance.
     */
    class CorruptedMutexException extends SharedException {}

    /**
     * A bounded wait on a Shared\* primitive exceeded its deadline. Thrown
     * by {@see Mutex::withLockTimeout}, {@see Pool::acquireTimeout} and
     * {@see Pool::withTimeout}. (Channel's bounded receives/sends report a
     * timeout via {@see Channel\RecvResult} / {@see Channel\SendResult}
     * instead of throwing.)
     *
     * Extends {@see \OxPHP\Async\AsyncException} so a single
     * `catch (AsyncException)` sweeps both Shared\* timeouts and Async\*
     * await timeouts. Note it does NOT extend {@see SharedException}.
     */
    class OperationTimeoutException extends \OxPHP\Async\AsyncException {}

    /**
     * Non-blocking acquisition found the lock held. Thrown by
     * {@see Mutex::tryWithLock}.
     */
    class ContentionException extends \OxPHP\Async\AsyncException {}

    /**
     * Wait-for cycle detected during a Mutex acquisition. Extends
     * {@see \OxPHP\Async\AsyncException} (was a child of the removed
     * Shared\TimeoutException in earlier releases).
     */
    class DeadlockException extends \OxPHP\Async\AsyncException {}
}

namespace OxPHP\Shared\Once {

    /** State of an {@see \OxPHP\Shared\Once} cell. */
    enum Status
    {
        case Uninitialized; // empty, accepts a write
        case Pending;       // a factory is running right now (some thread)
        case Ready;         // value published
        case Poisoned;      // a Poison-mode factory failed — terminal
    }

    /** Policy for what a failed `getOrInit()` factory does to the cell. */
    enum FailureMode: int
    {
        case Reset = 0;  // failure -> back to Uninitialized (retryable, default)
        case Poison = 1; // failure -> Poisoned forever
    }
}

namespace OxPHP\Shared\Map {

    /**
     * Lazy key→value iterator returned by {@see \OxPHP\Shared\Map::getMany()}.
     * Materialises one value at a time and skips keys absent at read time.
     *
     * @internal Obtain via {@see \OxPHP\Shared\Map::getMany()}; never construct
     *           directly.
     */
    final class KeyCursor implements \Iterator
    {
        public function rewind(): void {}
        public function valid(): bool {}
        public function current(): mixed {}
        public function key(): int|string {}
        public function next(): void {}
    }
}

namespace OxPHP\Shared\Channel {

    /** Discriminant for {@see RecvResult}. */
    enum RecvStatus
    {
        case Ok;
        case Empty;
        case Timeout;
        case Closed;
    }

    /** Discriminant for {@see SendResult}. */
    enum SendStatus
    {
        case Ok;
        case Full;
        case Timeout;
        case Closed;
    }

    /**
     * Result of {@see \OxPHP\Shared\Channel::tryRecv()} / `recv()` /
     * `recvTimeout()`. Use the `isX()` accessors or `status()` with an
     * exhaustive `match` to dispatch.
     */
    final class RecvResult
    {
        public function isOk(): bool {}
        public function isEmpty(): bool {}
        public function isTimeout(): bool {}
        public function isClosed(): bool {}

        /**
         * Payload when {@see isOk()}, otherwise throws.
         *
         * @throws \OxPHP\Shared\SharedException If called on a non-Ok variant.
         */
        public function value(): mixed {}

        /** Payload when {@see isOk()}, else `$default`. Never throws. */
        public function valueOr(mixed $default): mixed {}

        /** Discriminant for exhaustive `match` dispatch. */
        public function status(): RecvStatus {}
    }

    /**
     * Result of {@see \OxPHP\Shared\Channel::trySend()} / `send()` /
     * `sendTimeout()`. No payload — only a discriminant.
     */
    final class SendResult
    {
        public function isOk(): bool {}
        public function isFull(): bool {}
        public function isTimeout(): bool {}
        public function isClosed(): bool {}

        public function status(): SendStatus {}
    }
}

namespace OxPHP\Shared\Pool {

    /**
     * Scope-bound reference to an acquired Pool slot.
     *
     * Clone is forbidden. `get()` returns the pooled value owned by this
     * acquire — no copy is made, and the slot is exclusive to the acquiring
     * thread. The slot returns to the pool automatically when the Handle is
     * destroyed (RAII, including stack unwind on exception), or earlier via
     * {@see release()}.
     *
     * @internal Produced by {@see \OxPHP\Shared\Pool::acquire()} /
     *           {@see \OxPHP\Shared\Pool::tryAcquire()} /
     *           {@see \OxPHP\Shared\Pool::acquireTimeout()}; never constructed directly.
     */
    final class Handle
    {
        /**
         * The pooled resource. Always call inside the acquiring thread.
         *
         * @throws \OxPHP\Shared\StaleHandleException If the handle has already been released.
         */
        public function get(): mixed {}

        /**
         * Return the slot to the pool now, before scope end. Idempotent: a
         * second call (or a `__destruct` after an explicit release) is a
         * no-op.
         */
        public function release(): void {}
    }

    /**
     * Immutable snapshot of {@see \OxPHP\Shared\Pool} counters, returned by
     * {@see \OxPHP\Shared\Pool::stats()}. The volatile trio
     * (`inUse()`/`idle()`/`waiting()`) is captured as one point-in-time
     * sample: `idle` is read once and `inUse` is derived from it, so
     * `size() === inUse() + idle()` holds for the object. The counters are
     * not lock-coupled across the pool, so the snapshot is point-in-time,
     * not atomic.
     *
     * Counters are exposed as accessor methods — the snapshot is immutable and
     * holds no public state.
     */
    final class Stats
    {
        /** Slots currently checked out. */
        public function inUse(): int {}

        /** Free slots ready to hand out. */
        public function idle(): int {}

        /** Callers blocked in `acquire`. */
        public function waiting(): int {}

        /** Live slots: `inUse() + idle()`. */
        public function size(): int {}

        /** Configured hard budget. */
        public function maxSize(): int {}

        /** `inUse() / maxSize()`, or `0.0` when `maxSize()` is 0. */
        public function utilization(): float {}
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

// ═══════════════════════════════════════════════════════════════
//  OxPHP\Server — Worker runtime handle
// ═══════════════════════════════════════════════════════════════

namespace OxPHP\Server {

    /**
     * Unified PHP-side handle for the OxPHP worker runtime.
     *
     * Provides introspection (id, request counter, memory, RSS) and the
     * worker entry point (serve). Available in both traditional and worker
     * modes; methods that have no meaning outside worker mode throw
     * Exception\InvalidServeContextException.
     *
     * Stateless wrapper — every method reads from Rust thread-local state via
     * FFI at call time. No data is cached on the object. The class instance
     * itself is a per-thread singleton: Worker::current() returns the same
     * object throughout the worker thread's lifetime.
     */
    final class Worker
    {
        // ── Factory & mode predicate ──

        /**
         * Returns the per-thread Worker singleton.
         *
         * The same instance is returned for every call within a given OS
         * thread. Identity holds across requests in worker mode and across
         * the single request in traditional mode.
         */
        public static function current(): self {}

        /**
         * Whether this thread is running in worker mode.
         *
         * true  — long-lived PHP process, one bootstrap, many requests
         *         dispatched through serve().
         * false — traditional per-request lifecycle (php_request_startup /
         *         shutdown around each request).
         *
         * @see \oxphp_is_worker() Procedural equivalent; both return the same
         *      value (delegate to the same underlying check).
         */
        public static function isWorkerMode(): bool {}

        // ── Identity & lifecycle ──

        /**
         * Worker thread index, 0..N-1 where N is the configured pool size
         * (PHP_WORKERS). Stable for the lifetime of the thread.
         */
        public function id(): int {}

        /**
         * Unix timestamp (float, sub-second precision) when this OS thread
         * was spawned. Same value across every request handled by the
         * thread, in both modes.
         *
         * For per-request start time, use OxPHP\Http\RequestInterface::startTime().
         */
        public function startTime(): float {}

        // ── Request counter ──

        /**
         * Number of requests this OS thread has started, counted at the
         * start of each request before the handler body runs (1-based).
         *
         * Both modes: 1 inside the first request handled by this thread,
         * 2 inside the second, etc. The counter is bound to the OS thread,
         * not to the PHP request lifecycle — it survives across
         * php_request_shutdown / php_request_startup boundaries in
         * traditional mode just as it survives across handler invocations
         * in worker mode.
         */
        public function requestCount(): int {}

        // ── Memory observability ──

        /**
         * Current Zend allocator usage in bytes (zend_memory_usage(0)).
         *
         * Worker mode: cumulative across requests until the next gc cycle.
         * Traditional mode: scoped to the current request.
         */
        public function memoryUsage(): int {}

        /**
         * Process RSS in bytes.
         *
         * Reads /proc/self/statm (Linux) or getrusage(RUSAGE_SELF) (macOS).
         * Not cached — every call hits the OS. Cheap but not free; for
         * repeated checks within a handler, store the result in a local
         * variable.
         *
         * Use case: RSS-based recycle policy when extension-internal
         * allocations are not visible to memoryUsage().
         */
        public function rss(): int {}

        /**
         * Configured worker memory cap in bytes (WORKER_MAX_MEMORY_MIB × 1MB).
         * Returns 0 when no cap is configured.
         *
         * The cap is enforced by the worker loop in worker mode. In
         * traditional mode the value is reported for symmetry but not
         * enforced (PHP's per-request memory_limit applies instead).
         */
        public function maxMemoryBytes(): int {}

        // ── Graceful exit ──

        /**
         * Mark this worker for graceful exit after the current request
         * completes. The supervisor respawns a fresh worker, re-running
         * the outer scope of the entry script.
         *
         * Idempotent: subsequent calls are no-ops; the first call wins
         * and exitReason() reports 'scheduled'.
         *
         * No-op in traditional mode (the script is exiting anyway).
         */
        public function scheduleExit(): void {}

        /**
         * True iff scheduleExit() has been called for this worker, or
         * the worker loop has otherwise queued an exit (e.g. memory cap
         * exceeded). Always false in traditional mode.
         */
        public function isExitScheduled(): bool {}

        /**
         * Reason for the pending exit, or null when no exit is pending.
         * One of: 'scheduled' (scheduleExit() was called), 'max_memory'
         * (WORKER_MAX_MEMORY_MIB threshold crossed), 'error' (the worker
         * loop bailed). Always null in traditional mode.
         */
        public function exitReason(): ?string {}

        // ── Worker entry point ──

        /**
         * Enter the worker request-dispatch loop.
         *
         * Worker mode: the handler is invoked once per incoming request.
         * The loop multiplexes requests across PHP fibers — when a handler
         * suspends on I/O via oxphp_async_*() / oxphp_sleep(), the loop
         * accepts new requests and resumes ready fibers cooperatively.
         *
         * Traditional mode: throws Exception\InvalidServeContextException —
         * there is no persistent loop to enter; the request has already been
         * handed to the script.
         *
         * @throws \OxPHP\Server\Exception\InvalidServeContextException When called outside worker mode
         *
         * @see \oxphp_worker() Procedural equivalent. Both share the same
         *      dispatch loop and per-thread re-entry guard, but differ at the
         *      boundary: serve() throws InvalidServeContextException outside
         *      worker mode, whereas oxphp_worker() emits E_WARNING and
         *      returns false.
         */
        public function serve(callable $handler): void {}
    }
}

namespace OxPHP\Server\Exception {

    /**
     * Thrown when Worker::serve() is called outside worker mode.
     *
     * In traditional mode there is no persistent process to host a loop —
     * each request runs the script once and exits. The application should
     * either enable worker mode (WORKER_MODE_ENABLED=true) or stop calling
     * serve() in traditional contexts.
     */
    final class InvalidServeContextException extends \RuntimeException {}
}
