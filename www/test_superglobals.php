<?php
header('Content-Type: application/json');
echo json_encode([
    'method' => $_SERVER['REQUEST_METHOD'],
    'uri' => $_SERVER['REQUEST_URI'],
    'query_string' => $_SERVER['QUERY_STRING'] ?? '',
    'get' => $_GET,
    'post' => $_POST,
    'cookies' => $_COOKIE,
    'input' => file_get_contents('php://input'),
    'server_software' => $_SERVER['SERVER_SOFTWARE'] ?? '',
    'remote_addr' => $_SERVER['REMOTE_ADDR'] ?? '',
    'http_host' => $_SERVER['HTTP_HOST'] ?? '',
    'script_name' => $_SERVER['SCRIPT_NAME'] ?? '',
    'script_filename' => $_SERVER['SCRIPT_FILENAME'] ?? '',
    'document_root' => $_SERVER['DOCUMENT_ROOT'] ?? '',
    'server_protocol' => $_SERVER['SERVER_PROTOCOL'] ?? '',
    'gateway_interface' => $_SERVER['GATEWAY_INTERFACE'] ?? '',
], JSON_PRETTY_PRINT);
