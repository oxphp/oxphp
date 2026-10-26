<?php

declare(strict_types=1);

// A request whose cycle collection fatals inside a destructor still runs its
// shutdown functions in its own fiber, and a collection they ask for does
// nothing.
//
// A collection that runs inside a fiber calls destructors in a fiber of the
// collector's own. A fatal there is caught inside that fiber and
// raised again in the request with the engine's record of the current fiber
// cleared, which the worker puts back before the request's shutdown functions
// run.
//
// The collector stays where the fatal left it: the run it interrupted is still
// marked as running, so a later collection returns at once and never resumes
// the collector's fiber, which the fatal destroyed. That is why putting the
// record back does not reopen that fiber, and why the worker is retired for it
// all the same. A retiring worker ends the requests still parked on it, so this
// fatals itself: the suite line expects the 500 that answers, and what its
// shutdown function saw goes to a file for
// fibers/test_fatal_in_a_gc_destructor_keeps_the_request_fiber_current_probe,
// which the replacement worker serves.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

require_once __DIR__ . '/../engine_fatal.php';

if (!class_exists('OxphpGcDestructorFiberCurrentProbe', false)) {
    final class OxphpGcDestructorFiberCurrentProbe
    {
        /** Read by the probe on the next line. */
        public const STATE = '/tmp/oxphp-gc-destructor-fiber-current-state';

        /** The fiber the request runs in, and the one the destructor ran in, by id. */
        public static ?int $requestFiber = null;
        public static ?int $destructorFiber = null;

        public static function currentId(): ?int
        {
            $current = \Fiber::getCurrent();

            return $current === null ? null : spl_object_id($current);
        }
    }
}

if (!class_exists('OxphpGcDestructorFiberCurrentCycle', false)) {
    final class OxphpGcDestructorFiberCurrentCycle
    {
        public ?self $other = null;

        public function __destruct()
        {
            OxphpGcDestructorFiberCurrentProbe::$destructorFiber = OxphpGcDestructorFiberCurrentProbe::currentId();
            engine_fatal('OxphpGcDestructorFiberCurrent');
        }
    }
}

// is_file before unlink, not the silence operator: a registered error handler
// still runs under @.
if (is_file(OxphpGcDestructorFiberCurrentProbe::STATE)) {
    unlink(OxphpGcDestructorFiberCurrentProbe::STATE);
}
OxphpGcDestructorFiberCurrentProbe::$requestFiber = OxphpGcDestructorFiberCurrentProbe::currentId();
OxphpGcDestructorFiberCurrentProbe::$destructorFiber = null;
$workerStartTime = OxPHP\Server\Worker::current()->startTime();
error_clear_last();

register_shutdown_function(static function () use ($workerStartTime): void {
    $shutdownFiber = OxphpGcDestructorFiberCurrentProbe::currentId();
    $collected = gc_collect_cycles();

    file_put_contents(OxphpGcDestructorFiberCurrentProbe::STATE, json_encode([
        'last_error' => error_get_last()['message'] ?? null,
        'request_fiber' => OxphpGcDestructorFiberCurrentProbe::$requestFiber,
        'destructor_fiber' => OxphpGcDestructorFiberCurrentProbe::$destructorFiber,
        'shutdown_fiber' => $shutdownFiber,
        'collected' => $collected,
        'gc_running' => gc_status()['running'],
        'worker_start_time' => $workerStartTime,
        'exit_scheduled' => OxPHP\Server\Worker::current()->isExitScheduled(),
    ]));
});

$a = new OxphpGcDestructorFiberCurrentCycle();
$b = new OxphpGcDestructorFiberCurrentCycle();
$a->other = $b;
$b->other = $a;
unset($a, $b);
gc_collect_cycles();

echo "NOT-REACHED\n";
