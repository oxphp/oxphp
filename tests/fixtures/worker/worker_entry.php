<?php
static $requestCount = 0;
static $previousHeaders = [];

// A per-worker store for tests that need one object shared by every request the
// worker serves. That is the shape a real application has: WordPress, Laravel
// and Symfony build their database and cache clients once when the worker boots
// and hand the same ones to every request, so a client's connection is process
// state, not request state. Included test files reach it because PHP `include`
// runs in the includer's scope.
static $sharedState = [];

// Captured during the worker boot phase (before oxphp_worker enters its
// receive loop). With the request_time consistency fix these values must
// both be exactly 0.0 because no request is being processed yet. Passed
// into the closure so included test files can assert on them directly
// (PHP `include` runs in the includer's scope).
$bootInfo = [
    'request_time'       => oxphp_server_info()['request_time'],
    'request_start_time' => oxphp_http_request()->startTime(true),
];

oxphp_worker(function () use (&$requestCount, &$previousHeaders, &$sharedState, $bootInfo) {
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
