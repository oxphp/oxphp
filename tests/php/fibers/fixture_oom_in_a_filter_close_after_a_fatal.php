<?php

declare(strict_types=1);

// Inner self-request for
// fibers/test_oom_in_a_filter_close_after_a_fatal_runs_the_shutdown_functions.
//
// Fatals with a stream open in the frame it fatals in, and the stream's userland
// filter runs out of memory in its onClose(). The worker gives back what the
// frames a fatal abandoned were holding as it recovers from it, ahead of the
// request's shutdown functions, and closing the stream calls onClose() — so the
// memory runs out inside that recovery. The shutdown function says it ran, and
// what the last error was. Not displayed: the body is what the test reads.

// A worker never shuts a request down, so the error handler the TestCase of an
// earlier request installed is still standing on this thread, and it throws on
// every error it is handed.
set_error_handler(null);
ini_set('display_errors', '0');

require_once __DIR__ . '/../engine_fatal.php';

if (!class_exists('OxphpOomOnCloseFilter', false)) {
    class OxphpOomOnCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            echo "ONCLOSE-RAN\n";
            $s = 'x';
            for (;;) {
                $s .= $s;
            }
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }
}

// The filter map is per thread and this worker keeps it for its whole life, so
// once a request on this worker has registered the name it stays registered.
if (!in_array('oxphp-oom-on-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-oom-on-close', OxphpOomOnCloseFilter::class);
}

register_shutdown_function(static function (): void {
    echo 'SHUTDOWN-RAN: ', error_get_last()['message'] ?? 'no error', "\n";
});

// The frame the fatal abandons. $handle is one of its compiled variables, so the
// handle is given back by the recovery's release walk and by nothing else: the
// closure never returns, and a worker does not tear the request down. A closure
// rather than a named function, which the next include on this worker would
// declare again.
$fatal = static function (): void {
    $handle = fopen('php://memory', 'r+');
    // The chain takes a reference of its own, so discarding what this returns
    // leaves the filter alive until the stream is closed.
    stream_filter_append($handle, 'oxphp-oom-on-close');

    engine_fatal('OxphpFatalUnderAnOomOnClose');
};
$fatal();
