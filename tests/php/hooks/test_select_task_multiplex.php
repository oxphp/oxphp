<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('select_task_multiplex', 'hooks');

// ASYNC_WORKERS=1: three tasks each open a TCP client, send a request the server
// answers after 1s, and then wait for readiness with stream_select() rather than
// reading. The read hook cannot help here — nothing is read until select returns
// — so this measures the select itself. Hooked, the three waits overlap on one
// async worker thread; unhooked, select pins that thread and they serialize.
//
// Each task reports its own start and finish instead of timing the batch from
// here: this profile's async worker may still be running an earlier test's
// fire-and-forget task, which delays every task equally and says nothing about
// overlap.
$tasks = [];
for ($i = 0; $i < 3; $i++) {
    $tasks[] = oxphp_async(function (): array {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock === false) {
            return ['n' => -1, 'started' => 0.0, 'finished' => 0.0, 'body' => "connect failed: {$errstr} ({$errno})"];
        }
        stream_set_timeout($sock, 6);
        fwrite($sock, "GET /tests/hooks/fixture_inner_sleep.php HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

        $read = [$sock];
        $write = null;
        $except = null;

        $started = microtime(true);
        $n = stream_select($read, $write, $except, 5);
        $finished = microtime(true);

        $body = (string) stream_get_contents($sock);
        fclose($sock);

        return ['n' => $n, 'started' => $started, 'finished' => $finished, 'body' => $body];
    });
}
$results = oxphp_async_await_all($tasks);

$firstStart = min(array_column($results, 'started'));
$lastFinish = max(array_column($results, 'finished'));
$span = $lastFinish - $firstStart;

foreach ($results as $i => $r) {
    $t->assertSame("task {$i} saw exactly one readable stream", $r['n'], 1);
    $t->assertContains("task {$i} read the slow response in full", $r['body'], 'inner-done');
    $t->assertGreaterThan(
        "task {$i} really waited for the slow response",
        $r['finished'] - $r['started'],
        0.9
    );
}

// The measurement is the point of this test, so it is reported either way:
// meta() carries it on a pass, and assertLessThan (rather than a boolean) puts
// it in the failure too, where the number says whether the waits serialized
// outright or merely overlapped less than expected.
$t->meta('span', round($span, 3));
$t->assertLessThan(
    'the three waits overlapped on one async worker (serial would be >= 3s)',
    $span,
    2.0
);

$t->done();
