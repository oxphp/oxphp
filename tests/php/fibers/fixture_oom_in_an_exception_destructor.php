<?php

declare(strict_types=1);

// Inner self-request for
// fibers/test_oom_in_an_exception_destructor_runs_the_shutdown_functions.
//
// Throws an exception it leaves uncaught, whose destructor runs out of memory.
// The worker lets go of an uncaught exception after the handler has returned
// and before the request's shutdown functions run, so that is where the
// destructor runs. The shutdown function says it ran, and what the last error
// was. Not displayed: the body is what the test reads.

ini_set('display_errors', '0');

register_shutdown_function(static function (): void {
    echo 'SHUTDOWN-RAN: ', error_get_last()['message'] ?? 'no error', "\n";
});

// Anonymous: a named class declared here would be declared again by the next
// request on this worker to include this file.
throw new class ('thrown by the handler') extends RuntimeException {
    public function __destruct()
    {
        $s = 'x';
        for (;;) {
            $s .= $s;
        }
    }
};
