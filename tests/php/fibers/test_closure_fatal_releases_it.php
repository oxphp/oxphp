<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// The frame of a closure call holds a reference to the closure. The engine
// takes it when it pushes the frame and gives it back when the frame leaves —
// that reference is what lets a closure destroy itself in the middle of its own
// call. A frame a fatal abandons never leaves, so the worker has to give it back
// on the frame's behalf: it serves the next request on the same fiber, and a
// closure left holding on is held for the life of the worker.
//
// That is real memory, not a reference count on something long-lived: a closure
// declared inside a request is a new object per request, and it carries
// everything it closed over.
//
// The inner request below is what actually fatals — a request cannot report on
// its own fatal — and it reports through a shutdown function, which the worker
// runs after it has released the abandoned frames.

$t = new TestCase('closure_fatal_releases_it', 'fibers');

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 10);
fwrite($sock, "GET /tests/fibers/fixture_closure_fatal.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

// Hooked: parks this request's fiber so the worker is free to serve the inner
// one. Without the park there is no worker to serve it — this profile runs one.
sleep(1);

$resp = (string) stream_get_contents($sock);
fclose($sock);

// Without this the rest proves nothing: a fixture that returned normally would
// leave no abandoned frames for the worker to get wrong.
$t->assertNotContains('the inner request did not survive its fatal', $resp, 'NOT-REACHED');

$t->assertContains(
    'a closure the fatal was inside does not outlive its request',
    $resp,
    'CLOSURE-FREED'
);

// The worker is still the one serving this request, so reaching the end at all
// says the fatal left it in one piece.
$t->assertTrue('the worker still serves this request', oxphp_is_worker());

$t->done();
