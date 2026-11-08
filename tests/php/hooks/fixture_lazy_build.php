<?php

declare(strict_types=1);

// A request building a shared service the way a lazy container does: it marks
// the service as being built, waits, and marks it built. With role=probe it
// answers instead with when it started and what it found, which is what a
// request overlapping the build gets to see.
//
// $sharedState is the worker's own array, so it plays the container. The times
// come from one monotonic clock on one worker, so the script driving this can
// compare them directly.
header('Content-Type: application/json');

if (($_GET['role'] ?? 'probe') === 'probe') {
    echo json_encode(['started_at' => hrtime(true), 'seen' => $sharedState['lazy_build'] ?? null]);
    return;
}

require_once __DIR__ . '/net_helper.php';

// Work the request does before its first wait: the window under test opens at
// that wait, not when the request starts.
$busyUntil = hrtime(true) + (int) ($_GET['busy_ms'] ?? 0) * 1000000;
while (hrtime(true) < $busyUntil) {
}

// Ahead of everything that can suspend, the name lookup and the connect
// included: the script measures from here, and the window under test opens at
// the request's first wait.
$sharedState['lazy_build'] = ['building' => true, 'parked_at' => hrtime(true), 'ended_at' => null];

$ms = (int) ($_GET['park_ms'] ?? 0);
if (($_GET['via'] ?? 'socket') === 'sleep') {
    oxphp_sleep($ms / 1000);
} else {
    // The peer accepts this port and never says anything, so the read lasts
    // exactly as long as the stream's timeout.
    $sock = stream_socket_client('tcp://' . net_peer_ip() . ':4436', $errno, $errstr, 3.0);
    if ($sock === false) {
        http_response_code(500);
        echo json_encode(['error' => "connect failed: {$errstr}"]);
        return;
    }
    stream_set_timeout($sock, 0, $ms * 1000);
    fread($sock, 1);
    fclose($sock);
}

$sharedState['lazy_build']['building'] = false;
$sharedState['lazy_build']['ended_at'] = hrtime(true);
echo json_encode($sharedState['lazy_build']);
