<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// An exception a shutdown function throws is a fatal under every other SAPI: the
// engine calls those with no frame on the stack, and the tail of the call hands
// the exception to the path that reports it and aborts. A worker calls them from
// inside its request loop's own frame, where that path does not run — so unless
// the loop reports the exception itself, nothing does: the request answers the
// 200 of a request that did nothing wrong, and the scheduler's backstop drops
// the exception when the fiber parks.
//
// The exception has to come from another request — a request cannot check its
// own response — so this one parks and reads what the inner one answered. The
// last check is on this request rather than that one: dropping the exception is
// the backstop's job, and a report that left it pending instead would unwind
// this request on the first opcode after the resume.

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$connected = $sock !== false;

if ($connected) {
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_shutdown_throw.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
}

// Hooked: parks this request's fiber so the worker is free to serve the inner
// one. Without the park there is no worker to serve it — this profile runs one.
sleep(1);

// Reached only if the resume did not unwind on the inner request's exception.
$resumed = true;

$resp = $connected ? (string) stream_get_contents($sock) : '';
if ($connected) {
    fclose($sock);
}

$t = new TestCase('shutdown_throw_is_reported', 'fibers');

$t->assertTrue('inner self-request socket connected', $connected);

// Both halves: the inner request ran, and its shutdown function threw. Without
// the first, "no exception reached this request" would pass on a worker that
// never served the inner one at all.
$t->assertContains('the inner request armed its shutdown throw', $resp, 'SHUTDOWN-THROW-ARMED');
$t->assertContains('and the worker reported the exception it threw', $resp, 'thrown from a shutdown function');

$t->assertTrue('this request resumed instead of unwinding on it', $resumed);

$t->done();
