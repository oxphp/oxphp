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
// What this pins is the half a request can see of that: the fallback is a real
// sleep, raises nothing, and leaves the request usable. The decline itself shows
// up only from outside — with one worker, a 0.5s sleep inside a tick handler
// holds that worker for 0.37s while the same sleep at top level frees it in
// 0.03s, and without the refusal the tick-handler sleep frees it too. That
// belongs to a measurement, not to an assertion here: it would pin the worker
// being blocked as a contract, when the point is only that the fiber does not
// leave a frame the engine is running on someone else's behalf.
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
