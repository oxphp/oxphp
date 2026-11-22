<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_fatal_report_in_a_fiber_refuses_to_switch, on the worker it
// ran on.

$sameWorker = ($sharedState['report_switch_worker'] ?? false) === true;
$inReport = $sharedState['report_switch'] ?? null;
$resumedEarly = $sharedState['report_switch_resumed_early'] ?? false;
unset($sharedState['report_switch_worker'], $sharedState['report_switch'], $sharedState['report_switch_resumed_early']);

$fiber = new Fiber(static fn () => Fiber::suspend('suspended'));
try {
    $suspended = $fiber->start();
} catch (\FiberError $e) {
    $suspended = 'FiberError: ' . $e->getMessage();
}
if ($fiber->isSuspended()) {
    $fiber->resume();
}

$t = new TestCase('fatal_report_in_a_fiber_refuses_to_switch_probe', 'fibers');
$t->assertTrue('this runs on the worker whose request ended in the fatal', $sameWorker);
$t->assertSame(
    'Fiber::suspend() in an output handler the report ran is refused',
    $inReport,
    'FiberError: Cannot switch fibers in current execution context'
);
$t->assertFalse('the resumer did not get control back with the report unfinished', $resumedEarly);
$t->assertSame('a fiber suspends once the report is over', $suspended, 'suspended');

$t->done();
