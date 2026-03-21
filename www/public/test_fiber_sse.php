<?php
$count = min(max((int)($_GET['count'] ?? 5), 1), 100);
$delay = min(max((int)($_GET['delay'] ?? 500), 10), 5000) / 1000.0;

header('Content-Type: text/event-stream');
header('Cache-Control: no-cache');

for ($i = 0; $i < $count; $i++) {
    echo "data: " . json_encode(['counter' => $i, 'worker' => oxphp_worker_id()]) . "\n\n";
    oxphp_stream_flush();
    if ($i < $count - 1) {
        oxphp_sleep($delay);
    }
}
echo "event: done\ndata: {}\n\n";
oxphp_stream_flush();
