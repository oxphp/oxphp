<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A request that runs out of memory after its handler has returned — in the
// callback of an output buffer it left open, on the flush that ends it — is the
// request that failed, and the next one on the worker runs as if nothing had
// happened.
//
// From PHP 8.4.27 / 8.5.12 the engine reports running out of memory from a point
// where it can unwind the code that did, and keeps a note that it has until the
// request is shut down. A worker does not shut its requests down; with the note
// left standing, the engine refuses to call anything on that thread, and the
// next request got an empty response without its handler having run.
//
// Both inner requests run while this one is parked, so they land on the worker
// serving it, one after the other.

$t = new TestCase('oom_in_an_output_callback_keeps_the_worker', 'fibers');

$inner = static function (string $step) use ($t): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$step: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        return '';
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_oom_in_an_output_callback.php?step=$step HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    sleep(1);

    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$step response", $resp);

    return $resp;
};

$oom = $inner('oom');
$probe = $inner('probe');

// What the rest stands on: the first inner request ran out of memory, and was
// answered as the fatal it was.
$t->assertContains('the request that ran out of memory answers 500', $oom, ' 500 Internal Server Error');

// The handler ran, and the shutdown function after it.
$t->assertContains('the next request runs its handler', $probe, 'PROBE-RAN');
$t->assertContains('and its shutdown function', $probe, 'SHUTDOWN-RAN');

// On this worker, not on one started to replace it.
$started = sprintf('%.6f', OxPHP\Server\Worker::current()->startTime());
$t->assertContains('on the worker serving this request', $probe, "worker started: $started");

$t->done();
