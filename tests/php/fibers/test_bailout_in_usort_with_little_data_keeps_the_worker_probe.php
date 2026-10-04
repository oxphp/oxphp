<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_in_usort_with_little_data_keeps_the_worker, on the
// worker it ran on.

$t = new TestCase('bailout_in_usort_with_little_data_keeps_the_worker_probe', 'fibers');

OxphpBailoutLeak::assertKept($t, 'usort_small');

$t->done();
