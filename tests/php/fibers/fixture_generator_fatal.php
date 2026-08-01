<?php

declare(strict_types=1);

// Inner self-request for fibers/test_generator_fatal_releases_once.
//
// A fatal raised while a generator is running. Resuming a generator links its
// frame into the chain the fatal is reported from, but that frame is not part of
// what the fatal abandoned: it belongs to the generator object, which gives its
// variables back when it closes. A worker that releases the abandoned chain as
// it stands gives them back a second time — and the second time is on a value
// something else is still holding.
//
// The generator's variable is an object the class below holds as well, so one
// release leaves it there and two take it away. Which happened is read weakly:
// after a double release the strong reference is a reference to freed memory,
// and reading it would be the second bug rather than a report of the first.
//
// The generator is reported on the same way, because none of this is reached
// unless the generator is released at all. It is resumed by a method call, and
// the frame of an internal call holds the object it was called on — so a worker
// that leaves internal frames alone never lets the generator go, and the case
// above stays hidden behind a leak.
//
// Declared conditionally on purpose: the worker keeps this process across
// requests, and a second unconditional declaration of the same name would fail
// the request for a reason that has nothing to do with what is under test.

if (!class_exists('OxphpGeneratorFatalProbe', false)) {
    class OxphpGeneratorFatalProbe
    {
        /** The reference that has to outlive the fatal. */
        public static ?object $held = null;

        /** The same object, weakly — safe to read whichever way it went. */
        public static ?\WeakReference $weak = null;

        /** The generator itself, weakly. */
        public static ?\WeakReference $genWeak = null;
    }
}

if (!function_exists('oxphp_generator_that_fatals')) {
    function oxphp_generator_that_fatals(): \Generator
    {
        $held = new \stdClass();
        $held->tag = 'held-by-generator';
        OxphpGeneratorFatalProbe::$held = $held;
        OxphpGeneratorFatalProbe::$weak = \WeakReference::create($held);

        yield 1;

        // Raised with the generator's frame in the chain behind this one.
        trigger_error('fatal inside a running generator', E_USER_ERROR);

        yield 2; // not reached
    }

    function oxphp_run_generator_that_fatals(): void
    {
        // The generator is held by this frame and by the frame of the ->next()
        // below, and both are among the ones the fatal abandons — so it is
        // released inside the same fatal rather than at some later point that
        // would make this test a coin toss.
        $gen = oxphp_generator_that_fatals();
        OxphpGeneratorFatalProbe::$genWeak = \WeakReference::create($gen);
        $gen->current();
        $gen->next();
    }
}

// Runs after the worker has released what the fatal abandoned.
register_shutdown_function(static function (): void {
    echo 'GENERATOR-RELEASED:',
        OxphpGeneratorFatalProbe::$genWeak?->get() === null ? 'yes' : 'no', "\n";
    echo OxphpGeneratorFatalProbe::$weak?->get() !== null
        ? "GENERATOR-CV-KEPT\n"
        : "GENERATOR-CV-LOST\n";
});

oxphp_run_generator_that_fatals();

echo "NOT-REACHED\n";
