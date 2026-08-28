<?php

declare(strict_types=1);

// Ends the request that includes it in a fatal, on a worker whose functions
// are observed.
//
// A fatal is a longjmp: the frames it abandons are never returned from, so the
// end handler an observer installed on each of them never runs, and the
// engine's chain of open calls still names them afterwards. In a SAPI that
// shuts the request down that is harmless — request shutdown closes the open
// handlers as its first step. A worker does not shut the request down between
// requests, so what one request leaves on that chain is what the next one
// starts from, and the frames it names are freed on the way out.
//
// Out of memory rather than anything shorter to write, because the worker
// fixture installs an error handler: every diagnostic that can become an
// exception does, and an exception unwinds properly — the frames return, their
// handlers close, and nothing is left behind. Exhausting the heap is one of the
// few errors userland is never given a say in, so it is a fatal here and a
// fatal in any application, whatever it installs.
//
// Included from two callers rather than written out twice: both fatals have to
// abandon frames at the same addresses for the second one to close the chain
// into a loop, and the surest way to get the same addresses is the same frames.

ini_set('memory_limit', (string) (memory_get_usage(true) + 2 * 1024 * 1024));

$ballast = [];
while (true) {
    $ballast[] = str_repeat('x', 65536);
}
