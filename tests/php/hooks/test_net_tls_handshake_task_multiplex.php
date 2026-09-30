<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_tls_handshake_task_multiplex', 'hooks');

// Port 4434 of the peer accepts the connection and lets one second pass before it
// answers the client's hello. The wait is inside stream_socket_client() itself, in
// the handshake the engine runs as part of opening an ssl:// stream — the one wait
// of a TLS connection that is not a read or a write on an open stream.
$peer = net_peer_ip();

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($peer): array {
        $started = microtime(true);
        $context = stream_context_create(['ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
        $sock = @stream_socket_client("ssl://{$peer}:4434", $errno, $errstr, 5.0, STREAM_CLIENT_CONNECT, $context);
        $finished = microtime(true);
        if ($sock === false) {
            return ['body' => 'connect failed', 'started' => $started, 'finished' => $finished];
        }
        stream_set_timeout($sock, 3);
        $body = (string) fread($sock, 16);
        fclose($sock);

        return ['body' => $body, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

foreach ($results as $i => $r) {
    $t->assertSame("task {$i} completed the handshake and read the answer", $r['body'], 'tls-done');
    $t->assertGreaterThan(
        "task {$i}: the handshake really waited for the peer",
        $r['finished'] - $r['started'],
        0.9
    );
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertTrue(
    'the two handshakes overlapped on one async worker (span < 1.6s, serial would be >= 2s)',
    $span < 1.6
);

$t->done();
