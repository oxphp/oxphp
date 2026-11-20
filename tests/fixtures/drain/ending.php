<?php
// A stream the drain ends, for tests/graceful_drain.sh (Scenario L): run as a
// script in a classic process, and required by the worker's /ending route. It
// logs what runs after the drain's fatal error: its finally block, a shutdown
// function, and the destructors of an object a function's frame holds and of
// one a global holds. The stream runs in a closure so that, as a classic
// process's top-level script, its object is still a frame's and not a global.
// display_errors is off, as a production php.ini sets it: with it on and no
// output buffer, displaying the error is a write the cancelled request refuses,
// which ends the request before PHP flags its objects as destructed.

if (!class_exists('DrainEndingMarker', false)) {
    final class DrainEndingMarker
    {
        public function __construct(private string $name) {}

        public function __destruct()
        {
            error_log("ending-dtor {$this->name}");
        }
    }
}

ini_set('display_errors', '0');
register_shutdown_function(static function (): void {
    error_log('ending-shutdown ' . (error_get_last()['message'] ?? 'no error'));
});
$GLOBALS['drain_ending'] = new DrainEndingMarker('global');

(static function (): void {
    $frame = new DrainEndingMarker('frame');
    header('Content-Type: text/event-stream');
    try {
        for ($i = 0; ; $i++) {
            echo "data: tick $i\n\n";
            oxphp_stream_flush();
            if ($i === 0) {
                error_log('ending-started');
            }
            oxphp_sleep(0.5);
        }
    } finally {
        error_log('ending-finally');
    }
})();
