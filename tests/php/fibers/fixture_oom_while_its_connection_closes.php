<?php

declare(strict_types=1);

// Inner self-request for fibers/test_oom_while_a_connection_closes_does_not_run_on.
//
// Runs out of memory in a closure holding the only reference to a connection
// whose close has to wait: the other end is open and reads nothing, so the
// connection has no room left, and its write filter still holds a byte, which
// it writes out as the connection closes. The engine closes the connection as
// it unwinds the closure, so that write comes while the error is being unwound.
// What runs after the closure says so in the marker file; nothing should.

$marker = sys_get_temp_dir() . '/oxphp-oom-close-' . preg_replace('/[^0-9a-f]/', '', $_GET['run'] ?? '');

// A closure, not a function: a function declared here would be declared again by
// the next request on this worker to include this file.
$runOut = static function () use ($marker): void {
    $server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
    $client = stream_socket_client('tcp://' . stream_socket_get_name($server, false), $errno, $errstr, 3.0);
    $peer = stream_socket_accept($server, 3.0);
    // The other end outlives the closure, unread.
    $GLOBALS['far_end'] = [$server, $peer];

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
    $full = $sent === 0 && (int) @fwrite($client, 'x') === 0;
    stream_set_blocking($client, true);
    stream_set_timeout($client, 3);

    // One byte is less than base64 encodes on its own, so the filter holds it
    // until the stream closes.
    stream_filter_append($client, 'convert.base64-encode', STREAM_FILTER_WRITE);
    fwrite($client, 'y');

    file_put_contents($marker, ($full ? 'full' : 'room left') . ' ' . microtime(true));
    $s = 'x';
    for (;;) {
        $s .= $s;
    }
};

$runOut();

file_put_contents($marker, ' ran on', FILE_APPEND);
echo "RAN-ON\n";
