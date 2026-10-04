<?php

declare(strict_types=1);

// A request ended inside an internal function over objects that refer to
// themselves, which the cycle collector already holds as possible roots, does not
// retire the worker for them.
//
// Letting go of such objects frees nothing: each is kept alive by its own
// reference until the collector finds it, which is the garbage of the request
// ended and not something it leaked. What the function itself held —
// array_map() keeps the part of its result it had built, a few hundred bytes
// here — is all that is left behind, and it is far under the threshold. The
// collector is what tells the two apart, so it runs before the heap is read.
//
// It finds only what it has buffered. These objects are in its buffer because no
// collection ran in the request before the time limit ended it, and a request that
// built more than the collector's threshold of roots would have had one run, which
// takes the live ones out of the buffer and leaves what is released afterwards
// out of it: the case in the next file but one.

require_once __DIR__ . '/bailout_leak_probe.php';

OxphpBailoutLeak::arm('buffered_cycles');
OxphpBailoutLeak::mapUntilTimeout('buffered_cycles');

echo 'finished';
