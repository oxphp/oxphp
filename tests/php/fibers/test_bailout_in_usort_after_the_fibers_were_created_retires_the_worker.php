<?php

declare(strict_types=1);

// Once a worker has created its fibers, a request ended inside usort() that
// leaves more than the threshold behind retires it all the same.
//
// The allowance made for creating fibers is for the stretch in which they were
// created, and not for the rest of the worker's life: carried on, it would raise
// the threshold by the whole pool for good, and a leak of several MiB — more than
// the threshold, less than the threshold and the pool together — would stay.
// This runs on the worker the pair before it created its fibers on, which it
// says to its probe, because a probe on the replacement cannot tell.

require_once __DIR__ . '/bailout_leak_probe.php';

$burst = OxphpBailoutLeak::state('pool_small');
OxphpBailoutLeak::arm('pool_medium', [
    'pool_here' => ($burst['worker'] ?? null) === OxphpBailoutLeak::worker(),
]);
OxphpBailoutLeak::sortUntilTimeout(OxphpBailoutLeak::rows(OxphpBailoutLeak::MEDIUM_ROWS));

echo 'finished';
