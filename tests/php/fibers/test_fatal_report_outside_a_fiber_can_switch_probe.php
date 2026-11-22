<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_fatal_report_outside_a_fiber_can_switch, on the worker it
// ran on.

$sameWorker = ($sharedState['report_outside_worker'] ?? false) === true;
$inReport = $sharedState['report_outside_switch'] ?? null;
$inFiber = $sharedState['report_outside_in_fiber'] ?? null;
unset($sharedState['report_outside_worker'], $sharedState['report_outside_switch'], $sharedState['report_outside_in_fiber']);

$t = new TestCase('fatal_report_outside_a_fiber_can_switch_probe', 'fibers');
$t->assertTrue('this runs on the worker whose request ended in the fatal', $sameWorker);
$t->assertTrue('premise: the report ran inside the Fiber the request runs as', $inFiber === true);
$t->assertSame('a Fiber an output handler the report ran starts runs as usual', $inReport, 'started');

$t->done();
