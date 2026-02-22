<?php

/**
 * SSE (Server-Sent Events) endpoint.
 *
 * Streams real-time events to the browser via EventSource.
 * Query params:
 *   ?count=N  — number of events to send (default: 10, max: 100)
 *   ?delay=N  — milliseconds between events (default: 1000, max: 5000)
 */

$count = min(max((int)($_GET['count'] ?? 10), 1), 100);
$delay = min(max((int)($_GET['delay'] ?? 1000), 100), 5000);

header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');
header('X-Accel-Buffering: no');

for ($i = 0; $i < $count; $i++) {
    $data = json_encode([
        'counter' => $i + 1,
        'total'   => $count,
        'time'    => date('H:i:s'),
        'worker'  => oxphp_worker_id(),
    ]);

    echo "id: {$i}\n";
    echo "data: {$data}\n\n";
    oxphp_stream_flush();

    if ($i < $count - 1) {
        usleep($delay * 1000);
    }
}

// Send a final "done" event so the client knows to close
echo "event: done\ndata: {}\n\n";
oxphp_stream_flush();
