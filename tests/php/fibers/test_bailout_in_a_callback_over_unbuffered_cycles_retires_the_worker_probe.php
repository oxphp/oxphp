<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_in_a_callback_over_unbuffered_cycles_retires_the_worker,
// on the worker that replaced the one it ran on.

$t = new TestCase('bailout_in_a_callback_over_unbuffered_cycles_retires_the_worker_probe', 'fibers');

// The objects weighed more than the threshold and the collector held next to none
// of them as a possible root when the request was ended: with them buffered, this
// would be the case of the pair before it and the worker would be kept.
$state = OxphpBailoutLeak::state('unbuffered_cycles');
$t->assertTrue(
    'the objects it let go of weighed more than the threshold',
    ($state['held'] ?? 0) - ($state['heap'] ?? PHP_INT_MAX) > OxphpBailoutLeak::RETIRE_BYTES
);
$t->assertTrue(
    'and the collector held almost none of them as a possible root',
    ($state['roots'] ?? PHP_INT_MAX) < OxphpBailoutLeak::CYCLES / 10
);
OxphpBailoutLeak::assertRetired($t, 'unbuffered_cycles');

$t->done();
