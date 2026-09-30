<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_connection_closed_while_nested_operation_parked', 'hooks');

// An operation on a connection runs PHP's own code, which calls userland — the
// error handler for the notice a failed send raises is one — and the userland code
// can start an operation on another connection and park in it. Another fiber can close the
// first connection meanwhile, and what the write goes on to do when it is resumed
// reads the stream that close freed. Nothing can make that safe, so what is asked
// of the build is the same as for a read parked on the closed connection itself:
// the request is ended, and the worker keeps serving.
//
// A build that names only the innermost operation does not see the close: the
// connection the fiber is parked on is the other one, so no wake and no mark, and
// the write finishes on freed memory and the request answers as if nothing had
// happened.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooks/fixture_net_failed_write_handler_parks.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['nested_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}

$opened = isset($sharedState['nested_outer']);
$parked = $sharedState['nested_parked'] ?? false;
$t->assertTrue('the firing request opened the connection its write is on', $opened);
$t->assertTrue('and reached the read that parks it inside that write', $parked === true);
if (!$parked) {
    $t->done();
}

// A moment for the read to be waiting and not just about to.
oxphp_usleep(200_000);

fclose($sharedState['nested_outer']);
unset($sharedState['nested_outer'], $sharedState['nested_parked']);

$inner = oxphp_async_await($task);

$t->assertNotContains(
    'the write did not go on using the connection that was closed under it',
    $inner['body'],
    'nested-done'
);
$t->assertMatch(
    'the request was ended with a server error instead',
    $inner['body'],
    '#^HTTP/1\.[01] 500#'
);

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
