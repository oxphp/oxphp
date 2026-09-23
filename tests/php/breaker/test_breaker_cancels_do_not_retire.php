<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Two requests the server ended itself and one whose client hung up leave the
// worker serving.
//
// A deadline is the server deciding a request is over, not the handler failing
// at it, and it unwinds as a bailout. Counting one means a dependency that has
// gone slow retires a worker every three requests and re-runs the whole
// application bootstrap each time, right when the application is least able to
// afford it. A client hanging up does not end a worker-mode request that is not
// streaming at all: it runs to its own end and is read by how it ended, which
// here is completing — so a proxy with a short read timeout cannot retire a
// worker from outside either.
//
// The recycles counted here are the breaker trip from earlier in the suite; it
// must not have moved.

$test = new TestCase('breaker_cancels_do_not_retire', 'breaker');

// First: that the client's departure reached PHP at all. Its own suite line can
// only say curl gave up, which it would do just as happily against a server that
// never noticed. The marker is written by a shutdown function on that request
// and carries the connection state the interrupt handler set;
// PHP_CONNECTION_ABORTED is 1.
$marker = @file_get_contents('/tmp/oxphp-breaker-abort');
$test->assertTrue('the aborted request ran its shutdown function', is_string($marker) && $marker !== '');

// A request whose disconnect never reached PHP records CONNECTION_NORMAL (0), so
// an absent or unset bit fails either way.
$status = is_string($marker) && $marker !== '' ? (int) $marker : 0;
$test->assertSame(
    'the abort reached PHP',
    $status & CONNECTION_ABORTED,
    CONNECTION_ABORTED
);

// And that knowing did not end it: the request ran on past the write to its
// gone client, to the end of its script.
$test->assertTrue(
    'and the request ran on past its write to the end',
    is_file('/tmp/oxphp-breaker-abort-past-echo')
);

$worker = OxPHP\Server\Worker::current();

// The worker that replaced the retired one: the probe that observed it, 3
// throws, the probe after them, 2 timeouts, 1 abort, and this request.
$test->assertSame('still the worker that replaced the retired one', $worker->requestCount(), 9);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no second recycle', $recycles['total'], 1);
    $test->assertSame('none of the three tripped the breaker', $recycles['error'], 1);
}

$test->done();
