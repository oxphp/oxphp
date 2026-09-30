<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_tls_shared_second_fiber_refused', 'hooks');

// One TLS connection, two fibers: the first is parked in a read waiting for the
// peer, the second reads the same stream. Whatever arrives next on that stream is
// the answer to the first fiber's wait, and a TLS stream cannot even be read from
// two places without corrupting the record layer — so the second read must not
// get to touch it. It is refused the way a tcp:// read is: no data, and the
// stream's timeout flag says why, at once rather than after the stream's own
// timeout and without disturbing the holder.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 9);
    fwrite($sock, "GET /tests/hooks/fixture_net_tls_hold.php?timeout=3 HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
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
oxphp_usleep(200_000);

$shared = $sharedState['net_tls'];
stream_set_timeout($shared, 5);
$started = microtime(true);
$data = fread($shared, 16);
$elapsed = microtime(true) - $started;
$meta = stream_get_meta_data($shared);

$t->assertTrue('the second fiber got no data from the holder\'s stream', $data === false || $data === '');
$t->assertTrue('and was told the read timed out', $meta['timed_out'] === true);
$t->assertLessThan('at once, not after its own five-second timeout', $elapsed, 2.0);

// The holder is unaffected: it reads out its own three-second timeout and ends as
// any request would.
$inner = oxphp_async_await($task);
$t->assertContains('the holder finished its own read undisturbed', $inner['body'], 'tls-hold-done:');
$t->assertMatch('and was not ended by the second fiber', $inner['body'], '#^HTTP/1\.[01] 200#');

if (isset($sharedState['net_tls']) && is_resource($sharedState['net_tls'])) {
    fclose($sharedState['net_tls']);
}
unset($sharedState['net_tls'], $sharedState['net_tls_parked']);

$t->done();
