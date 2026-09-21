<?php

declare(strict_types=1);

// A client hanging up on a handler that asked to outlive it, as in
// test_breaker_abort_ignored, while the handler holds an object whose destructor
// throws.
//
// In worker mode the write does not end the request, so the handler reaches its
// own return and the object is given back there, by the request rather than by
// the worker cleaning up after it. The destructor runs at that return and its
// throw leaves the request as an uncaught exception, which the engine reports as
// an E_ERROR. That report is not the request failing: a destructor that throws
// is an application outcome anywhere else, and filed as a failure, three clients
// hanging up on such a handler retire the worker.
//
// The same has to hold whichever half of the mechanism the client's departure
// reaches, because neither ends the request any more.
//
// Markers, cleared on the way in, so the probe reads this request and not an
// earlier one:
//
//  - the destructor marker says the destructor ran at all. Without it the
//    neutrality the probe reports is that of a handler whose object was never
//    destroyed;
//  - the past-echo marker must be PRESENT: written after the echo, it exists
//    only because the write did not end the request, which is also what puts
//    the destructor at the function's own return rather than in the worker's
//    cleanup.
//
// Files rather than statics, for the reason test_breaker_abort_ignored gives.

foreach ([
    '/tmp/oxphp-breaker-dtor-throw-destructed',
    '/tmp/oxphp-breaker-dtor-throw-past-echo',
] as $marker) {
    if (is_file($marker)) {
        unlink($marker);
    }
}

// Declared conditionally: this profile's single worker keeps serving across the
// three requests.
if (!class_exists('OxphpBreakerThrowingDestructor', false)) {
    final class OxphpBreakerThrowingDestructor
    {
        public function __destruct()
        {
            @file_put_contents('/tmp/oxphp-breaker-dtor-throw-destructed', 'reached');
            throw new \RuntimeException('thrown by a destructor the cleanup ran');
        }
    }
}

if (!function_exists('oxphp_breaker_hold_through_the_write')) {
    function oxphp_breaker_hold_through_the_write(): void
    {
        // Held by this frame's variable and nothing else.
        $held = new OxphpBreakerThrowingDestructor();

        ignore_user_abort(true);

        // Native and blocking in this profile; longer than the suite line's
        // --max-time, so the client is gone by the write.
        usleep(2_000_000);

        echo "the client is gone; nobody reads this write\n";

        @file_put_contents('/tmp/oxphp-breaker-dtor-throw-past-echo', 'reached');
    }
}

// An exception handler an earlier request on this worker installed would be
// handed the throw instead of the engine reporting it.
set_exception_handler(null);

oxphp_breaker_hold_through_the_write();
