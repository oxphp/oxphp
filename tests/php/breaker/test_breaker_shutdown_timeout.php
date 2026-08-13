<?php

declare(strict_types=1);

// A deadline that lands in the shutdown window rather than in the handler.
//
// The shape is an application that defers work to the end of the request —
// flushing a metrics buffer, writing a session, posting a log batch — to a
// dependency that has gone slow. The handler finishes well inside its deadline
// and the deadline expires while the shutdown function is still working.
//
// It matters because max_execution_time is the one cancellation that does not
// come through the interrupt handler: the engine checks EG(timed_out) itself and
// calls zend_timeout(), which arrives as a bailout indistinguishable from a
// fatal. In the handler that bailout reaches the arm that recognises it by the
// PHP_CONNECTION_TIMEOUT bit; here it is swallowed by the engine's own zend_try
// around the shutdown functions and never reaches any arm at all. It is still
// the server ending the request, so it must stay neutral for the breaker —
// otherwise one slow dependency rotates the whole pool three requests at a time,
// which is the harm the cancellation cases above exist to prevent.
//
// A busy loop rather than sleep(), for the same reason as test_breaker_timeout:
// the deadline is CPU time on some platforms and this profile runs no hooks. The
// bound is shorter than the runner's request timeout so that a build where the
// deadline stopped firing fails one line instead of holding the single worker.
//
// No TestCase: this request has nothing to report.

set_time_limit(1);

register_shutdown_function(static function (): void {
    $stop = microtime(true) + 5.0;
    while (microtime(true) < $stop) {
        // spin until max_execution_time ends the request
    }

    echo "unreachable: max_execution_time must end this request\n";
});

echo "the handler itself ends well inside its deadline\n";
