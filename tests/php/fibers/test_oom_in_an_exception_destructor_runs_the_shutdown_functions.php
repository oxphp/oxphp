<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A request whose uncaught exception runs out of memory in its destructor — as
// the worker lets go of it, after the handler has returned — still runs its
// shutdown functions, as a request that runs out of memory in the handler does.
//
// From PHP 8.4.27 / 8.5.12 the engine reports running out of memory from a point
// where it can unwind the code that did, and keeps a note that it has until the
// request is shut down; while the note stands it refuses to call anything. The
// worker looked for that note before letting go of the exception rather than
// after, so the shutdown functions met it: on PHP 8.4 they were skipped without a
// word, on PHP 8.5 each was reported as a "Couldn't execute method" error.
//
// The inner request runs while this one is parked, so it lands on the worker
// serving it.

$t = new TestCase('oom_in_an_exception_destructor_runs_the_shutdown_functions', 'fibers');

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
if ($sock !== false) {
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_oom_in_an_exception_destructor.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    sleep(1);

    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta('response', $resp);

    $t->assertContains('the request answers 500', $resp, ' 500 Internal Server Error');
    // Both halves at once: the shutdown function ran, and it ran after the
    // destructor had run out of memory — nothing else in the request allocates
    // past the limit.
    $t->assertContains('its shutdown function runs, after the destructor ran out of memory', $resp, 'SHUTDOWN-RAN: Allowed memory size');
}

$t->done();
