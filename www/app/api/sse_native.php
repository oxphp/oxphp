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

// A stream outlives max_execution_time, which is 30 s unless the PHP
// configuration sets another value.
set_time_limit(0);

// Keep running when the client leaves, so that the connection_aborted() check
// below sees it. Without this call the flush() that finds the client gone does
// not return, as under php-fpm, and the check is never reached.
ignore_user_abort(true);

// Disable PHP output buffering so echo goes directly to ub_write
while (ob_get_level()) {
    ob_end_flush();
}

for ($i = 0; $i < $count; $i++) {
    // A client that left is found out by the previous pass's flush().
    if (connection_aborted()) {
        break;
    }

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
