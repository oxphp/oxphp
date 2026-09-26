<?php

declare(strict_types=1);

// A persistent constructor to a data source that accepts the connection and never
// answers — a replica gone quiet, a second database behind a dead proxy. The test
// holds a listening socket it never accepts on: the kernel completes the TCP
// handshake from the listen backlog, so mysqlnd connects and then waits for a
// server greeting that does not come, for as long as its own read timeout allows,
// which out of the box is a day. The test ends it by closing the listener, which
// resets the connection under it.
//
// Both edges of the constructor are published, so the test can read, at the
// moment it measures, that this one was still inside it rather than that it had
// been there at some point.
$sharedState['black_hole_entered'] = microtime(true);
unset($sharedState['black_hole_left']);

try {
    new PDO((string) ($sharedState['black_hole_dsn'] ?? ''), 'nobody', 'nothing', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => 'black-hole-' . bin2hex(random_bytes(4)),
    ]);
    echo "black-hole-built\n";
} catch (\Throwable $e) {
    echo 'black-hole-failed:' . str_replace("\n", ' ', $e->getMessage()) . "\n";
} finally {
    $sharedState['black_hole_left'] = microtime(true);
}
