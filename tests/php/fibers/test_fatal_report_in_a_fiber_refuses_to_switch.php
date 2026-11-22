<?php

declare(strict_types=1);

// A fatal error raised inside a Fiber is displayed with the server's parking held
// down: between the server's snapshot of the objects alive and the engine
// flagging them all on its way into the bailout, nothing else may run. An output
// handler the message passes through can suspend that Fiber, which would leave
// the report half done and the hold standing over whatever the Fiber's resumer
// runs next, for as long as the Fiber stays suspended. Switching is refused
// there instead, as the engine refuses it in a tick handler.
//
// The handler records what its Fiber::suspend() did, and the resumer whether it
// got control back with the report unfinished; the probe that follows reads
// both. The fatal is raised with nothing being compiled: a FiberError thrown
// while PHP compiles is raised as a fatal error of its own instead
// (zend_throw_error()), which would end the handler before it could say what it
// got — so not engine_fatal(). A class declared at run time fatals once its name
// is taken, and no error handler is handed that; declared twice so that it
// fatals on a worker no earlier request has declared it on.

if (!function_exists('oxphp_report_switch_fatal')) {
    function oxphp_report_switch_fatal(): void
    {
        class OxphpReportSwitchFatal
        {
        }
    }
}

ini_set('display_errors', '1');
$sharedState['report_switch_worker'] = true;
unset($sharedState['report_switch'], $sharedState['report_switch_resumed_early']);

$fiber = new Fiber(static function () use (&$sharedState): void {
    ob_start(static function (string $buffer) use (&$sharedState): string {
        if (!isset($sharedState['report_switch']) && str_contains($buffer, 'OxphpReportSwitchFatal')) {
            try {
                Fiber::suspend();
                $sharedState['report_switch'] = 'suspended';
            } catch (\FiberError $e) {
                $sharedState['report_switch'] = 'FiberError: ' . $e->getMessage();
            }
        }

        return $buffer;
    }, 1);
    oxphp_report_switch_fatal();
    oxphp_report_switch_fatal();
});

$fiber->start();
if ($fiber->isSuspended()) {
    // Resumed rather than left suspended, so that the report finishes and the
    // fatal ends this request as it would have.
    $sharedState['report_switch_resumed_early'] = true;
    $fiber->resume();
}

echo 'finished';
