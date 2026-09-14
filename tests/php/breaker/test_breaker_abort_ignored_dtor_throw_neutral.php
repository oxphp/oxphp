<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three clients hanging up on a handler holding an object whose destructor
// throws leave the worker serving. See test_breaker_abort_ignored_dtor_throw.

$test = new TestCase('breaker_abort_ignored_dtor_throw_neutral', 'breaker');

$test->assertTrue(
    'the cleanup ran the destructor',
    is_file('/tmp/oxphp-breaker-dtor-throw-destructed')
);

$test->assertFalse(
    'the request unwound on the write, not after it',
    is_file('/tmp/oxphp-breaker-dtor-throw-past-echo')
);

$worker = OxPHP\Server\Worker::current();

// The worker from the block above: its probe's five, the three aborts, and this.
$test->assertSame('still the worker that answered the previous probe', $worker->requestCount(), 9);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no ninth recycle', $recycles['total'], 8);
    $test->assertSame('the destructors did not trip the breaker', $recycles['error'], 8);
}

$test->done();
