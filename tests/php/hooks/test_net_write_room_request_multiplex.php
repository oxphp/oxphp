<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_write_room_request_multiplex', 'hooks');

// A write that has to wait for room in the socket's buffer, on one end of a
// stream_socket_pair() whose other end nobody is reading. The wait is the engine's
// own poll() for writability, below the stream layer, and with the net category it
// suspends this request fiber.
//
// PHP_WORKERS=1 makes that decisive: the request that reads the far end dry is
// served by this same worker thread, and it runs a second after it was fired. If
// the write held the thread, that request could only start after the write had
// given up at its own timeout; if the write parked, it starts while the write is
// still waiting, and what it drains is what lets the writer move on.
$pair = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
$t->assertTrue('the socket pair was created', is_array($pair));
$sharedState['net_drain_reader'] = $pair[1];
unset($sharedState['net_drain_ran_at'], $sharedState['net_drain_bytes']);

// Fill what the socket's buffer holds before the write under test starts, so that
// write meets a full buffer at its first byte and any byte it gets through is room
// that appeared while it waited. Without this, the first send() of a large write
// succeeds in part on its own and the count says nothing about the wait.
stream_set_blocking($pair[0], false);
$filled = 0;
$block = str_repeat('x', 65536);
while (($sent = fwrite($pair[0], $block)) > 0) {
    $filled += $sent;
}
stream_set_blocking($pair[0], true);
stream_set_timeout($pair[0], 3);
$t->assertTrue('the buffer was filled before the write', $filled > 0);

$http = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('the inner request connected', $http !== false);
if ($http === false) {
    $t->done();
}

$t0 = microtime(true);
fwrite($http, "GET /tests/hooks/fixture_inner_net_drain.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

// Far more than a unix socket's buffer holds, so the write cannot complete without
// the far end being read, and it started against a buffer that is already full.
$payload = str_repeat('x', 16 * 1024 * 1024);
// PHP raises a notice when a write gives up at its timeout ("Send of N bytes
// failed"); the worker's error handler would turn it into an exception, and `@` does
// not keep a handler from being called.
set_error_handler(static fn (): bool => true);
$written = fwrite($pair[0], $payload);
restore_error_handler();
$elapsed = microtime(true) - $t0;
$ranAt = $sharedState['net_drain_ran_at'] ?? null;

$t->assertTrue('the request that drains the far end ran', is_float($ranAt));
if (is_float($ranAt)) {
    $t->assertLessThan(
        'and ran while the write was still waiting, not after it had given up at its three-second timeout',
        $ranAt - $t0,
        2.0
    );
}
$t->assertTrue('the write resumed once the draining request had made room, from a buffer that was full when it started', is_int($written) && $written > 0);
$t->assertLessThan('and did not send the whole payload, which nothing read', (int) $written, strlen($payload));

$inner = (string) stream_get_contents($http);
$t->assertContains('the draining request finished as usual', $inner, 'inner-done');
fclose($http);
unset($sharedState['net_drain_reader'], $sharedState['net_drain_ran_at'], $sharedState['net_drain_bytes']);
fclose($pair[0]);
fclose($pair[1]);

$t->done();
