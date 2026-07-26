<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('select_cancel_hooked', 'hooks');

// A task parked inside a hooked stream_select() whose awaiter gives up must
// unwind at once, not sit out the select's own timeout. The socket is connected
// but nothing is ever sent, so readiness never arrives on its own.
$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
$t->assertTrue('probe server listening', $server !== false);
$addr = stream_socket_get_name($server, false);

$task = oxphp_async(function () use ($addr): int {
    $sock = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    if ($sock === false) {
        return -1;
    }
    $read = [$sock];
    $write = null;
    $except = null;
    $n = stream_select($read, $write, $except, 30);
    fclose($sock);

    return (int) $n;
});

$t0 = microtime(true);
$caught = null;
try {
    oxphp_async_await($task, 0.5);
} catch (\Throwable $ex) {
    $caught = $ex::class;
}
$elapsed = microtime(true) - $t0;

fclose($server);

$t->assertNotNull('the awaiter gave up', $caught);
$t->assertLessThan('the cancelled task unwound promptly', $elapsed, 3.0);

$t->done();
