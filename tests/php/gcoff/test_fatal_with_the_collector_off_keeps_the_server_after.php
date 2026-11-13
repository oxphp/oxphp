<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/gcoff_state.php';

// Follows gcoff/test_fatal_with_the_collector_off_keeps_the_server_probe, on the
// same worker: what a guard lowered over a buffer that does not exist comes to.
// A collection — this one, or the worker's own between requests — moves the
// collector's next free slot to the first of that buffer, and the next possible
// root is written there.

$t = new TestCase('fatal_with_the_collector_off_keeps_the_server_after', 'gcoff');

$t->assertSame(
    'this request runs on the worker the fatal ended a request on',
    OxPHP\Server\Worker::current()->startTime(),
    oxphp_gcoff_recorded_worker()
);

gc_collect_cycles();

// Each assignment leaves the object before it referred to by itself alone,
// which is what makes the engine offer it to the collector as a possible root.
$node = null;
for ($i = 0; $i < 2000; $i++) {
    $node = new \stdClass();
    $node->self = $node;
}
$node = null;
gc_collect_cycles();

$t->assertSame('the collector buffered none of them', gc_status()['roots'], 0);

$t->done();
