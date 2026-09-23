<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// A client leaving does not unwind a worker-mode request that is not streaming,
// at the interrupt or at the write after it.
//
// The three aborts earlier in this suite establish that none of this counts
// against the worker. Whether each of them was already running when its client
// left rests on the timing of the suite lines, though, and their probe does not
// read the marker that would say the interrupt let the request go on.
//
// So: a single abort, against a worker that is free to take it. It is dispatched
// straight away and the disconnect raises an interrupt against a running
// request. The past-sleep marker is on the far side of that interrupt, and the
// past-echo marker on the far side of the write after it. The handler calls
// ignore_user_abort(true), but in worker mode a request that is not streaming
// is let through either way; what that setting decides is tested on a stream,
// in the fibers suite.

$test = new TestCase('breaker_abort_ignored_honoured', 'breaker');

// The request ran on past the point where its client left. Absent means the
// interrupt handler unwound it there.
$test->assertTrue(
    'the handler outlived its client',
    is_file('/tmp/oxphp-breaker-abort-ignored-past-sleep')
);

// And then the write let it through as well: the SAPI's output path reads the
// same cancel cell the interrupt handler let pass, and makes the same decision.
// Absent means the write ended the request where it stood.
$test->assertTrue(
    'the write is not where it ended either',
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
