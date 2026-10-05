<?php

declare(strict_types=1);

// A request ended inside an internal function over objects that refer to
// themselves, which the cycle collector does not hold, retires the worker.
//
// The engine raises the collector's guard for the whole of a bailout, and while
// it is up nothing is added to the buffer of possible roots. What the walk after
// the bailout lets go of therefore never reaches the collector, and an object
// that is kept alive by its own reference is then out of its sight for good:
// nothing frees it, which makes it a leak in the proper sense, and the check that
// runs the collector before it reads the heap counts it with the rest. Cycles
// the collector had buffered before the bailout are the other case, in
// fibers/test_bailout_in_a_callback_over_buffered_cycles_keeps_the_worker.

require_once __DIR__ . '/bailout_leak_probe.php';

// Kept out of the backtrace PHP 8.5 takes of a fatal, as on 8.4. With the
// arguments in it, array_map()'s closure — and the cycles it holds — would be
// held by that backtrace past the walk, and let go of only at the end of the
// request, once the collector's guard is down again: each object then goes into
// its buffer, the collection before the heap is read frees them, and nothing is
// left to leak. The walk has to be what lets go of them.
ini_set('zend.exception_ignore_args', '1');

OxphpBailoutLeak::arm('unbuffered_cycles');
OxphpBailoutLeak::mapUntilTimeout('unbuffered_cycles', false);

echo 'finished';
