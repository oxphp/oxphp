<?php

declare(strict_types=1);

// A write on one connection that, while it is still going on, makes a read on
// another one park. The write is to a socket whose other end is closed, so PHP
// reports "Send of N bytes failed" as a notice from inside the write and calls the
// error handler there; the handler reads from a second socket that never gets
// anything, and this fiber is suspended in that read — one operation deep inside
// the write — until the read's own timeout of two seconds runs out.
//
// The first socket is handed to the request that fired this one through the
// worker's shared state, so that it can be closed from there while this fiber is
// parked.
try {
    [$outer, $outer_peer] = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    [$quiet, $quiet_peer] = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    fclose($outer_peer);
    $sharedState['nested_outer'] = $outer;

    $handled = 0;
    set_error_handler(static function () use (&$handled, &$sharedState, $quiet): bool {
        if ($handled++ > 0) {
            return true;
        }
        stream_set_timeout($quiet, 2);
        $sharedState['nested_parked'] = true;
        fread($quiet, 1);
        return true;
    });
    fwrite($outer, 'x');
    restore_error_handler();

    echo 'nested-done:' . $handled;
} catch (\Throwable $e) {
    echo 'nested-failed:' . str_replace("\n", ' ', $e->getMessage());
}
