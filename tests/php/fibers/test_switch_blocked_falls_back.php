<?php

declare(strict_types=1);
declare(ticks=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('switch_blocked_falls_back', 'fibers');

// The engine blocks fiber switching around a declare(ticks) handler — the VM
// wraps the call in zend_fiber_switch_block()/unblock(). A suspend point reached
// from inside such a handler must take its blocking path: switching away would
// return into a frame the VM is running on the ticked code's behalf, not the
// handler's. The other blocked context is pcntl's signal dispatch, which this
// suite cannot reach without the extension.
//
// What this pins is that the fallback is a real sleep, raises nothing, and
// leaves the request usable. It is a guard, not a proof that the suspension was
// refused: measured from outside with a single worker, a sleep inside a tick
// handler holds the worker for its full duration whether or not the suspend
// point declines — while the same sleep at top level frees the worker at once.
// So the decline is not observable from here, and this file does not claim it.
$state = ['ran' => false, 'elapsed' => 0.0, 'error' => null];

$tick = static function () use (&$state): void {
    if ($state['ran']) {
        return; // ticks=1 fires per statement; measure the first one only
    }
    $state['ran'] = true;

    $t0 = microtime(true);
    try {
        oxphp_sleep(0.05);
    } catch (\Throwable $e) {
        $state['error'] = $e::class . ': ' . $e->getMessage();
    }
    $state['elapsed'] = microtime(true) - $t0;
};

register_tick_function($tick);
$spin = 0;
$spin++;
unregister_tick_function($tick);

$t->assertTrue('the tick handler ran', $state['ran']);
$t->assertNull('sleeping inside a tick handler raises nothing', $state['error']);
$t->assertGreaterThan('sleeping inside a tick handler still sleeps', $state['elapsed'], 0.04);

// The request is still usable afterwards — whatever the suspend point did, it
// did not leave the fiber in a state the scheduler cannot drive.
$after = microtime(true);
oxphp_sleep(0.02);
$t->assertGreaterThan('the request can still sleep normally', microtime(true) - $after, 0.015);
$t->assertNotNull('the request still has its fiber', \Fiber::getCurrent());

$t->done();
