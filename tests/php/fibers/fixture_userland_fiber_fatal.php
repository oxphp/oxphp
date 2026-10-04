<?php

declare(strict_types=1);

// Inner self-request for fibers/test_fatal_inside_a_userland_fiber_keeps_the_worker.
//
// A fatal raised inside a fiber this request runs rather than in the request's
// own. The engine catches the bailout inside that fiber, destroys the fiber's
// VM stack on its way back to whoever started it, and only then bails out again
// there — so by the time the worker gives back what the request was holding, the
// frames the fatal was raised in are gone. Running out of memory in a recursion
// is the shape that matters: the fiber's stack has grown across many pages by
// then, and freeing that much lets the allocator unmap some of it, so following
// those frames faults rather than merely reading stale memory.
//
// What the worker can still give back is the request's own frames below the
// point that started the fiber, and this one holds an object there.
//
// ?in=nested runs the recursion one fiber further down, inside a fiber that a
// fiber started.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpUserlandFiberFatalProbe', false)) {
    final class OxphpUserlandFiberFatalProbe
    {
        /** What the request's frame was holding, weakly. */
        public static ?\WeakReference $weak = null;
    }
}

if (!function_exists('oxphp_userland_fiber_fatal_recurse')) {
    function oxphp_userland_fiber_fatal_recurse(int $depth): void
    {
        oxphp_userland_fiber_fatal_recurse($depth + 1);
    }
}

if (!function_exists('oxphp_userland_fiber_fatal_run')) {
    function oxphp_userland_fiber_fatal_run(string $in): void
    {
        // Held by this frame's variable and nothing else.
        $held = new \ArrayObject([str_repeat('x', 1 << 16)]);
        OxphpUserlandFiberFatalProbe::$weak = \WeakReference::create($held);

        $recurse = static function (): void {
            oxphp_userland_fiber_fatal_recurse(0);
        };

        if ($in === 'nested') {
            $outer = new \Fiber(static function () use ($recurse): void {
                (new \Fiber($recurse))->start();
            });
            $outer->start();
            return;
        }

        (new \Fiber($recurse))->start();
    }
}

OxphpUserlandFiberFatalProbe::$weak = null;

// Cleared so the fatal below is one, rather than whatever a handler an earlier
// request on this worker installed makes of it.
set_error_handler(null);
error_clear_last();

// Runs after the worker has dealt with the abandoned frames.
register_shutdown_function(static function (): void {
    echo 'last error: ', error_get_last()['message'] ?? 'none', "\n";
    echo OxphpUserlandFiberFatalProbe::$weak?->get() === null ? "HELD-FREED\n" : "HELD-KEPT\n";
});

oxphp_userland_fiber_fatal_run($_GET['in'] ?? 'fiber');

echo "NOT-REACHED\n";
