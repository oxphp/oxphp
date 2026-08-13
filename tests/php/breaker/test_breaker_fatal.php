<?php

declare(strict_types=1);

// A request that ends in a real fatal — the failure the consecutive-error
// breaker exists for. Declaring a class whose name is already taken raises
// E_COMPILE_ERROR, and that is a zend_bailout: the worker catches it around the
// handler call and finalizes the request as a failed one.
//
// The declarations are wrapped in a condition the compiler cannot fold so both
// are bound at runtime. Two unconditional declarations would be rejected while
// the file is compiled, which makes the file unusable rather than fatal at
// request time — and in worker mode the first declaration outlives its request
// anyway, so from the second request on it is the one at the top that raises.
//
// Deliberately not `require` of a missing file, and deliberately not an
// exception:
//
//   - `require` of a missing file emits E_WARNING first, and a set_error_handler
//     left installed by an earlier request on this worker (TestCase installs
//     one) turns that warning into a thrown ErrorException before the fatal is
//     ever reached — the request then ends as an uncaught exception.
//   - An uncaught exception unwinds cleanly and does not count towards the
//     breaker at all. test_breaker_throw is the case that pins that.
//
// Either substitution silently stops testing the breaker. E_COMPILE_ERROR
// cannot be routed to a user error handler, so this one holds whatever earlier
// requests left behind.
//
// No TestCase: the fatal is the whole point of the file, and the response it
// produces is asserted by the runner as a 500 rather than by a JSON body.

if (count($_SERVER) > 0) {
    class BreakerDoubleDeclare
    {
    }
}

if (count($_SERVER) > 0) {
    class BreakerDoubleDeclare
    {
    }
}

echo "unreachable: declaring a class twice must be fatal\n";
