<?php

declare(strict_types=1);

// A request ended by its time limit inside array_multisort() leaves fiber
// switching unblocked on its worker. The sort runs with switching blocked, and
// the block is the thread's: had the end of the request skipped lifting it,
// every Fiber the worker ran from then on would throw FiberError.
//
// The first comparison records whether starting a Fiber there throws, then
// spins until the time limit ends the request; the probe that follows checks a
// Fiber switches on the same worker.

require_once __DIR__ . '/multisort_probe.php';

$sharedState['multisort_timeout_worker'] = true;
set_time_limit(1);
multisort_probe_sort(static function () use (&$sharedState): void {
    try {
        (new Fiber(static fn () => null))->start();
        $sharedState['multisort_timeout_switch_refused'] = false;
    } catch (\FiberError) {
        $sharedState['multisort_timeout_switch_refused'] = true;
    }
    $spin = 0;
    while (true) {
        $spin++;
    }
});

echo 'finished';
