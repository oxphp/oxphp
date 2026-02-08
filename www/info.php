<?php
header('X-Custom-Header: oxphp-test');
http_response_code(201);
echo json_encode([
    'sapi' => php_sapi_name(),
    'version' => PHP_VERSION,
    'time' => time(),
    'server' => $_SERVER['SERVER_SOFTWARE'] ?? 'unknown',
]);
