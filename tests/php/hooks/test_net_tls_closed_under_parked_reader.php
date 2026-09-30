<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_tls_closed_under_parked_reader', 'hooks');

// The TLS twin of the tcp:// case in the db profile: a read that parks holds the
// php_stream and its TLS state in a C frame across the suspension, and the worker
// goes on serving other fibers meanwhile. If one of them closes that same stream,
// PHP frees both, and the reader resuming into them — or OpenSSL touching the
// session it was in the middle of — writes to memory that is gone.
//
// What is asserted is that the parked request is ended, promptly and diagnosably,
// rather than resumed onto freed memory, and that nothing waited for the stream's
// own timeout to find out.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    // Past the holder's own six-second read timeout, so a build that never tells
    // it about the close is read to the end rather than cut off here.
    stream_set_timeout($sock, 9);
    fwrite($sock, "GET /tests/hooks/fixture_net_tls_hold.php?timeout=6 HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['net_tls_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}

$opened = isset($sharedState['net_tls']);
$parked = $opened && ($sharedState['net_tls_parked'] ?? false);
$t->assertTrue('the holding request opened the shared TLS stream before this one woke', $opened);
$t->assertTrue('and it reached the read that parks it', $parked);
if (!$parked) {
    $t->done();
}

// A moment for the holder to be inside the read and not just about to enter it.
oxphp_usleep(200_000);

$started = microtime(true);
fclose($sharedState['net_tls']);
unset($sharedState['net_tls'], $sharedState['net_tls_parked']);

$inner = oxphp_async_await($task);
$waited = microtime(true) - $started;

$t->assertNotContains(
    'the parked request did not go on using the stream freed under it',
    $inner['body'],
    'tls-hold-done:'
);
$t->assertMatch(
    'it was ended with a server error instead',
    $inner['body'],
    '#^HTTP/1\.[01] 500#'
);
$t->assertLessThan(
    'and it learned of the close then, not at its own read timeout six seconds later',
    $waited,
    3.0
);

// The worker kept serving: a fatal in one multiplexed request must not take the
// others, or the next ones, with it.
$after = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooks/fixture_inner_sleep.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});
$next = oxphp_async_await($after);
$t->assertContains('the worker served the next request as usual', $next['body'], 'inner-done');

$t->done();
