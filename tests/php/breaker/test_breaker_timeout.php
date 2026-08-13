<?php

declare(strict_types=1);

// A request the engine ends on max_execution_time, without ever suspending.
//
// This is the shape a synchronous application has when a dependency goes slow:
// the handler is inside one blocking call, the worker has nothing else to do,
// and the deadline is what ends the request. It unwinds through the same
// zend_error_noreturn as every other cancellation, so it arrives at the worker
// as a bailout — which is why it has to be marked as a cancellation, or three
// slow requests in a row would retire the worker.
//
// A busy loop rather than sleep(): the deadline is CPU time on some platforms
// and this profile runs no hooks, so a sleep would not necessarily reach it.
//
// The loop's own bound is deliberately shorter than the runner's 15 s request
// timeout. If the deadline ever stops firing, this request ends by itself and
// fails one line; a bound past the runner's would instead hold the profile's
// single worker while the runner gave up on this request and the next ones,
// turning one broken thing into a screenful of red.

set_time_limit(1);

$stop = microtime(true) + 5.0;
while (microtime(true) < $stop) {
    // spin until max_execution_time ends the request
}

echo "unreachable: max_execution_time must end this request\n";
