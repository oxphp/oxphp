<?php

$sapi       = php_sapi_name();
$php_ver    = PHP_VERSION;
$zts        = PHP_ZTS ? 'Enabled' : 'Disabled';
$os         = PHP_OS;
$workers    = getenv('PHP_WORKERS') ?: 'auto';
$executor   = getenv('EXECUTOR') ?: 'sapi';
$opcache    = function_exists('opcache_get_status') && opcache_get_status() !== false;
$server_sw  = $_SERVER['SERVER_SOFTWARE'] ?? 'unknown';
$request_id = $_SERVER['HTTP_X_REQUEST_ID'] ?? '-';
$remote     = $_SERVER['REMOTE_ADDR'] ?? '-';

$extensions = get_loaded_extensions();
sort($extensions);
$ext_list      = implode(', ', $extensions);
$ext_count     = count($extensions);
$opcache_label = $opcache ? 'Enabled' : 'Disabled';

$routes = [
    ['GET',    '/',               'Dashboard (this page)'],
    ['GET',    '/echo',           'Echo tool — inspect any request'],
    ['GET',    '/upload',         'File upload form'],
    ['GET',    '/cookies',        'Cookie manager'],
    ['GET',    '/opcache',        'OPcache dashboard'],
    ['*',      '/api/echo',       'Echo back request details (JSON)'],
    ['QUERY',  '/api/search',     'HTTP QUERY method demo'],
    ['POST',   '/api/upload',     'Handle file upload'],
    ['GET',    '/api/cookies/*',  'Set / read / clear cookies'],
    ['GET',    '/api/slow?ms=N',  'Sleep N ms (timeout test)'],
    ['GET',    '/api/error?type=X', 'Trigger PHP errors'],
    ['GET',    '/api/large?kb=N', 'Generate N KB (compression test)'],
    ['GET',    '/api/headers',    'Inspect all headers'],
    ['GET',    '/api/info',       'Server info JSON'],
    ['GET',    '/api/async?mode=X', 'Async demo (parallel, race, compute)'],
    ['GET',    '/api/csp?mode=X', 'Channel CSP demo (fanin, pipeline, poll)'],
];

$routes_html = '';
foreach ($routes as [$m, $p, $desc]) {
    $badge = $m === '*' ? '<span class="badge method">ALL</span>'
        : '<span class="badge method">' . h($m) . '</span>';
    $link = str_starts_with($p, '/api/') || str_contains($p, '?') || str_contains($p, '*')
        ? h($p) : '<a href="' . h($p) . '">' . h($p) . '</a>';
    $routes_html .= "<tr><td>$badge</td><td class=\"mono\">$link</td><td>$desc</td></tr>";
}

$features = [
    ['HTTP Methods',  'GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS, QUERY'],
    ['Routing',       'Framework mode (front controller)'],
    ['Compression',   'Brotli, Zstandard and gzip (auto, negotiated per Accept-Encoding)'],
    ['Rate Limiting', 'Per-IP sliding window (RATE_LIMIT / RATE_WINDOW_SECONDS)'],
    ['Request IDs',   'X-Request-ID (auto-generated or pass-through)'],
    ['Timeouts',      'max_execution_time / set_time_limit(), HEADER_TIMEOUT_SECONDS'],
    ['TLS',           'TLS_CERT + TLS_KEY (native rustls)'],
    ['Error Pages',   'Custom HTML per status code (ERROR_PAGES_DIR)'],
    ['Static Files',  'MIME detection, in-memory cache ≤1 MiB, compressed up to 3 MiB, streaming above, HTTP caching (ETag/304)'],
    ['Observability', '/health, /metrics (Prometheus), /config (internal server)'],
    ['Async Promises', 'oxphp_async() + oxphp_async_await() — parallel closures on dedicated thread pool'],
    ['Fiber Multiplexing', 'Cooperative multitasking — oxphp_sleep() / oxphp_async_await() yield the worker to other requests'],
    ['Worker Pool',   'PHP ZTS threads — static (N) or dynamic (MIN:MAX)'],
    ['OPcache + JIT', 'Shared memory bytecode cache with JIT compilation'],
];

$features_html = '';
foreach ($features as [$name, $desc]) {
    $features_html .= "<tr><td><strong>$name</strong></td><td>$desc</td></tr>";
}

layout('Dashboard', <<<HTML
<div class="grid-2">
    <div class="card">
        <div class="card-header">Runtime</div>
        <div class="card-body">
            <table class="table-kv">
                <tr><td>Server</td><td class="mono">{$server_sw}</td></tr>
                <tr><td>SAPI</td><td class="mono">{$sapi}</td></tr>
                <tr><td>PHP</td><td class="mono">{$php_ver}</td></tr>
                <tr><td>ZTS</td><td class="mono">{$zts}</td></tr>
                <tr><td>OS</td><td class="mono">{$os}</td></tr>
                <tr><td>Executor</td><td class="mono">{$executor}</td></tr>
                <tr><td>Workers</td><td class="mono">{$workers}</td></tr>
                <tr><td>OPcache</td><td class="mono">{$opcache_label}</td></tr>
                <tr><td>Request ID</td><td class="mono">{$request_id}</td></tr>
                <tr><td>Remote</td><td class="mono">{$remote}</td></tr>
            </table>
        </div>
    </div>

    <div class="card">
        <div class="card-header">Features</div>
        <div class="card-body">
            <table class="table-kv">{$features_html}</table>
        </div>
    </div>
</div>

<div class="card">
    <div class="card-header">Routes</div>
    <div class="card-body">
        <table class="table-routes">
            <thead><tr><th>Method</th><th>Path</th><th>Description</th></tr></thead>
            <tbody>{$routes_html}</tbody>
        </table>
    </div>
</div>

<div class="card">
    <div class="card-header">Loaded Extensions ({$ext_count})</div>
    <div class="card-body"><p class="mono ext-list">{$ext_list}</p></div>
</div>
HTML);
