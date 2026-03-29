<?php
/**
 * OxPHP — Default page.
 *
 * Confirms the server is running and PHP is operational.
 * CLI clients (curl, wget, httpie) get clean plain text.
 * Browsers get a styled HTML page.
 */

$server   = $_SERVER['SERVER_SOFTWARE'] ?? 'OxPHP';
$php_ver  = PHP_VERSION;
$sapi     = PHP_SAPI;
$time     = gmdate('Y-m-d H:i:s') . ' UTC';
$ua       = $_SERVER['HTTP_USER_AGENT'] ?? '';

// ── CLI clients → plain text ──────────────────────────────────────

if (preg_match('/^(curl|Wget|HTTPie|fetch|http)/i', $ua)) {
    header('Content-Type: text/plain; charset=utf-8');

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

// ── Browsers → HTML ───────────────────────────────────────────────

$esc = htmlspecialchars($server, ENT_QUOTES | ENT_HTML5, 'UTF-8');
?>
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>OxPHP</title>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }

:root {
    --bg: #0f1117;
    --text: #e2e4e9;
    --subtitle: #6b6f82;
    --label: #4a4e5e;
    --value: #8b8fa3;
    --ox: #B7472A;
    --php: #777BB4;
    --glow-ox: rgba(183,71,42,0.15);
    --glow-php: rgba(119,123,180,0.08);
    --link: #4a4e5e;
    --link-hover: #777BB4;
    color-scheme: dark;
}

@media (prefers-color-scheme: light) {
    :root {
        --bg: #f5f5f7;
        --text: #1d1d1f;
        --subtitle: #86868b;
        --label: #aeaeb2;
        --value: #48484a;
        --ox: #A33D22;
        --php: #5b5ea6;
        --glow-ox: rgba(163,61,34,0.08);
        --glow-php: rgba(91,94,166,0.05);
        --link: #aeaeb2;
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
.info .value { color: var(--value); }

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
<div class="page">
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
</div>
</body>
</html>
