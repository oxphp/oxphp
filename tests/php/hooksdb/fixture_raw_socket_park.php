<?php

declare(strict_types=1);

// Parks a fiber inside a socket read on a stream the whole worker shares, so the
// request that fires this one can close that stream while the read is still
// waiting for its reply.
//
// A raw stream_socket_client() rather than one of the clients on purpose: fclose()
// on a resource passes through no guarded entry point at all, so nothing holds it
// back until this read is done — which is what makes it the shortest way to a
// stream freed under a parked reader.
try {
    if (!isset($sharedState['rawsock']) || !is_resource($sharedState['rawsock'])) {
        $host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
        $sock = stream_socket_client("tcp://{$host}:6379", $errno, $errstr, 3.0);
        if ($sock === false) {
            echo "raw-park-connect-failed:{$errstr} ({$errno})";
            return;
        }
        $sharedState['rawsock'] = $sock;
    }

    $sock = $sharedState['rawsock'];

    // The read has to be one that waits — blocking, a nonzero timeout, nothing
    // buffered — because that is the only shape the hook parks on. Ten seconds
    // sets the two outcomes far apart — "came back because it was told" against
    // "came back because it gave up" — without pushing a build that never notices
    // the close past the runner's own per-test ceiling, which would report the
    // whole row as a failed request instead of as the assertion it is.
    stream_set_timeout($sock, 10);

    // A list nothing ever pushes to, held for longer than this read's own
    // deadline, so the only two ways out of the park are that deadline and the
    // close.
    fwrite($sock, "BLPOP oxphp:parked:never 20\r\n");

    // Set between the write and the read, so the request that closes the stream
    // can tell "the holder is about to park" from "the holder has not got here
    // yet" instead of relying on a sleep alone.
    $sharedState['rawsock_parked'] = true;

    $reply = fread($sock, 1024);
    echo 'raw-park-done:' . var_export($reply, true);
} catch (\Throwable $e) {
    echo 'raw-park-failed:' . str_replace("\n", ' ', $e->getMessage());
}
