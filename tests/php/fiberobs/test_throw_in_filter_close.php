<?php

declare(strict_types=1);

// The other way userland ends a step of the recovery: by throwing rather than
// by fataling. It arrives at the same place by a longer road, and the road is
// the point — three errors come out of this one request:
//
//   1. the request's own fatal, below;
//   2. the throw from onclose(), reported as an uncaught exception. The calls
//      the release walk makes run with no current frame beneath them, so there
//      is nothing for the engine to unwind into and it reports the throw where
//      it stands. That report does not end the request: it is raised in the one
//      form that says so, and execution carries on into the release of the
//      exception object that the report ends with;
//   3. that release running the exception's __destruct — because the flag that
//      keeps this walk from running any destructor was put on the objects that
//      existed when the request fataled, and this one was built after. The
//      fatal it raises is raised inside the walk.
//
// So a filter that throws costs the worker exactly what a filter that fatals
// costs it, and it is the same guard that saves it: with the guard around the
// release walk taken out, this request loses its worker just as the fatal one
// does, and the request after it is answered by a replacement.
//
// No assertions and no test JSON: the suite line checks the 500 this produces.

// Both handler slots, because a worker resets neither: what is installed here
// on arrival is whatever the last TestCase to run on this worker left behind.
// The error handler would turn the fatal below into an ErrorException. The
// exception handler would take the throw from onclose() before the engine's own
// reporting ever sees it — answering this request with the previous test's JSON
// and ending it with exit(), which reaches the same guard by a shorter road and
// never gets as far as step 3.
set_error_handler(null);
set_exception_handler(null);

if (!class_exists('OxPHPFatalOnDestructException', false)) {
    class OxPHPFatalOnDestructException extends RuntimeException
    {
        public function __destruct()
        {
            trigger_error(
                'fatal raised while the recovery was dropping the exception its walk was handed',
                E_USER_ERROR
            );
        }
    }
}

if (!class_exists('OxPHPThrowOnCloseFilter', false)) {
    class OxPHPThrowOnCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            @file_put_contents('/tmp/oxphp-filter-close-marker', (string) time());

            throw new OxPHPFatalOnDestructException('thrown from a filter the recovery walk was releasing');
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }
}

if (!in_array('oxphp-throw-on-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-throw-on-close', OxPHPThrowOnCloseFilter::class);
}

@unlink('/tmp/oxphp-filter-close-marker');

function oxphp_fatal_under_throwing_filter(): void
{
    $handle = fopen('php://memory', 'r+');
    stream_filter_append($handle, 'oxphp-throw-on-close');

    trigger_error('the request fatal', E_USER_ERROR);
}

oxphp_fatal_under_throwing_filter();
