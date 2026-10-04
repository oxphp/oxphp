<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_in_usort_retires_the_worker, on the worker that
// replaced the one it ran on — whose statics are gone, so what it left is read
// from a file.

$t = new TestCase('bailout_in_usort_retires_the_worker_probe', 'fibers');

OxphpBailoutLeak::assertRetired($t, 'usort');

$t->done();
