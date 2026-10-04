<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_outside_an_internal_function_keeps_the_worker, on
// the worker it ran on.

$t = new TestCase('bailout_outside_an_internal_function_keeps_the_worker_probe', 'fibers');

// These are the rows the sort in the retiring case leaves behind, so what is held
// here is as much as that case leaves: a threshold raised past it would make this
// test pass for nothing, and the retiring case go red at the same time.
$state = OxphpBailoutLeak::state('loop');
$t->assertTrue(
    'what it kept on purpose is held, and weighs at least what its rows hold',
    count(OxphpBailoutLeak::$retained) === OxphpBailoutLeak::BIG_ROWS
        && memory_get_usage() - ($state['heap'] ?? PHP_INT_MAX)
            > OxphpBailoutLeak::BIG_ROWS * OxphpBailoutLeak::ROW_BYTES
);
OxphpBailoutLeak::assertKept($t, 'loop');

$t->done();
