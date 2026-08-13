<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three requests the server ended itself — deadlines and a client that hung up
// — leave the worker serving.
//
// A cancellation is the server deciding a request is over, not the handler
// failing at it, and every one of them unwinds as a bailout. Counting them
// means a dependency that has gone slow retires a worker every three requests
// and re-runs the whole application bootstrap each time, right when the
// application is least able to afford it, and it means a proxy with a short
// read timeout can do the same from outside.
//
// The recycles counted here are the breaker trip from earlier in the suite; it
// must not have moved.

$test = new TestCase('breaker_cancels_do_not_retire', 'breaker');

// First: that the abort above was a cancellation at all. Its own suite line can
// only say curl gave up, which it would do just as happily against a server that
// never noticed — and then the neutrality this test reports would be the
// neutrality of an ordinary completed request. The marker is written by a
// shutdown function on the cancelled request and carries the connection state
// the interrupt handler set; PHP_CONNECTION_ABORTED is 1.
$marker = @file_get_contents('/tmp/oxphp-breaker-abort');
$test->assertTrue('the aborted request ran its shutdown function', is_string($marker) && $marker !== '');

// A request that ran to completion records CONNECTION_NORMAL (0), so an absent
// or unset bit fails either way.
$status = is_string($marker) && $marker !== '' ? (int) $marker : 0;
$test->assertSame(
    'the abort reached PHP as a cancellation',
    $status & CONNECTION_ABORTED,
    CONNECTION_ABORTED
);

$worker = OxPHP\Server\Worker::current();

// The worker that replaced the retired one: the probe that observed it, 3
// throws, the probe after them, 2 timeouts, 1 abort, and this request.
$test->assertSame('still the worker that replaced the retired one', $worker->requestCount(), 9);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no second recycle', $recycles['total'], 1);
    $test->assertSame('cancellations did not trip the breaker', $recycles['error'], 1);
}

$test->done();
