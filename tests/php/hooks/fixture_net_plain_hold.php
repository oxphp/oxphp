<?php

declare(strict_types=1);

require_once __DIR__ . '/net_helper.php';

// Parks a fiber in a plain read on a tcp:// connection the worker shares, to a
// peer port that accepts and never sends, so the read lasts its full timeout.
//
// Both edges of the parking are published, keyed by the caller's `key`: a flag
// that only says "it parked at some point" stays set after the read is over, and
// the test needs to know whether it was still parked when it looked.
$key = preg_replace('/[^a-z0-9]/', '', (string) ($_GET['key'] ?? ''));

try {
    $sock = stream_socket_client('tcp://' . net_peer_ip() . ':4436', $errno, $errstr, 3.0);
    if ($sock === false) {
        echo "plain-hold-connect-failed:{$errstr} ({$errno})";
        return;
    }
    stream_set_timeout($sock, 4);
    $sharedState["plainhold_sock_{$key}"] = $sock;
    $sharedState["plainhold_parked_{$key}"] = microtime(true);

    $data = fread($sock, 16);
    $sharedState["plainhold_done_{$key}"] = microtime(true);
    echo 'plain-hold-done:' . json_encode([
        'data' => $data,
        'timed_out' => stream_get_meta_data($sock)['timed_out'],
    ]);
} catch (\Throwable $e) {
    echo 'plain-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
