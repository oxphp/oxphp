<?php
header('Content-Type: application/json');
echo json_encode([
    'handler'     => 'framework_index',
    'request_uri' => $_SERVER['REQUEST_URI'] ?? '',
    'method'      => $_SERVER['REQUEST_METHOD'] ?? '',
]);
