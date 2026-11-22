<?php

declare(strict_types=1);

// The other side of fibers/test_fatal_report_in_a_fiber_refuses_to_switch. A
// worker-mode request runs as a Fiber of its own, and a fatal error raised in it
// outside any Fiber the application started is displayed with switching left
// alone: the scheduler refuses a suspension of the request's Fiber at once, so
// the report cannot be left half done from there, and a Fiber an output handler
// starts runs as it would anywhere else.
//
// The handler records whether it ran inside a Fiber at all — the request's —
// and what starting a Fiber returned; the probe that follows reads both. The
// fatal is raised as in that test, and for the same reasons.

if (!function_exists('oxphp_report_outside_fatal')) {
    function oxphp_report_outside_fatal(): void
    {
        class OxphpReportOutsideFatal
        {
        }
    }
}

ini_set('display_errors', '1');
$sharedState['report_outside_worker'] = true;
unset($sharedState['report_outside_switch'], $sharedState['report_outside_in_fiber']);

ob_start(static function (string $buffer) use (&$sharedState): string {
    if (!isset($sharedState['report_outside_switch']) && str_contains($buffer, 'OxphpReportOutsideFatal')) {
        $sharedState['report_outside_in_fiber'] = Fiber::getCurrent() !== null;
        $fiber = new Fiber(static fn () => Fiber::suspend('started'));
        try {
            $sharedState['report_outside_switch'] = $fiber->start();
            $fiber->resume();
        } catch (\FiberError $e) {
            $sharedState['report_outside_switch'] = 'FiberError: ' . $e->getMessage();
        }
    }

    return $buffer;
}, 1);
oxphp_report_outside_fatal();
oxphp_report_outside_fatal();

echo 'finished';
