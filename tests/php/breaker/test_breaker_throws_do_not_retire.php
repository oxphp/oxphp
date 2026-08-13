<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three uncaught application exceptions in a row leave the worker serving.
//
// An exception that reaches the top is an application outcome: the engine
// unwound it cleanly, the request got its 500, and the worker's own state is
// intact. Counting it as a worker defect makes a dependency that is failing for
// every request rotate the whole pool three requests at a time — and the same
// arm carries the exceptions an application's error handler raises out of the
// request's own input parse, which any client can provoke by posting a body
// over max_input_vars.
//
// The one recycle counted here is the breaker trip from earlier in the suite;
// it must not have moved.

$test = new TestCase('breaker_throws_do_not_retire', 'breaker');

$worker = OxPHP\Server\Worker::current();

// The replacement worker's own count: the probe that observed it, 3 throws,
// and this request.
$test->assertSame('still the worker that replaced the retired one', $worker->requestCount(), 5);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no second recycle', $recycles['total'], 1);
    $test->assertSame('uncaught exceptions did not trip the breaker', $recycles['error'], 1);
}

$test->done();
