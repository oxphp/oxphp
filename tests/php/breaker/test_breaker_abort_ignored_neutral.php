<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three clients hanging up on a handler that asked to outlive them leave the
// worker serving.
//
// The abort earlier in this suite reaches PHP through the interrupt handler,
// which marks the request as one the server ended. These do not. They asked to
// outlive their clients, so the handler records the disconnect and returns
// without unwinding — and in worker mode the write that follows no longer ends
// the request either: it runs on to the end of its handler with nobody reading
// what it writes, so its own cleanup gets to run. Either way this is the same
// event as the abort above — a client that went away — so it has to read the
// same way to the breaker. Counting it means a proxy with a short read timeout, or a
// flood of clients giving up at once, rotates the pool three requests at a time
// and re-runs the whole application bootstrap each time.
//
// The recycles asserted here are the trips from earlier in the suite; they must
// not have moved.

$test = new TestCase('breaker_abort_ignored_neutral', 'breaker');

// First: that the last of the aborts above reached PHP and got to the end of
// its request. Their suite lines can only say curl gave up, which it would do
// just as happily against a server that never noticed — and then the neutrality
// reported below would be the neutrality of three ordinary completed requests.
// The fixture clears both markers on the way in, so both statements below are
// about that request and not about an earlier one.
$test->assertTrue(
    'the aborted request reached the end of its request',
    is_file('/tmp/oxphp-breaker-abort-ignored')
);

// And that the write let it through rather than ending it, which is the whole
// point of this case: the line after the echo is reached only by a request that
// got past that write. Absent means the write ended the request where it stood,
// and every `finally` and destructor the handler still had ahead of it went
// unrun on a worker that carries on serving.
$test->assertTrue(
    'the request ran past the write to the end of its handler',
    is_file('/tmp/oxphp-breaker-abort-ignored-past-echo')
);

$worker = OxPHP\Server\Worker::current();

// The worker that replaced the one retired above: the probe that observed it,
// the three aborts, and this request.
$test->assertSame('still the worker that answered the previous probe', $worker->requestCount(), 5);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no ninth recycle', $recycles['total'], 8);
    $test->assertSame('the aborts did not trip the breaker', $recycles['error'], 8);
}

$test->done();
