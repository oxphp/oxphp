<?php

declare(strict_types=1);

// Inner self-request for fibers/test_stream_errors_stay_with_their_request.
//
// With store=1: reports the stream errors it can read at its own start, which
// must be none, then stores one of its own that nobody else may read.
//
// With close=1: releases a stream whose filter still holds a byte, on a connection
// with no room left, and reports what its own error handler heard of that.

header('Content-Type: text/plain');

if (isset($_GET['store'])) {
    $seen = [];
    if (function_exists('stream_last_errors')) {
        foreach (stream_last_errors() as $error) {
            $seen[] = $error->message;
        }
        // A directory, so its message is not the one the parked request stored.
        fopen('/tmp', 'w', false, stream_context_create(['stream' => ['error_mode' => StreamErrorMode::Silent]]));
    }
    echo 'STORED-AT-START:', json_encode($seen), "\n";

    return;
}

$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
$client = stream_socket_client('tcp://' . stream_socket_get_name($server, false), $errno, $errstr, 3.0);
$peer = stream_socket_accept($server, 3.0);

// Filled until a round written after a pause sends nothing: the acknowledgements
// still on their way free room for a while after the first refusal.
stream_set_blocking($client, false);
$chunk = str_repeat('x', 65536);
$rounds = 0;
do {
    $sent = 0;
    while (($n = (int) @fwrite($client, $chunk)) > 0) {
        $sent += $n;
    }
    usleep(200_000);
} while ($sent > 0 && ++$rounds < 20);

// A write on a stream that does not block just reports nothing written, so back
// to blocking, for the close's write to time out and say so. One byte is less
// than base64 encodes on its own, so the filter holds it until the stream closes.
stream_set_blocking($client, true);
stream_set_timeout($client, 1);
stream_filter_append($client, 'convert.base64-encode', STREAM_FILTER_WRITE);
fwrite($client, 'y');

$heard = [];
set_error_handler(static function (int $severity, string $message) use (&$heard): bool {
    $heard[] = $message;

    return true;
});
// Not fclose(), which opens an operation of its own: released like this, nothing
// here has one open, and the write's error joins whichever is on top.
$client = null;
restore_error_handler();
echo 'HEARD:', json_encode($heard), "\n";
