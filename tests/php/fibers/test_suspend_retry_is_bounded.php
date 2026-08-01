<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('suspend_retry_is_bounded', 'fibers');

// Suspending an OxPHP request fiber from userland is refused, and a program is
// free to catch that refusal and try again. This pins that it cannot do so
// forever. The exchange runs inside the scheduler tick, so an unbounded one is
// not a stuck request — the tick never returns, and every other request on the
// worker waits behind it.
//
// PHP_WORKERS=1, so the retrying request can only run while this one is
// suspended in the sleep below, and this one can only be resumed by a tick that
// returned. If the refusals were unbounded, this test would not finish at all:
// the assertions below are reached only if the worker got its scheduler back.
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('retrying request connected', $sock !== false);
stream_set_timeout($sock, 10);
fwrite($sock, "GET /tests/fibers/fixture_suspend_retry.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

$started = microtime(true);
sleep(3); // hooked: suspends this fiber, so the worker runs its event loop

$reply = (string) stream_get_contents($sock);
fclose($sock);

$t->assertTrue(
    'this request got its worker back (' . round(microtime(true) - $started, 2) . 's)',
    microtime(true) - $started < 8.0
);
$t->assertTrue('the retrying request ended', $reply !== '');
$t->assertNotNull('this request still has its fiber', \Fiber::getCurrent());

// And the worker is still usable afterwards, not left with a fiber mid-retry.
$after = microtime(true);
oxphp_sleep(0.02);
$t->assertGreaterThan('the worker still serves this request', microtime(true) - $after, 0.015);

$t->done();
