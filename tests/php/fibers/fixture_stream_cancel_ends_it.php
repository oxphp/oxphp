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
//
// ?open=1 sends the headers before the park, as a real event stream does.
// That changes the path out: once its headers are sent a stream is no longer
// watched for its client leaving, so the park ends unnoticed, and it is the next
// flush that finds the client gone and interrupts the request. Without it the
// headers are still unsent at the park, the leaving marks the request cancelled
// there, and it is the write that has to end it.

require_once __DIR__ . '/write_cancel_probe.php';

// The header alone makes the request a stream, which is what the exemption keys
// on, so it is set before the client is taken away.
header('Content-Type: text/event-stream');

if (($_GET['open'] ?? '') === '1') {
    echo ": open\n\n";
    oxphp_stream_flush();
}

OxphpWriteCancelProbe::$stage = 'parked';
sleep(2);

OxphpWriteCancelProbe::$stage = 'before-write';
echo "data: nobody is left to read this\n\n";
oxphp_stream_flush();

OxphpWriteCancelProbe::$stage = 'after-write';
