<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('sleep_request_multiplex', 'hooks');

// PHP_WORKERS=1: the inner self-request below can only be served while THIS
// request fiber is suspended. The request is written to a raw socket up
// front; while this fiber hook-sleeps 2s, the single worker serves the
// inner request (which itself hook-sleeps 1s) and the response lands in
// the socket buffer — read instantly after waking. If sleep() blocked the
// thread instead, the inner request could not be served during it and the
// read below would hit its socket timeout.
$t0 = microtime(true);
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);

stream_set_timeout($sock, 5);
fwrite($sock, "GET /tests/hooks/fixture_inner_sleep.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

sleep(2);                                   // hooked: suspends this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);
$elapsed = microtime(true) - $t0;

$t->assertContains('inner self-request served a full response', $resp, 'inner-done');
$t->assertTrue('inner was served DURING the outer sleep (elapsed < 2.7s, serial would be >= 3s)',
    $elapsed < 2.7);
$t->assertTrue('outer sleep honored its duration (elapsed >= 2.0s)', $elapsed >= 2.0);

$t->done();
