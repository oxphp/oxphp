<?php

declare(strict_types=1);

// Inner request for fibers/test_write_cancel_releases_frames.
//
// Parks while holding an object in a local variable. The test closes this
// request's connection during the park; when the request resumes, the first
// thing it does is write, and a write to a request whose client has gone ends
// the request there. That ending raises no error — nothing is reported, nothing
// is logged — which is what separates it from a cancellation delivered by
// interrupting the script, and what this fixture exists to exercise.
//
// Everything the request was holding at that point is the worker's to give
// back: the worker keeps serving, and what it does not release stays allocated
// for the rest of its life.
//
// ?in=shutdown does the same from a shutdown function of a request that has
// already had a fatal. The engine runs shutdown functions under a guard of its
// own, so the write ends the shutdown function rather than the handler, and the
// worker has to recognise that separately; and a request that has had a fatal
// is one whose cancellation the worker files differently, which must not change
// what it gives back.
//
// ?in=destructor holds an object with a destructor instead. This ending marks no
// object destructed — a fatal does, but only once its message has been
// displayed, and not at all when that display is itself a write to a cancelled
// request — so the destructor runs inside the worker's cleanup. It sleeps there,
// which under the runtime hooks is a point a request can park at.

require_once __DIR__ . '/write_cancel_probe.php';

if (!class_exists('OxphpWriteCancelDestructs', false)) {
    final class OxphpWriteCancelDestructs
    {
        public function __destruct()
        {
            OxphpWriteCancelProbe::$stageAtDestruct = OxphpWriteCancelProbe::$stage;
            OxphpWriteCancelProbe::$stage = 'destructing';
            // Hooked. Long enough for the test's own poll to come round many
            // times, if this lets it.
            usleep(300_000);
            OxphpWriteCancelProbe::$stage = 'destructed';
        }
    }
}

if (!function_exists('oxphp_write_after_client_left')) {
    function oxphp_write_after_client_left(bool $destructs = false): void
    {
        // Held by this frame's variable and nothing else.
        $held = $destructs
            ? new OxphpWriteCancelDestructs()
            : new \ArrayObject([str_repeat('x', 1 << 16)]);
        OxphpWriteCancelProbe::$weak = \WeakReference::create($held);

        OxphpWriteCancelProbe::$stage = 'parked';
        // Hooked: parks this request. The test takes the client away meanwhile.
        sleep(2);

        OxphpWriteCancelProbe::$stage = 'before-write';
        echo "nobody is left to read this\n";
        OxphpWriteCancelProbe::$stage = 'after-write';
    }
}

error_clear_last();

if (($_GET['in'] ?? '') === 'shutdown') {
    // Cleared so the fatal below is one, rather than whatever a handler an
    // earlier request on this worker installed makes of it.
    set_error_handler(null);

    register_shutdown_function('oxphp_write_after_client_left');
    trigger_error('fatal ahead of the shutdown function', E_USER_ERROR);
}

// Runs after the worker has dealt with the abandoned frames. Writes nothing:
// the client is gone, so a write here would end this function too.
register_shutdown_function(static function (): void {
    OxphpWriteCancelProbe::$report = [
        'stage' => OxphpWriteCancelProbe::$stage,
        'freed' => OxphpWriteCancelProbe::freed(),
        'last_error' => error_get_last()['message'] ?? null,
    ];
});

oxphp_write_after_client_left(($_GET['in'] ?? '') === 'destructor');
