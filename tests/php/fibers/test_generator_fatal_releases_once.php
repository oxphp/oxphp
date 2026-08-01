<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A fatal in a worker abandons the frames it was raised from, and the worker
// releases what they were holding — it serves the next request on the same
// fiber, so anything left there is held for the life of the worker.
//
// A generator running at that moment is in those frames without being one of
// them. The engine links a generator's frame to whatever resumed it, but the
// frame is the generator object's: allocated apart from the VM stack and given
// back when the generator closes. Released as part of the chain as well, every
// variable it held is given up twice, and the second one is on a value that has
// already gone back to whoever else was holding it.
//
// The inner request below is what actually fatals — a request cannot report on
// its own fatal — and it reports through a shutdown function, which the worker
// runs after it has released the abandoned frames.

$t = new TestCase('generator_fatal_releases_once', 'fibers');

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 10);
fwrite($sock, "GET /tests/fibers/fixture_generator_fatal.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

// Hooked: parks this request's fiber so the worker is free to serve the inner
// one. Without the park there is no worker to serve it — this profile runs one.
sleep(1);

$resp = (string) stream_get_contents($sock);
fclose($sock);

// Without this the rest proves nothing: a fixture that returned normally would
// leave no abandoned frames for the worker to get wrong.
$t->assertNotContains('the inner request did not survive its fatal', $resp, 'NOT-REACHED');

// The generator is resumed by a method call, and the frame of an internal call
// holds the object it was called on. Left there, the generator is never released
// at all — which is a leak of its own, and it also hides the case below.
$t->assertContains(
    'an internal call the fatal was inside gives back what it held',
    $resp,
    'GENERATOR-RELEASED:yes'
);

$t->assertContains(
    "a generator's variables are released once, not twice",
    $resp,
    'GENERATOR-CV-KEPT'
);

// The worker is still the one serving this request, so reaching the end at all
// says the fatal left it in one piece.
$t->assertTrue('the worker still serves this request', oxphp_is_worker());

$t->done();
