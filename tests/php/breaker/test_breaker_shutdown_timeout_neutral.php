<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three deadlines that expired in the shutdown window, and the worker is still
// the one that served them.
//
// The bounded half of the same guarantee the fatal blocks above assert: a
// request that came apart in its shutdown functions counts, and a request the
// server ended there does not — even though both arrive at exactly the same
// place, through the same swallowed bailout, with nothing but
// PHP_CONNECTION_TIMEOUT to tell them apart.
//
// Three rather than two, deliberately. Two would leave this green under any
// behaviour at all, because two is below the threshold; three is the number that
// retires a worker if these are counted, so the assertion has something to say.

$test = new TestCase('breaker_shutdown_timeout_neutral', 'breaker');

$worker = OxPHP\Server\Worker::current();

// The probe that opened this block, three deadlines, and this request.
$test->assertSame('still the worker that served them', $worker->requestCount(), 5);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no worker was recycled for them', $recycles['total'], 6);
    $test->assertSame('and none went for consecutive errors', $recycles['error'], 6);
}

$test->done();
