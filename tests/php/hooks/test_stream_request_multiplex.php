<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_request_multiplex', 'hooks');

// PHP_WORKERS=1 makes this self-request decisive: the response this fiber waits
// for can only be produced by the very worker thread running this fiber. A
// hooked socket read parks the request fiber and lets the worker accept and
// serve the inner request; a blocking read would pin the worker inside recv()
// and the inner request would never be picked up at all.
//
// This exercises the HTTP scheduler's readiness pass, whereas
// test_stream_task_multiplex covers the async-task scheduler.
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner socket connected', $sock !== false);
stream_set_timeout($sock, 4);
fwrite($sock, "GET /tests/hooks/fixture_inner_sleep.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

$t0 = microtime(true);
$resp = (string) stream_get_contents($sock);
$elapsed = microtime(true) - $t0;
fclose($sock);

$t->assertContains('the inner request was served while this fiber waited', $resp, 'inner-done');
$t->assertTrue(
    'the read parked this request fiber instead of pinning the worker (elapsed < 2.0s)',
    $elapsed < 2.0
);
$t->assertGreaterThan('the read really waited for the slow response', $elapsed, 0.9);

$t->done();
