<?php
// Server-sent events until the client goes, in the loop docs/features/sse.md
// recommends: one event every 50 ms, ending on connection_aborted(). ?n= caps
// the events (default 200). ?ms= runs that long before anything is sent, and
// with ?n=0 that is an ordinary handler answering once — a readiness probe at
// ?ms=0. Each stream records on its way out whether it saw its client leave,
// from a shutdown function, which runs whether the loop exits on its own check
// or the flush that found the connection gone unwinds it.
$n = isset($_GET['n']) ? max(0, min((int) $_GET['n'], 1000)) : 200;
usleep(max(0, min(isset($_GET['ms']) ? (int) $_GET['ms'] : 0, 30000)) * 1000);
if ($n > 0) {
    register_shutdown_function(function () {
        file_put_contents('/tmp/streams', (connection_aborted() ? 'aborted' : 'complete') . "\n", FILE_APPEND);
    });
}
header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');
for ($i = 0; $i < $n && !connection_aborted(); $i++) {
    echo "data: {$i}\n\n";
    oxphp_stream_flush();
    usleep(50000);
}
