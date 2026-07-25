<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_task_multiplex', 'hooks');

// ASYNC_WORKERS=1: three tasks each open a TCP client to this server and read a
// response the server holds for 1s. A hooked socket read suspends the task
// fiber, so the single async worker thread interleaves all three reads.
// Blocking reads pin that thread and serialize them.
//
// The server side is concurrent either way: fixture_inner_sleep.php sleeps via
// the hooked sleep(), so the single PHP worker serves all three request fibers
// at once. The only variable this test measures is the client-side read.
//
// Each task reports when its read started and finished rather than letting the
// whole batch be timed from here: this profile's async worker may still be
// finishing an earlier test's fire-and-forget task when this one starts, which
// delays every task equally and says nothing about overlap.
$tasks = [];
for ($i = 0; $i < 3; $i++) {
    $tasks[] = oxphp_async(function (): array {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock === false) {
            return ['body' => "connect failed: {$errstr} ({$errno})", 'started' => 0.0, 'finished' => 0.0];
        }
        stream_set_timeout($sock, 6);
        fwrite($sock, "GET /tests/hooks/fixture_inner_sleep.php HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

        $started = microtime(true);
        $body = (string) stream_get_contents($sock);
        $finished = microtime(true);
        fclose($sock);

        return ['body' => $body, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

$firstStart = min(array_column($results, 'started'));
$lastFinish = max(array_column($results, 'finished'));
$span = $lastFinish - $firstStart;

foreach ($results as $i => $r) {
    $t->assertContains("task {$i} read the slow response in full", $r['body'], 'inner-done');
    $t->assertGreaterThan(
        "task {$i} really waited for the slow response",
        $r['finished'] - $r['started'],
        0.9
    );
}

$t->assertTrue(
    'the three reads overlapped on one async worker (span < 2.0s, serial would be >= 3s)',
    $span < 2.0
);

$t->done();
