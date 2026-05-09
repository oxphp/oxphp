<?php

$functions = [
    [
        'name'    => 'oxphp_request_id',
        'sig'     => 'oxphp_request_id(): string',
        'params'  => [],
        'return'  => 'string — 16-character hex request ID.',
        'desc'    => 'Returns the unique request ID assigned by the server. The same value is sent in the <code>X-Request-ID</code> response header. If the client sends <code>X-Request-ID</code>, the server passes it through instead of generating one.',
        'example' => '$id = oxphp_request_id();
echo $id; // "67b9a3c100000042"

// Use in structured logging
error_log(json_encode([
    "request_id" => oxphp_request_id(),
    "action"     => "user.login",
    "user_id"    => $user->id,
]));',
    ],
    [
        'name'    => 'oxphp_worker_id',
        'sig'     => 'oxphp_worker_id(): int',
        'params'  => [],
        'return'  => 'int — Zero-based worker thread index.',
        'desc'    => 'Returns the index of the PHP ZTS worker thread handling the current request. Worker indices range from <code>0</code> to <code>PHP_WORKERS - 1</code>. Useful for per-worker caching, debugging, and log correlation.',
        'example' => '$wid = oxphp_worker_id();
echo "Handled by worker #{$wid}";

// Per-worker temp file to avoid collisions
$tmp = "/tmp/worker_{$wid}_buffer.dat";',
    ],
    [
        'name'    => 'oxphp_server_info',
        'sig'     => 'oxphp_server_info(): array',
        'params'  => [],
        'return'  => 'array — Associative array with keys: <code>version</code>, <code>worker_id</code>, <code>request_time</code>, <code>worker_mode</code>.',
        'desc'    => 'Returns server metadata for the current request. The <code>request_time</code> is a Unix timestamp with microsecond precision, set before <code>php_request_startup()</code> for accurate timing.',
        'example' => '$info = oxphp_server_info();
// [
//     "version"      => "0.1.0",
//     "worker_id"    => 3,
//     "request_time" => 1740000000.123456,
//     "worker_mode"  => true
// ]

header("X-Worker: " . $info["worker_id"]);',
    ],
    [
        'name'    => 'oxphp_is_worker',
        'sig'     => 'oxphp_is_worker(): bool',
        'params'  => [],
        'return'  => 'bool — <code>true</code> if running in worker mode, <code>false</code> in traditional mode.',
        'desc'    => 'Checks whether the server is running in worker mode. In worker mode, PHP boots once and handles multiple requests via <code>oxphp_worker()</code>. In traditional mode, each request spawns a fresh PHP process. Use this to conditionally enable worker-specific logic such as connection pooling or static caches.',
        'example' => 'if (oxphp_is_worker()) {
    // Worker mode: reuse persistent DB connection
    $db = $GLOBALS["db"] ?? ($GLOBALS["db"] = new PDO($dsn));
} else {
    // Traditional mode: connect per request
    $db = new PDO($dsn);
}',
    ],
    [
        'name'    => 'oxphp_finish_request',
        'sig'     => 'oxphp_finish_request(): bool',
        'params'  => [],
        'return'  => 'bool — <code>true</code> on success, <code>false</code> if already called.',
        'desc'    => 'Flushes the response to the client and marks the request as finished. Any code after this call continues executing without blocking the HTTP response. Similar to <code>fastcgi_finish_request()</code> in PHP-FPM.',
        'example' => '// Send response immediately
echo json_encode(["status" => "accepted"]);
oxphp_finish_request();

// Background work — client already got 200 OK
send_notification_email($user);
update_analytics($event);
cleanup_temp_files();',
    ],
    [
        'name'    => 'oxphp_is_streaming',
        'sig'     => 'oxphp_is_streaming(): bool',
        'params'  => [],
        'return'  => 'bool — <code>true</code> if streaming mode is active.',
        'desc'    => 'Checks whether the current request is in streaming mode (Server-Sent Events, chunked transfer encoding). In streaming mode, output is flushed to the client immediately rather than buffered.',
        'example' => 'if (oxphp_is_streaming()) {
    // SSE event
    echo "event: update\n";
    echo "data: " . json_encode($payload) . "\n\n";
    flush();
}',
    ],
    [
        'name'    => 'oxphp_stream_flush',
        'sig'     => 'oxphp_stream_flush(): bool',
        'params'  => [],
        'return'  => 'bool — <code>true</code> on success, <code>false</code> if the request was already finished.',
        'desc'    => 'Activates streaming mode and flushes the current output buffer to the client as an HTTP chunk. This is the primary function for implementing Server-Sent Events (SSE). On the first call it enables streaming via the C bridge; subsequent calls send buffered output. Native <code>flush()</code> also works if <code>Content-Type: text/event-stream</code> is set and output buffering is disabled.',
        'example' => 'header("Content-Type: text/event-stream");
header("Cache-Control: no-cache");

for ($i = 0; $i < 10; $i++) {
    echo "id: $i\n";
    echo "data: " . json_encode(["counter" => $i]) . "\n\n";
    oxphp_stream_flush();
    sleep(1);
}',
    ],
    [
        'name'    => 'oxphp_sleep',
        'version' => '0.2.0',
        'sig'     => 'oxphp_sleep(float $seconds): void',
        'params'  => [
            ['name' => '$seconds', 'type' => 'float', 'desc' => 'Duration to sleep in seconds (e.g. 0.5 for 500ms).'],
        ],
        'return'  => 'void',
        'desc'    => 'Cooperative sleep that suspends the current fiber, allowing the worker thread to handle other requests during the wait. When called inside a fiber (worker mode with multiplexing), the fiber is suspended and a timer is registered. The scheduler resumes it after the specified duration. Falls back to blocking <code>usleep()</code> outside a fiber.',
        'example' => 'oxphp_worker(function () {
    // Non-blocking: other requests proceed during sleep
    oxphp_sleep(0.1);  // 100ms cooperative sleep
    echo "done";
});

// SSE with cooperative sleep
oxphp_worker(function () {
    header("Content-Type: text/event-stream");
    for ($i = 0; $i < 10; $i++) {
        echo "data: " . json_encode(["counter" => $i]) . "\n\n";
        oxphp_stream_flush();
        oxphp_sleep(1.0); // yields fiber, worker handles other requests
    }
});',
    ],
    [
        'name'    => 'oxphp_usleep',
        'version' => '0.2.0',
        'sig'     => 'oxphp_usleep(int $microseconds): void',
        'params'  => [
            ['name' => '$microseconds', 'type' => 'int', 'desc' => 'Duration to sleep in microseconds.'],
        ],
        'return'  => 'void',
        'desc'    => 'Cooperative microsecond sleep. Identical to <code>oxphp_sleep()</code> but accepts microseconds as an integer, consistent with PHP\'s built-in <code>usleep()</code>. Falls back to blocking <code>usleep()</code> when not inside a fiber.',
        'example' => 'oxphp_worker(function () {
    oxphp_usleep(50000);  // 50ms cooperative sleep
    echo "done";
});',
    ],
    [
        'name'    => 'oxphp_worker',
        'sig'     => 'oxphp_worker(callable $handler): bool',
        'params'  => [
            ['name' => '$handler', 'type' => 'callable', 'desc' => 'Callback invoked once per HTTP request. Receives no arguments.'],
        ],
        'return'  => 'bool — <code>true</code> on graceful shutdown, <code>false</code> if worker mode is not enabled.',
        'desc'    => 'Enters the persistent worker mode loop. Calls the handler for each HTTP request. Between requests, a soft reset cleans per-request state (superglobals, output buffers) without destroying the PHP heap, so bootstrap state (autoloaders, DB connections) persists. Workers are recycled when they exceed <code>WORKER_MAX_MEMORY_MIB</code>, or on demand via <code>Worker::scheduleExit()</code>. Only available when <code>WORKER_MODE_ENABLED=true</code> with <code>ENTRY_FILE</code> set.',
        'example' => '// worker.php — persistent worker entry point
require __DIR__ . "/vendor/autoload.php";
$db = new PDO("mysql:host=localhost;dbname=app", "root", "");

oxphp_worker(function () use ($db) {
    $uri = $_SERVER["REQUEST_URI"];

    if ($uri === "/api/users") {
        $users = $db->query("SELECT id, name FROM users")->fetchAll();
        header("Content-Type: application/json");
        echo json_encode($users);
    } else {
        http_response_code(404);
        echo "Not Found";
    }
});',
    ],
    [
        'name'    => 'oxphp_async',
        'version' => '0.2.0',
        'sig'     => 'oxphp_async(Closure $closure, mixed ...$args): int|false',
        'params'  => [
            ['name' => '$closure', 'type' => 'Closure', 'desc' => 'The closure to execute asynchronously on a dedicated worker thread.'],
            ['name' => '...$args', 'type' => 'mixed', 'desc' => 'Arguments passed to the closure via deep copy (serialized across threads).'],
        ],
        'return'  => 'int|false — Promise ID on success, <code>false</code> if async pool is disabled or queue is full.',
        'desc'    => 'Dispatches a closure for asynchronous execution on the dedicated async worker pool (separate from HTTP workers). Variables captured via <code>use</code> are serialized to the async thread. Supported types: null, bool, int, float, string, array. Objects and resources are rejected.',
        'example' => '$p = oxphp_async(function(int $x, int $y): int {
    return $x + $y;
}, 10, 20);

$result = oxphp_async_await($p); // 30',
    ],
    [
        'name'    => 'oxphp_async_await',
        'version' => '0.2.0',
        'sig'     => 'oxphp_async_await(int $promise_id, ?float $timeout = null): mixed',
        'params'  => [
            ['name' => '$promise_id', 'type' => 'int', 'desc' => 'Promise ID returned by <code>oxphp_async()</code>.'],
            ['name' => '$timeout', 'type' => '?float', 'default' => 'null', 'desc' => 'Timeout in seconds. <code>null</code> waits indefinitely.'],
        ],
        'return'  => 'mixed — The return value of the closure.',
        'desc'    => 'Blocks the current thread until the async task completes and returns its result. The return value is deserialized from the async worker thread. Each promise can only be awaited once. Throws <code>OxPHP\Async\AsyncException</code> on failure or <code>OxPHP\Async\TimeoutException</code> on timeout.',
        'example' => '$p = oxphp_async(fn(): string => "hello");
$result = oxphp_async_await($p); // "hello"

// With timeout:
try {
    $result = oxphp_async_await($p, 2.0);
} catch (\OxPHP\Async\TimeoutException $e) {
    // task took longer than 2 seconds
}',
    ],
    [
        'name'    => 'oxphp_async_await_all',
        'version' => '0.2.0',
        'sig'     => 'oxphp_async_await_all(array $promise_ids, ?float $timeout = null): array',
        'params'  => [
            ['name' => '$promise_ids', 'type' => 'array', 'desc' => 'Array of promise IDs from <code>oxphp_async()</code>.'],
            ['name' => '$timeout', 'type' => '?float', 'default' => 'null', 'desc' => 'Per-promise timeout in seconds.'],
        ],
        'return'  => 'array — Associative array mapping promise ID =&gt; result value.',
        'desc'    => 'Awaits multiple promises and returns all results. Blocks until every promise completes. Throws if any promise fails or times out.',
        'example' => '$p1 = oxphp_async(fn() => 1);
$p2 = oxphp_async(fn() => 2);
$p3 = oxphp_async(fn() => 3);
$results = oxphp_async_await_all([$p1, $p2, $p3]);
// [$p1 => 1, $p2 => 2, $p3 => 3]',
    ],
    [
        'name'    => 'oxphp_async_await_any',
        'version' => '0.2.0',
        'sig'     => 'oxphp_async_await_any(array $promise_ids, ?float $timeout = null): array',
        'params'  => [
            ['name' => '$promise_ids', 'type' => 'array', 'desc' => 'Array of promise IDs from <code>oxphp_async()</code>.'],
            ['name' => '$timeout', 'type' => '?float', 'default' => 'null', 'desc' => 'Overall timeout in seconds.'],
        ],
        'return'  => 'array — <code>[\'id\' => int, \'value\' => mixed]</code> — the winning promise.',
        'desc'    => 'Races multiple promises using <code>futures::select_all</code> and returns the first to complete. Non-winning promises remain individually awaitable via <code>oxphp_async_await()</code>. On timeout, all specified promises are cancelled.',
        'example' => '$p1 = oxphp_async(fn() => slow_api_a()); // 500ms
$p2 = oxphp_async(fn() => slow_api_b()); // 100ms
$winner = oxphp_async_await_any([$p1, $p2]);
// ["id" => $p2, "value" => ...]  (fastest wins)
$other = oxphp_async_await($p1); // still awaitable',
    ],
    [
        'name'    => 'oxphp_register_decorator',
        'version' => '0.2.0',
        'sig'     => 'oxphp_register_decorator(string $class): bool',
        'params'  => [
            ['name' => '$class', 'type' => 'string', 'desc' => 'Fully qualified class name implementing <code>OxPHP\Decorator\AttributeInterface</code>.'],
        ],
        'return'  => 'bool — <code>true</code> on success, <code>false</code> with <code>E_WARNING</code> on validation failure.',
        'desc'    => 'Registers a PHP class as an attribute-based decorator. The class must implement <code>OxPHP\Decorator\AttributeInterface</code> and be marked with <code>#[Attribute(...)]</code>. Once registered, any function, method, or class annotated with this attribute will have <code>before()</code>/<code>after()</code> called around each invocation. Call once during application bootstrap.',
        'example' => '#[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Timer implements OxPHP\Decorator\AttributeInterface {
    public function __construct(
        public readonly string $label = "",
    ) {}

    public function before(OxPHP\Decorator\Context $ctx): void {
        $this->start = hrtime(true);
    }

    public function after(OxPHP\Decorator\Context $ctx): void {
        $ms = (hrtime(true) - $this->start) / 1e6;
        error_log("[Timer] {$ctx->target}: {$ms}ms");
    }
}

oxphp_register_decorator(Timer::class);

#[Timer(label: "api")]
function handle_request(): void { /* ... */ }',
    ],
];

// ── Build function sections ──────────────────────────
$sections = '';
$idx = 0;
foreach ($functions as $fn) {
    $sig_html = h($fn['sig']);
    $example_html = h($fn['example']);
    $delay = $idx * 80;

    // Parameters table
    $params_html = '<div class="fn-params">No parameters.</div>';
    if (!empty($fn['params'])) {
        $param_rows = '';
        foreach ($fn['params'] as $p) {
            $default = isset($p['default']) ? " = {$p['default']}" : '';
            $param_rows .= '<tr><td class="mono">' . $p['name'] . '</td><td class="mono">' . $p['type'] . $default . '</td><td>' . $p['desc'] . '</td></tr>';
        }
        $params_html = '<table class="fn-param-table">'
            . '<thead><tr><th>Name</th><th>Type</th><th>Description</th></tr></thead>'
            . '<tbody>' . $param_rows . '</tbody></table>';
    }

    // Live result
    $live = '';
    if (function_exists($fn['name'])) {
        $val = match ($fn['name']) {
            'oxphp_request_id'        => '<span class="mono">"' . h(oxphp_request_id()) . '"</span>',
            'oxphp_worker_id'         => '<span class="mono">' . oxphp_worker_id() . '</span>',
            'oxphp_server_info'       => '<pre class="fn-live-pre">' . h(json_encode(oxphp_server_info(), JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES)) . '</pre>',
            'oxphp_is_worker'         => '<span class="mono">' . (oxphp_is_worker() ? 'true' : 'false') . '</span>',
            'oxphp_is_streaming'      => '<span class="mono">' . (oxphp_is_streaming() ? 'true' : 'false') . '</span>',
            'oxphp_stream_flush'      => '<span class="mono dim">not called &mdash; would activate streaming</span>',
            'oxphp_finish_request'    => '<span class="mono dim">not called &mdash; would end response</span>',
            'oxphp_sleep'             => '<span class="mono dim">not called &mdash; would suspend fiber</span>',
            'oxphp_usleep'            => '<span class="mono dim">not called &mdash; would suspend fiber</span>',
            'oxphp_worker'            => '<span class="mono dim">not called &mdash; enters worker loop</span>',
            'oxphp_async'             => '<span class="mono dim">not called &mdash; dispatches closure to async pool</span>',
            'oxphp_async_await'       => '<span class="mono dim">not called &mdash; blocks until promise completes</span>',
            'oxphp_async_await_all'   => '<span class="mono dim">not called &mdash; awaits multiple promises</span>',
            'oxphp_async_await_any'   => '<span class="mono dim">not called &mdash; races promises, returns fastest</span>',
            'oxphp_register_decorator' => '<span class="mono dim">not called &mdash; registers a decorator class</span>',
            default                   => '',
        };
        $live = '<div class="fn-live"><span class="fn-live-label">Live result</span>' . $val . '</div>';
    }

    $sections .= '<div class="fn-entry fn-slide" id="' . $fn['name'] . '" style="animation-delay:' . $delay . 'ms">'
        . '<div class="fn-header">'
        . '<h2><a href="#' . $fn['name'] . '">' . $fn['name'] . '</a></h2>'
        . '<div class="fn-version">' . h($fn['version'] ?? '0.1.0') . '</div></div>'
        . '<div class="fn-sig-block"><code>' . $sig_html . '</code></div>'
        . '<p class="fn-description">' . $fn['desc'] . '</p>'
        . $live
        . '<div class="fn-details">'
        . '<div class="fn-detail-section"><h3>Parameters</h3>' . $params_html . '</div>'
        . '<div class="fn-detail-section"><h3>Return</h3><p>' . $fn['return'] . '</p></div>'
        . '<div class="fn-detail-section"><h3>Example</h3>'
        . '<pre class="fn-code"><code>&lt;?php' . "\n" . $example_html . '</code></pre>'
        . '</div></div></div>';

    $idx++;
}

// ── Table of contents ─────────────────────────────────
$toc = '';
foreach ($functions as $fn) {
    $short_desc = strip_tags(explode('.', $fn['desc'])[0]);
    $toc .= '<li><a href="#' . $fn['name'] . '">' . $fn['name'] . '</a><span class="toc-desc">' . $short_desc . '</span></li>';
}

$fn_count = count($functions);

layout('Server Functions', <<<HTML
<style>
    /* ── Layout: sidebar right, content left ─────────── */
    .fn-layout {
        display: grid;
        grid-template-columns: 1fr 280px;
        gap: 32px;
        align-items: start;
    }

    @media (max-width: 900px) {
        .fn-layout {
            grid-template-columns: 1fr;
        }
        .fn-sidebar { position: static !important; }
    }

    /* ── Sidebar (sticky TOC) ────────────────────────── */
    .fn-sidebar {
        position: sticky;
        top: 80px;
        max-height: calc(100vh - 100px);
        overflow-y: auto;
    }

    .fn-sidebar .card { margin-bottom: 0; }

    .fn-toc { list-style: none; padding: 0; margin: 0; }
    .fn-toc li {
        border-bottom: 1px solid rgba(255,255,255,0.06);
    }
    .fn-toc li:last-child { border-bottom: none; }
    .fn-toc a {
        display: block;
        padding: 10px 16px;
        font-family: var(--mono);
        font-size: 13px;
        color: var(--color-muted);
        text-decoration: none;
        transition: color 0.2s, background 0.2s, border-left 0.2s;
        border-left: 3px solid transparent;
    }
    .fn-toc a:hover {
        color: var(--color-text);
        background: rgba(119,123,180,0.06);
    }
    .fn-toc a.active {
        color: #c4c8f0;
        background: rgba(119,123,180,0.1);
        border-left-color: var(--color-php);
    }
    .toc-desc {
        display: block;
        font-family: -apple-system, BlinkMacSystemFont, 'Inter', sans-serif;
        font-size: 11px;
        opacity: 0.5;
        margin-top: 2px;
        line-height: 1.3;
        padding: 6px 18px;
    }

    .fn-sidebar-count {
        padding: 12px 16px;
        font-size: 12px;
        color: var(--color-muted);
        border-bottom: 1px solid var(--color-border);
    }
    .fn-sidebar-count strong { color: var(--color-text); }

    /* ── Slide-in animation ──────────────────────────── */
    @keyframes fn-slide-in {
        from {
            opacity: 0;
            transform: translateY(24px);
        }
        to {
            opacity: 1;
            transform: translateY(0);
        }
    }

    .fn-slide {
        opacity: 0;
        animation: fn-slide-in 0.4s ease-out forwards;
    }

    /* ── Function entries ─────────────────────────────── */
    .fn-entry {
        background: var(--color-surface);
        border: 1px solid var(--color-border);
        border-radius: var(--radius);
        padding: 24px;
        margin-bottom: 20px;
        transition: border-color 0.3s;
    }
    .fn-entry:last-child { margin-bottom: 0; }
    .fn-entry:target,
    .fn-entry:hover {
        border-color: rgba(119,123,180,0.3);
    }

    .fn-header { display: flex; align-items: baseline; justify-content: space-between; margin-bottom: 12px; }
    .fn-header h2 { margin: 0; font-size: 18px; font-weight: 600; }
    .fn-header h2 a { color: var(--color-php); text-decoration: none; }
    .fn-header h2 a:hover { text-decoration: underline; }
    .fn-version {
        font-size: 11px;
        color: var(--color-muted);
        font-family: var(--mono);
        background: rgba(119,123,180,0.1);
        padding: 2px 8px;
        border-radius: 4px;
    }

    .fn-sig-block {
        margin-bottom: 16px;
        padding: 10px 16px;
        background: rgba(119,123,180,0.08);
        border-left: 3px solid var(--color-php);
        border-radius: 0 6px 6px 0;
    }
    .fn-sig-block code { font-size: 14px; color: #c4c8f0; }

    .fn-description {
        line-height: 1.6;
        font-size: 14px;
        color: var(--color-muted);
        margin-bottom: 16px;
    }
    .fn-description code {
        font-size: 12px;
        background: rgba(255,255,255,0.06);
        padding: 2px 6px;
        border-radius: 3px;
        color: var(--color-text);
    }

    /* ── Live result ──────────────────────────────────── */
    .fn-live {
        margin-bottom: 16px;
        padding: 12px 16px;
        background: rgba(183,71,42,0.06);
        border: 1px solid rgba(183,71,42,0.15);
        border-radius: 6px;
        font-size: 13px;
    }
    .fn-live-label {
        display: block;
        font-weight: 600;
        color: var(--color-ox);
        text-transform: uppercase;
        font-size: 10px;
        letter-spacing: 0.8px;
        margin-bottom: 6px;
    }
    .fn-live .mono { word-break: break-all; }
    .fn-live-pre {
        margin: 0;
        padding: 0;
        background: none;
        border: none;
        font-size: 12px;
        color: var(--color-text);
        white-space: pre;
    }

    /* ── Details (params, return, example) ────────────── */
    .fn-details {
        display: grid;
        gap: 16px;
    }

    .fn-detail-section h3 {
        font-size: 11px;
        text-transform: uppercase;
        letter-spacing: 0.8px;
        color: var(--color-muted);
        margin: 0 0 8px;
        font-weight: 600;
    }

    .fn-params { font-size: 13px; opacity: 0.6; font-style: italic; }

    .fn-param-table { font-size: 13px; width: 100%; border-collapse: collapse; }
    .fn-param-table th {
        font-weight: 600;
        text-transform: none;
        letter-spacing: 0;
        text-align: left;
        padding: 6px 12px 6px 0;
        border-bottom: 1px solid var(--color-border);
        font-size: 12px;
        color: var(--color-muted);
    }
    .fn-param-table td {
        padding: 6px 12px 6px 0;
        border-bottom: 1px solid rgba(255,255,255,0.04);
    }

    .fn-code {
        margin: 0;
        padding: 14px 16px;
        background: rgba(0,0,0,0.4);
        border: 1px solid rgba(255,255,255,0.06);
        border-radius: 6px;
        font-size: 13px;
        line-height: 1.5;
        overflow-x: auto;
    }
    .fn-code code { color: #c8ccd8; }

    .dim { opacity: 0.5; }
</style>

<div class="fn-layout">
    <div class="fn-content">
        {$sections}
    </div>

    <aside class="fn-sidebar">
        <div class="card">
            <div class="card-header">oxphp_sapi Extension</div>
            <div class="fn-sidebar-count">
                <strong>{$fn_count}</strong> functions &mdash; always available, no <code style="font-size:11px;background:rgba(255,255,255,0.06);padding:1px 4px;border-radius:3px">require</code> needed
            </div>
            <ul class="fn-toc">{$toc}</ul>
        </div>
    </aside>
</div>

<script>
(function() {
    const entries = document.querySelectorAll('.fn-entry');
    const links = document.querySelectorAll('.fn-toc a');
    if (!entries.length || !links.length) return;

    const observer = new IntersectionObserver(function(items) {
        items.forEach(function(item) {
            if (item.isIntersecting) {
                links.forEach(function(l) { l.classList.remove('active'); });
                const id = item.target.id;
                const active = document.querySelector('.fn-toc a[href="#' + id + '"]');
                if (active) active.classList.add('active');
            }
        });
    }, { rootMargin: '-80px 0px -60% 0px', threshold: 0 });

    entries.forEach(function(e) { observer.observe(e); });

    // Activate first link by default
    if (links[0]) links[0].classList.add('active');
})();
</script>
HTML);
