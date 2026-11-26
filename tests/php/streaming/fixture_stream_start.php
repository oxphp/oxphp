<?php

declare(strict_types=1);

require_once __DIR__ . '/stream_probe.php';

// Inner request for streaming/test_stream_start_sends_the_first_write_while_the_script_runs.
//
// Writes the way an application server adapter runs a streamed response's
// callback: under ob_implicit_flush() and a pass-through output buffer of chunk
// size 1, so each write reaches the server and is flushed as it is made.
// ?start=1 starts the stream with oxphp_stream_start(), where it exists, and
// sends text/csv; ?sse=1 sends text/event-stream, which starts it by itself.
// After its first line it waits up to three seconds for the client to create
// the marker ?token= names, and its last line says whether the client did.

if (isset($_GET['sse'])) {
    header('Content-Type: text/event-stream');
} else {
    if (function_exists('oxphp_stream_start')) {
        oxphp_stream_start();
    }
    header('Content-Type: text/csv');
}
http_response_code(202);
header('X-Stream-Start: 1');

ob_implicit_flush(true);
ob_start(static fn (string $buffer): string => $buffer, 1);

echo "a\n";
$acked = stream_probe_wait_for(stream_probe_marker((string) ($_GET['token'] ?? '')), 3.0);
echo $acked ? "b:acked\n" : "b:timeout\n";
