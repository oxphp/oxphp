<?php

declare(strict_types=1);

// A worker that had to create a great many fibers at once is still retired when
// a request ended inside usort() in that stretch left a lot behind.
//
// The fibers' stacks are the worker's own growth and are not counted against a
// request, but they must not hide a leak either: this is the case of the plain
// retire, with the pool of fibers growing by its full size over the same stretch.
// The suite line before this one retired the worker this runs on, so its pool
// starts nearly empty, which is what makes the burst create them.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

$t = new TestCase('bailout_in_usort_among_many_new_fibers_retires_the_worker', 'fibers');

OxphpBailoutLeak::arm('pool_big');
$grew = OxphpBailoutLeak::burst(OxphpBailoutLeak::BIG_ROWS);
$t->assertTrue("the heap grew past the threshold over the burst ($grew bytes)", $grew > OxphpBailoutLeak::RETIRE_BYTES);

$t->done();
