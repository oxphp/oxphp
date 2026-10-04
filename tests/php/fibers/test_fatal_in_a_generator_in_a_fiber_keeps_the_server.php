<?php

declare(strict_types=1);

// A fatal raised while a generator was running inside a Fiber ends the request,
// not the server — and does not leave the server to come apart later, which is
// the part this covers.
//
// The generator's own frame is on the heap, so it outlives the fiber the fatal
// ended. The call it was in the middle of making is not: that frame was pushed
// on the fiber's VM stack, and the engine frees those pages on its way out
// without releasing anything on them. Whoever closes the generator afterwards
// walks the calls it had half pushed — the engine skips that walk only while its
// unclean-shutdown flag is up, and a worker has to lower that flag to serve
// anything else. So the walk happens, over pages that have gone back to the
// allocator, and it writes to them as well as reading them.
//
// `yield strtoupper(...)` is the shape, not decoration: the call to strtoupper
// is pushed before its argument is computed, so it stays pending for as long as
// the recursion inside it runs, and a pending call is the whole problem. A bare
// `yield recurse()` leaves none, and the close then touches nothing but the
// generator's own frame. Running out of memory is what makes the pages the
// pending call sits on get unmapped rather than merely reused.
//
// Nothing refers to the generator once the request is over — the frames that did
// are the fiber's own, which the engine freed without releasing them — so the
// close comes from the object store being torn down when the worker goes. The
// worker is asked to go for that reason, and because a request that fatals
// inside a fiber leaves that fiber unable to answer Fiber::getCurrent(), which
// later lines on this worker would otherwise inherit.
//
// The shutdown function below fills the pages the fiber's stack went back to
// before asking the worker to go, and that is load-bearing rather than tidying
// up after itself. Freed pages nothing has claimed since still read as the
// frames that were on them, so a close that walks them finds exactly what it
// expects and the only damage is the stack top it leaves pointing into them.
// Hand them to anything else first and the same walk reads a function pointer
// out of someone else's bytes and follows it. Which of the two a build gets is
// the allocator's choice, not the server's, so this asks for the one that says
// so.
//
// A worker on its way out ends the requests still parked on it, so this cannot
// be an inner request read back by an outer one on the same worker. It fatals
// itself: the suite line expects the 500 that answers, and what the request saw
// goes to a file for
// fibers/test_fatal_in_a_generator_in_a_fiber_keeps_the_server_probe, which the
// replacement worker serves. That it is served at all is the half the fix is
// about.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpGeneratorFiberFatalProbe', false)) {
    final class OxphpGeneratorFiberFatalProbe
    {
        /** Read by the probe on the next line. */
        public const STATE = '/tmp/oxphp-generator-fiber-fatal-state';

        /** What the request's frame was holding, weakly. */
        public static ?\WeakReference $weak = null;

        /** How far the generator got before the fatal came. */
        public static bool $generatorStarted = false;
        public static bool $innerCallEntered = false;
        public static bool $yielded = false;

        /** When the worker this ran on was spawned, for the probe to compare. */
        public static float $workerStartTime = 0.0;
    }
}

if (!function_exists('oxphp_generator_fiber_fatal_recurse')) {
    function oxphp_generator_fiber_fatal_recurse(int $depth): string
    {
        if ($depth === 0) {
            OxphpGeneratorFiberFatalProbe::$innerCallEntered = true;
        }
        return oxphp_generator_fiber_fatal_recurse($depth + 1);
    }
}

if (!function_exists('oxphp_generator_fiber_fatal_gen')) {
    function oxphp_generator_fiber_fatal_gen(): \Generator
    {
        OxphpGeneratorFiberFatalProbe::$generatorStarted = true;
        yield strtoupper(oxphp_generator_fiber_fatal_recurse(0));
    }
}

if (!function_exists('oxphp_generator_fiber_fatal_run')) {
    function oxphp_generator_fiber_fatal_run(): void
    {
        // Held by this frame's variable and nothing else. This frame is below
        // the call that enters the fiber, which is the part the worker can still
        // give back.
        $held = new \ArrayObject([str_repeat('x', 1 << 16)]);
        OxphpGeneratorFiberFatalProbe::$weak = \WeakReference::create($held);

        $fiber = new \Fiber(static function (): void {
            foreach (oxphp_generator_fiber_fatal_gen() as $value) {
                OxphpGeneratorFiberFatalProbe::$yielded = $value !== null;
            }
        });
        $fiber->start();
    }
}

// is_file before unlink, not the silence operator: a registered error handler
// still runs under @.
if (is_file(OxphpGeneratorFiberFatalProbe::STATE)) {
    unlink(OxphpGeneratorFiberFatalProbe::STATE);
}
OxphpGeneratorFiberFatalProbe::$weak = null;
OxphpGeneratorFiberFatalProbe::$generatorStarted = false;
OxphpGeneratorFiberFatalProbe::$innerCallEntered = false;
OxphpGeneratorFiberFatalProbe::$yielded = false;
OxphpGeneratorFiberFatalProbe::$workerStartTime = OxPHP\Server\Worker::current()->startTime();

// Cleared so the fatal below is one, rather than whatever a handler an earlier
// request on this worker installed makes of it.
set_error_handler(null);
error_clear_last();

// Runs after the worker has dealt with the abandoned frames, and before it goes.
register_shutdown_function(static function (): void {
    // First, so the state written next reports it, and so the close this is
    // about happens at all: it comes from the object store being torn down.
    OxPHP\Server\Worker::current()->scheduleExit();

    file_put_contents(OxphpGeneratorFiberFatalProbe::STATE, json_encode([
        'last_error' => error_get_last()['message'] ?? null,
        'freed' => OxphpGeneratorFiberFatalProbe::$weak !== null
            && OxphpGeneratorFiberFatalProbe::$weak->get() === null,
        'generator_started' => OxphpGeneratorFiberFatalProbe::$generatorStarted,
        'inner_call_entered' => OxphpGeneratorFiberFatalProbe::$innerCallEntered,
        'yielded' => OxphpGeneratorFiberFatalProbe::$yielded,
        'worker_start_time' => OxphpGeneratorFiberFatalProbe::$workerStartTime,
        'exit_scheduled' => OxPHP\Server\Worker::current()->isExitScheduled(),
    ]));

    // Last, because this is the part that may not finish. Blocks the size of a
    // VM stack page, asked for in the order the allocator hands its lowest free
    // run out first — which is where the call the generator had half pushed sat,
    // since it was pushed before the recursion grew the stack above it. The byte
    // is picked so that reading a pointer out of one of them and following it
    // goes nowhere a process may go: 0x7f repeated is not a canonical address.
    // Well under the limit the recursion just hit, all of which went back when
    // it did, and dropped before this returns so the teardown runs unencumbered.
    $dirty = [];
    for ($i = 0; $i < 3000; $i++) {
        $dirty[] = str_repeat("\x7f", (1 << 14) - 32);
    }
    $dirty = null;
});

oxphp_generator_fiber_fatal_run();

echo "NOT-REACHED\n";
