<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The request after the one whose recovery ran into a fatal of its own.
//
// Two halves, and both are needed. The worker that took the fatal has to be the
// one answering this — a worker that lost its serve loop is replaced by the
// pool, and the replacement counts this as its first request, so a request
// count above one is the loop having survived rather than the pool having
// covered for it. And the recovery has to have run to its end, not merely to
// have been survived: the last thing it does is lower the flag zend_bailout
// raises on the cycle collector, and only zend_activate lowers that flag
// otherwise — which worker mode runs once per worker, not once per request. A
// collector that still collects is that flag being down.

$t = new TestCase('worker_serves_after_filter_close_fatal', 'fiberobs');

$worker = OxPHP\Server\Worker::current();

$t->assertTrue('worker mode is on', OxPHP\Server\Worker::isWorkerMode());

// The walk has to have reached the handle, or the request before this one
// proved nothing: it would have been an ordinary fatal with no second one
// inside the recovery.
$t->assertTrue(
    'the release walk reached the stream the fatal abandoned',
    is_file('/tmp/oxphp-filter-close-marker')
);

$t->assertGreaterThan(
    'the worker that took the fatal is still serving',
    $worker->requestCount(),
    1
);

// Frames of this request go where the abandoned ones were, so anything the
// recovery left pointing up there is left pointing at these. Nested on purpose:
// one call deep would put a frame over the first of them and leave the rest as
// they were, which reads the same as a recovery that cleaned up after itself.
$fill = static function (int $depth) use (&$fill): int {
    $pad = str_repeat('x', 64);
    return $depth > 0 ? $fill($depth - 1) + strlen($pad) : 0;
};
$fill(24);

// A cycle nothing else can reach, so what the collector reports is this one.
$a = new stdClass();
$b = new stdClass();
$a->peer = $b;
$b->peer = $a;
unset($a, $b);

$t->assertGreaterThan(
    'the cycle collector still collects',
    gc_collect_cycles(),
    0
);

$t->done();
