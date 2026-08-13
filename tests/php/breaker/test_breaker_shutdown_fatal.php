<?php

declare(strict_types=1);

// A request whose handler ends normally and whose shutdown function then
// fatals. The engine runs registered shutdown functions under a zend_try of its
// own, so the bailout stops there and the worker is handed an ordinary return —
// which is exactly why this case needs a test of its own rather than being a
// second spelling of test_breaker_fatal.
//
// The same double declaration that file uses, and for the same reason:
// E_COMPILE_ERROR is never routed to a user error handler, so the fatal does not
// depend on what a set_error_handler left installed by an earlier request on
// this worker would make of a warning. Both declarations are wrapped in a
// condition the compiler cannot fold so both are bound at runtime — the first
// request fatals on the second one, and every later request on the first,
// because in worker mode the class outlives the request that declared it.
//
// No TestCase: its constructor installs handlers, and the response this file
// produces is asserted by the runner rather than by a JSON body.

register_shutdown_function(static function (): void {
    if (count($_SERVER) > 0) {
        class BreakerShutdownDoubleDeclare
        {
        }
    }

    if (count($_SERVER) > 0) {
        class BreakerShutdownDoubleDeclare
        {
        }
    }

    echo "unreachable: declaring a class twice must be fatal\n";
});

echo "the handler itself ends without incident\n";
