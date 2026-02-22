<?php

if ($_SERVER['REQUEST_METHOD'] !== 'QUERY') {
    json_response(405, [
        'error'  => 'Method Not Allowed',
        'detail' => 'This endpoint requires the QUERY method (draft-ietf-httpbis-safe-method-w-body).',
        'hint'   => 'curl -X QUERY -H "Content-Type: application/json" -d \'{"q":"test"}\' URL',
    ]);
    return;
}

$body = file_get_contents('php://input');
$ct   = $_SERVER['CONTENT_TYPE'] ?? '';
$data = str_contains($ct, 'json') ? json_decode($body, true) : null;

json_response(200, [
    'method'       => 'QUERY',
    'content_type' => $ct,
    'raw_body'     => $body,
    'parsed'       => $data,
    'query_string' => $_SERVER['QUERY_STRING'] ?? '',
    'get'          => $_GET,
    'note'         => 'QUERY is safe and idempotent, like GET but with a request body.',
]);
