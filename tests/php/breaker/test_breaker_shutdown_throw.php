<?php

declare(strict_types=1);

// An uncaught exception thrown from a shutdown function.
//
// Not a fatal, and that is the point: the loop reports it itself, and the call
// it reports it with asks for no bailout, so nothing raises CG(unclean_shutdown)
// and neither of the two flags the shutdown window sets is touched. The request
// arrives at the breaker looking exactly like one that did nothing wrong.
//
// The documented rule for an uncaught exception is neutral — the engine is
// intact, the request is answered 500 — and neutral is what this has to be here
// too, in both directions: it must not retire a worker, and it must not clear a
// run of fatals either.
//
// No TestCase, and no clearing of the error and exception handlers an earlier
// request on this worker installed: nothing calls them for this exception. The
// user exception handler is deliberately not invoked for a shutdown function's
// throw — that slot is thread-wide and outlives the request that installed it —
// and the report goes out at E_ERROR, which is never routed to a user error
// handler.

register_shutdown_function(static function (): void {
    throw new RuntimeException('breaker: uncaught exception from a shutdown function');
});

echo "the handler itself ends without incident\n";
