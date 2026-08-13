<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// And the same conclusion for the fatal raised while the registry is freed
// rather than while it is called. Three of them retire the worker too.
//
// A separate block because it is a separate swallow: the call site's bailout is
// contained by the zend_try inside php_call_shutdown_functions, the free site's
// by the zend_catch inside php_free_shutdown_functions. Each leaves the worker
// the same way and each needs its own witness.

$test = new TestCase('breaker_shutdown_dtor_retires', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('a sixth worker was recycled', $recycles['total'], 6);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 6);
}

$test->done();
