<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// What a request leaves behind for its end to let go of — the closure of an
// async task it dispatched and never awaited, an object it put in $_GET — can
// hold an object whose destructor throws or raises a fatal error. Released
// while the request is still ending, that is the request's own failure: it
// answers 500. Released after the response has gone out — the closure of a
// task that was still running — it is nobody's, and the worker carries on.
// Either way the worker survives, and so does this request, parked on it
// meanwhile: the inner requests below land on the same worker while this one
// waits, which a worker that went down with the fatal would leave unanswered.
//
// ?case= unstarted | started | superglobal-throw | superglobal-fatal

$case = (string) ($_GET['case'] ?? '');
$t = new TestCase("fatal_releasing_what_a_request_left_keeps_the_worker ($case)", 'fibers');

$run = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . '/oxphp-release-left-' . $run;

$inner = static function (string $query) use ($t): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$query: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        return '';
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_fatal_releasing_what_a_request_left.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$query response", $resp);

    return $resp;
};

$resp = $inner("step=$case&run=$run");

if ($case === 'started') {
    // The closure goes once the cancelled task has stopped; usleep() parks
    // this request so the worker can get there.
    for ($i = 0; $i < 100 && !is_file("$base.ran"); $i++) {
        usleep(50_000);
    }
    $t->assertMatch('the inner request answered before the release', $resp, '#^HTTP/\S+ 200 #');
    $t->assertContains('after its handler ran to the end', $resp, 'DISPATCHED');
} else {
    $t->assertMatch("the failure is the inner request's own", $resp, '#^HTTP/\S+ 500 #');
}

// What the rest stands on: the destructor ran.
$t->assertTrue('the destructor of what the inner request left ran', is_file("$base.ran"));

// This request resumed, and the engine still calls what it is asked to.
$t->assertSame(
    'a callback runs in the request the worker resumed after it',
    array_map(static fn (int $n): int => $n * 2, [1, 2]),
    [2, 4]
);

$probe = $inner("step=probe&run=$run");
$t->assertMatch('the next request answers 200', $probe, '#^HTTP/\S+ 200 #');
$t->assertContains('and runs its handler', $probe, 'PROBE-RAN');
$t->assertContains('and a callback', $probe, 'callback: [2,4]');
$t->assertNotContains('with nothing of the failure in its body', $probe, 'OxphpReleaseLeftFatal');

// On this worker, not on one started to replace it.
$started = sprintf('%.6f', OxPHP\Server\Worker::current()->startTime());
$t->assertContains('on the worker serving this request', $probe, "worker started: $started");

foreach (glob("$base.*") ?: [] as $file) {
    unlink($file);
}

$t->done();
