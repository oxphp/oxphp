<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/ini_probe_state.php';

// A directive that does not travel with its request is put back once the last
// request in flight has ended.
//
// fibers/test_ini_is_the_requests_own, just before this in the suite, had an
// inner request tighten open_basedir while the outer one was parked. That
// directive stays on the worker while any request is in flight, so the inner
// request could not put it back as it ended — the outer one was still running
// on it. The outer one was the last out, and put it back as it ended; this
// request is the next one in and must find the value the outer one found.
//
// A worker that goes idle puts every directive back in the reset it runs before
// its next request, which would pass this test with the end of the last request
// doing nothing. The outer request left a task running to keep this worker
// busy, so this request is taken without that reset — as long as the task has
// not finished, which is the first thing checked.

$t = new TestCase('ini_the_worker_keeps_goes_back_with_the_last_request', 'fibers');

$t->assertTrue(
    'the task the test before this one left is still running, so no reset ran ahead of this request',
    !is_file(OxphpIniProbeState::TASK_DONE)
);
$t->assertNotNull(
    'the test before this one recorded the baseline',
    OxphpIniProbeState::$basedirBaseline
);
$t->assertSame(
    'open_basedir is back at that baseline',
    ini_get('open_basedir'),
    OxphpIniProbeState::$basedirBaseline
);

$t->done();
