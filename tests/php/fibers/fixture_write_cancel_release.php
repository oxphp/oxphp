<?php

declare(strict_types=1);

// Inner request for fibers/test_write_cancel_releases_frames.
//
// Parks while holding an object in a local variable. The test closes this
// request's connection during the park; when the request resumes, the first
// thing it does is write. A write belonging to a request whose client has gone
// used to end the request there; on a non-streaming worker-mode request it no
// longer does, and the script runs on to the end of its handler so that its own
// cleanup gets to run. Either way the write raises no error — nothing is
// reported, nothing is logged — which is what separates this from a
// cancellation delivered by interrupting the script.
//
// Everything the request was holding is the worker's to give back if the
// request does not give it back itself: the worker keeps serving, and what
// nobody releases stays allocated for the rest of its life.
//
// ?in=shutdown does the same from a shutdown function of a request that has
// already had a fatal. The engine runs shutdown functions under a guard of its
// own, so a write that ended the request there would end the shutdown function
// rather than the handler; it does not, and the function runs past it. A request
// that has had a fatal is filed differently by the worker, which must not change
// what it gives back.
//
// ?in=stream and ?in=stream-throw make the request a stream first. A stream is
// still ended at the write once its client has gone — its loop has no other
// bound — so these are the requests that take the path where the worker gives
// back what the abandoned frames were holding. stream-throw holds an object
// whose destructor throws: it runs during that give-back, and the throw must end
// nothing but itself.
//
// ?in=destructor holds an object with a destructor instead. The destructor now
// runs at the function's own return, as part of the request, because the
// request gets to reach that return. It sleeps there, which under the runtime
// hooks is a point a request can park at. It used to run inside the worker's
// cleanup instead: the ending marked no object destructed — a fatal does, but
// only once its message has been displayed, and not at all when that display is
// itself a write to a cancelled request — so the abandoned frame's locals were
// released by the frame walk afterwards, which ran it there.

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

if (!class_exists('OxphpWriteCancelThrows', false)) {
    final class OxphpWriteCancelThrows
    {
        public function __destruct()
        {
            OxphpWriteCancelProbe::$stageAtDestruct = OxphpWriteCancelProbe::$stage;
            throw new \RuntimeException('thrown from a destructor during the give-back');
        }
    }
}

if (!function_exists('oxphp_write_after_client_left')) {
    function oxphp_write_after_client_left(string $holding = 'plain', bool $stream = false): void
    {
        // Held by this frame's variable and nothing else.
        $held = match ($holding) {
            'destructs' => new OxphpWriteCancelDestructs(),
            'throws' => new OxphpWriteCancelThrows(),
            default => new \ArrayObject([str_repeat('x', 1 << 16)]),
        };
        OxphpWriteCancelProbe::$weak = \WeakReference::create($held);

        OxphpWriteCancelProbe::$stage = 'parked';
        // Hooked: parks this request. The test takes the client away meanwhile.
        sleep(2);

        OxphpWriteCancelProbe::$stage = 'before-write';
        echo "nobody is left to read this\n";
        if ($stream) {
            oxphp_stream_flush();
        }
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
// the client is gone and nobody would read it.
register_shutdown_function(static function (): void {
    OxphpWriteCancelProbe::$report = [
        'stage' => OxphpWriteCancelProbe::$stage,
        'freed' => OxphpWriteCancelProbe::freed(),
        'last_error' => error_get_last()['message'] ?? null,
    ];
});

$in = $_GET['in'] ?? '';
$stream = $in === 'stream' || $in === 'stream-throw';
if ($stream) {
    // The header alone is what makes the request a stream. Nothing is sent
    // before the park: a stream that has already sent its headers is no longer
    // watched for its client leaving until its next flush, which is a
    // different path out (see the test).
    header('Content-Type: text/event-stream');
}

oxphp_write_after_client_left(
    match ($in) {
        'destructor' => 'destructs',
        'stream-throw' => 'throws',
        default => 'plain',
    },
    $stream
);
