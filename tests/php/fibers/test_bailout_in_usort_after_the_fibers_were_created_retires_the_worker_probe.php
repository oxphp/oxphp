<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_in_usort_after_the_fibers_were_created_retires_the_worker,
// on the worker that replaced the one it ran on.

$t = new TestCase('bailout_in_usort_after_the_fibers_were_created_retires_the_worker_probe', 'fibers');

$state = OxphpBailoutLeak::state('pool_medium');
$t->assertTrue('the trigger ran on the worker that created the fibers', ($state['pool_here'] ?? false) === true);
OxphpBailoutLeak::assertRetired($t, 'pool_medium');

$t->done();
