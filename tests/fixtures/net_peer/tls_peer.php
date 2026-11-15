<?php

declare(strict_types=1);

// A TLS server whose timing is fixed by the port a client picks, for the tests that
// need a wait inside a TLS stream to last a known time. Single process, one event
// loop: every connection is a record with a due time, so any number of clients can
// be kept waiting at once and none of them holds up the others.
//
//   4433  completes the handshake at once, sends "tls-done" one second later
//   4434  waits one second, then completes the handshake and sends "tls-done"
//   4435  completes the handshake at once and never sends anything
//   4436  accepts the TCP connection and never starts the handshake
//   8080  HTTP: answers the request after the number of milliseconds its path
//         starts with — `GET /2000/name` waits two seconds — one second for a
//         path with no number, with the body "curl-done:" followed by the path
//   8443  the same over TLS, the handshake completed at once
//
// The certificate is made here, at start-up, so nothing has to be generated or
// mounted from outside and the peer runs from the stock PHP image.

$delay = 1.0;

$key = openssl_pkey_new(['private_key_bits' => 2048]);
$csr = openssl_csr_new(['commonName' => 'oxphp-net-peer'], $key);
$cert = openssl_csr_sign($csr, null, $key, 1);
openssl_x509_export($cert, $certPem);
openssl_pkey_export($key, $keyPem);
$pem = sys_get_temp_dir() . '/net_peer.pem';
file_put_contents($pem, $certPem . $keyPem);

$context = stream_context_create(['ssl' => [
    'local_cert' => $pem,
    'verify_peer' => false,
    'allow_self_signed' => true,
]]);

$listeners = [];
foreach ([4433, 4434, 4435, 4436, 8080, 8443] as $port) {
    $server = stream_socket_server("tcp://0.0.0.0:{$port}", $errno, $errstr, STREAM_SERVER_BIND | STREAM_SERVER_LISTEN, $context);
    if ($server === false) {
        fwrite(STDERR, "net peer: cannot listen on {$port}: {$errstr}\n");
        exit(1);
    }
    $listeners[$port] = $server;
}

/** @var list<array{conn: resource, port: int, due: float, phase: string}> $pending */
$pending = [];
/** @var list<resource> $open */
$open = [];
/** @var list<array{conn: resource, accepted: float, request: string}> $http */
$http = [];

function handshake($conn): bool
{
    stream_set_blocking($conn, true);
    stream_set_timeout($conn, 5);
    $ok = @stream_socket_enable_crypto($conn, true, STREAM_CRYPTO_METHOD_TLS_SERVER);
    return $ok === true;
}

while (true) {
    $read = array_values($listeners);
    $write = $except = null;
    if (@stream_select($read, $write, $except, 0, 20000) > 0) {
        foreach ($read as $listener) {
            $conn = @stream_socket_accept($listener, 0);
            if ($conn === false) {
                continue;
            }
            $port = (int) substr((string) stream_socket_get_name($listener, false), -4);
            $now = microtime(true);
            switch ($port) {
                case 4433:
                    if (handshake($conn)) {
                        $pending[] = ['conn' => $conn, 'port' => $port, 'due' => $now + $delay, 'phase' => 'write'];
                    }
                    break;
                case 4434:
                    $pending[] = ['conn' => $conn, 'port' => $port, 'due' => $now + $delay, 'phase' => 'handshake'];
                    break;
                case 4435:
                    if (handshake($conn)) {
                        $open[] = $conn;
                    }
                    break;
                case 8080:
                    $http[] = ['conn' => $conn, 'accepted' => $now, 'request' => ''];
                    break;
                case 8443:
                    if (handshake($conn)) {
                        $http[] = ['conn' => $conn, 'accepted' => $now, 'request' => ''];
                    }
                    break;
                default:
                    $open[] = $conn;
            }
        }
    }

    $now = microtime(true);
    foreach ($pending as $i => $p) {
        if ($p['due'] > $now) {
            continue;
        }
        unset($pending[$i]);
        if ($p['phase'] === 'handshake' && !handshake($p['conn'])) {
            continue;
        }
        fwrite($p['conn'], 'tls-done');
        $open[] = $p['conn'];
    }

    // An HTTP request is read as it arrives; its delay counts from the accept, and
    // the answer goes out once both the request is complete and the delay is over.
    foreach ($http as $i => $h) {
        stream_set_blocking($h['conn'], false);
        $data = @fread($h['conn'], 8192);
        if (is_string($data)) {
            $h['request'] .= $data;
            $http[$i] = $h;
        }
        if (!str_contains($h['request'], "\r\n\r\n")) {
            if (feof($h['conn'])) {
                fclose($h['conn']);
                unset($http[$i]);
            }
            continue;
        }
        $path = explode(' ', $h['request'], 3)[1] ?? '/';
        $delayMs = preg_match('#^/(\d+)#', $path, $m) ? (int) $m[1] : 1000;
        if ($h['accepted'] + $delayMs / 1000 > $now) {
            continue;
        }
        unset($http[$i]);
        $body = 'curl-done:' . $path;
        fwrite($h['conn'], "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: " . strlen($body)
            . "\r\nConnection: close\r\n\r\n" . $body);
        $open[] = $h['conn'];
    }

    // Connections stay open until the client lets go of them, so a test that closes
    // its end is what ends them and a test that does not keeps the peer's side
    // waiting.
    foreach ($open as $i => $conn) {
        stream_set_blocking($conn, false);
        $data = @fread($conn, 8192);
        if (($data === '' || $data === false) && feof($conn)) {
            fclose($conn);
            unset($open[$i]);
        }
    }
}
