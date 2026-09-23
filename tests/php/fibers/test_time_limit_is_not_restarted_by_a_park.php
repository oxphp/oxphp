<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/time_limit_probe.php';

// A request's time limit keeps counting across its parks.
//
// The ini directives a request changes are taken off the worker while it is
// parked and applied again when it resumes, so the requests the worker runs in
// between do not see them. max_execution_time is the exception, and has to be:
// the execution timer is one per thread, and its handler stops it whenever the
// value moves and starts it again from the full limit — so a limit that moved
// with the request would begin again from zero at every resume, and a request
// that parks often enough would never reach it.
//
// The inner request sets a one-second limit and spends about three seconds
// busy in short slices with a park after each. It has to be ended part-way, and
// it has to have parked along the way for that to say anything. A ticker request
// runs beside it and counts each time it gets to run; the inner request counts a
// sleep as a park only when the ticker ran across it.

$t = new TestCase('time_limit_is_not_restarted_by_a_park', 'fibers');

OxphpTimeLimitProbe::reset();

// Sent first and read last: it runs beside the inner request for as long as
// that one lasts.
$ticker = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('the ticker connected', $ticker !== false);
stream_set_timeout($ticker, 10);
fwrite($ticker, "GET /tests/fibers/fixture_time_limit_ticker.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

fiber_inner_request('/tests/fibers/fixture_time_limit_across_parks.php', 10.0);

$tickerBody = (string) stream_get_contents($ticker);
fclose($ticker);
$t->assertContains('the ticker ran to its end', $tickerBody, 'ticked');

$t->assertTrue(
    'the inner request parked more than once before it stopped ('
        . OxphpTimeLimitProbe::$parks . ' parks)',
    OxphpTimeLimitProbe::$parks > 1
);
$t->assertFalse(
    'and it was ended by its limit rather than running to the end',
    OxphpTimeLimitProbe::$finished
);
$t->assertTrue(
    'and the limit is what ended it: its shutdown functions saw the timeout bit',
    OxphpTimeLimitProbe::$timedOut
);

$t->done();
