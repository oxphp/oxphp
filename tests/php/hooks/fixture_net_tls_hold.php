<?php

declare(strict_types=1);

// Parks a fiber inside a TLS read on a stream the whole worker shares, so the
// request that fires this one can close or read that stream while the read is
// still waiting for an answer.
//
// Port 4435 of the peer, the default, completes the handshake and then says nothing,
// so the only ways out of this read are the stream's own timeout and whatever
// another fiber does to the stream. `?port=4433` picks the peer that sends its
// record a second after the handshake instead.
try {
    $host = getenv('NET_PEER') ?: 'hooks-netpeer';
    $port = (int) ($_GET['port'] ?? 4435);
    $context = stream_context_create(['ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
    $sock = stream_socket_client("ssl://{$host}:{$port}", $errno, $errstr, 3.0, STREAM_CLIENT_CONNECT, $context);
    if ($sock === false) {
        echo "tls-hold-connect-failed:{$errstr} ({$errno})";
        return;
    }
    $sharedState['net_tls'] = $sock;

    stream_set_timeout($sock, (int) ($_GET['timeout'] ?? 6));

    // Set between the connect and the read, so the request that acts on the stream
    // can tell "the holder is about to park" from "the holder has not got here
    // yet" instead of relying on a sleep alone.
    $sharedState['net_tls_parked'] = true;

    $reply = fread($sock, 16);
    echo 'tls-hold-done:' . var_export($reply, true);
} catch (\Throwable $e) {
    echo 'tls-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
