<?php
/**
 * OxPHP Welcome Page — standalone, no layout() dependency.
 * Designed to be shipped with the demo server image.
 */

$server_sw  = htmlspecialchars($_SERVER['SERVER_SOFTWARE'] ?? 'OxPHP', ENT_QUOTES | ENT_HTML5, 'UTF-8');
$php_ver    = PHP_VERSION;
$sapi       = PHP_SAPI;
?>
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>OxPHP — Asynchronous PHP Application Server</title>
    <style>
    :root {
        --color-ox: #B7472A;
        --color-php: #777BB4;
        --color-bg: #0f1117;
        --color-surface: #181b23;
        --color-border: #282c36;
        --color-text: #e2e4e9;
        --color-muted: #8b8fa3;
        --color-accent: #777BB4;
        --radius: 8px;
        --mono: 'SF Mono', 'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace;
    }

    * { margin: 0; padding: 0; box-sizing: border-box; }

    body {
        font-family: -apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', sans-serif;
        background: var(--color-bg);
        color: var(--color-text);
        line-height: 1.6;
        min-height: 100vh;
        display: flex;
        flex-direction: column;
        -webkit-font-smoothing: antialiased;
    }

    a { color: var(--color-accent); text-decoration: none; }
    a:hover { text-decoration: underline; }
    code { font-family: var(--mono); font-size: 0.8125rem; color: var(--color-accent); }

    .container { max-width: 1200px; margin: 0 auto; padding: 0 24px; width: 100%; }

    /* ── Header ── */
    header {
        background: var(--color-surface);
        border-bottom: 1px solid var(--color-border);
        position: sticky;
        top: 0;
        z-index: 100;
    }
    header nav {
        max-width: 1200px;
        margin: 0 auto;
        padding: 0 24px;
        height: 56px;
        display: flex;
        align-items: center;
        gap: 24px;
    }
    .logo { display: flex; align-items: center; text-decoration: none; }
    .nav-links { display: flex; gap: 4px; flex: 1; }
    .nav-link {
        color: var(--color-muted);
        text-decoration: none;
        padding: 6px 12px;
        border-radius: var(--radius);
        font-size: 0.875rem;
        transition: color 0.15s, background 0.15s;
    }
    .nav-link:hover { color: var(--color-text); background: var(--color-border); text-decoration: none; }
    .meta { font-size: 0.75rem; color: var(--color-muted); font-family: var(--mono); }

    /* ── Hero ── */
    .hero {
        text-align: center;
        padding: 64px 0 48px;
    }
    .hero h1 {
        font-size: 3rem;
        font-weight: 800;
        letter-spacing: -1px;
        margin-bottom: 16px;
        line-height: 1.1;
    }
    .hero .ox { color: var(--color-ox); }
    .hero .php { color: var(--color-php); }
    .hero .tagline {
        font-size: 1.25rem;
        color: var(--color-muted);
        max-width: 640px;
        margin: 0 auto 36px;
        line-height: 1.5;
    }
    .hero .links {
        display: flex;
        gap: 12px;
        justify-content: center;
        flex-wrap: wrap;
    }
    .hero .links a {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        padding: 10px 24px;
        border-radius: var(--radius);
        font-size: 0.875rem;
        font-weight: 500;
        text-decoration: none;
        transition: opacity 0.15s;
    }
    .hero .links a:hover { opacity: 0.85; text-decoration: none; }
    .btn-primary { background: var(--color-accent); color: #fff; }
    .btn-outline { background: transparent; border: 1px solid var(--color-border); color: var(--color-text); }

    /* ── Main ── */
    main { flex: 1; padding: 0 0 48px; }

    /* ── Section titles ── */
    .section-title {
        font-size: 1.25rem;
        font-weight: 600;
        margin: 48px 0 20px;
        padding-bottom: 10px;
        border-bottom: 1px solid var(--color-border);
    }

    /* ── Feature cards ── */
    .features-grid {
        display: grid;
        grid-template-columns: repeat(3, 1fr);
        gap: 20px;
    }
    @media (max-width: 900px) { .features-grid { grid-template-columns: repeat(2, 1fr); } }
    @media (max-width: 600px) { .features-grid { grid-template-columns: 1fr; } }

    .feature-card {
        background: var(--color-surface);
        border: 1px solid var(--color-border);
        border-radius: var(--radius);
        padding: 20px;
    }
    .feature-card .icon {
        width: 40px;
        height: 40px;
        border-radius: 8px;
        display: flex;
        align-items: center;
        justify-content: center;
        font-size: 1.25rem;
        margin-bottom: 12px;
        background: rgba(119, 123, 180, 0.1);
    }
    .feature-card h3 { font-size: 0.9375rem; font-weight: 600; margin-bottom: 6px; }
    .feature-card p { font-size: 0.8125rem; color: var(--color-muted); line-height: 1.5; }

    /* ── Cards ── */
    .card {
        background: var(--color-surface);
        border: 1px solid var(--color-border);
        border-radius: var(--radius);
        margin-bottom: 20px;
        overflow: hidden;
    }
    .card-header {
        padding: 12px 20px;
        font-weight: 600;
        font-size: 0.875rem;
        border-bottom: 1px solid var(--color-border);
        color: var(--color-muted);
        text-transform: uppercase;
        letter-spacing: 0.05em;
    }
    .card-body { padding: 16px 20px; }

    /* ── Grid ── */
    .grid-2 { display: grid; grid-template-columns: 1fr 1fr; gap: 20px; }
    @media (max-width: 768px) { .grid-2 { grid-template-columns: 1fr; } }

    /* ── Code blocks ── */
    pre {
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: var(--radius);
        padding: 16px;
        overflow-x: auto;
        font-family: var(--mono);
        font-size: 0.8125rem;
        line-height: 1.5;
        color: var(--color-text);
        white-space: pre-wrap;
        word-break: break-word;
    }
    .code-block { position: relative; }
    .copy-btn {
        position: absolute;
        top: 8px;
        right: 8px;
        background: var(--color-border);
        border: none;
        color: var(--color-muted);
        padding: 4px 10px;
        border-radius: 4px;
        font-size: 0.75rem;
        cursor: pointer;
        transition: color 0.15s;
    }
    .copy-btn:hover { color: var(--color-text); }

    .kw { color: var(--color-ox); }
    .str { color: #86efac; }
    .cmt { color: var(--color-muted); }

    /* ── Tables ── */
    .env-table { width: 100%; border-collapse: collapse; }
    .env-table th {
        text-align: left;
        padding: 8px 12px;
        font-size: 0.75rem;
        color: var(--color-muted);
        text-transform: uppercase;
        letter-spacing: 0.05em;
        border-bottom: 1px solid var(--color-border);
    }
    .env-table td {
        padding: 8px 12px;
        border-bottom: 1px solid var(--color-border);
        font-size: 0.8125rem;
    }
    .env-table tr:last-child td { border-bottom: none; }

    /* ── Function grid ── */
    .fn-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
    @media (max-width: 768px) { .fn-grid { grid-template-columns: 1fr; } }
    .fn-item {
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: var(--radius);
        padding: 12px 16px;
    }
    .fn-item p { font-size: 0.8125rem; color: var(--color-muted); margin-top: 4px; }

    /* ── Footer ── */
    footer {
        border-top: 1px solid var(--color-border);
        padding: 16px 24px;
        text-align: center;
        font-size: 0.75rem;
        color: var(--color-muted);
        display: flex;
        justify-content: center;
        gap: 8px;
    }
    </style>
</head>
<body>
    <header>
        <nav>
            <a href="/" class="logo">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 100" width="90" height="30">
                    <text x="50%" y="55%" text-anchor="middle" dominant-baseline="middle"
                        style="font-family:system-ui,-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;font-weight:800;font-size:64px;letter-spacing:-2px">
                        <tspan fill="#B7472A">Ox</tspan><tspan fill="#777BB4">PHP</tspan>
                    </text>
                </svg>
            </a>
            <div class="nav-links">
                <a href="https://github.com/oxphp/oxphp/tree/main/docs" class="nav-link" target="_blank">Docs</a>
                <a href="https://github.com/oxphp/oxphp" class="nav-link" target="_blank">GitHub</a>
            </div>
            <div class="meta"><?= $server_sw ?></div>
        </nav>
    </header>

    <main>
        <div class="container">

        <!-- ── Hero ── -->

        <div class="hero">
            <h1><span class="ox">Ox</span><span class="php">PHP</span></h1>
            <p class="tagline">Asynchronous PHP application server written in Rust.<br>Replaces nginx&nbsp;+&nbsp;PHP&#8209;FPM with a single binary.</p>
            <div class="links">
                <a href="https://github.com/oxphp/oxphp/tree/main/docs" target="_blank" class="btn-primary">Documentation</a>
                <a href="https://github.com/oxphp/oxphp" target="_blank" class="btn-outline">GitHub</a>
            </div>
        </div>

        <!-- ── Features ── -->

        <h2 class="section-title">Features</h2>

        <div class="features-grid">
            <div class="feature-card">
                <div class="icon">&#x26A1;</div>
                <h3>Native PHP via Custom SAPI</h3>
                <p>Executes PHP natively through a custom <code>sapi_module_struct</code> &mdash; no CGI, no FastCGI, no external process. Full ZTS thread safety with OPcache + JIT.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F9F5;</div>
                <h3>Multi-Threaded Worker Pool</h3>
                <p>Dedicated OS thread per PHP worker with ZTS isolation. Static (<code>PHP_WORKERS=N</code>) or dynamic (<code>MIN:MAX</code>) scaling with automatic dead worker respawning.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F310;</div>
                <h3>Modern HTTP Stack</h3>
                <p>Built on Hyper + Tokio for async I/O. HTTP/1.1 with keep-alive, Brotli compression, native TLS via rustls, and configurable timeouts.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F4CA;</div>
                <h3>Built-in Observability</h3>
                <p>Prometheus metrics at <code>/metrics</code>, health checks at <code>/health</code>, structured JSON access logging, and auto-generated <code>X-Request-ID</code> headers.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F6E1;</div>
                <h3>Production Ready</h3>
                <p>Per-IP rate limiting, cooperative execution deadlines, graceful shutdown with connection draining, custom error pages, and bounded request queues with backpressure.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F500;</div>
                <h3>Flexible Routing</h3>
                <p>Three modes: <strong>Traditional</strong> (direct file mapping), <strong>Framework</strong> (front controller), and <strong>SPA</strong> (HTML5 history). Static file serving with MIME detection and caching.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F4E1;</div>
                <h3>SSE &amp; Streaming</h3>
                <p>Server-Sent Events and chunked transfer encoding with real-time flush. Use <code>oxphp_stream_flush()</code> to push data as it becomes available.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F9E9;</div>
                <h3>Plugin System</h3>
                <p>Typed event dispatcher with priority ordering. Core features (rate limiting, metrics, compression) are implemented as plugins using the same API available to extensions.</p>
            </div>
            <div class="feature-card">
                <div class="icon">&#x1F680;</div>
                <h3>High Performance</h3>
                <p>mimalloc allocator, zero-clone hot path, lock-free response slot pool, pre-allocated buffers. Benchmarked at ~32.7k req/s &mdash; outperforming nginx&nbsp;+&nbsp;PHP&#8209;FPM.</p>
            </div>
        </div>

        <!-- ── Quick Start ── -->

        <h2 class="section-title">Quick Start</h2>

        <div class="card">
            <div class="card-header">Dockerfile</div>
            <div class="card-body">
                <div class="code-block">
                    <button class="copy-btn" onclick="copyBlock(this)">Copy</button>
                    <pre><span class="kw">FROM</span> <span class="str">ghcr.io/oxphp/oxphp:nightly</span>

<span class="kw">COPY</span> --chown=www-data:www-data . /var/www/html</pre>
                </div>
                <p style="margin-top: 12px; font-size: 0.8125rem; color: var(--color-muted)">
                    Place your PHP application in the build context. The document root is <code>/var/www/html/public</code>.
                </p>
            </div>
        </div>

        <div class="card">
            <div class="card-header">Build &amp; Run</div>
            <div class="card-body">
                <div class="code-block">
                    <button class="copy-btn" onclick="copyBlock(this)">Copy</button>
                    <pre><span class="cmt"># Build your image</span>
docker build -t my-app ghcr.io/oxphp/oxphp:nightly

<span class="cmt"># Run with defaults (auto-scaled workers, port 8080)</span>
docker run -p 8080:8080 my-app

<span class="cmt"># Run with custom configuration</span>
docker run -p 8080:8080 \
  -e PHP_WORKERS=8 \
  -e RATE_LIMIT=100 \
  -e INDEX_FILE=index.php \
  my-app</pre>
                </div>
            </div>
        </div>

        <div class="card">
            <div class="card-header">Example project structure</div>
            <div class="card-body">
                <div class="code-block">
<pre>my-app/
├── Dockerfile
├── public/          <span class="cmt"># &larr; DOCUMENT_ROOT</span>
│   ├── index.php    <span class="cmt"># front controller (framework mode)</span>
│   └── assets/      <span class="cmt"># static files (served directly)</span>
├── app/             <span class="cmt"># application code (outside doc root)</span>
│   ├── routes.php
│   └── ...
└── preload.php      <span class="cmt"># OPcache preload (optional)</span></pre>
                </div>
            </div>
        </div>

        <!-- ── Configuration ── -->

        <h2 class="section-title">Configuration</h2>

        <p style="font-size: 0.875rem; color: var(--color-muted); margin-bottom: 16px">
            All configuration is via environment variables. No config files needed.
        </p>

        <div class="card">
            <div class="card-header">Server</div>
            <div class="card-body">
                <table class="env-table">
                    <thead><tr><th>Variable</th><th>Default</th><th>Description</th></tr></thead>
                    <tbody>
                        <tr><td><code>LISTEN_ADDR</code></td><td><code>0.0.0.0:8080</code></td><td>HTTP listen address</td></tr>
                        <tr><td><code>DOCUMENT_ROOT</code></td><td><code>/var/www/html/public</code></td><td>Document root path</td></tr>
                        <tr><td><code>INDEX_FILE</code></td><td><em>(empty)</em></td><td>Routing mode: <code>index.php</code> = framework, <code>index.html</code> = SPA</td></tr>
                        <tr><td><code>TOKIO_WORKERS</code></td><td><code>0</code></td><td>Async I/O threads (0 = single-threaded)</td></tr>
                        <tr><td><code>COMPRESSION_ENABLED</code></td><td><code>true</code></td><td>Brotli compression</td></tr>
                    </tbody>
                </table>
            </div>
        </div>

        <div class="grid-2">
            <div class="card">
                <div class="card-header">PHP Workers</div>
                <div class="card-body">
                    <table class="env-table">
                        <thead><tr><th>Variable</th><th>Default</th></tr></thead>
                        <tbody>
                            <tr><td><code>PHP_WORKERS</code></td><td><code>0</code> (auto: cpu&times;2)</td></tr>
                            <tr><td><code>QUEUE_CAPACITY</code></td><td>workers &times; 128</td></tr>
                            <tr><td><code>PHP_WORKERS_IDLE_SECONDS</code></td><td><code>30</code></td></tr>
                        </tbody>
                    </table>
                    <p style="margin-top: 12px; font-size: 0.8125rem; color: var(--color-muted)">
                        <code>PHP_WORKERS=8</code> (static) or <code>PHP_WORKERS=2:16</code> (dynamic scaling).
                    </p>
                </div>
            </div>
            <div class="card">
                <div class="card-header">Worker Mode</div>
                <div class="card-body">
                    <table class="env-table">
                        <thead><tr><th>Variable</th><th>Default</th></tr></thead>
                        <tbody>
                            <tr><td><code>WORKER_FILE</code></td><td><em>(none)</em></td></tr>
                            <tr><td><code>WORKER_MAX_REQUESTS</code></td><td><code>0</code> (unlimited)</td></tr>
                            <tr><td><code>WORKER_MAX_MEMORY_MIB</code></td><td><code>0</code> (unlimited)</td></tr>
                        </tbody>
                    </table>
                    <p style="margin-top: 12px; font-size: 0.8125rem; color: var(--color-muted)">
                        Set <code>WORKER_FILE=../worker.php</code> for persistent PHP with soft reset between requests.
                    </p>
                </div>
            </div>
            <div class="card">
                <div class="card-header">Timeouts</div>
                <div class="card-body">
                    <table class="env-table">
                        <thead><tr><th>Variable</th><th>Default</th></tr></thead>
                        <tbody>
                            <tr><td><code>REQUEST_TIMEOUT_SECONDS</code></td><td><code>120</code></td></tr>
                            <tr><td><code>HEADER_TIMEOUT_SECONDS</code></td><td><code>5</code></td></tr>
                            <tr><td><code>IDLE_TIMEOUT_SECONDS</code></td><td><code>60</code></td></tr>
                            <tr><td><code>DRAIN_TIMEOUT_SECONDS</code></td><td><code>30</code></td></tr>
                        </tbody>
                    </table>
                </div>
            </div>
        </div>

        <div class="grid-2">
            <div class="card">
                <div class="card-header">Security</div>
                <div class="card-body">
                    <table class="env-table">
                        <thead><tr><th>Variable</th><th>Default</th></tr></thead>
                        <tbody>
                            <tr><td><code>RATE_LIMIT</code></td><td><code>0</code> (off)</td></tr>
                            <tr><td><code>RATE_WINDOW_SECONDS</code></td><td><code>60</code> sec</td></tr>
                            <tr><td><code>TLS_CERT</code></td><td><em>(none)</em></td></tr>
                            <tr><td><code>TLS_KEY</code></td><td><em>(none)</em></td></tr>
                        </tbody>
                    </table>
                </div>
            </div>
            <div class="card">
                <div class="card-header">Observability</div>
                <div class="card-body">
                    <table class="env-table">
                        <thead><tr><th>Variable</th><th>Default</th></tr></thead>
                        <tbody>
                            <tr><td><code>INTERNAL_ADDR</code></td><td><em>(none)</em></td></tr>
                            <tr><td><code>ACCESS_LOG</code></td><td><code>true</code></td></tr>
                            <tr><td><code>LOG_LEVEL</code></td><td><code>info</code></td></tr>
                            <tr><td><code>ERROR_PAGES_DIR</code></td><td><em>(none)</em></td></tr>
                        </tbody>
                    </table>
                    <p style="margin-top: 12px; font-size: 0.8125rem; color: var(--color-muted)">
                        Set <code>INTERNAL_ADDR=0.0.0.0:9090</code> to enable <code>/health</code>, <code>/metrics</code>, <code>/config</code>.
                    </p>
                </div>
            </div>
        </div>

        <!-- ── PHP Functions ── -->

        <h2 class="section-title">PHP Functions</h2>

        <p style="font-size: 0.875rem; color: var(--color-muted); margin-bottom: 16px">
            OxPHP exposes these functions to PHP scripts via the <code>oxphp_sapi</code> extension.
        </p>

        <div class="fn-grid">
            <div class="fn-item">
                <code>oxphp_request_id(): string</code>
                <p>Returns the 16-char hex request ID for the current request.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_worker_id(): int</code>
                <p>Returns the zero-based worker thread index.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_server_info(): array</code>
                <p>Returns SAPI name, version, worker ID, request timestamp, and worker mode flag.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_finish_request(): bool</code>
                <p>Sends response immediately, continues background execution.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_stream_flush(): bool</code>
                <p>Activates streaming mode and flushes output as a chunk.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_is_worker(): bool</code>
                <p>Returns true if running in worker mode, false in traditional mode.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_is_streaming(): bool</code>
                <p>Returns true if chunked/SSE streaming mode is active.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_request_heartbeat(int $time = 10): bool</code>
                <p>Extends the execution deadline by N seconds.</p>
            </div>
            <div class="fn-item">
                <code>oxphp_worker(callable $handler): bool</code>
                <p>Enters persistent worker loop. Calls handler per request with soft reset.</p>
            </div>
        </div>

        <!-- ── Documentation ── -->

        <h2 class="section-title">Documentation</h2>

        <div class="card">
            <div class="card-body">
                <table class="env-table">
                    <thead><tr><th>Section</th><th>Content</th></tr></thead>
                    <tbody>
                        <tr><td><a href="https://github.com/oxphp/oxphp/tree/main/docs/en/getting-started" target="_blank">Getting Started</a></td><td>Installation, Docker setup, quick start guide</td></tr>
                        <tr><td><a href="https://github.com/oxphp/oxphp/tree/main/docs/en/architecture" target="_blank">Architecture</a></td><td>Request lifecycle, worker pool, SAPI bridge, event system</td></tr>
                        <tr><td><a href="https://github.com/oxphp/oxphp/tree/main/docs/en/features" target="_blank">Features</a></td><td>Routing, TLS, compression, rate limiting, error pages, timeouts</td></tr>
                        <tr><td><a href="https://github.com/oxphp/oxphp/tree/main/docs/en/php" target="_blank">PHP Integration</a></td><td>Custom functions, superglobals, OPcache configuration</td></tr>
                        <tr><td><a href="https://github.com/oxphp/oxphp/tree/main/docs/en/operations" target="_blank">Operations</a></td><td>Configuration reference, health checks, metrics, graceful shutdown</td></tr>
                    </tbody>
                </table>
                <p style="margin-top: 16px; font-size: 0.8125rem; color: var(--color-muted)">
                    Available in:
                    <a href="https://github.com/oxphp/oxphp/tree/main/docs/en" target="_blank">English</a> &middot;
                    <a href="https://github.com/oxphp/oxphp/tree/main/docs/ru" target="_blank">Русский</a> &middot;
                    <a href="https://github.com/oxphp/oxphp/tree/main/docs/be" target="_blank">Беларуская</a> &middot;
                    <a href="https://github.com/oxphp/oxphp/tree/main/docs/zh" target="_blank">中文</a>
                </p>
            </div>
        </div>

        </div><!-- /.container -->
    </main>

    <footer>
        <span style="font-family: var(--mono)"><?= $server_sw ?></span>
        <span>&middot; PHP <?= $php_ver ?> (<?= $sapi ?>)</span>
    </footer>

    <script>
    function copyBlock(btn) {
        const pre = btn.parentElement.querySelector('pre');
        navigator.clipboard.writeText(pre.textContent).then(() => {
            btn.textContent = 'Copied!';
            setTimeout(() => btn.textContent = 'Copy', 1500);
        });
    }
    </script>
</body>
</html>
