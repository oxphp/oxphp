<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// A fatal raised after the client left still counts.
//
// The three requests before this one each lost their client and then fataled in
// a shutdown function. Neither half is in doubt on its own: a client
// leaving is neutral, a fatal counts. What is in doubt is which of the two the
// request is filed as when both happen, and the answer has to be the fatal —
// the cancellation says the client stopped waiting, while the fatal says this
// worker's engine state is wreckage, and only one of those is a statement about
// the worker.
//
// Read the other way it is a lever: if the earlier cancellation won, an
// application whose handler fatals on every request would keep its worker for
// as long as clients kept hanging up on it. That is not a corner — it is the
// abort storm this whole suite is about, with a fatal underneath it.

$test = new TestCase('breaker_abort_then_shutdown_fatal_counts', 'breaker');

// That the shutdown function ran at all, so a failure below is about how the
// fatal was read and not about the fatal never having been raised.
$test->assertTrue(
    'the shutdown function ran',
    is_file('/tmp/oxphp-breaker-abort-then-shutdown-fatal')
);

$worker = OxPHP\Server\Worker::current();

// A fresh worker: this request is the first it has served. Anything else means
// the three fatals above were filed as the cancellations that preceded them.
$test->assertSame('the three retired the worker', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('a tenth recycle', $recycles['total'], 10);
    $test->assertSame('and it was the breaker that caused it', $recycles['error'], 10);
}

$test->done();
