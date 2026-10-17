<?php
// A stream for a client that reads nothing, for tests/graceful_drain.sh
// (Scenario K): run as a script in a classic process, and required by the
// worker's /flood route. It writes 64 KiB at a time and flushes each, so its
// flushes soon find no room for more and it waits inside one, which is where
// the drain has to end it. Every sixteenth flush that returns is logged, for
// the script to tell when the stream has stopped moving. ?ping=1 answers
// 'pong', for the script to tell when the server is up.

if (isset($_GET['ping'])) {
    echo 'pong';
    return;
}

header('Content-Type: application/octet-stream');
$chunk = str_repeat('f', 65536);
for ($i = 1; ; $i++) {
    echo $chunk;
    oxphp_stream_flush();
    if ($i % 16 === 0) {
        error_log("flood-flushed $i");
    }
}
// unreachable
