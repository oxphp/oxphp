<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// ignore_user_abort(true) is honoured, and the write is what ends the request.
//
// The three aborts earlier in this suite establish that none of this counts
// against the worker. Whether each of them was already running when its client
// left rests on the timing of the suite lines, though, and their probe does not
// read the marker that would say the interrupt was declined.
//
// So: a single abort, against a worker that is free to take it. It is dispatched
// straight away, the disconnect raises an interrupt against a running request,
// and the setting is the only thing standing between that interrupt and an
// unwind. The past-sleep marker is on the far side of it.

$test = new TestCase('breaker_abort_ignored_honoured', 'breaker');

// The request ran on past the point where its client left. Absent means the
// interrupt handler unwound it there — which is the one thing
// ignore_user_abort(true) asks it not to do.
$test->assertTrue(
    'the handler outlived its client',
    is_file('/tmp/oxphp-breaker-abort-ignored-past-sleep')
);

// And then the write ended it anyway, which is the arm under test: the SAPI's
// output path reads the cancel cell the interrupt handler declined to act on.
$test->assertFalse(
    'the write is where it ended',
    is_file('/tmp/oxphp-breaker-abort-ignored-past-echo')
);

// Its shutdown functions still ran, as they do on every arm here.
$test->assertTrue(
    'the aborted request reached the end of its request',
    is_file('/tmp/oxphp-breaker-abort-ignored')
);

$worker = OxPHP\Server\Worker::current();

// The probe that observed the retire above, the single abort, and this request.
$test->assertSame('still the worker that answered the previous probe', $worker->requestCount(), 3);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no tenth recycle', $recycles['total'], 9);
    $test->assertSame('the abort did not trip the breaker', $recycles['error'], 9);
}

$test->done();
