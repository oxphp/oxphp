<?php
$php_version = PHP_VERSION;
$sapi_name = php_sapi_name();
$zts = PHP_ZTS ? 'Enabled' : 'Disabled';
$os = PHP_OS;
$workers = getenv('PHP_WORKERS') ?: 'auto';
$executor = getenv('EXECUTOR') ?: 'sapi';
$opcache = function_exists('opcache_get_status') && opcache_get_status() !== false;
$extensions = get_loaded_extensions();
sort($extensions);
?>
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>OxPHP</title>
    <style>
        :root {
            --rust: #B7472A;
            --php: #777BB4;
            --bg: #ffffff;
            --bg-card: #f6f6f6;
            --fg: #1a1a1a;
            --fg-muted: #666666;
            --border: #e0e0e0;
            --tag-bg: #eaeaea;
            --tag-fg: #444444;
            --green: #1a7f37;
            --red: #cf222e;
        }

        @media (prefers-color-scheme: dark) {
            :root {
                --bg: #0d1117;
                --bg-card: #161b22;
                --fg: #e6edf3;
                --fg-muted: #8b949e;
                --border: #30363d;
                --tag-bg: #21262d;
                --tag-fg: #c9d1d9;
                --green: #3fb950;
                --red: #f85149;
            }
        }

        * { margin: 0; padding: 0; box-sizing: border-box; }

        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif;
            background: var(--bg);
            color: var(--fg);
            line-height: 1.5;
            min-height: 100vh;
            padding: 3rem 1.5rem;
        }

        .container {
            max-width: 720px;
            margin: 0 auto;
        }

        header {
            text-align: center;
            margin-bottom: 3rem;
        }

        h1 {
            font-size: 3rem;
            font-weight: 700;
            letter-spacing: -0.02em;
            margin-bottom: 0.5rem;
        }

        .logo-ox { color: var(--rust); }
        .logo-php { color: var(--php); }

        .tagline {
            color: var(--fg-muted);
            font-size: 1.1rem;
        }

        .grid {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 1rem;
            margin-bottom: 2rem;
        }

        @media (max-width: 540px) {
            .grid { grid-template-columns: 1fr; }
            h1 { font-size: 2.2rem; }
        }

        .card {
            background: var(--bg-card);
            border: 1px solid var(--border);
            border-radius: 8px;
            padding: 1.25rem;
        }

        .card h2 {
            font-size: 0.75rem;
            text-transform: uppercase;
            letter-spacing: 0.08em;
            color: var(--fg-muted);
            margin-bottom: 0.75rem;
        }

        .card-wide {
            grid-column: 1 / -1;
        }

        .stat {
            font-size: 1.5rem;
            font-weight: 600;
        }

        .stat-label {
            font-size: 0.85rem;
            color: var(--fg-muted);
        }

        .row {
            display: flex;
            justify-content: space-between;
            padding: 0.4rem 0;
            border-bottom: 1px solid var(--border);
            font-size: 0.9rem;
        }

        .row:last-child { border-bottom: none; }

        .row-label { color: var(--fg-muted); }

        .badge {
            display: inline-block;
            padding: 0.15rem 0.5rem;
            border-radius: 4px;
            font-size: 0.8rem;
            font-weight: 500;
        }

        .badge-on {
            background: color-mix(in srgb, var(--green) 15%, transparent);
            color: var(--green);
        }

        .badge-off {
            background: color-mix(in srgb, var(--red) 15%, transparent);
            color: var(--red);
        }

        .features {
            display: flex;
            flex-wrap: wrap;
            gap: 0.4rem;
        }

        .tag {
            display: inline-block;
            background: var(--tag-bg);
            color: var(--tag-fg);
            padding: 0.2rem 0.6rem;
            border-radius: 4px;
            font-size: 0.8rem;
        }

        .ext-list {
            display: flex;
            flex-wrap: wrap;
            gap: 0.3rem;
        }

        .ext {
            font-size: 0.75rem;
            color: var(--fg-muted);
            background: var(--tag-bg);
            padding: 0.1rem 0.45rem;
            border-radius: 3px;
            font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
        }

        .fn-table {
            width: 100%;
            border-collapse: collapse;
            font-size: 0.85rem;
        }

        .fn-table td {
            padding: 0.5rem 0;
            border-bottom: 1px solid var(--border);
            vertical-align: top;
        }

        .fn-table tr:last-child td { border-bottom: none; }

        .fn-name {
            font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
            font-size: 0.8rem;
            white-space: nowrap;
            padding-right: 1rem;
            color: var(--php);
            font-weight: 500;
        }

        .fn-desc {
            color: var(--fg-muted);
            font-size: 0.8rem;
        }

        .fn-return {
            font-family: 'SF Mono', 'Fira Code', 'Consolas', monospace;
            font-size: 0.7rem;
            color: var(--fg-muted);
            opacity: 0.7;
        }

        footer {
            text-align: center;
            color: var(--fg-muted);
            font-size: 0.8rem;
            margin-top: 2rem;
        }

        footer a {
            color: var(--php);
            text-decoration: none;
        }
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1><span class="logo-ox">Ox</span><span class="logo-php">PHP</span></h1>
            <p class="tagline">Async PHP application server powered by Rust</p>
        </header>

        <div class="grid">
            <div class="card">
                <h2>SAPI</h2>
                <div class="stat">OxPHP <small>(<?= htmlspecialchars($sapi_name) ?>)</small></div>
                <div class="stat-label">PHP <?= htmlspecialchars($php_version) ?></div>
            </div>

            <div class="card">
                <h2>Workers</h2>
                <div class="stat"><?= htmlspecialchars($workers) ?></div>
                <div class="stat-label"><?= htmlspecialchars($executor) ?> executor</div>
            </div>

            <div class="card card-wide">
                <h2>Runtime</h2>
                <div class="row">
                    <span class="row-label">Thread Safety (ZTS)</span>
                    <span class="badge <?= PHP_ZTS ? 'badge-on' : 'badge-off' ?>"><?= $zts ?></span>
                </div>
                <div class="row">
                    <span class="row-label">OPcache</span>
                    <span class="badge <?= $opcache ? 'badge-on' : 'badge-off' ?>"><?= $opcache ? 'Enabled' : 'Disabled' ?></span>
                </div>
                <div class="row">
                    <span class="row-label">Compression</span>
                    <?php $comp = getenv('COMPRESSION') !== 'off'; ?>
                    <span class="badge <?= $comp ? 'badge-on' : 'badge-off' ?>"><?= $comp ? 'Brotli' : 'Off' ?></span>
                </div>
                <div class="row">
                    <span class="row-label">Platform</span>
                    <span><?= htmlspecialchars($os) ?> / <?= PHP_INT_SIZE * 8 ?>-bit</span>
                </div>
                <div class="row">
                    <span class="row-label">Request Timeout</span>
                    <span><?= htmlspecialchars(getenv('REQUEST_TIMEOUT_SECS') ?: '120') ?>s</span>
                </div>
            </div>

            <div class="card card-wide">
                <h2>Features</h2>
                <div class="features">
                    <span class="tag">HTTP/1.1 &amp; HTTP/2 (TLS с ALPN)</span>
                    <span class="tag">Static Files</span>
                    <span class="tag">PHP Execution</span>
                    <span class="tag">Custom SAPI</span>
                    <span class="tag">Bounded Queue</span>
                    <span class="tag">Rate Limiting</span>
                    <span class="tag">Request IDs</span>
                    <span class="tag">Access Logging</span>
                    <span class="tag">Health Checks</span>
                    <span class="tag">Prometheus Metrics</span>
                    <span class="tag">TLS</span>
                    <span class="tag">Brotli</span>
                    <span class="tag">OPcache</span>
                    <span class="tag">Graceful Shutdown</span>
                    <span class="tag">Event System</span>
                    <span class="tag">Plugin System</span>
                </div>
            </div>

            <div class="card card-wide">
                <h2>PHP Functions</h2>
                <table class="fn-table">
                    <tr>
                        <td class="fn-name">oxphp_request_id()</td>
                        <td>
                            <div>Unique ID for the current request</div>
                            <div class="fn-desc">16-char hex string, useful for tracing and log correlation across services.</div>
                            <div class="fn-return">: string</div>
                        </td>
                    </tr>
                    <tr>
                        <td class="fn-name">oxphp_worker_id()</td>
                        <td>
                            <div>ID of the PHP worker thread handling this request</div>
                            <div class="fn-desc">Zero-based index. Useful for per-worker debugging and resource partitioning.</div>
                            <div class="fn-return">: int</div>
                        </td>
                    </tr>
                    <tr>
                        <td class="fn-name">oxphp_server_info()</td>
                        <td>
                            <div>Server runtime information</div>
                            <div class="fn-desc">Returns <code class="fn-return">["sapi" =&gt; "oxphp", "version" =&gt; "1.0.0", "worker_id" =&gt; 3, "request_time" =&gt; 1739488012.345]</code></div>
                            <div class="fn-return">: array</div>
                        </td>
                    </tr>
                    <tr>
                        <td class="fn-name">oxphp_request_heartbeat(<span class="fn-desc">int $time = 10</span>)</td>
                        <td>
                            <div>Extend the request timeout</div>
                            <div class="fn-desc">Resets the timeout counter for long-running tasks (reports, exports, etc.) without changing the global timeout.</div>
                            <div class="fn-return">: bool</div>
                        </td>
                    </tr>
                    <tr>
                        <td class="fn-name">oxphp_finish_request()</td>
                        <td>
                            <div>Send response immediately, continue execution</div>
                            <div class="fn-desc">Like fastcgi_finish_request(). Flushes output to the client while PHP continues to run (analytics, cleanup, emails).</div>
                            <div class="fn-return">: bool</div>
                        </td>
                    </tr>
                </table>
            </div>

            <div class="card card-wide">
                <h2>Loaded Extensions (<?= count($extensions) ?>)</h2>
                <div class="ext-list">
                    <?php foreach ($extensions as $ext): ?>
                        <span class="ext"><?= htmlspecialchars($ext) ?></span>
                    <?php endforeach; ?>
                </div>
            </div>
        </div>

        <footer>
            OxPHP &mdash; <a href="https://github.com/oxphp/oxphp">github.com/oxphp/oxphp</a>
        </footer>
    </div>
</body>
</html>
