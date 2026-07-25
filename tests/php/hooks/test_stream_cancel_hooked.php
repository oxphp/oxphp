<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_cancel_hooked', 'hooks');

// The scheduler's cancellation pass covers descriptor waits as well as sleeps:
// a task parked in a hooked read must unwind when its awaiter gives up, rather
// than waiting out the socket timeout. Mirrors test_sleep_cancel_hooked, with
// the park happening inside fread() instead of sleep().
//
// The peer accepts and never writes, so the only two ways out of that read are
// cancellation (fast) or the 5s socket timeout (slow). The marker written from
// finally tells them apart.
$marker = sys_get_temp_dir() . '/oxphp_streamcancel_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function () use ($marker): int {
    $server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
    $addr = stream_socket_get_name($server, false);
    $client = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    $peer = stream_socket_accept($server, 3.0);
    stream_set_timeout($client, 5);

    try {
        fread($client, 16);      // parks on the descriptor; nothing ever arrives
    } finally {
        file_put_contents($marker, 'cancelled');
        fclose($client);
        fclose($peer);
        fclose($server);
    }

    return 0;
});

$timedOut = false;
try {
    oxphp_async_await($task, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timedOut = true;
}
$t->assertTrue('the outer await timed out, arming cancellation', $timedOut);

$deadline = microtime(true) + 2.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}

$t->assertTrue(
    'the parked read was cancelled and unwound well before its 5s socket timeout',
    file_exists($marker)
);

if (file_exists($marker)) {
    unlink($marker);
}

$t->done();
