<?php

declare(strict_types=1);

// The guarded render staged by fibers/test_cancel_runs_userland_cleanup, and
// the reader its test looks at it through.
//
// Declared in a file of its own so both requests have it: the test reads the
// mark while the fixture's request is still standing on it, and a function has
// to be declared in a request before that request can reflect over it.
//
// The shape is taken from userland, not invented for the test. Code that
// renders something which may contain itself marks the thing on the way in,
// renders, and unmarks it on the way out, so that a nested render of the same
// thing can be recognised and refused. The mark lives in a static, because it
// has to outlive the call that set it and be visible to the calls below it. The
// unmark is put in `finally`, because that is the construct the language offers
// for "this runs however the block is left" — and it covers every way a block
// can be left except one.
//
// Nothing here is about output buffering, sessions, or any state the server
// owns. The state is userland's, the cleanup is userland's, and the only thing
// asked of the server is that it not take the request away between the two.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

require_once __DIR__ . '/write_cancel_probe.php';

if (!function_exists('oxphp_cancel_guarded_render')) {
    /**
     * Marks the worker, parks, writes, and unmarks in `finally`.
     *
     * The write is the point of it. A write belonging to a request whose client
     * has gone is where such a request used to be ended, and this one is
     * placed inside the guarded window on purpose: were it ended there,
     * `finally` would not run and the mark would be left standing on the
     * worker.
     */
    function oxphp_cancel_guarded_render(string $key): void
    {
        static $seen = [];

        $seen[$key] = true;

        try {
            OxphpWriteCancelProbe::$stage = 'parked';
            // Hooked: parks this request. The test takes the client away
            // meanwhile, so the write below is already a write to nobody.
            sleep(2);

            OxphpWriteCancelProbe::$stage = 'before-write';
            echo "nobody is left to read this\n";
            // Asked after the write, because the write is where the server
            // learns of the client from this request's side. A script that
            // would rather stop early has only this to go on now.
            OxphpWriteCancelProbe::$abortedAfterWrite = connection_aborted();
            OxphpWriteCancelProbe::$stage = 'after-write';
        } finally {
            unset($seen[$key]);
            OxphpWriteCancelProbe::$stage = 'cleaned';
        }
    }
}

if (!function_exists('cancel_finally_guard_seen')) {
    /**
     * What oxphp_cancel_guarded_render() currently has marked on this worker.
     *
     * Reflection, because that is the only way to read a function's static from
     * outside it, and the static is where the defect leaves its evidence. The
     * table is per thread, so a reading taken from one request is a reading of
     * what every other request on that worker will find.
     *
     * @return array<string, bool>
     */
    function cancel_finally_guard_seen(): array
    {
        $statics = (new ReflectionFunction('oxphp_cancel_guarded_render'))->getStaticVariables();

        return $statics['seen'] ?? [];
    }
}
