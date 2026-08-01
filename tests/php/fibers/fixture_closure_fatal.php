<?php

declare(strict_types=1);

// Inner self-request for fibers/test_closure_fatal_releases_it.
//
// A fatal raised inside a closure call. The frame of a closure call holds a
// reference to the closure object — the engine takes one when it pushes the
// frame and gives it back when the frame leaves, which is what lets a closure
// destroy itself mid-call. A frame the fatal abandons never leaves, so the
// worker has to give that reference back on its behalf; left there, the closure
// survives the request that made it, and with it everything it closed over.
//
// A closure declared here is a new object per request, so this is memory the
// worker keeps per fatal, not a reference count on something long-lived.
//
// Declared conditionally on purpose: the worker keeps this process across
// requests, and a second unconditional declaration of the same name would fail
// the request for a reason that has nothing to do with what is under test.

if (!class_exists('OxphpClosureFatalProbe', false)) {
    class OxphpClosureFatalProbe
    {
        /** The closure, weakly: reading it says whether it outlived its request. */
        public static ?\WeakReference $weak = null;
    }
}

if (!function_exists('oxphp_run_closure_that_fatals')) {
    function oxphp_run_closure_that_fatals(): void
    {
        $fn = static function (): void {
            trigger_error('fatal inside a closure', E_USER_ERROR);
        };

        OxphpClosureFatalProbe::$weak = \WeakReference::create($fn);

        // Two references from here on: this frame's variable and the frame of
        // the call itself. Both are among the ones the fatal abandons, so the
        // closure is answered for inside the same fatal.
        $fn();
    }
}

// Runs after the worker has released what the fatal abandoned.
register_shutdown_function(static function (): void {
    echo OxphpClosureFatalProbe::$weak?->get() === null
        ? "CLOSURE-FREED\n"
        : "CLOSURE-HELD\n";
});

oxphp_run_closure_that_fatals();

echo "NOT-REACHED\n";
