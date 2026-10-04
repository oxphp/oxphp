<?php

declare(strict_types=1);

// A request ended inside usort() leaves the worker alone when what it leaves
// behind is small.
//
// The same trigger as test_bailout_in_usort_retires_the_worker over a few
// hundred rows. A worker recycled for every request that is ended inside an
// internal function would rotate under any load that cancels often; what a
// worker is recycled for is the heap the request left, not the fact that it was
// ended there.
//
// It runs after test_bailout_outside_an_internal_function_keeps_the_worker, which
// leaves the worker holding far more than the threshold: growth from a window
// before this one is not this request's to answer for.

require_once __DIR__ . '/bailout_leak_probe.php';

OxphpBailoutLeak::arm('usort_small');
OxphpBailoutLeak::sortUntilTimeout(OxphpBailoutLeak::rows(OxphpBailoutLeak::SMALL_ROWS));

echo 'finished';
