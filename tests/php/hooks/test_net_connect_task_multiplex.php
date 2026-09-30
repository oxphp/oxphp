<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_connect_task_multiplex', 'hooks');

// ASYNC_WORKERS=1: two tasks each connect to an address that answers nothing, with
// a one-second connect timeout. The wait for the connection to complete is inside
// the engine's own connect path, and a hooked one suspends the task fiber there, so
// the single async worker thread runs both waits side by side. Without the
// suspension the thread is held for each in turn.
//
// Each task reports when its connect started and finished rather than timing the
// batch from here: this profile's async worker may still be finishing an earlier
// test's fire-and-forget task, which delays every task equally and says nothing
// about overlap.
$silent = net_silent_address(1);

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($silent): array {
        $started = microtime(true);
        $sock = @stream_socket_client("tcp://{$silent}:80", $errno, $errstr, 1.0);
        $finished = microtime(true);
        if (is_resource($sock)) {
            fclose($sock);
        }
        return ['connected' => $sock !== false, 'errno' => $errno, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

foreach ($results as $i => $r) {
    $t->assertFalse("task {$i}: nothing answers at that address, so the connect did not succeed", $r['connected']);
    $t->assertGreaterThan(
        "task {$i}: the connect really waited out its timeout",
        $r['finished'] - $r['started'],
        0.9
    );
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertTrue(
    'the two connects overlapped on one async worker (span < 1.6s, serial would be >= 2s)',
    $span < 1.6
);

$t->done();
