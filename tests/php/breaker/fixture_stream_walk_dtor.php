<?php

declare(strict_types=1);

// Inner request for the stream-walk block of this suite; staged by
// stream_walk.php.
//
// A stream that parks holding an object in a local variable. Its client leaves
// during the park, and a stream whose client has gone is ended at its next
// write — so it never reaches its own return, and the object is released by the
// worker as it gives back the frames the ended request abandoned. Its
// destructor runs there, and ?dtor= says what it does:
//
//  - throw: throws. With no frame to throw into, the engine reports it as an
//    uncaught exception — an application outcome, and the request stays what
//    its ending made it, a cancellation.
//  - fatal: raises a fatal. The engine flags every object alive on the worker as
//    already destructed on its way into one, the requests multiplexed beside
//    this one included, and that is the engine state the breaker exists to
//    retire a worker over. It has to count.
//
// display_errors off, which is where production runs. With it on and nothing
// buffered, the engine's display of the fatal is itself a write to this ended
// request, and that write ends the fatal before it gets as far as flagging
// anything.

require_once __DIR__ . '/stream_walk.php';

$id = (int) ($_GET['id'] ?? 0);
$dtor = (string) ($_GET['dtor'] ?? '');

ini_set('display_errors', '0');
// Both slots are the thread's, and an earlier request on this worker may have
// filled either: an error handler would be offered the fatal's warnings first,
// an exception handler would be handed the throw.
set_error_handler(null);
set_exception_handler(null);

if (!class_exists('OxphpBreakerStreamWalkHeld', false)) {
    final class OxphpBreakerStreamWalkHeld
    {
        public function __construct(private string $dtor, private int $id)
        {
        }

        public function __destruct()
        {
            file_put_contents(
                stream_walk_dtor_file($this->dtor, $this->id),
                (string) stream_walk_read(stream_walk_stage_file($this->dtor, $this->id))
            );
            if ($this->dtor === 'throw') {
                throw new \RuntimeException('thrown by a destructor the give-back ran');
            }
            // E_COMPILE_ERROR, a fatal no error handler can intercept; see
            // breaker_redeclare.php for why it is a require.
            require __DIR__ . '/breaker_redeclare.php';
            require __DIR__ . '/breaker_redeclare.php';
        }
    }
}

if (!function_exists('oxphp_breaker_stream_walk_hold')) {
    function oxphp_breaker_stream_walk_hold(string $dtor, int $id): void
    {
        // Held by this frame's variable and nothing else.
        $held = new OxphpBreakerStreamWalkHeld($dtor, $id);

        $stage = stream_walk_stage_file($dtor, $id);
        file_put_contents($stage, 'parked');
        // oxphp_sleep, not sleep: this profile runs no hooks.
        oxphp_sleep(2.0);

        file_put_contents($stage, 'before-write');
        echo "data: nobody is left to read this\n\n";
        oxphp_stream_flush();
        file_put_contents($stage, 'after-write');
    }
}

// The header alone is what makes the request a stream. Nothing is sent before
// the park: a stream that has sent its headers is not watched for its client
// leaving until its next flush.
header('Content-Type: text/event-stream');

oxphp_breaker_stream_walk_hold($dtor, $id);
