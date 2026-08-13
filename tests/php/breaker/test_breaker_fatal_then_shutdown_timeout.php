<?php

declare(strict_types=1);

// A real fatal in the handler, and then a deadline that expires while the
// shutdown function runs.
//
// The bit that says a deadline expired is set once per request and never
// cleared until the next one, so by the time the shutdown window is over it
// answers "did this request's deadline expire at some point", not "is the
// deadline what ended this window". A request that had already come apart in
// its handler must not be re-read as a cancellation because of it: the engine
// state the next request on this worker inherits was wrecked by the fatal, and
// nothing that happened afterwards puts it back.
//
// It is reachable on purpose, not in theory. Shipping the fatal somewhere —
// an error collector, a log sink — is what an application registers a shutdown
// function for, and a slow one there is the same slow dependency the neutral
// deadline case is argued from. If this were neutral, any application could keep
// a worker that fatals on every request alive forever by registering one slow
// shutdown function.
//
// Order matters inside the file: the deadline and the registration have to be in
// place before the fatal, because nothing after it runs.
//
// No TestCase: the fatal is the point of the file.

set_time_limit(1);

register_shutdown_function(static function (): void {
    $stop = microtime(true) + 5.0;
    while (microtime(true) < $stop) {
        // spin until max_execution_time ends the request
    }
});

if (count($_SERVER) > 0) {
    class BreakerFatalThenTimeout
    {
    }
}

if (count($_SERVER) > 0) {
    class BreakerFatalThenTimeout
    {
    }
}

echo "unreachable: declaring a class twice must be fatal\n";
