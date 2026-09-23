<?php

declare(strict_types=1);

// Inner request for fibers/test_stream_that_asked_outlives_its_client.
//
// The streaming fixture beside it, with one difference: this one calls
// ignore_user_abort(true) before its client is taken away. A stream is ended at
// its next write when its client leaves, because nothing else bounds its loop —
// unless the script asked to outlive its client, which is the one thing that
// call means. Asked, it has to get past the write and finish on its own.
//
// ?open=1 sends the headers before the park, as a real event stream does.
// That changes the path out: once its headers are sent a stream is no longer
// watched for its client leaving, so the park ends unnoticed, and it is the next
// flush that finds the client gone and would interrupt the request. Without it the
// headers are still unsent at the park, the leaving marks the request cancelled
// there, and it is the write that would end it.

require_once __DIR__ . '/write_cancel_probe.php';

ignore_user_abort(true);

header('Content-Type: text/event-stream');

if (($_GET['open'] ?? '') === '1') {
    echo ": open\n\n";
    oxphp_stream_flush();
}

OxphpWriteCancelProbe::$stage = 'parked';
sleep(2);

// Two writes, and the stage below is reached only past both: a build that
// ignores the call ends the stream at one of them, and which one depends on
// whether the server has noticed the client leave by the first.
OxphpWriteCancelProbe::$stage = 'before-write';
echo "data: nobody is left to read this\n\n";
oxphp_stream_flush();
echo "data: nor this\n\n";
oxphp_stream_flush();

OxphpWriteCancelProbe::$stage = 'after-write';
