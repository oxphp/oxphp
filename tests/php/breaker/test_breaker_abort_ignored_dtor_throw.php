<?php

declare(strict_types=1);

// A client hanging up on a handler that asked to outlive it, as in
// test_breaker_abort_ignored, while the handler holds an object whose destructor
// throws.
//
// The write ends the request with a bare bailout, and the worker's cleanup then
// gives back what the handler's frame was holding. Nothing marked that object
// destructed on the way — a bare bailout never does, and a fatal does only once
// its message is out — so its destructor runs inside the cleanup, with no frame
// beneath it, and the engine reports the throw as an uncaught exception: an
// E_ERROR. That report is not the request failing. The
// request was already a cancellation when the destructor ran, and a destructor
// that throws is an application outcome anywhere else; filed as a failure, three
// clients hanging up on such a handler retire the worker.
//
// Markers, cleared on the way in, so the probe reads this request and not an
// earlier one:
//
//  - the destructor marker says the destructor ran at all. Without it the
//    neutrality the probe reports is that of a handler whose object was never
//    destroyed;
//  - the past-echo marker must be ABSENT: written after the echo, it exists only
//    if the write did not end the request, and then the destructor ran at the
//    function's own return, as an ordinary throw in the handler.
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

        echo "the client is gone; this write is where the request unwinds\n";

        @file_put_contents('/tmp/oxphp-breaker-dtor-throw-past-echo', 'reached');
    }
}

// An exception handler an earlier request on this worker installed would be
// handed the throw instead of the engine reporting it.
set_exception_handler(null);

oxphp_breaker_hold_through_the_write();
