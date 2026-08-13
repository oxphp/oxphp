<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three uncaught exceptions from shutdown functions, and the worker is still
// serving — the same answer this profile already gives for an exception the
// handler throws.
//
// The engine is intact either way: an exception unwinds cleanly, the request is
// answered 500 with its log line, and nothing about the worker needs replacing.
// The only difference is where the loop learns of it, and that must not change
// the verdict.

$test = new TestCase('breaker_shutdown_throws_do_not_retire', 'breaker');

$worker = OxPHP\Server\Worker::current();

// The probe that opened this block, three throws, and this request.
$test->assertSame('still the worker that served them', $worker->requestCount(), 5);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no worker was recycled for them', $recycles['total'], 7);
    $test->assertSame('and none went for consecutive errors', $recycles['error'], 7);
}

$test->done();
