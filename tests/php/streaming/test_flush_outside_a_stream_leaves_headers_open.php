<?php

declare(strict_types=1);

require_once __DIR__ . '/stream_probe.php';

// Runner-side test: outside streaming mode a flush sends nothing — the response
// goes out whole when its script ends — so it leaves the headers open:
// headers_sent() stays false, and a header() set after it reaches the client.

flush();
$sent = headers_sent();
header('X-After-Flush: 1');

stream_probe_report('flush_outside_a_stream_leaves_headers_open', [
    'headers_sent() is false after a flush outside a stream' => $sent === false,
]);
