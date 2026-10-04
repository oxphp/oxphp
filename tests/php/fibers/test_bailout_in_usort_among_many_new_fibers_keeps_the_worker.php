<?php

declare(strict_types=1);

// A worker that had to create a great many fibers at once is not retired for
// that, when a request ended inside usort() in the same stretch left next to
// nothing behind.
//
// Every fiber keeps a VM stack of 16 KiB, a structure and an object for as long as
// the worker lives, and a worker that runs a couple of hundred requests at a time
// creates that many of them over a few milliseconds — more than the threshold a
// retire is judged by. That is growth the server asked for itself and nothing a
// request left behind, and a worker retired for it would be retired again on the
// replacement, which meets the same burst with an empty pool. The suite line
// before this one retired the worker this runs on, so the burst has fibers to
// create, which is asserted: a worker that already had them would pass this for
// nothing.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

$t = new TestCase('bailout_in_usort_among_many_new_fibers_keeps_the_worker', 'fibers');

OxphpBailoutLeak::arm('pool_small');
$grew = OxphpBailoutLeak::burst(OxphpBailoutLeak::SMALL_ROWS);
$t->assertTrue("the heap grew past the threshold over the burst ($grew bytes), though the sort left a fraction of it", $grew > OxphpBailoutLeak::RETIRE_BYTES);

$t->done();
