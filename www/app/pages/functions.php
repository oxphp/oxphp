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
        'return'  => 'array — Associative array with keys: <code>sapi</code>, <code>version</code>, <code>worker_id</code>, <code>request_time</code>.',
        'desc'    => 'Returns server metadata for the current request. The <code>request_time</code> is a Unix timestamp with microsecond precision, set before <code>php_request_startup()</code> for accurate timing.',
        'example' => '$info = oxphp_server_info();
// [
//     "sapi"         => "oxphp",
//     "version"      => "0.1.0",
//     "worker_id"    => 3,
//     "request_time" => 1740000000.123456
// ]

header("X-Worker: " . $info["worker_id"]);',
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
        'name'    => 'oxphp_request_heartbeat',
        'sig'     => 'oxphp_request_heartbeat(int $time = 10): bool',
        'params'  => [
            ['name' => '$time', 'type' => 'int', 'default' => '10', 'desc' => 'Seconds to extend the timeout by.'],
        ],
        'return'  => 'bool — Always <code>true</code>.',
        'desc'    => 'Signals that the script is still alive and extends the request timeout. Call periodically in long-running loops to prevent the server from killing the request due to <code>REQUEST_TIMEOUT_SECS</code>.',
        'example' => '// Process large CSV import without hitting timeout
$handle = fopen("large_import.csv", "r");
while (($row = fgetcsv($handle)) !== false) {
    oxphp_request_heartbeat(30);
    import_row($row);
}
fclose($handle);',
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
            'oxphp_is_streaming'      => '<span class="mono">' . (oxphp_is_streaming() ? 'true' : 'false') . '</span>',
            'oxphp_request_heartbeat' => '<span class="mono">' . (oxphp_request_heartbeat() ? 'true' : 'false') . '</span>',
            'oxphp_stream_flush'      => '<span class="mono dim">not called &mdash; would activate streaming</span>',
            'oxphp_finish_request'    => '<span class="mono dim">not called &mdash; would end response</span>',
            default                   => '',
        };
        $live = '<div class="fn-live"><span class="fn-live-label">Live result</span>' . $val . '</div>';
    }

    $sections .= '<div class="fn-entry fn-slide" id="' . $fn['name'] . '" style="animation-delay:' . $delay . 'ms">'
        . '<div class="fn-header">'
        . '<h2><a href="#' . $fn['name'] . '">' . $fn['name'] . '</a></h2>'
        . '<div class="fn-version">0.1.0</div></div>'
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
