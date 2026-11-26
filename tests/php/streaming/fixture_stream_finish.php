<?php

declare(strict_types=1);

require_once __DIR__ . '/stream_probe.php';

// Inner request for streaming/test_stream_finish_request_ends_the_stream.
//
// ?mode=flush streams a line, then writes a second one without flushing it;
// ?mode=start starts a stream with oxphp_stream_start() and writes both lines
// without flushing either. Either way it then calls oxphp_finish_request() and
// oxphp_stream_start() after it, and waits up to three seconds for the client
// to create the marker ?token= names once the response has ended. It records
// in "<marker>.state" whether the client did, and what that late
// oxphp_stream_start() returned — the client has nothing left to read from
// this request.

$marker = stream_probe_marker((string) ($_GET['token'] ?? ''));

if (($_GET['mode'] ?? '') === 'start') {
    oxphp_stream_start();
    echo "a\n";
} else {
    echo "a\n";
    oxphp_stream_flush();
}
echo "b\n";
oxphp_finish_request();
$lateStart = oxphp_stream_start();

$acked = stream_probe_wait_for($marker, 3.0);
file_put_contents("$marker.state.tmp", json_encode(['acked' => $acked, 'late_start' => $lateStart]));
rename("$marker.state.tmp", "$marker.state");
