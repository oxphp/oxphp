<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_bailout_leak_retire_waits_for_a_parked_neighbour, on the
// worker that replaced the one it ran on.

$t = new TestCase('bailout_leak_retire_waits_for_a_parked_neighbour_probe', 'fibers');

OxphpBailoutLeak::assertRetired($t, 'neighbour');

$t->done();
