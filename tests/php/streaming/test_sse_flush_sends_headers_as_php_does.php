<?php

declare(strict_types=1);

require_once __DIR__ . '/stream_probe.php';

// Runner-side test: an event stream's Content-Type puts the response in
// streaming mode without sending anything, and PHP's own flush() then sends
// its headers and says so, as under PHP-FPM.

header('Content-Type: text/event-stream');
$before = headers_sent();
flush();
$after = headers_sent();

stream_probe_report('sse_flush_sends_headers_as_php_does', [
    'headers_sent() is false before the first flush' => $before === false,
    'and true once flush() has sent them' => $after === true,
]);
flush();
