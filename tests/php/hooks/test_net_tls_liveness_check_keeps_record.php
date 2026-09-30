<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_tls_liveness_check_keeps_record', 'hooks');

// feof() asks a TLS stream whether the connection is alive, and when the descriptor
// is readable it answers by reading one record into the TLS session to look at it.
// Asked of a stream another fiber is parked on, that look takes the record the parked
// fiber is waiting for: the descriptor is empty from then on, nothing tells the
// parked fiber anything arrived, and it sits out its own timeout with the record in
// the session.
//
// Decisive only while the asking fiber holds the thread through the moment the record
// arrives — otherwise the scheduler wakes the parked fiber first and it wins the race.
// So this request spins on feof() across that moment, and only then gives the thread
// back. The peer on 4433 sends its record a second after the handshake.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})", 'finished' => microtime(true)];
    }
    stream_set_timeout($sock, 9);
    fwrite($sock, "GET /tests/hooks/fixture_net_tls_hold.php?timeout=4&port=4433 HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body, 'finished' => microtime(true)];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['net_tls_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$parked = isset($sharedState['net_tls']) && ($sharedState['net_tls_parked'] ?? false);
$t->assertTrue('the holding request opened the shared TLS stream and reached its read', $parked);
if (!$parked) {
    $t->done();
}
$parkedAt = microtime(true);
oxphp_usleep(200_000);

$shared = $sharedState['net_tls'];
$checks = 0;
while (microtime(true) < $parkedAt + 1.8) {
    feof($shared);
    $checks++;
}
$t->assertGreaterThan('the liveness check ran many times across the moment the record arrived', $checks, 100);

$inner = oxphp_async_await($task, 8.0);
$t->assertContains('the parked reader still got the peer\'s record', $inner['body'], "tls-hold-done:'tls-done'");
$t->assertLessThan(
    'and got it when it arrived, not when its own four-second timeout ran out',
    $inner['finished'] - $parkedAt,
    3.0
);

if (isset($sharedState['net_tls']) && is_resource($sharedState['net_tls'])) {
    fclose($sharedState['net_tls']);
}
unset($sharedState['net_tls'], $sharedState['net_tls_parked']);

$t->done();
