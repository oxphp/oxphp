<?php

declare(strict_types=1);
declare(ticks=1);

// A request ended by its time limit inside a declare(ticks) handler leaves fiber
// switching unblocked on its worker. The engine blocks switching around the
// handler and lifts the block only when the handler returns; the block is the
// thread's, so a fatal that leaves the handler without returning would leave
// every Fiber the worker runs from then on throwing FiberError, and no request
// on it parking.
//
// The handler records whether starting a Fiber there throws, then spins until
// the time limit ends the request; the probe that follows checks a Fiber
// switches on the same worker.

$sharedState['tick_timeout_worker'] = true;
set_time_limit(1);

// A tick function outlives the request that registered it on a worker. The
// probe compiles no ticks, but later code that does would run this one again,
// so it spins only once.
register_tick_function(static function () use (&$sharedState): void {
    static $ran = false;
    if ($ran) {
        return;
    }
    $ran = true;

    try {
        (new Fiber(static fn () => null))->start();
        $sharedState['tick_timeout_switch_refused'] = false;
    } catch (\FiberError) {
        $sharedState['tick_timeout_switch_refused'] = true;
    }
    $spin = 0;
    while (true) {
        $spin++;
    }
});

$tick = 1;

echo 'finished';
