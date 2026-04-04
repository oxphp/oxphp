<?php
static $requestCount = 0;
static $previousHeaders = [];

oxphp_worker(function () use (&$requestCount, &$previousHeaders) {
    $requestCount++;

    // If the request targets a test PHP file, include it directly inside
    // the worker callback so tests run in real worker context.
    $uri = parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH);
    if (preg_match('#^/tests/.+\.php$#', $uri)) {
        $testFile = $_SERVER['DOCUMENT_ROOT'] . $uri;
        if (file_exists($testFile)) {
            include $testFile;
            return;
        }
    }

    header('Content-Type: application/json');

    $action = $_GET['action'] ?? 'default';

    $response = match ($action) {
        'is_worker'       => ['is_worker' => oxphp_is_worker()],
        'state_persists'  => ['request_count' => $requestCount],
        'superglobals'    => ['get' => $_GET, 'server' => $_SERVER['REQUEST_METHOD'] ?? ''],
        'check_output'    => ['clean' => true],
        'check_headers'   => [
            'prev' => $previousHeaders,
            'current_id' => oxphp_request_id(),
        ],
        'server_info'     => oxphp_server_info(),
        default           => ['action' => $action, 'request_count' => $requestCount],
    };

    $previousHeaders = headers_list();

    echo json_encode($response, JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES);
});
