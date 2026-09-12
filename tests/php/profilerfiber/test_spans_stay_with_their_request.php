<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A profiled request that parks must come back to its own profile.
//
// The profiler holds a request's span stack (Rust) and its observer state —
// mode, span counter, open-frame mirror, truncation flag (C) — in one slot per
// worker thread rather than one per request. Worker mode admits a new request
// while earlier ones are parked, so a second profiled request used to reset that
// slot out from under this one: the spans collected before the suspend were
// cleared, and the mode the neighbour's own teardown left behind decided what
// this request recorded after the resume.
//
// PHP_WORKERS=1, so the inner self-request below can only be served while this
// fiber is parked, which is exactly the window the defect needs. It is profiled
// in its own right and calls a function this request never calls, so a bleed
// shows up as the neighbour's frames rather than as emptiness.

$t = new TestCase('spans_stay_with_their_request', 'profilerfiber');

if (!function_exists('pf_outer_fn')) {
    function pf_outer_fn(int $n): int
    {
        return $n * 2;
    }
}

// The premise, read before anything else: this request really is recording a
// full profile. Without it every assertion below is satisfied by a run that
// profiled nothing.
$t->assertTrue('this request is being profiled', OxPHP\Profile\is_active());

$sum = 0;
for ($i = 0; $i < 10; $i++) {
    $sum += pf_outer_fn($i);
}
$t->assertSame('outer work ran before the suspend', $sum, 90);

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

fwrite($sock, "GET /tests/profilerfiber/fixture_inner_profiled.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "X-OxPHP-Profile: test-token\r\n"
    . "Connection: close\r\n\r\n");

sleep(2);                                   // hooked: parks this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

// The other edge of the window. Without this the rest proves nothing: if the
// neighbour never ran while this request was parked, no state was ever at risk.
$t->assertContains('the neighbour was served while this request was parked',
    $resp, 'INNER-PROFILED-OK');

// And the mechanism, read at the moment of measurement rather than inferred
// from the neighbour having finished: the observer is still recording for THIS
// request. The neighbour's teardown drops the thread's mode to Off, so a slot
// that did not travel with the fiber answers false here and everything below
// this line goes unrecorded.
$t->assertTrue('the observer is still recording after the resume',
    OxPHP\Profile\is_active());

for ($i = 0; $i < 10; $i++) {
    $sum += pf_outer_fn($i);
}
$t->assertSame('outer work ran after the resume', $sum, 180);

// What the run itself holds is read back by profilerfiber/test_runs_are_separate,
// which runs last: the index entry is written from a spawned task after this
// response is already on the wire.

$t->done();
