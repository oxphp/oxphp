<?php

function json_response(int $status, array $data): void {
    http_response_code($status);
    header('Content-Type: application/json');
    echo json_encode($data, JSON_THROW_ON_ERROR | JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES);
}

function request_headers(): array {
    $headers = [];
    foreach ($_SERVER as $key => $value) {
        if (str_starts_with($key, 'HTTP_')) {
            $name = str_replace('_', '-', strtolower(substr($key, 5)));
            $headers[$name] = $value;
        }
    }
    if (isset($_SERVER['CONTENT_TYPE'])) {
        $headers['content-type'] = $_SERVER['CONTENT_TYPE'];
    }
    if (isset($_SERVER['CONTENT_LENGTH'])) {
        $headers['content-length'] = $_SERVER['CONTENT_LENGTH'];
    }
    return $headers;
}

function h(string $s): string {
    return htmlspecialchars($s, ENT_QUOTES | ENT_HTML5, 'UTF-8');
}

function layout(string $title, string $content): void {
    $nav_items = [
        '/'        => 'Dashboard',
        '/echo'    => 'Echo',
        '/upload'  => 'Upload',
        '/cookies' => 'Cookies',
        '/opcache'   => 'OPcache',
        '/functions' => 'Functions',
    ];

    $current = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);
    $nav_html = '';
    foreach ($nav_items as $href => $label) {
        $active = $current === $href ? ' active' : '';
        $nav_html .= "<a href=\"{$href}\" class=\"nav-link{$active}\">{$label}</a>";
    }

    $server = h($_SERVER['SERVER_SOFTWARE'] ?? 'OxPHP');
    $request_id = h($_SERVER['HTTP_X_REQUEST_ID'] ?? '-');

    $sapi = PHP_SAPI;
    $php_ver = PHP_VERSION;

    echo <<<HTML
    <!DOCTYPE html>
    <html lang="en">
    <head>
        <meta charset="UTF-8">
        <meta name="viewport" content="width=device-width, initial-scale=1.0">
        <title>{$title} — OxPHP</title>
        <link rel="stylesheet" href="/assets/css/app.css">
    </head>
    <body>
        <header>
            <nav>
                <a href="/" class="logo"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 100" width="90" height="30"><text x="50%" y="55%" text-anchor="middle" dominant-baseline="middle" style="font-family:system-ui,-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;font-weight:800;font-size:64px;letter-spacing:-2px"><tspan fill="#B7472A">Ox</tspan><tspan fill="#777BB4">PHP</tspan></text></svg></a>
                <div class="nav-links">{$nav_html}</div>
                <div class="meta mono">
                    <span title="Request ID">{$request_id}</span>
                </div>
            </nav>
        </header>
        <main>
            <h1>{$title}</h1>
            {$content}
        </main>
        <footer>
            <span class="mono">{$server}</span>
            <span>&middot; PHP {$php_ver} ({$sapi})</span>
        </footer>
        <script src="/assets/js/app.js"></script>
    </body>
    </html>
    HTML;
}
