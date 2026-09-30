<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_cancel_plain_write_keeps_connection', 'hooks');

// A write that has to wait for room, on an ordinary tcp:// connection, is cancelled
// when its task is. What a cancelled wait inside OpenSSL's loop gets is a shutdown
// of the connection, because that loop would otherwise go on waiting for what is
// left of its own timeout; a plain write has no such loop — the wait returns, and
// the write fails the way a timed-out one does — so the connection has to come out
// of it still usable.
//
// The task fills a connection nobody reads, is cancelled in the write, and from its
// finally block has the far end send four bytes and reads them on the client. A
// connection that was shut down answers that with nothing, or with a reset. The
// read is a direct recvfrom(), so that nothing in it can suspend and be handed the
// cancellation again.
$marker = sys_get_temp_dir() . '/oxphp_netcancelwrite_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function (string $marker): int {
    $server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
    $name = stream_socket_get_name($server, false);
    $client = stream_socket_client("tcp://{$name}", $errno, $errstr, 3.0);
    $peer = stream_socket_accept($server, 3.0);
    stream_set_timeout($client, 5);
    try {
        // More than the two ends of a loopback connection can hold between them.
        @fwrite($client, str_repeat('x', 64 * 1024 * 1024));
    } finally {
        @fwrite($peer, 'pong');
        $got = @stream_socket_recvfrom($client, 4);
        file_put_contents($marker, json_encode(['got' => $got]));
        fclose($client);
        fclose($peer);
        fclose($server);
    }
    return 0;
}, $marker);

$timedOut = false;
try {
    oxphp_async_await($task, 0.3);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timedOut = true;
}
$t->assertTrue('the outer await timed out, arming cancellation', $timedOut);

$deadline = microtime(true) + 4.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}
$t->assertTrue('the task got through its finally block', file_exists($marker));

if (file_exists($marker)) {
    $report = json_decode((string) file_get_contents($marker), true);
    unlink($marker);
    $t->assertSame(
        'the connection was still usable after its write was cancelled',
        $report['got'] ?? null,
        'pong'
    );
}

$alive = oxphp_async(static fn(): string => 'alive');
$t->assertSame('the pool still runs tasks', oxphp_async_await($alive, 3.0), 'alive');

$t->done();
