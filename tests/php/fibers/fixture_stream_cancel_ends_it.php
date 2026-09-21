<?php

declare(strict_types=1);

// Inner request for fibers/test_stream_cancel_still_ends_the_request.
//
// Enters streaming mode, parks, and writes again once its client has gone. The
// ordinary worker-mode request is let go on to the end of its handler when that
// happens, so that its own `finally` runs; this one must not be. A streaming
// loop is bounded by the client it writes to and by nothing else, so a request
// in this shape that is not ended at the write is not ended at all, and it
// holds the worker thread for the life of the process.
//
// The write after the park is therefore the last thing this fixture is expected
// to reach. Anything after it says the exemption is gone.

require_once __DIR__ . '/write_cancel_probe.php';

header('Content-Type: text/event-stream');

// Entering streaming mode is what the exemption keys on, so it has to have
// happened before the client is taken away.
echo ": open\n\n";
oxphp_stream_flush();

OxphpWriteCancelProbe::$stage = 'parked';
sleep(2);

OxphpWriteCancelProbe::$stage = 'before-write';
echo "data: nobody is left to read this\n\n";
oxphp_stream_flush();

OxphpWriteCancelProbe::$stage = 'after-write';
