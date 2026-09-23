<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// A completed request clears the run even when its response went nowhere.
//
// The four requests before this one are fatal, fatal, a request that finished
// with its client already gone, then fatal. Whether the worker is still the one
// that served them says which way the third was read: it clears the run and the
// last fatal is the first of a new one, or it is read as a cancellation, leaves
// the count where it was, and the last fatal is the third in a row and retires
// the worker.
//
// The distinction is the whole of it. Nothing in that third request failed —
// the handler ran to its end and its response was complete. All that went wrong
// was the server's own flush of that response onto a connection whose client
// had left, which is a fact about the client. Reading it as neutral lets the count
// carry across arbitrarily many healthy requests: a worker that fataled twice
// an hour ago would be retired by a single fatal now, provided the requests in
// between had all been served to clients who hung up. Under the abort storm
// this whole suite is about, that is not a rare arrangement.

$test = new TestCase('breaker_abort_buffered_clears', 'breaker');

// That the request under test really did reach its end, rather than dying
// somewhere in the middle and clearing nothing for a different reason.
$test->assertTrue(
    'the buffered request reached its end',
    is_file('/tmp/oxphp-breaker-abort-buffered')
);

$worker = OxPHP\Server\Worker::current();

// The previous probe, the two fatals, the buffered request, the last fatal, and
// this request. A fresh worker answering 1 here is the retire this must not do.
$test->assertSame('the worker survived the last fatal', $worker->requestCount(), 8);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no tenth recycle', $recycles['total'], 9);
    $test->assertSame('the run was cleared, not carried', $recycles['error'], 9);
}

$test->done();
