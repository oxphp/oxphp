<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_first_stretch_keeps_the_worker, on the worker it
// ran on.

$t = new TestCase('bailout_first_stretch_keeps_the_worker_probe', 'fibers');

$state = OxphpBailoutLeak::state('first_stretch');
$t->assertTrue('the trigger was the first request of a worker of its own', ($state['fresh'] ?? false) === true);

// What it kept on purpose is more than the threshold: a threshold raised past it
// would make this test pass for nothing.
$t->assertTrue(
    'what it kept on purpose is held, and is more than the threshold',
    count(OxphpBailoutLeak::$retained) === 1
        && memory_get_usage() - ($state['heap'] ?? PHP_INT_MAX) > OxphpBailoutLeak::RETIRE_BYTES
);
OxphpBailoutLeak::assertKept($t, 'first_stretch');

$t->done();
