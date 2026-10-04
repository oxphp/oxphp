<?php

declare(strict_types=1);

// A request ended inside usort() retires the worker when what it leaves behind
// is large.
//
// usort() sorts a copy of the array it is given and swaps the copy in only once
// it is done, so a request that is ended in the middle of a comparator leaves
// that copy behind — and the copy holds a reference to every element, so none of
// them is ever freed. Nothing in the abandoned frames names it: an internal
// function's locals are not on any frame the walk can see. The time limit is
// what ends the request here, which is answered 504 and counted as no failure at
// all, so nothing but the heap it left can say the worker is worse for it. The
// probe on the next line reads that the worker was recycled on its own schedule.

require_once __DIR__ . '/bailout_leak_probe.php';

OxphpBailoutLeak::arm('usort');
OxphpBailoutLeak::sortUntilTimeout(OxphpBailoutLeak::rows(OxphpBailoutLeak::BIG_ROWS));

echo 'finished';
