<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_cancel_cleanup_connect_holds_thread', 'hooks');

// Once a task has been cancelled, it is only being cleaned up, and what the net
// category would park stays on the thread from then on: a second suspension hands
// the cancellation to code that has already been told — the pool asks for it again
// on every turn while the task is alive — and a finally block or destructor that
// connects, or reads a local socket, would be thrown out of before it finished.
// Before the category existed those waits held the thread and ran to their end, and
// so they do now.
//
// The task waits at something the cancellation reaches, is cancelled there when its
// awaiter gives up, and from finally waits for something that never comes, with a
// one-second timeout. That wait must come back with its own failure after that
// second, not with the cancellation a second time within milliseconds. It makes no
// difference which wait took the first cancellation: a connect, which is the net
// category's own, or a read on a plain tcp:// stream, which the streams hook takes.
// Nor which wait comes after it: a connect to an address that never answers, or a
// read on a socket pair whose far end stays silent — the shape of a rollback sent
// over a unix socket from a finally block.
$peer = net_peer_ip();

$scenarios = [
    'connect, then a connect' => ['connect', 'connect', 3],
    'streams read, then a connect' => ['streams read', 'connect', 4],
    'streams read, then a local read' => ['streams read', 'local read', 5],
];

foreach ($scenarios as $name => [$first, $cleanup, $slot]) {
    // An address of its own for each scenario: see net_silent_address().
    $silent = net_silent_address($slot);
    $marker = sys_get_temp_dir() . '/oxphp_netcancelclean_' . getmypid() . '_' . uniqid('', true);

    $task = oxphp_async(function (string $marker, string $silent, string $peer, string $first, string $cleanup): int {
        $report = [];
        $sock = null;
        $pair = [];
        try {
            if ($first === 'connect') {
                @stream_socket_client("tcp://{$silent}:80", $errno, $errstr, 5.0);
            } else {
                // Accepts the connection and never says anything.
                $sock = @stream_socket_client("tcp://{$peer}:4436", $errno, $errstr, 3.0);
                stream_set_timeout($sock, 5);
                fread($sock, 16);
            }
        } finally {
            $started = microtime(true);
            try {
                if ($cleanup === 'connect') {
                    @stream_socket_client("tcp://{$silent}:80", $errno, $errstr, 1.0);
                } else {
                    $pair = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
                    stream_set_timeout($pair[0], 1);
                    @fread($pair[0], 16);
                }
                $report['outcome'] = 'returned';
            } catch (\Throwable $e) {
                $report['outcome'] = get_class($e);
            }
            $report['seconds'] = microtime(true) - $started;
            file_put_contents($marker, json_encode($report));
            foreach ([$sock, ...$pair] as $resource) {
                if (is_resource($resource)) {
                    fclose($resource);
                }
            }
        }
        return 0;
    }, $marker, $silent, $peer, $first, $cleanup);

    $timedOut = false;
    try {
        oxphp_async_await($task, 0.2);
    } catch (\OxPHP\Async\TimeoutException $e) {
        $timedOut = true;
    }
    $t->assertTrue("{$name}: the outer await timed out, arming cancellation", $timedOut);

    $deadline = microtime(true) + 4.0;
    while (!file_exists($marker) && microtime(true) < $deadline) {
        usleep(20000);
    }
    $t->assertTrue("{$name}: the task got through its finally block", file_exists($marker));

    if (file_exists($marker)) {
        $report = json_decode((string) file_get_contents($marker), true);
        unlink($marker);
        $t->assertSame(
            "{$name}: the wait made during cleanup was not handed the cancellation again",
            $report['outcome'] ?? null,
            'returned'
        );
        $t->assertGreaterThan(
            "{$name}: and it ran its own timeout, on the thread, instead of being cut short",
            (float) ($report['seconds'] ?? 0.0),
            0.8
        );
    }
}

$alive = oxphp_async(static fn(): string => 'alive');
$t->assertSame('the pool still runs tasks', oxphp_async_await($alive, 3.0), 'alive');

$t->done();
