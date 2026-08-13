<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three handler fatals, each followed by a deadline expiring in its shutdown
// window, and the worker is gone anyway.
//
// The half that keeps the neutral case above from swallowing the whole
// mechanism. A deadline is neutral for the request it ended; it is not a pardon
// for a request that had already come apart before it. The two are told apart by
// whether the request was already failed when the shutdown window opened, not by
// the deadline bit alone — that bit is raised once and stands for the rest of the
// request, so by the end of the window it no longer says what ended what.
//
// Without the distinction any application could keep a permanently broken worker
// in the pool by registering one slow shutdown function, which is the case the
// whole profile is about.

$test = new TestCase('breaker_fatal_then_timeout_still_counts', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('a seventh worker was recycled', $recycles['total'], 7);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 7);
}

$test->done();
