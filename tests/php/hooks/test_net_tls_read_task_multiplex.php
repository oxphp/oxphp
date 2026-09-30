<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_tls_read_task_multiplex', 'hooks');

// Port 4433 of the peer completes the handshake at once and then sends its
// answer one second after the client connected. The read that waits for it is the
// engine's TLS read loop — not a plain socket read, so the streams hook does not
// see it — and a hooked one suspends the task fiber inside it. Two tasks on one
// async worker then wait together; without the suspension the thread serves one
// wait after the other.
$peer = net_peer_ip();

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($peer): array {
        $started = microtime(true);
        $context = stream_context_create(['ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
        $sock = @stream_socket_client("ssl://{$peer}:4433", $errno, $errstr, 3.0, STREAM_CLIENT_CONNECT, $context);
        if ($sock === false) {
            return ['body' => 'connect failed', 'started' => $started, 'finished' => microtime(true)];
        }
        stream_set_timeout($sock, 6);
        $body = (string) fread($sock, 16);
        $finished = microtime(true);
        fclose($sock);

        return ['body' => $body, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

foreach ($results as $i => $r) {
    $t->assertSame("task {$i} read the peer's answer", $r['body'], 'tls-done');
    $t->assertGreaterThan(
        "task {$i} really waited for the answer",
        $r['finished'] - $r['started'],
        0.9
    );
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertTrue(
    'the two TLS reads overlapped on one async worker (span < 1.6s, serial would be >= 2s)',
    $span < 1.6
);

$t->done();
