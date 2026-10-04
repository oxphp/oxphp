<?php

declare(strict_types=1);

// A fatal raised inside a destructor the cycle collector calls during a request
// ends the request, not the server.
//
// Since PHP 8.4 a collection that runs inside a fiber calls destructors in a
// fiber of the collector's own, and a worker-mode request runs inside one.
// So this takes the route a fatal inside a userland Fiber takes — see
// fibers/test_fatal_inside_a_userland_fiber_keeps_the_worker — with no Fiber in
// the script: the collector's fiber is destroyed before the request is handed
// the bailout, and the worker must not follow the frames that were on it.
// Running out of memory in a recursion is what makes following them fault.
//
// A fatal in the middle of a collection retires the worker, and a retiring
// worker ends the requests still parked on it — so this cannot be an inner request
// read back by an outer one on the same worker. It fatals itself instead: the
// suite line expects the 500 that answers, and what the request saw is written
// to a file for fibers/test_fatal_in_a_gc_destructor_keeps_the_server_probe,
// which is served next by the worker that replaces this one. That it is served
// at all is the half the fix is about.

if (!class_exists('OxphpGcDestructorFatalProbe', false)) {
    final class OxphpGcDestructorFatalProbe
    {
        /** Read by the probe on the next line. */
        public const STATE = '/tmp/oxphp-gc-destructor-fatal-state';

        /** What the request's frame was holding, weakly. */
        public static ?\WeakReference $weak = null;

        /** The fiber the request runs in, and the one the destructor ran in, by id. */
        public static ?int $requestFiber = null;
        public static ?int $destructorFiber = null;
    }
}

if (!function_exists('oxphp_gc_destructor_fatal_recurse')) {
    function oxphp_gc_destructor_fatal_recurse(int $depth): void
    {
        oxphp_gc_destructor_fatal_recurse($depth + 1);
    }
}

if (!class_exists('OxphpGcDestructorFatalCycle', false)) {
    final class OxphpGcDestructorFatalCycle
    {
        public ?self $other = null;

        public function __destruct()
        {
            $current = \Fiber::getCurrent();
            OxphpGcDestructorFatalProbe::$destructorFiber = $current === null ? null : spl_object_id($current);
            oxphp_gc_destructor_fatal_recurse(0);
        }
    }
}

if (!function_exists('oxphp_gc_destructor_fatal_run')) {
    function oxphp_gc_destructor_fatal_run(): void
    {
        // Held by this frame's variable and nothing else.
        $held = new \ArrayObject([str_repeat('x', 1 << 16)]);
        OxphpGcDestructorFatalProbe::$weak = \WeakReference::create($held);

        $current = \Fiber::getCurrent();
        OxphpGcDestructorFatalProbe::$requestFiber = $current === null ? null : spl_object_id($current);

        $a = new OxphpGcDestructorFatalCycle();
        $b = new OxphpGcDestructorFatalCycle();
        $a->other = $b;
        $b->other = $a;
        unset($a, $b);
        gc_collect_cycles();
    }
}

// is_file before unlink, not the silence operator: a registered error handler
// still runs under @.
if (is_file(OxphpGcDestructorFatalProbe::STATE)) {
    unlink(OxphpGcDestructorFatalProbe::STATE);
}
OxphpGcDestructorFatalProbe::$weak = null;
OxphpGcDestructorFatalProbe::$requestFiber = null;
OxphpGcDestructorFatalProbe::$destructorFiber = null;

// Cleared so the fatal below is one, rather than whatever a handler an earlier
// request on this worker installed makes of it.
set_error_handler(null);
error_clear_last();

// Runs after the worker has dealt with the abandoned frames.
register_shutdown_function(static function (): void {
    file_put_contents(OxphpGcDestructorFatalProbe::STATE, json_encode([
        'last_error' => error_get_last()['message'] ?? null,
        'freed' => OxphpGcDestructorFatalProbe::$weak !== null && OxphpGcDestructorFatalProbe::$weak->get() === null,
        'request_fiber' => OxphpGcDestructorFatalProbe::$requestFiber,
        'destructor_fiber' => OxphpGcDestructorFatalProbe::$destructorFiber,
    ]));
});

oxphp_gc_destructor_fatal_run();

echo "NOT-REACHED\n";
