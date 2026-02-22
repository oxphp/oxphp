<?php

/**
 * SSE endpoint using only native PHP functions (no oxphp_* calls).
 *
 * Streaming activates automatically when Content-Type: text/event-stream
 * is detected in the SAPI header handler. Standard flush() triggers
 * chunk delivery via sapi_flush().
 *
 * Query params:
 *   ?count=N  — number of events to send (default: 10, max: 100)
 *   ?delay=N  — milliseconds between events (default: 1000, max: 5000)
 */

$count = min(max((int)($_GET['count'] ?? 10), 1), 100);
$delay = min(max((int)($_GET['delay'] ?? 1000), 100), 5000);

header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');
header('X-Accel-Buffering: no');

// Disable PHP output buffering so echo goes directly to ub_write
while (ob_get_level()) {
    ob_end_flush();
}

for ($i = 0; $i < $count; $i++) {
    $data = json_encode([
        'counter' => $i + 1,
        'total'   => $count,
        'time'    => date('H:i:s'),
        'worker'  => function_exists('oxphp_worker_id') ? oxphp_worker_id() : -1,
        'mode'    => 'native',
    ]);

    echo "id: {$i}\n";
    echo "data: {$data}\n\n";
    flush();

    if ($i < $count - 1) {
        usleep($delay * 1000);
    }
}

echo "event: done\ndata: {}\n\n";
flush();
