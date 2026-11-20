<?php

declare(strict_types=1);

// Parks a fiber in a read for its reply on a raw Redis connection the worker
// shares, the way a userland client such as Predis does after sending a command.
// A userland client has no guarded entry point in front of its writes, so the
// socket level is the only thing between another fiber's command and this
// exchange.
//
// Both edges of the parking are published, keyed by the caller's `key`: a flag
// that only says "it parked at some point" stays set after the read is over, and
// the tests need to know whether it was still parked when they looked.
$key = preg_replace('/[^a-z0-9]/', '', (string) ($_GET['key'] ?? ''));
$hold = (float) ($_GET['hold'] ?? 1);

try {
    $host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
    $sock = stream_socket_client("tcp://{$host}:6379", $errno, $errstr, 3.0);
    if ($sock === false) {
        echo "raw-hold-connect-failed:{$errstr} ({$errno})";
        return;
    }
    // Past the pop's own timeout, so the pop's empty answer is what ends the read.
    stream_set_timeout($sock, 10);
    $sharedState["rawwrite_sock_{$key}"] = $sock;

    fwrite($sock, "BLPOP oxphp:rawwrite:never:{$key} {$hold}\r\n");
    $sharedState["rawwrite_parked_{$key}"] = microtime(true);

    $reply = fread($sock, 1024);
    $sharedState["rawwrite_done_{$key}"] = microtime(true);
    echo 'raw-hold-done:' . json_encode($reply);
} catch (\Throwable $e) {
    echo 'raw-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
