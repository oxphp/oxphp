<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_fatal_in_a_tick_handler_lifts_the_switch_block, on the
// worker it ran on: a worker started afresh would begin with switching unblocked
// whatever the ended request left.

$sameWorker = ($sharedState['tick_timeout_worker'] ?? false) === true;
$refusedInTick = $sharedState['tick_timeout_switch_refused'] ?? null;
unset($sharedState['tick_timeout_worker'], $sharedState['tick_timeout_switch_refused']);

$fiber = new Fiber(static fn () => Fiber::suspend('suspended'));
try {
    $suspended = $fiber->start();
} catch (\FiberError $e) {
    $suspended = 'FiberError: ' . $e->getMessage();
}
if ($fiber->isSuspended()) {
    $fiber->resume();
}

$t = new TestCase('fatal_in_a_tick_handler_lifts_the_switch_block_probe', 'fibers');
$t->assertTrue('this runs on the worker whose request was ended', $sameWorker);
$t->assertSame('premise: switching was blocked in the tick handler the time limit ended', $refusedInTick, true);
$t->assertSame('a fiber suspends', $suspended, 'suspended');

$t->done();
