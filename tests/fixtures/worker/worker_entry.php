<?php
static $requestCount = 0;
static $previousHeaders = [];

oxphp_worker(function () use (&$requestCount, &$previousHeaders) {
    $requestCount++;
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
