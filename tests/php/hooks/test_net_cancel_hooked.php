<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_cancel_hooked', 'hooks');

// The scheduler's cancellation pass covers the waits the net category parks, not
// only the descriptor waits of the streams hook: a task parked in a connect, in a
// TLS handshake, in a TLS read or in a read on a local socket must unwind when its
// awaiter gives up, rather than waiting out its own five-second timeout.
//
// Each task does one of the four and writes a marker from finally. The outer await
// times out after 0.2s, which arms the cancellation, and the marker turns up within
// a second if the park was left and after five if the wait ran to its end. The
// TLS waits cannot be left by returning from the wait alone — the engine's loop
// inside them looks at the clock and at the connection, not at what the wait said —
// so the last scenario is also the check that such a loop is actually ended.
$peer = net_peer_ip();
$silent = net_silent_address();
$prefix = sys_get_temp_dir() . '/oxphp_netcancel_' . getmypid() . '_' . uniqid('', true) . '_';

$scenarios = [
    'connect' => function (string $marker) use ($silent): int {
        try {
            @stream_socket_client("tcp://{$silent}:80", $errno, $errstr, 5.0);
        } finally {
            file_put_contents($marker, 'cancelled');
        }
        return 0;
    },
    'tls handshake' => function (string $marker) use ($peer): int {
        try {
            $context = stream_context_create(['ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
            $sock = @stream_socket_client("ssl://{$peer}:4436", $errno, $errstr, 5.0, STREAM_CLIENT_CONNECT, $context);
        } finally {
            file_put_contents($marker, 'cancelled');
        }
        return 0;
    },
    'tls read' => function (string $marker) use ($peer): int {
        $context = stream_context_create(['ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
        $sock = @stream_socket_client("ssl://{$peer}:4435", $errno, $errstr, 3.0, STREAM_CLIENT_CONNECT, $context);
        try {
            stream_set_timeout($sock, 5);
            fread($sock, 16);
        } finally {
            file_put_contents($marker, 'cancelled');
            fclose($sock);
        }
        return 0;
    },
    'local socket read' => function (string $marker): int {
        $pair = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
        try {
            stream_set_timeout($pair[0], 5);
            fread($pair[0], 16);
        } finally {
            file_put_contents($marker, 'cancelled');
            fclose($pair[0]);
            fclose($pair[1]);
        }
        return 0;
    },
];

foreach ($scenarios as $name => $scenario) {
    $marker = $prefix . str_replace(' ', '_', $name);
    $started = microtime(true);
    $task = oxphp_async($scenario, $marker);

    $timedOut = false;
    try {
        oxphp_async_await($task, 0.2);
    } catch (\OxPHP\Async\TimeoutException $e) {
        $timedOut = true;
    }
    $t->assertTrue("{$name}: the outer await timed out, arming cancellation", $timedOut);

    $deadline = microtime(true) + 2.0;
    while (!file_exists($marker) && microtime(true) < $deadline) {
        usleep(20000);
    }
    $unwound = microtime(true) - $started;

    $t->assertTrue("{$name}: the parked wait was cancelled and unwound", file_exists($marker));
    $t->assertLessThan("{$name}: and well before its five-second timeout", $unwound, 1.5);
    if (file_exists($marker)) {
        unlink($marker);
    }
}

// Cancelling those did not cost the pool its workers.
$alive = oxphp_async(static fn(): string => 'alive');
$t->assertSame('the pool still runs tasks', oxphp_async_await($alive, 3.0), 'alive');

$t->done();
