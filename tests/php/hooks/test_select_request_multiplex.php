<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('select_request_multiplex', 'hooks');

// The select counterpart of test_stream_request_multiplex, and decisive for the
// same reason: PHP_WORKERS=1 means the response this fiber waits for can only be
// produced by the very worker thread running this fiber. A hooked
// stream_select() parks the request fiber and lets the worker accept and serve
// the inner request; a select that blocked would pin the worker and the inner
// request would never be picked up, so the wait would run out its timeout with
// nothing to show.
//
// test_select_task_multiplex covers the async-task scheduler; this is the HTTP
// one, whose readiness pass is a separate call site.
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner socket connected', $sock !== false);
stream_set_timeout($sock, 4);
fwrite($sock, "GET /tests/hooks/fixture_inner_sleep.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

$read = [$sock];
$write = null;
$except = null;

$t0 = microtime(true);
$ready = stream_select($read, $write, $except, 4);
$elapsed = microtime(true) - $t0;

$resp = (string) stream_get_contents($sock);
fclose($sock);

$t->assertSame('select reported the socket readable', $ready, 1);
$t->assertContains('the inner request was served while this fiber waited', $resp, 'inner-done');
$t->assertTrue(
    'select parked this request fiber instead of pinning the worker (elapsed < 2.0s)',
    $elapsed < 2.0
);
$t->assertGreaterThan('select really waited for the slow response', $elapsed, 0.9);

$t->done();
