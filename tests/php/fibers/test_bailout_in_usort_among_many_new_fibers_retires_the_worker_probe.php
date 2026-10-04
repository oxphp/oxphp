<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_in_usort_among_many_new_fibers_retires_the_worker,
// on the worker that replaced the one it ran on.

$t = new TestCase('bailout_in_usort_among_many_new_fibers_retires_the_worker_probe', 'fibers');

OxphpBailoutLeak::assertRetired($t, 'pool_big');

$t->done();
