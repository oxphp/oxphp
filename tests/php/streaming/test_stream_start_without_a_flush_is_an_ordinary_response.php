<?php

declare(strict_types=1);

require_once __DIR__ . '/stream_probe.php';

// Runner-side test: oxphp_stream_start() sends nothing, so a request that calls
// it and never flushes is answered as any other — with its own status and
// headers, and a Content-Length rather than a chunked body.

if (!function_exists('oxphp_stream_start')) {
    echo "FAIL: oxphp_stream_start() does not exist\n";

    return;
}

$started = oxphp_stream_start();
$streaming = oxphp_is_streaming();
$sent = headers_sent();
$again = oxphp_stream_start();
http_response_code(201);
header('X-Foo: 1');

stream_probe_report('stream_start_without_a_flush_is_an_ordinary_response', [
    'oxphp_stream_start() returns true' => $started === true,
    'and puts the request in streaming mode' => $streaming === true,
    'without sending its headers' => $sent === false,
    'a second call returns true too' => $again === true,
]);
