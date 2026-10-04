<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_in_a_callback_over_buffered_cycles_keeps_the_worker,
// on the worker it ran on.

$t = new TestCase('bailout_in_a_callback_over_buffered_cycles_keeps_the_worker_probe', 'fibers');

// What the objects weighed while the request held them, and that the collector
// held every one of them as a possible root when the request was ended: a
// threshold raised past the first, or objects the collector did not hold for the
// second, would make this test pass for nothing or for another reason.
$state = OxphpBailoutLeak::state('buffered_cycles');
$t->assertTrue(
    'the objects it let go of weighed more than the threshold',
    ($state['held'] ?? 0) - ($state['heap'] ?? PHP_INT_MAX) > OxphpBailoutLeak::RETIRE_BYTES
);
$t->assertTrue(
    'and the collector held each of them as a possible root',
    ($state['roots'] ?? 0) >= OxphpBailoutLeak::CYCLES
);
OxphpBailoutLeak::assertKept($t, 'buffered_cycles');

$t->done();
