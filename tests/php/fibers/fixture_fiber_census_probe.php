<?php

declare(strict_types=1);

/**
 * Inner request for the request-fiber census test.
 *
 * Reports what oxphp_worker_request_fibers_active says about the worker while
 * this request is running on it. The outer request is parked on the read of
 * this one, so the worker is carrying two fibers, and this one was admitted by
 * the event-loop tick rather than by the blocking branch of the serve loop.
 *
 * The scrape is taken over a NON-blocking socket and polled by hand. Every way
 * PHP has of waiting is hooked in this profile, and a hooked wait parks the
 * fiber — which returns control to the serve loop, whose next turn publishes
 * the census afresh. Publishing it is exactly what the tick is here to be shown
 * doing on its own, so the reading has to be taken without ever handing control
 * back. The hook only takes over a read that was going to wait, so a
 * non-blocking stream is served without a suspension and the whole exchange
 * stays inside the tick.
 */

header('Content-Type: application/json');

$id = OxPHP\Server\Worker::current()->id();

$sock = @stream_socket_client('tcp://127.0.0.1:9090', $errno, $errstr, 3.0);
if ($sock === false) {
    echo json_encode(['error' => "connect failed: $errstr ($errno)"]);

    return;
}

stream_set_blocking($sock, false);
fwrite($sock, "GET /metrics HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");

// Poll rather than wait: see above. Bounded by wall-clock rather than by a
// number of turns, so a slow answer ends the loop instead of the test.
$raw = '';
$deadline = microtime(true) + 3.0;
while (microtime(true) < $deadline) {
    $chunk = fread($sock, 65536);
    if ($chunk === false || ($chunk === '' && feof($sock))) {
        break;
    }
    $raw .= $chunk;
}
fclose($sock);

preg_match_all(
    '/^oxphp_worker_request_fibers_active\{worker="(\d+)"\} ([\d.e+-]+)$/mi',
    $raw,
    $matches,
    PREG_SET_ORDER
);

$series = [];
foreach ($matches as $m) {
    $series[$m[1]] = (float) $m[2];
}

// The label is the stats slot, which a worker takes by its id modulo the number
// of slots — the same arithmetic the server does when it hands a worker thread
// its slot, so a recycled worker with an id past the pool size still resolves.
$label = $series === [] ? null : (string) ($id % count($series));

echo json_encode([
    'worker' => $id,
    'label' => $label,
    'own' => $label === null ? null : ($series[$label] ?? null),
    'series' => $series,
    'scraped' => $raw !== '',
]);
