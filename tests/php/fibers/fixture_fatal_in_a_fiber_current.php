<?php

declare(strict_types=1);

// Inner self-request for fibers/test_request_fiber_stays_current_after_a_fatal_in_a_fiber.
//
// ?step=ident reports the fiber it runs in, then has the cycle collector call a
// destructor and reports the fiber that destructor ran in. ?step=fatal reports
// its fiber and then fatals inside a Fiber it starts; its shutdown function
// reports the fiber it runs in and the error that ended the request.
//
// Each report is one line of JSON, so the test can tell them apart from
// whatever the fatal itself prints.
//
// Must not suspend: every step has to run and finish inside the window the
// outer request is parked for.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

require_once __DIR__ . '/../engine_fatal.php';

if (!function_exists('oxphp_fatal_in_a_fiber_current_id')) {
    function oxphp_fatal_in_a_fiber_current_id(): ?int
    {
        $current = \Fiber::getCurrent();

        return $current === null ? null : spl_object_id($current);
    }
}

if (!class_exists('OxphpFatalInAFiberCurrentCycle', false)) {
    final class OxphpFatalInAFiberCurrentCycle
    {
        public static bool $destructed = false;
        public static ?int $destructorFiber = null;

        public ?self $other = null;

        public function __destruct()
        {
            self::$destructed = true;
            self::$destructorFiber = oxphp_fatal_in_a_fiber_current_id();
        }
    }
}

if (($_GET['step'] ?? '') === 'fatal') {
    error_clear_last();

    register_shutdown_function(static function (): void {
        echo json_encode([
            'shutdown_fiber' => oxphp_fatal_in_a_fiber_current_id(),
            'last_error' => error_get_last()['message'] ?? null,
        ]), "\n";
    });

    echo json_encode(['fiber' => oxphp_fatal_in_a_fiber_current_id()]), "\n";

    (new \Fiber(static function (): void {
        engine_fatal('OxphpFatalInAFiberCurrent');
    }))->start();

    echo "NOT-REACHED\n";

    return;
}

$fiber = oxphp_fatal_in_a_fiber_current_id();

OxphpFatalInAFiberCurrentCycle::$destructed = false;
OxphpFatalInAFiberCurrentCycle::$destructorFiber = null;
$a = new OxphpFatalInAFiberCurrentCycle();
$b = new OxphpFatalInAFiberCurrentCycle();
$a->other = $b;
$b->other = $a;
unset($a, $b);
gc_collect_cycles();

echo json_encode([
    'fiber' => $fiber,
    'destructed' => OxphpFatalInAFiberCurrentCycle::$destructed,
    'destructor_fiber' => OxphpFatalInAFiberCurrentCycle::$destructorFiber,
]), "\n";
