<?php
/**
 * OxPHP — Default page.
 *
 * Serves "/" as the landing page, "/robots.txt", and 404 for any other
 * path. Works in both classic per-request mode and worker mode
 * (WORKER_MODE_ENABLED=true): the request logic lives in a handler the
 * server invokes once per request.
 * CLI clients (curl, wget, httpie) get clean plain text.
 * Browsers get a styled HTML page.
 */

$handle = static function (): void {
    $req  = oxphp_http_request();
    $path = $req->path();
    $ua   = $req->header('User-Agent') ?? '';

    // ── /robots.txt ───────────────────────────────────────────────
    if ($path === '/robots.txt') {
        header('Content-Type: text/plain; charset=utf-8');
        echo "User-agent: *\nDisallow:\n";
        return;
    }

    $server  = $_SERVER['SERVER_SOFTWARE'] ?? 'OxPHP';
    $php_ver = PHP_VERSION;
    $sapi    = PHP_SAPI;
    $time    = gmdate('Y-m-d H:i:s') . ' UTC';

    $not_found = $path !== '/';
    if ($not_found) {
        http_response_code(404);
    }

    // ── CLI clients → plain text ──────────────────────────────────
    if (preg_match('/^(curl|Wget|HTTPie|fetch|http)/i', $ua)) {
        header('Content-Type: text/plain; charset=utf-8');

        if ($not_found) {
            echo "\n  404 Not Found\n\n  {$path}\n\n";
            return;
        }

        echo <<<TEXT

      OxPHP is running.

      server   {$server}
      php      {$php_ver} ({$sapi})
      time     {$time}

      https://github.com/oxphp/oxphp

    TEXT;
        echo "\n";
        return;
    }

    // ── Browsers → HTML ───────────────────────────────────────────
    $esc   = htmlspecialchars($server, ENT_QUOTES | ENT_HTML5, 'UTF-8');
    $epath = htmlspecialchars($path, ENT_QUOTES | ENT_HTML5, 'UTF-8');
    ?>
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta name="description" content="OxPHP — an asynchronous PHP application server written in Rust, replacing nginx and PHP-FPM with a single binary.">
<title><?= $not_found ? '404 — OxPHP' : 'OxPHP' ?></title>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }

:root {
    --bg: #0f1117;
    --text: #e2e4e9;
    --subtitle: #9296a8;
    --label: #7e8294;
    --value: #b0b5cc;
    --ox: #B7472A;
    --php: #777BB4;
    --glow-ox: rgba(183,71,42,0.15);
    --glow-php: rgba(119,123,180,0.08);
    --link: #7e8294;
    --link-hover: #777BB4;
    color-scheme: dark;
}

@media (prefers-color-scheme: light) {
    :root {
        --bg: #f5f5f7;
        --text: #1d1d1f;
        --subtitle: #6e6e73;
        --label: #6e6e73;
        --value: #232323;
        --ox: #A33D22;
        --php: #5b5ea6;
        --glow-ox: rgba(163,61,34,0.08);
        --glow-php: rgba(91,94,166,0.05);
        --link: #6e6e73;
        --link-hover: #5b5ea6;
        color-scheme: light;
    }
}

body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
    background: var(--bg);
    color: var(--text);
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    -webkit-font-smoothing: antialiased;
}

.page { text-align: center; padding: 48px 24px; }

h1 {
    font-size: clamp(3rem, 8vw, 5rem);
    font-weight: 800;
    letter-spacing: -2px;
    line-height: 1;
    margin-bottom: 12px;
    text-shadow: 0 0 60px var(--glow-ox), 0 0 120px var(--glow-php);
}

.ox  { color: var(--ox); }
.php { color: var(--php); }

.subtitle {
    font-size: 1.25rem;
    color: var(--subtitle);
    margin-bottom: 40px;
}

.cursor {
    display: inline-block;
    width: 2px;
    height: 1.15em;
    background: var(--php);
    margin-left: 2px;
    vertical-align: text-bottom;
    animation: blink 1s step-end infinite;
}
@keyframes blink { 50% { opacity: 0; } }

.info {
    font-family: 'SF Mono', 'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace;
    font-size: 0.875rem;
    line-height: 2.2;
    display: inline-block;
    text-align: left;
}

.info .row { display: flex; gap: 16px; }
.info .label { color: var(--label); min-width: 64px; text-align: right; }
.info .value { color: var(--value); font-weight: 600; }

.link {
    margin-top: 40px;
}
.link a {
    color: var(--link);
    text-decoration: none;
    font-family: 'SF Mono', 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 0.8125rem;
    transition: color 0.2s;
}
.link a:hover { color: var(--link-hover); }
</style>
</head>
<body>
<main class="page">
<?php if ($not_found): ?>
    <h1><span class="ox">4</span><span class="php">04</span></h1>
    <p class="subtitle">not found</p>
    <div class="info">
        <div class="row"><span class="label">path</span> <span class="value"><?= $epath ?></span></div>
    </div>
    <div class="link">
        <a href="/">back to start</a>
    </div>
<?php else: ?>
    <h1><span class="ox">Ox</span><span class="php">PHP</span></h1>
    <p class="subtitle">is running<span class="cursor"></span></p>
    <div class="info">
        <div class="row"><span class="label">server</span> <span class="value"><?= $esc ?></span></div>
        <div class="row"><span class="label">php</span> <span class="value"><?= $php_ver ?> (<?= $sapi ?>)</span></div>
        <div class="row"><span class="label">time</span> <span class="value"><?= $time ?></span></div>
    </div>
    <div class="link">
        <a href="https://github.com/oxphp/oxphp">github.com/oxphp/oxphp</a>
    </div>
<?php endif; ?>
</main>
</body>
</html>
<?php
};

// ── Dispatch: worker mode loops internally, classic mode runs once ──

if (oxphp_is_worker()) {
    oxphp_worker($handle);
} else {
    $handle();
}
