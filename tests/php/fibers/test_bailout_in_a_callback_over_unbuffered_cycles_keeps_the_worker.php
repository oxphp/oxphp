<?php

declare(strict_types=1);

// A request ended inside an internal function over objects that refer to
// themselves, which the cycle collector did not hold when the request was ended,
// does not retire the worker for them either.
//
// The engine raises the collector's guard for the whole of a bailout, and while
// it is up nothing is added to the buffer of possible roots. The worker lowers
// it for the walk that lets go of what the abandoned frames held, so what the
// walk lets go of reaches the collector like anything else a request drops, and
// the collection that runs before the heap is read frees it. Cycles the
// collector had buffered before the bailout are the other case, in
// fibers/test_bailout_in_a_callback_over_buffered_cycles_keeps_the_worker.

require_once __DIR__ . '/bailout_leak_probe.php';

// Kept out of the backtrace PHP 8.5 takes of a fatal, as on 8.4. With the
// arguments in it, array_map()'s closure — and the cycles it holds — would be
// held by that backtrace past the walk, and let go of only at the end of the
// request: they would reach the collector without the walk, and this would
// test nothing. The walk has to be what lets go of them.
ini_set('zend.exception_ignore_args', '1');

OxphpBailoutLeak::arm('unbuffered_cycles');
OxphpBailoutLeak::mapUntilTimeout('unbuffered_cycles', false);

echo 'finished';
