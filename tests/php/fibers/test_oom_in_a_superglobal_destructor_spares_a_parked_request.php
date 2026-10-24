<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A destructor that runs out of memory while the worker runs it between two
// slices of its requests — here, on what a finished request left in $_GET,
// which the worker drops once that request's response is out — belongs to no
// request, and the request the worker resumes next goes on as if nothing had
// happened.
//
// From PHP 8.4.27 / 8.5.12 the engine reports running out of memory from a point
// where it can unwind the code that did, and keeps a note that it has until the
// request is shut down; while the note stands it calls no callback and runs no
// destructor. Left standing between two slices, the note and the exit() the
// engine unwinds with went to the next request resumed on the thread: it was
// unwound where it had parked, and answered with an empty body.
//
// This request is that next one: the inner request runs while it is parked, so
// it lands on the worker serving this one, and the worker resumes this one after
// dropping the inner request's superglobals.

$t = new TestCase('oom_in_a_superglobal_destructor_spares_a_parked_request', 'fibers');

$run = bin2hex(random_bytes(6));
$marker = sys_get_temp_dir() . '/oxphp-superglobal-oom-' . $run;

$inner = static function (string $query) use ($t): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$query: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        return '';
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_oom_in_a_superglobal_destructor.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    sleep(1);

    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$query response", $resp);

    return $resp;
};

$oom = $inner("step=oom&run=$run");

// What the rest stands on: the inner request ran to its end and answered, and
// the destructor of what it left in $_GET ran after that, while this request
// was parked.
$t->assertContains('the inner request answers 200', $oom, ' 200 OK');
$t->assertContains('and ran its handler to the end', $oom, 'OOM-ARMED');
$t->assertTrue('the destructor of what it left in $_GET ran', is_file($marker));

// This request resumed and the engine calls what it is asked to.
$t->assertSame(
    'a callback runs in the request the worker resumed after it',
    array_map(static fn (int $n): int => $n * 2, [1, 2]),
    [2, 4]
);

$probe = $inner('step=probe');
$t->assertContains('the next request runs its handler', $probe, 'PROBE-RAN');
$t->assertContains('and a callback', $probe, 'callback: [2,4]');
$t->assertContains('and its shutdown function', $probe, 'SHUTDOWN-RAN');

// On this worker, not on one started to replace it.
$started = sprintf('%.6f', OxPHP\Server\Worker::current()->startTime());
$t->assertContains('on the worker serving this request', $probe, "worker started: $started");

if (is_file($marker)) {
    unlink($marker);
}

$t->done();
