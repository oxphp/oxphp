<?php

declare(strict_types=1);
declare(ticks=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('sleep_in_tick_handler', 'fibers');

// A tick handler runs on a frame the VM entered on the ticked code's behalf, and
// the engine marks that frame as one a userland fiber may not switch away from.
// oxphp_sleep() is reachable from there all the same, so this pins what happens
// when it is called: a real sleep, nothing raised, and a request that is still
// usable and still on its fiber afterwards. Measured from outside with a single
// worker, the sleep holds the worker for its full duration — unlike the same
// sleep at top level, which frees it at once.
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
