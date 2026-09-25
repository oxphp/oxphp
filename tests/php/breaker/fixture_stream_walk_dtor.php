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
// ?at=session moves the hold and the write into the session save handler: the
// request parks, returns without writing, and the worker writes its session
// afterwards, past the request's own end. The save handler's write is then the
// write the stream is ended at, and the give-back is from its frame.
//
// display_errors off, which is where production runs. With it on and nothing
// buffered, the engine's display of the fatal is itself a write to this ended
// request, and that write ends the fatal before it gets as far as flagging
// anything.

require_once __DIR__ . '/stream_walk.php';

$id = (int) ($_GET['id'] ?? 0);
$dtor = (string) ($_GET['dtor'] ?? '');
$at = (string) ($_GET['at'] ?? 'handler');

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

if (!class_exists('OxphpBreakerStreamWalkSession', false)) {
    final class OxphpBreakerStreamWalkSession implements SessionHandlerInterface
    {
        public function __construct(private string $dtor, private int $id)
        {
        }

        public function open(string $path, string $name): bool
        {
            return true;
        }

        public function close(): bool
        {
            return true;
        }

        public function read(string $id): string
        {
            return '';
        }

        public function write(string $id, string $data): bool
        {
            // Held by this frame's variable and nothing else, as in the handler.
            $held = new OxphpBreakerStreamWalkHeld($this->dtor, $this->id);

            $stage = stream_walk_stage_file($this->dtor, $this->id);
            file_put_contents($stage, 'session-write');
            echo "data: nobody is left to read this\n\n";
            file_put_contents($stage, 'after-session-write');

            return true;
        }

        public function destroy(string $id): bool
        {
            return true;
        }

        public function gc(int $max_lifetime): int|false
        {
            return 0;
        }
    }
}

// The header alone is what makes the request a stream. Nothing is sent before
// the park: a stream that has sent its headers is not watched for its client
// leaving until its next flush.
header('Content-Type: text/event-stream');

if ($at === 'session') {
    // Not registered as a shutdown function: the worker's own session write is
    // the window this is about.
    session_set_save_handler(new OxphpBreakerStreamWalkSession($dtor, $id), false);
    session_id("oxphpbreakerstreamwalk$dtor$id");
    session_start();
    // Changed, so the session is written rather than only touched.
    $_SESSION['id'] = $id;

    $stage = stream_walk_stage_file($dtor, $id);
    file_put_contents($stage, 'parked');
    oxphp_sleep(2.0);
    file_put_contents($stage, 'returned');
    return;
}

oxphp_breaker_stream_walk_hold($dtor, $id);
